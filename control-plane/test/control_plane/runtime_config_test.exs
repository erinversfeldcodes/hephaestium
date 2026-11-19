defmodule ControlPlane.RuntimeConfigTest do
  @moduledoc """
  How the database connection is chosen at start (#140). The control plane and its tests
  can connect through Postgres's Unix socket, which a script that stops every process with
  a TCP connection to port 5432 does not see.
  """

  use ExUnit.Case, async: false

  @runtime Path.expand("../../config/runtime.exs", __DIR__)

  # The configuration `config/runtime.exs` gives for the Repo with this environment.
  defp repo_config(environment) do
    names = ["HEPH_PG_SOCKET_DIR", "DATABASE_URL"]
    previous = for name <- names, do: {name, System.get_env(name)}

    try do
      for name <- names, do: System.delete_env(name)
      for {name, value} <- environment, do: System.put_env(name, value)

      config = Config.Reader.read!(@runtime, env: :test, target: :host)
      get_in(config, [:control_plane, ControlPlane.Repo]) || []
    after
      for {name, value} <- previous do
        if value, do: System.put_env(name, value), else: System.delete_env(name)
      end
    end
  end

  test "with a socket directory the Repo connects through it" do
    config = repo_config([{"HEPH_PG_SOCKET_DIR", "/tmp"}])

    assert config[:socket_dir] == "/tmp"
  end

  test "without one the Repo's configuration is as it was" do
    refute Keyword.has_key?(repo_config([]), :socket_dir)
  end

  test "an empty setting is no setting" do
    refute Keyword.has_key?(repo_config([{"HEPH_PG_SOCKET_DIR", ""}]), :socket_dir)
  end

  test "a connection URL is still honoured beside it" do
    config =
      repo_config([
        {"HEPH_PG_SOCKET_DIR", "/tmp"},
        {"DATABASE_URL", "ecto://u:p@localhost/hephaestium_test"}
      ])

    assert config[:socket_dir] == "/tmp"
    assert config[:url] == "ecto://u:p@localhost/hephaestium_test"
  end
end
