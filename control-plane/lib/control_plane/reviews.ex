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
         {:ok, plan} <- with_universal(plan, ref),
         {:ok, texts, missing} <- lens_texts(plan["lenses"], ref) do
      asked(workload, pull, number, texts, missing, plan)
    end
  end

  @doc """
  What the verdicts of the reviews at this head commit say, for the lenses `asked`
  names, in this order of what matters:

    * `{:revise, findings}`: a lens that blocks found something that blocks;
    * `{:blocked, reasons}`: a lens that blocks could not review, so there is no
      approval (its run did not end well, it left no verdict, or it has no text);
    * `{:pending, lenses}`: a review is still to end;
    * `{:approved, lines}`: every lens that blocks approved, and the lines say what
      each lens said, and what no lens looked at.

  A lens whose own text says `blocking: false` is advisory: it is waited for and
  reported, and what it says never stops the change. A verdict for another commit is
  never read here, because the reviews are looked up at this one.
  """
  @spec assess(%Workload{}, String.t(), String.t(), result()) ::
          {:revise, [map()]}
          | {:blocked, [{String.t(), String.t()}]}
          | {:pending, [String.t()]}
          | {:approved, [String.t()]}
  def assess(%Workload{} = workload, number, sha, asked) do
    found = reviews_at(workload.repository, number, sha)

    lenses =
      (asked.requested ++ asked.existing)
      |> Enum.uniq()
      |> Enum.sort()
      |> Enum.map(&{&1, lens_state(found[&1], &1, sha)})

    missing = for pack <- asked.missing, do: {pack, {:blocking, {:could_not, "no lens text"}}}
    all = lenses ++ missing

    revise =
      for {pack, {:blocking, {:said, verdict}}} <- all,
          verdict["verdict"] != "approved",
          finding <- verdict["findings"] || [],
          finding["blocking"] == true,
          do: %{lens: pack, finding: finding}

    blocked = for {pack, {:blocking, {:could_not, why}}} <- all, do: {pack, why}
    pending = for {pack, {_, :pending}} <- all, do: pack

    cond do
      revise != [] -> {:revise, revise}
      blocked != [] -> {:blocked, blocked}
      pending != [] -> {:pending, pending}
      true -> {:approved, summary(all, asked)}
    end
  end

  # Whether a lens blocks, and what its review came to.
  defp lens_state(nil, _pack, _sha),
    do: {:blocking, {:could_not, "its review was never asked for"}}

  defp lens_state(%Workload{} = review, pack, sha) do
    kind = if blocks?(review), do: :blocking, else: :advisory

    outcome =
      cond do
        review.state not in Workloads.terminal_states() ->
          :pending

        review.state == "completed" ->
          case review.record do
            %{"review" => %{"verdict" => verdict, "commit" => ^sha, "lens" => ^pack} = said}
            when verdict in ["approved", "needs_revision", "failed"] ->
              {:said, said}

            _ ->
              {:could_not, "it completed with no verdict in its record"}
          end

        true ->
          {:could_not, "its review ended #{review.state}" <> reason(review.reason)}
      end

    {kind, outcome}
  end

  defp reason(reason) when reason in [nil, ""], do: ""
  defp reason(reason), do: ": #{reason}"

  # A lens says in its own front matter whether it blocks. Without it, it does.
  defp blocks?(%Workload{workload: %{"review" => %{"lensDocument" => text}}}) do
    case Regex.run(~r/\A---\n(.*?)\n---/s, text) do
      [_, front] -> not Regex.match?(~r/^blocking:\s*false\s*$/m, front)
      _ -> true
    end
  end

  defp blocks?(_), do: true

  defp summary(all, asked) do
    lines =
      for {pack, {kind, outcome}} <- all do
        "- #{pack}: " <> said(kind, outcome)
      end

    lines ++
      for(
        {text, items} <- [
          {"No lens applied to the components", asked.unpacked},
          {"No component owns the files", asked.unowned}
        ],
        items != [],
        do: "#{text}: #{Enum.join(items, ", ")}."
      )
  end

  defp said(:blocking, {:said, verdict}), do: verdict["verdict"]

  defp said(:advisory, {:said, verdict}),
    do: "#{verdict["verdict"]} (advisory, so it did not block)"

  defp said(kind, {:could_not, why}),
    do:
      "no verdict, #{why}" <>
        if(kind == :advisory, do: " (advisory, so it did not block)", else: "")

  defp said(_kind, :pending), do: "still running"

  # The newest review of each lens at this pull request and head commit.
  defp reviews_at(repository, number, sha) do
    number = to_string(number)

    Repo.all(
      from(w in Workload,
        where:
          w.repository == ^repository and
            fragment("?->'review'->>'pullRequestNumber' = ?", w.workload, ^number) and
            fragment("?->'repository'->>'baseCommit' = ?", w.workload, ^sha),
        order_by: [asc: w.inserted_at]
      )
    )
    |> Map.new(&{&1.workload["review"]["lens"], &1})
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

  # The lenses a component cannot name: a lens says in its own front matter that it
  # `applies: always`, and is then asked for on every change that is reviewed, once, beside
  # the ones the diff selected (#135). A change with nothing selected has no files to
  # review, and asks for nothing. The release's packs are listed, and each lens read, so
  # a listing or a lens that cannot be read means nothing is asked for until it can be.
  defp with_universal(%{"lenses" => []} = plan, _ref), do: {:ok, plan}

  defp with_universal(%{"lenses" => selected} = plan, ref) do
    chosen = for %{"pack" => pack} <- selected, do: pack

    with {:ok, packs} <- pack_names(ref) do
      results =
        for pack <- packs, pack not in chosen do
          endpoint = "repos/#{@athenaeum}/contents/packs/#{pack}/review.md?ref=#{ref}"

          case GitHub.raw(endpoint) do
            {:ok, text} -> if applies_always?(text), do: {:ok, pack}, else: :no
            {:error, message} -> if message =~ "404", do: :no, else: {:error, pack, message}
          end
        end

      case Enum.find(results, &match?({:error, _, _}, &1)) do
        {:error, pack, message} ->
          {:error, "could not read the #{pack} lens from #{@athenaeum} at #{ref}: #{message}"}

        nil ->
          extra = for {:ok, pack} <- results, do: %{"pack" => pack, "components" => []}
          {:ok, %{plan | "lenses" => selected ++ extra}}
      end
    end
  end

  defp pack_names(ref) do
    case GitHub.get("repos/#{@athenaeum}/contents/packs?ref=#{ref}") do
      {:ok, entries} when is_list(entries) ->
        {:ok,
         for(
           %{"type" => "dir", "name" => name} <- entries,
           is_binary(name) and Regex.match?(~r/\A[a-z0-9][a-z0-9-]{0,40}\z/, name),
           do: name
         )}

      {:ok, _} ->
        {:error, "could not list the lenses of #{@athenaeum} at #{ref}: not a list of packs"}

      {:error, message} ->
        {:error, "could not list the lenses of #{@athenaeum} at #{ref}: #{message}"}
    end
  end

  # Only the front matter counts: `applies: always`, with any spaces around it.
  defp applies_always?(text) do
    case Regex.run(~r/\A---\n(.*?)\n---/s, text) do
      [_, front] -> Regex.match?(~r/^applies:[ \t]*always[ \t]*$/m, front)
      _ -> false
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
