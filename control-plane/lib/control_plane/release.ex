defmodule ControlPlane.Release do
  @moduledoc """
  What a release runs before a new version takes traffic: the migrations, run from the
  release itself because a release has no Mix and so no `mix ecto.migrate`.

  The platform's release command is `bin/migrate`, which calls `migrate/0`. A migration that
  fails raises, the command exits non-zero, and the deploy stops with the old version still
  running.
  """

  @app :control_plane

  @doc """
  Brings the database of each repo to the current schema, running every migration that has
  not run. Safe to run again: a database that is current is left as it is.
  """
  @spec migrate(module() | [module()]) :: :ok
  def migrate(repos \\ repos())

  def migrate(repos) when is_list(repos) do
    Enum.each(repos, &migrate/1)
  end

  def migrate(repo) when is_atom(repo) do
    _ = Application.load(@app)
    use_migration_url(repo)

    {:ok, _, _} =
      Ecto.Migrator.with_repo(repo, fn repo ->
        Ecto.Migrator.run(repo, migrations_path(), :up, all: true)
      end)

    :ok
  end

  # The application may connect through a pooler, which lends a server connection for one
  # transaction at a time. A migration holds a session-level advisory lock for its whole run,
  # so it connects to the database directly when a URL for that is given.
  defp use_migration_url(repo) do
    case System.get_env("HEPH_MIGRATION_DATABASE_URL", "") do
      "" ->
        :ok

      url ->
        Application.put_env(
          @app,
          repo,
          Keyword.put(Application.get_env(@app, repo, []), :url, url)
        )
    end
  end

  defp repos, do: Application.fetch_env!(@app, :ecto_repos)

  defp migrations_path, do: Application.app_dir(@app, "priv/repo/migrations")
end
