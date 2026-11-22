import Config

# Where the connection comes from when it is not the default of the environment.
if url = System.get_env("DATABASE_URL") do
  config :control_plane, ControlPlane.Repo, url: url
end

# A release has no development default to fall back on, so it does not start without a database.
if config_env() == :prod and System.get_env("DATABASE_URL", "") == "" do
  raise "DATABASE_URL is not set: a release cannot start without a database"
end

# A setting that is a whole number in a range, or nothing when the variable is not set.
whole_number = fn name, range ->
  case System.get_env(name) do
    value when value in [nil, ""] ->
      nil

    value ->
      case Integer.parse(value) do
        {number, ""} ->
          if number in range,
            do: number,
            else: raise("#{name} must be a whole number from #{range.first} to #{range.last}")

        _ ->
          raise "#{name} must be a whole number from #{range.first} to #{range.last}, not #{inspect(value)}"
      end
  end
end

# Over TLS, checking the server's certificate against the system's roots and its name against the
# host connected to. A hosted database requires it, and a connection that does not check would
# accept anyone's certificate.
if System.get_env("HEPH_DB_SSL") == "true" do
  config :control_plane, ControlPlane.Repo,
    ssl: [
      verify: :verify_peer,
      cacerts: :public_key.cacerts_get(),
      customize_hostname_check: [match_fun: :public_key.pkix_verify_hostname_match_fun(:https)]
    ]
end

if pool_size = whole_number.("HEPH_DB_POOL_SIZE", 1..1000) do
  config :control_plane, ControlPlane.Repo, pool_size: pool_size
end

# The listener the platform's health check calls. Off unless a port is named.
if port = whole_number.("HEPH_HEALTH_PORT", 1..65535) do
  config :control_plane, :health, port: port
end

# The HTTP API behind a GitHub login. Off unless a port is named. A secret is read from its
# variable, or from the file a `_FILE` variable names (a platform's mounted secret); it is never
# printed, and a message about it names the variable only.
secret = fn name ->
  case {System.get_env(name, ""), System.get_env(name <> "_FILE", "")} do
    {"", ""} -> nil
    {value, ""} -> String.trim(value)
    {_, path} -> path |> File.read!() |> String.trim()
  end
end

if port = whole_number.("HEPH_API_PORT", 1..65535) do
  secret_key_base = secret.("HEPH_API_SECRET_KEY_BASE")

  if secret_key_base in [nil, ""] do
    raise "HEPH_API_SECRET_KEY_BASE is not set: the API will not start without a key to sign sessions with"
  end

  config :control_plane, :api,
    port: port,
    secret_key_base: secret_key_base,
    client_id: secret.("HEPH_API_GITHUB_CLIENT_ID"),
    client_secret: secret.("HEPH_API_GITHUB_CLIENT_SECRET"),
    base_url: System.get_env("HEPH_API_BASE_URL", ""),
    allowed_logins:
      "HEPH_API_ALLOWED_LOGINS"
      |> System.get_env("")
      |> String.split(",", trim: true)
      |> Enum.map(&String.trim/1)
end

# How Oban's nodes tell each other that work arrived. By default the database does, with
# LISTEN/NOTIFY, which a pooled connection in transaction mode cannot carry; `pg` uses Erlang's
# process groups instead and sends nothing through the database.
case System.get_env("HEPH_OBAN_NOTIFIER") do
  value when value in [nil, "", "postgres"] -> :ok
  "pg" -> config :control_plane, Oban, notifier: Oban.Notifiers.PG
  value -> raise "HEPH_OBAN_NOTIFIER must be pg or postgres, not #{inspect(value)}"
end

# Connect through Postgres's Unix socket in this directory instead of TCP (#140). Another
# project's script on the same machine stops every BEAM that holds a TCP connection to port
# 5432, whichever project it belongs to; a socket connection is not one.
case System.get_env("HEPH_PG_SOCKET_DIR") do
  dir when dir in [nil, ""] -> :ok
  dir -> config :control_plane, ControlPlane.Repo, socket_dir: dir
end

# How long a run that did not complete keeps its checkout, in whole hours.
keep_hours =
  case System.get_env("HEPH_WORKSPACE_KEEP_HOURS") do
    value when value in [nil, ""] ->
      nil

    value ->
      case Integer.parse(value) do
        {hours, ""} when hours >= 0 ->
          hours

        _ ->
          raise "HEPH_WORKSPACE_KEEP_HOURS must be a whole number of hours, not #{inspect(value)}"
      end
  end

# The free space, in megabytes, below which a run is not started. 0 turns the check off.
min_free_mb =
  case System.get_env("HEPH_MIN_FREE_MB") do
    value when value in [nil, ""] ->
      nil

    value ->
      case Integer.parse(value) do
        {megabytes, ""} when megabytes >= 0 ->
          megabytes

        _ ->
          raise "HEPH_MIN_FREE_MB must be a whole number of megabytes, not #{inspect(value)}"
      end
  end

runner =
  [
    executor: System.get_env("HEPH_EXECUTOR"),
    runs_dir: System.get_env("HEPH_RUNS_DIR"),
    workspace_keep_hours: keep_hours,
    min_free_mb: min_free_mb
  ]
  |> Enum.reject(fn {_key, value} -> value in [nil, ""] end)

if runner != [] do
  config :control_plane, :runner, runner
end

intake =
  [
    repositories:
      "HEPH_REPOSITORIES"
      |> System.get_env("")
      |> String.split(",", trim: true)
      |> Enum.map(&String.trim/1),
    heph: System.get_env("HEPH_BIN"),
    gh: System.get_env("HEPH_GH")
  ]
  |> Enum.reject(fn {_key, value} -> value in [nil, "", []] end)

if intake != [] do
  config :control_plane, :intake, intake
end

# Settling a ticket: how many times a cancelled check is re-run, how many
# revisions a failing pull request gets, and the repositories whose green pull
# requests are merged (none of any unless set).
integer = fn name ->
  case System.get_env(name) do
    value when value in [nil, ""] -> []
    value -> [{name, String.to_integer(value)}]
  end
end

# The review App that approves a pull request on GitHub: its id and its private key, which is
# in a file (the laptop's way) or in a variable (a platform's secret store). With the id or the
# key missing nothing is approved. A key file is never read here, only where it is.
review_app =
  for {name, key} <- [
        {"HEPH_REVIEW_APP_ID", :app_id},
        {"HEPH_REVIEW_APP_KEY", :key},
        {"HEPH_REVIEW_APP_KEY_FILE", :key_file}
      ],
      value = System.get_env(name, "") |> String.trim(),
      value != "",
      do:
        {key,
         case key do
           :key_file -> Path.expand(value)
           # The key's own lines are what is signed with; only the ends of the value are trimmed.
           :key -> String.trim_trailing(System.get_env(name)) <> "\n"
           _ -> value
         end}

if review_app != [], do: config(:control_plane, :review_app, review_app)

settle =
  for({"HEPH_RERUNS", count} <- integer.("HEPH_RERUNS"), do: {:reruns, count}) ++
    for({"HEPH_REVISIONS", count} <- integer.("HEPH_REVISIONS"), do: {:revisions, count}) ++
    case System.get_env("HEPH_AUTOMERGE", "")
         |> String.split(",", trim: true)
         |> Enum.map(&String.trim/1) do
      [] -> []
      repositories -> [automerge: repositories]
    end

# How many rounds a run reads its own change under the lenses it needs before it pushes. Zero
# reads nothing; not set, it is two.
prepush_rounds =
  case System.get_env("HEPH_PREPUSH_ROUNDS") do
    value when value in [nil, ""] ->
      []

    value ->
      case Integer.parse(value) do
        {rounds, ""} when rounds >= 0 ->
          [prepush_rounds: rounds]

        _ ->
          raise "HEPH_PREPUSH_ROUNDS must be a whole number of rounds, not #{inspect(value)}"
      end
  end

settle =
  settle ++
    prepush_rounds ++
    case System.get_env("HEPH_REVIEWS", "")
         |> String.split(",", trim: true)
         |> Enum.map(&String.trim/1) do
      [] -> []
      repositories -> [reviews: repositories]
    end ++
    case System.get_env("HEPH_COMPLETENESS", "")
         |> String.split(",", trim: true)
         |> Enum.map(&String.trim/1) do
      [] -> []
      repositories -> [completeness: repositories]
    end ++
    case System.get_env("HEPH_CLOSURES", "")
         |> String.split(",", trim: true)
         |> Enum.map(&String.trim/1) do
      [] -> []
      repositories -> [closures: repositories]
    end ++
    case Integer.parse(System.get_env("HEPH_COMPLETENESS_INTERVAL_HOURS", "")) do
      {hours, ""} when hours > 0 -> [completeness_interval_hours: hours]
      _ -> []
    end ++
    case Integer.parse(System.get_env("HEPH_COMPLETENESS_RECHECKS", "")) do
      {times, ""} when times >= 0 -> [completeness_rechecks: times]
      _ -> []
    end ++
    case System.get_env("HEPH_ATHENAEUM_REF", "") do
      "" -> []
      ref -> [athenaeum_ref: ref]
    end

if settle != [] do
  config :control_plane, :settle, settle
end
