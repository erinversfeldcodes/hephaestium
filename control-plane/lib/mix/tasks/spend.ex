defmodule Mix.Tasks.Spend do
  @shortdoc "What a ticket, an epic, a branch or the last days spent"

  @moduledoc """
  Print what was spent, from the ledger (ADR 0036), so it can be read without SQL.

      mix spend --ticket 59
      mix spend --epic 71
      mix spend --branch heph/59-apply-the-dependabot-updates
      mix spend --days 7

  `--repo owner/name` says which repository; it is not needed when exactly one is configured
  (`HEPH_REPOSITORIES`). Each unit is on its own: dollars and tokens are never added to each
  other.
  """

  use Mix.Task

  require Logger

  @switches [repo: :string, ticket: :integer, epic: :integer, branch: :string, days: :integer]
  @selectors [:ticket, :epic, :branch, :days]

  @impl Mix.Task
  def run(args) do
    # The configuration first: the repository a person did not name is the one configured.
    Mix.Task.run("app.config")

    {options, _rest, invalid} = OptionParser.parse(args, strict: @switches)

    if invalid != [], do: usage("not understood: #{inspect(invalid)}")

    what = what(options)
    repository = repository(options)
    # A report to read, not the SQL behind it.
    Logger.configure(level: :warning)
    start_repo()

    Mix.shell().info(ControlPlane.Spend.report(repository, what))
  end

  defp what(options) do
    case Enum.filter(@selectors, &Keyword.has_key?(options, &1)) do
      [selector] -> {selector, Keyword.fetch!(options, selector)}
      _ -> usage("give exactly one of --ticket, --epic, --branch or --days")
    end
  end

  defp repository(options) do
    case options[:repo] do
      repository when is_binary(repository) ->
        repository

      nil ->
        case Application.get_env(:control_plane, :intake, [])[:repositories] do
          [only] -> only
          _ -> usage("say which repository with --repo owner/name")
        end
    end
  end

  defp usage(message) do
    Mix.raise("""
    #{message}

    usage: mix spend [--repo owner/name] (--ticket N | --epic N | --branch NAME | --days N)
    """)
  end

  # The database and nothing else: starting the whole application would start intake.
  defp start_repo do
    unless Process.whereis(ControlPlane.Repo) do
      {:ok, _} = Application.ensure_all_started(:postgrex)
      {:ok, _} = Application.ensure_all_started(:ecto_sql)
      {:ok, _} = ControlPlane.Repo.start_link(pool_size: 2)
    end

    :ok
  end
end
