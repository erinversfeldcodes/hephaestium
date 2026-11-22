defmodule ControlPlane.ReleaseTest do
  @moduledoc """
  What a release does before a new version takes traffic: it brings an empty database to the
  current schema. Run here against a database created for the test, as the release command
  runs it against the hosted one.
  """

  use ExUnit.Case, async: false

  # An unreachable database is logged at error level by the driver, as it should be; the
  # tests that make one do not need it on the screen.
  @moduletag :capture_log

  alias ControlPlane.{Release, ScratchRepo}

  @migrations Path.expand("../../priv/repo/migrations", __DIR__)

  setup do
    suite = ControlPlane.Repo.config()
    database = "hephaestium_release#{System.get_env("MIX_TEST_PARTITION")}"

    config =
      suite
      |> Keyword.take([:hostname, :socket_dir, :username, :password, :port])
      |> Keyword.merge(database: database, pool_size: 2)

    # Migrating twice in one VM loads each migration file twice, which the compiler warns of.
    ignore = Code.get_compiler_option(:ignore_module_conflict)
    Code.put_compiler_option(:ignore_module_conflict, true)

    previous = Application.get_env(:control_plane, ScratchRepo)
    Application.put_env(:control_plane, ScratchRepo, config)
    _ = Ecto.Adapters.Postgres.storage_down(config)
    :ok = Ecto.Adapters.Postgres.storage_up(config)

    on_exit(fn ->
      Code.put_compiler_option(:ignore_module_conflict, ignore)
      System.delete_env("HEPH_MIGRATION_DATABASE_URL")
      Application.put_env(:control_plane, ScratchRepo, previous)
      Ecto.Adapters.Postgres.storage_down(config)
    end)

    %{config: config}
  end

  defp query!(config, sql) do
    {:ok, conn} = Postgrex.start_link(config)
    result = Postgrex.query!(conn, sql, [])
    GenServer.stop(conn)
    result.rows
  end

  test "an empty database comes out with the whole schema", %{config: config} do
    assert :ok = Release.migrate(ScratchRepo)

    tables =
      query!(config, "select tablename from pg_tables where schemaname = 'public'")
      |> List.flatten()

    assert "workloads" in tables
    assert "events" in tables
    assert "oban_jobs" in tables
  end

  test "every migration in the project is recorded as run", %{config: config} do
    :ok = Release.migrate(ScratchRepo)

    [[recorded]] = query!(config, "select count(*) from schema_migrations")
    assert recorded == length(File.ls!(@migrations))
  end

  test "running it again changes nothing and does not fail", %{config: config} do
    :ok = Release.migrate(ScratchRepo)
    [[before]] = query!(config, "select count(*) from schema_migrations")

    assert :ok = Release.migrate(ScratchRepo)
    assert query!(config, "select count(*) from schema_migrations") == [[before]]
  end

  test "a database that cannot be reached fails the release rather than passing it" do
    Application.put_env(:control_plane, ScratchRepo,
      hostname: "127.0.0.1",
      port: 1,
      database: "nowhere",
      username: "nobody",
      pool_size: 1
    )

    assert_raise DBConnection.ConnectionError, fn -> Release.migrate(ScratchRepo) end
  end

  test "the migration URL, when set, is the database it migrates instead of DATABASE_URL's", %{
    config: config
  } do
    user = URI.encode_www_form(config[:username] || System.get_env("USER"))
    password = if config[:password], do: ":" <> URI.encode_www_form(config[:password]), else: ""

    # Pooled for the application, direct for the migration: here the first points nowhere.
    Application.put_env(:control_plane, ScratchRepo,
      url: "ecto://nobody@127.0.0.1:1/nowhere",
      pool_size: 1
    )

    System.put_env(
      "HEPH_MIGRATION_DATABASE_URL",
      "ecto://#{user}#{password}@localhost/#{config[:database]}"
    )

    assert :ok = Release.migrate(ScratchRepo)

    tables =
      query!(config, "select tablename from pg_tables where schemaname = 'public'")
      |> List.flatten()

    assert "workloads" in tables
  end
end
