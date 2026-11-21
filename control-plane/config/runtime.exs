import Config

# Where the connection comes from when it is not the default of the environment.
if url = System.get_env("DATABASE_URL") do
  config :control_plane, ControlPlane.Repo, url: url
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

# The review App that approves a pull request on GitHub: its id and the file that holds
# its private key. Both are needed; with either missing nothing is approved. The key is
# never read here, only where it is.
review_app =
  for {name, key} <- [{"HEPH_REVIEW_APP_ID", :app_id}, {"HEPH_REVIEW_APP_KEY_FILE", :key_file}],
      value = System.get_env(name, "") |> String.trim(),
      value != "",
      do: {key, if(key == :key_file, do: Path.expand(value), else: value)}

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
