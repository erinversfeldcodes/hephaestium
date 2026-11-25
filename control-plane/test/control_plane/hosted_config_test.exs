defmodule ControlPlane.HostedConfigTest do
  @moduledoc """
  The settings a hosted control plane is started with, read from environment variables by
  `config/runtime.exs`. Each is off unless set, so a control plane started on a laptop with
  none of them behaves as it did before they existed.
  """

  use ExUnit.Case, async: false

  @runtime Path.expand("../../config/runtime.exs", __DIR__)
  @names ~w(DATABASE_URL HEPH_PG_SOCKET_DIR HEPH_DB_SSL HEPH_DB_POOL_SIZE HEPH_OBAN_NOTIFIER
            HEPH_HEALTH_PORT HEPH_RECURRING_JOBS HEPH_REVIEW_APP_ID HEPH_REVIEW_APP_KEY HEPH_REVIEW_APP_KEY_FILE
            HEPH_API_PORT HEPH_EXECUTOR_LINK HEPH_API_SECRET_KEY_BASE HEPH_API_SECRET_KEY_BASE_FILE
            HEPH_API_GITHUB_CLIENT_ID HEPH_API_GITHUB_CLIENT_SECRET
            HEPH_API_GITHUB_CLIENT_SECRET_FILE HEPH_API_ALLOWED_LOGINS HEPH_API_BASE_URL)

  defp read(environment, env \\ :test) do
    previous = for name <- @names, do: {name, System.get_env(name)}

    try do
      for name <- @names, do: System.delete_env(name)
      for {name, value} <- environment, do: System.put_env(name, value)
      Config.Reader.read!(@runtime, env: env, target: :host)[:control_plane] || []
    after
      for {name, value} <- previous do
        if value, do: System.put_env(name, value), else: System.delete_env(name)
      end
    end
  end

  describe "the health listener" do
    test "is off unless a port is named" do
      refute Keyword.has_key?(read([]), :health)
      refute Keyword.has_key?(read([{"HEPH_HEALTH_PORT", ""}]), :health)
    end

    test "listens on the port that is named" do
      assert read([{"HEPH_HEALTH_PORT", "8080"}])[:health] == [port: 8080]
    end

    test "a port that is not a number stops the start, naming the setting" do
      for bad <- ["http", "-1", "70000", "80.5"] do
        error = assert_raise RuntimeError, fn -> read([{"HEPH_HEALTH_PORT", bad}]) end
        assert error.message =~ "HEPH_HEALTH_PORT"
      end
    end
  end

  describe "the API" do
    @api [
      {"HEPH_API_PORT", "4000"},
      {"HEPH_API_SECRET_KEY_BASE", String.duplicate("k", 64)},
      {"HEPH_API_GITHUB_CLIENT_ID", "the-id"},
      {"HEPH_API_GITHUB_CLIENT_SECRET", "the-secret"},
      {"HEPH_API_ALLOWED_LOGINS", "octocat, Hubot"},
      {"HEPH_API_BASE_URL", "https://heph.example"}
    ]

    test "is off unless a port is named" do
      refute Keyword.has_key?(read([]), :api)
      refute Keyword.has_key?(read(List.keydelete(@api, "HEPH_API_PORT", 0)), :api)
    end

    test "reads its settings, the allowlist as a list of logins" do
      api = read(@api)[:api]
      assert api[:port] == 4000
      assert api[:allowed_logins] == ["octocat", "Hubot"]
      assert api[:base_url] == "https://heph.example"
      assert api[:client_secret] == "the-secret"
    end

    test "reads a secret from the file a _FILE variable names" do
      path = Path.join(System.tmp_dir!(), "heph-api-secret-#{System.unique_integer([:positive])}")
      File.write!(path, "from-a-file\n")
      on_exit(fn -> File.rm(path) end)

      environment =
        List.keydelete(@api, "HEPH_API_GITHUB_CLIENT_SECRET", 0) ++
          [{"HEPH_API_GITHUB_CLIENT_SECRET_FILE", path}]

      assert read(environment)[:api][:client_secret] == "from-a-file"
    end

    test "does not start without its key, and the message does not carry a secret" do
      environment = List.keydelete(@api, "HEPH_API_SECRET_KEY_BASE", 0)
      error = assert_raise RuntimeError, fn -> read(environment) end
      assert error.message =~ "HEPH_API_SECRET_KEY_BASE"
      refute error.message =~ "the-secret"
    end
  end

  describe "Oban's notifier" do
    test "is Oban's own default unless set" do
      refute Keyword.has_key?(read([])[Oban] || [], :notifier)
      refute Keyword.has_key?(read([{"HEPH_OBAN_NOTIFIER", "postgres"}])[Oban] || [], :notifier)
    end

    test "pg keeps notifications off the database connection" do
      assert read([{"HEPH_OBAN_NOTIFIER", "pg"}])[Oban][:notifier] == Oban.Notifiers.PG
    end

    test "anything else stops the start, naming the setting" do
      error = assert_raise RuntimeError, fn -> read([{"HEPH_OBAN_NOTIFIER", "redis"}]) end
      assert error.message =~ "HEPH_OBAN_NOTIFIER"
    end
  end

  describe "the database connection" do
    test "uses TLS with the system's roots and a hostname check when asked" do
      ssl = read([{"HEPH_DB_SSL", "true"}])[ControlPlane.Repo][:ssl]

      assert ssl[:verify] == :verify_peer
      assert is_list(ssl[:cacerts]) and ssl[:cacerts] != []
      assert Keyword.has_key?(ssl, :customize_hostname_check)
    end

    test "is not TLS unless asked" do
      refute Keyword.has_key?(read([])[ControlPlane.Repo] || [], :ssl)
      refute Keyword.has_key?(read([{"HEPH_DB_SSL", "false"}])[ControlPlane.Repo] || [], :ssl)
    end

    test "has the pool size that is named" do
      assert read([{"HEPH_DB_POOL_SIZE", "12"}])[ControlPlane.Repo][:pool_size] == 12
    end

    test "a pool size that is not a whole number stops the start, naming the setting" do
      error = assert_raise RuntimeError, fn -> read([{"HEPH_DB_POOL_SIZE", "many"}]) end
      assert error.message =~ "HEPH_DB_POOL_SIZE"
    end

    test "a release with no database URL does not start" do
      error = assert_raise RuntimeError, fn -> read([], :prod) end
      assert error.message =~ "DATABASE_URL"
    end

    test "a release with one starts" do
      config = read([{"DATABASE_URL", "ecto://u:p@db.example.com/heph"}], :prod)
      assert config[ControlPlane.Repo][:url] == "ecto://u:p@db.example.com/heph"
    end
  end

  describe "the review App's key" do
    test "can be the key itself, for a platform that keeps secrets as variables" do
      pem = "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\n"

      assert read([{"HEPH_REVIEW_APP_ID", "5862313"}, {"HEPH_REVIEW_APP_KEY", pem}])[
               :review_app
             ] == [app_id: "5862313", key: pem]
    end

    test "the file path still works beside it" do
      config =
        read([{"HEPH_REVIEW_APP_ID", "1"}, {"HEPH_REVIEW_APP_KEY_FILE", "/run/secrets/app.pem"}])

      assert config[:review_app] == [app_id: "1", key_file: "/run/secrets/app.pem"]
    end
  end

  describe "recurring jobs" do
    @preview Path.expand("../../fly.preview.toml", __DIR__)

    test "are left as configured unless they are turned off" do
      refute Keyword.has_key?(read([]), Oban)
      refute Keyword.has_key?(read([{"HEPH_RECURRING_JOBS", "on"}]), Oban)
    end

    test "off leaves Oban no cron plugin, no staging poll and no leader election" do
      oban = read([{"HEPH_RECURRING_JOBS", "off"}])[Oban]
      assert oban[:plugins] == []
      assert oban[:stage_interval] == :infinity
      assert oban[:peer] == false

      scheduled = ControlPlane.Schedule.oban([repo: ControlPlane.Repo] ++ oban, false)
      assert scheduled[:plugins] == []
    end

    test "off also stops the lapsed-lease sweep, which polls the database on a timer" do
      link = read([{"HEPH_RECURRING_JOBS", "off"}])[:executor_link]
      assert link[:reap] == false

      api = [
        {"HEPH_API_PORT", "4000"},
        {"HEPH_API_SECRET_KEY_BASE", String.duplicate("k", 64)},
        {"HEPH_API_GITHUB_CLIENT_ID", "the-id"},
        {"HEPH_API_GITHUB_CLIENT_SECRET", "the-secret"},
        {"HEPH_API_ALLOWED_LOGINS", "octocat"},
        {"HEPH_API_BASE_URL", "https://heph.example"},
        {"HEPH_EXECUTOR_LINK", "workers"}
      ]

      on = read(api)[:executor_link]
      assert on[:enabled] == true
      refute Keyword.has_key?(on, :reap)

      preview = [{"HEPH_RECURRING_JOBS", "off"} | api]
      link = read(preview)[:executor_link]
      assert link[:enabled] == true
      assert link[:reap] == false
    end

    test "a value that is neither on nor off stops the start" do
      error = assert_raise RuntimeError, fn -> read([{"HEPH_RECURRING_JOBS", "never"}]) end
      assert error.message =~ "HEPH_RECURRING_JOBS"
    end

    test "a preview's configuration turns them off, and drops the check that queries the database" do
      config = File.read!(@preview)
      assert config =~ ~r/^\s*HEPH_RECURRING_JOBS\s*=\s*"off"/m
      refute config =~ ~r/^\s*\[checks/m, "a periodic database check keeps Neon's compute awake"
      refute config =~ "/health"
      assert config =~ ~r/auto_stop_machines\s*=\s*"stop"/
      assert config =~ ~r/auto_start_machines\s*=\s*true/
      assert config =~ ~r/min_machines_running\s*=\s*0/
    end
  end
end
