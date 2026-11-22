defmodule ControlPlane.Prepush do
  @moduledoc """
  What a run is given so that it reads its own change before it pushes: the lenses of the
  pinned Athenaeum release, with their texts, and how many rounds to read them in (ADR 0035).

  The executor holds no GitHub token until the push, and which lenses a change needs depends
  on what the agent changed, so the whole library travels with the workload and the executor
  chooses from it. The release is pinned (`HEPH_ATHENAEUM_REF`) and so does not change under
  a ref: the library is read once for it, and kept. A library that could not be read is not
  kept, and no workload is built without it, as no review is asked for when a lens cannot be
  read: a run that was meant to read its change and quietly did not is worse than one that
  waits.

  Only a first run and a revision are given them. A review of the pushed commit is a read of
  its own and never is, so that nothing a first round produced reaches it.
  """

  alias ControlPlane.{CodeHost, Reviews}

  @athenaeum "erinversfeldcodes/athenaeum"
  @default_rounds 2

  @doc """
  Give the workload the lenses and the rounds, for a repository whose reviews are enabled
  and unless the rounds are set to 0. Any other workload is returned as it was.
  """
  @spec add(map(), String.t()) :: {:ok, map()} | {:error, String.t()}
  def add(workload, repository) do
    rounds = rounds()

    if Reviews.enabled?(repository) and rounds > 0 do
      with {:ok, library} <- library() do
        {:ok, workload |> Map.put("lenses", library) |> Map.put("prepushRounds", rounds)}
      end
    else
      {:ok, workload}
    end
  end

  @doc "How many rounds a run reads in: `HEPH_PREPUSH_ROUNDS`, or two. Zero reads nothing."
  @spec rounds() :: non_neg_integer()
  def rounds, do: settings()[:prepush_rounds] || @default_rounds

  @doc "Forget the libraries that were kept."
  @spec forget() :: :ok
  def forget do
    for {{__MODULE__, _ref} = key, _} <- :persistent_term.get(), do: :persistent_term.erase(key)
    :ok
  end

  defp library do
    case settings()[:athenaeum_ref] do
      ref when is_binary(ref) and ref != "" ->
        kept(ref)

      _ ->
        {:error, "HEPH_ATHENAEUM_REF is not set, so there are no lenses to read the change under"}
    end
  end

  defp kept(ref) do
    case :persistent_term.get({__MODULE__, ref}, nil) do
      nil ->
        with {:ok, library} <- read(ref) do
          :persistent_term.put({__MODULE__, ref}, library)
          {:ok, library}
        end

      library ->
        {:ok, library}
    end
  end

  # Every pack of the release that has a lens. A pack with none (a 404) is not a lens; a lens
  # that cannot be read for any other reason means the library cannot be.
  defp read(ref) do
    with {:ok, packs} <- Reviews.pack_names(ref) do
      results =
        for pack <- packs do
          case CodeHost.file(@athenaeum, "packs/#{pack}/review.md", ref) do
            {:ok, text} -> {:ok, %{"pack" => pack, "document" => text}}
            {:error, message} -> if message =~ "404", do: :none, else: {:error, pack, message}
          end
        end

      case Enum.find(results, &match?({:error, _, _}, &1)) do
        {:error, pack, message} ->
          {:error, "could not read the #{pack} lens from #{@athenaeum} at #{ref}: #{message}"}

        nil ->
          {:ok, for({:ok, lens} <- results, do: lens)}
      end
    end
  end

  defp settings, do: Application.get_env(:control_plane, :settle) || []
end
