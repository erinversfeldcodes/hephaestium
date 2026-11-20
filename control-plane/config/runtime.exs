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

runner =
  [
    executor: System.get_env("HEPH_EXECUTOR"),
    runs_dir: System.get_env("HEPH_RUNS_DIR")
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

settle =
  for({"HEPH_RERUNS", count} <- integer.("HEPH_RERUNS"), do: {:reruns, count}) ++
    for({"HEPH_REVISIONS", count} <- integer.("HEPH_REVISIONS"), do: {:revisions, count}) ++
    case System.get_env("HEPH_AUTOMERGE", "")
         |> String.split(",", trim: true)
         |> Enum.map(&String.trim/1) do
      [] -> []
      repositories -> [automerge: repositories]
    end

settle =
  settle ++
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
    case System.get_env("HEPH_ATHENAEUM_REF", "") do
      "" -> []
      ref -> [athenaeum_ref: ref]
    end

if settle != [] do
  config :control_plane, :settle, settle
end
