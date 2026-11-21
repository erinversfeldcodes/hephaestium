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

  describe "the review App" do
    defp review_app(environment) do
      names = ["HEPH_REVIEW_APP_ID", "HEPH_REVIEW_APP_KEY_FILE"]
      previous = for name <- names, do: {name, System.get_env(name)}

      try do
        for name <- names, do: System.delete_env(name)
        for {name, value} <- environment, do: System.put_env(name, value)

        config = Config.Reader.read!(@runtime, env: :test, target: :host)
        get_in(config, [:control_plane, :review_app])
      after
        for {name, value} <- previous do
          if value, do: System.put_env(name, value), else: System.delete_env(name)
        end
      end
    end

    test "its id and key file are read, and the file's path is made absolute" do
      assert review_app([
               {"HEPH_REVIEW_APP_ID", "5862313"},
               {"HEPH_REVIEW_APP_KEY_FILE", "~/.config/hephaestium/review-app.pem"}
             ]) == [
               app_id: "5862313",
               key_file: Path.join(System.user_home!(), ".config/hephaestium/review-app.pem")
             ]
    end

    test "with nothing set there is no review App" do
      assert review_app([]) == nil
    end

    test "an empty setting is no setting" do
      assert review_app([{"HEPH_REVIEW_APP_ID", " "}, {"HEPH_REVIEW_APP_KEY_FILE", ""}]) == nil
    end

    test "either one alone is kept, and the App is still not enabled by it" do
      assert review_app([{"HEPH_REVIEW_APP_ID", "5862313"}]) == [app_id: "5862313"]
    end
  end
end
