defmodule ControlPlane.Reviews do
  @moduledoc """
  Asking for reviews of a pull request (ADR 0025).

  For a repository that opted in (`HEPH_REVIEWS`), a pull request whose checks are all
  green is read by the lenses its change needs, each at the commit that is its head.
  The files it changes and its manifest at that commit say which lenses, through
  `heph lenses`; each lens's text is read from the Athenaeum release the operator
  pinned (`HEPH_ATHENAEUM_REF`), never from anywhere a pull request can change. A
  review is a workload: the workload that opened the pull request again, on the head
  commit, with a `review` section, and that workload as its parent.

  A review is asked for once for a pull request, a head commit and a lens, however
  often this is called. A lens whose text is not there is reported and does not stop
  the others; anything that may pass (GitHub or `heph` failing, the manifest not
  readable) is an error, and nothing is asked for.
  """

  import Ecto.Query

  alias ControlPlane.{GitHub, Programs, Repo, Workload, Workloads}

  @athenaeum "erinversfeldcodes/athenaeum"

  @type result :: %{
          requested: [String.t()],
          existing: [String.t()],
          missing: [String.t()],
          unpacked: [String.t()],
          unowned: [String.t()]
        }

  @doc "Whether reviews are asked for on this repository's pull requests."
  @spec enabled?(String.t()) :: boolean()
  def enabled?(repository), do: repository in (settings()[:reviews] || [])

  @doc "Whether a workload is a review."
  @spec review?(%Workload{}) :: boolean()
  def review?(%Workload{workload: %{"review" => _}}), do: true
  def review?(_), do: false

  @doc """
  Ask for the reviews the pull request needs at its head commit, as far as they
  have not been asked for.
  """
  @spec request(%Workload{}, map(), String.t()) :: {:ok, result()} | {:error, String.t()}
  def request(%Workload{} = workload, pull, number) do
    sha = get_in(pull, ["head", "sha"])
    repository = workload.repository

    with {:ok, ref} <- athenaeum_ref(),
         {:ok, changed} <- changed_files(repository, number),
         {:ok, manifest} <- manifest(repository, sha),
         {:ok, plan} <- lenses(manifest, changed),
         {:ok, texts, missing} <- lens_texts(plan["lenses"], ref) do
      asked(workload, pull, number, texts, missing, plan)
    end
  end

  defp athenaeum_ref do
    case settings()[:athenaeum_ref] do
      ref when is_binary(ref) and ref != "" ->
        {:ok, ref}

      _ ->
        {:error, "HEPH_ATHENAEUM_REF is not set, so there is no release to read the lenses from"}
    end
  end

  defp changed_files(repository, number) do
    case GitHub.get_all("repos/#{repository}/pulls/#{number}/files") do
      {:ok, files} -> {:ok, for(%{"filename" => name} <- files, do: name)}
      {:error, message} -> {:error, "could not read the pull request's files: #{message}"}
    end
  end

  defp manifest(repository, sha) do
    case GitHub.raw("repos/#{repository}/contents/.athenaeum/manifest.toml?ref=#{sha}") do
      {:ok, text} -> {:ok, text}
      {:error, message} -> {:error, "could not read the manifest at #{sha}: #{message}"}
    end
  end

  # heph reads the manifest from a file and the changed files from its arguments, and
  # says what each lens needs, and what no lens will look at, as one line of JSON.
  defp lenses(_manifest, []), do: {:ok, %{"lenses" => [], "unpacked" => [], "unowned" => []}}

  defp lenses(manifest, changed) do
    path = Path.join(System.tmp_dir!(), "heph-manifest-#{System.unique_integer([:positive])}")

    try do
      File.write!(path, manifest)
      arguments = ["lenses", "--manifest", path | Enum.flat_map(changed, &["--changed", &1])]

      with {:ok, output} <- heph(arguments),
           {:ok, %{"lenses" => lenses} = plan} when is_list(lenses) <- Jason.decode(output) do
        {:ok, plan}
      else
        {:error, message} when is_binary(message) -> {:error, message}
        _ -> {:error, "heph did not say which lenses the change needs"}
      end
    after
      File.rm(path)
    end
  end

  defp heph(arguments) do
    command = Application.get_env(:control_plane, :intake, []) |> Keyword.get(:heph, "heph")

    if Programs.allowed?(command) do
      errors = Path.join(System.tmp_dir!(), "heph-errors-#{System.unique_integer([:positive])}")

      try do
        case System.cmd("/bin/sh", [
               "-c",
               ~s(errors=$1; shift; exec "$@" 2>"$errors"),
               "sh",
               errors,
               command | arguments
             ]) do
          {output, 0} -> {:ok, output}
          {output, _} -> {:error, String.trim(output <> " " <> read(errors))}
        end
      after
        File.rm(errors)
      end
    else
      {:error, Programs.refusal(command)}
    end
  rescue
    error in ErlangError -> {:error, "heph could not be run: #{inspect(error.original)}"}
  end

  defp read(path) do
    case File.read(path) do
      {:ok, text} -> text
      {:error, _} -> ""
    end
  end

  # Each lens's text. A text that is not there is missing; one that could not be read
  # for any other reason may be there, so nothing is asked for until it can be.
  defp lens_texts(lenses, ref) do
    results =
      for %{"pack" => pack} <- lenses do
        endpoint = "repos/#{@athenaeum}/contents/packs/#{pack}/review.md?ref=#{ref}"

        case GitHub.raw(endpoint) do
          {:ok, text} ->
            {:ok, pack, text}

          {:error, message} ->
            if message =~ "404", do: {:missing, pack}, else: {:error, pack, message}
        end
      end

    case Enum.find(results, &match?({:error, _, _}, &1)) do
      {:error, pack, message} ->
        {:error, "could not read the #{pack} lens from #{@athenaeum} at #{ref}: #{message}"}

      nil ->
        {:ok, for({:ok, pack, text} <- results, do: {pack, text}),
         for({:missing, pack} <- results, do: pack)}
    end
  end

  defp asked(workload, pull, number, texts, missing, plan) do
    sha = pull["head"]["sha"]

    {existing, wanted} =
      Enum.split_with(texts, fn {pack, _} -> exists?(workload.repository, number, sha, pack) end)

    results =
      for {pack, text} <- wanted do
        {pack, Workloads.start_review(workload, review(workload, pull, number, pack, text))}
      end

    case Enum.find(results, &match?({_, {:error, _}}, &1)) do
      {pack, {:error, reason}} ->
        {:error, "could not ask for the #{pack} review: #{inspect(reason)}"}

      nil ->
        {:ok,
         %{
           requested: for({pack, _} <- results, do: pack) |> Enum.sort(),
           existing: for({pack, _} <- existing, do: pack) |> Enum.sort(),
           missing: Enum.sort(missing),
           unpacked: plan["unpacked"] || [],
           unowned: plan["unowned"] || []
         }}
    end
  end

  defp review(workload, pull, number, pack, text) do
    workload.workload
    |> Map.put("id", Ecto.UUID.generate())
    |> Map.delete("revision")
    |> put_in(["repository", "baseCommit"], pull["head"]["sha"])
    |> Map.put("review", %{
      "pullRequestNumber" => to_string(number),
      "pullRequestUrl" => "https://github.com/#{workload.repository}/pull/#{number}",
      "lens" => pack,
      "lensDocument" => text
    })
  end

  defp exists?(repository, number, sha, pack) do
    number = to_string(number)

    Repo.exists?(
      from(w in Workload,
        where:
          w.repository == ^repository and
            fragment("?->'review'->>'lens' = ?", w.workload, ^pack) and
            fragment("?->'review'->>'pullRequestNumber' = ?", w.workload, ^number) and
            fragment("?->'repository'->>'baseCommit' = ?", w.workload, ^sha)
      )
    )
  end

  defp settings, do: Application.get_env(:control_plane, :settle) || []
end
