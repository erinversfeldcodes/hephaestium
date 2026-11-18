import Config

# Where the connection comes from when it is not the default of the environment.
if url = System.get_env("DATABASE_URL") do
  config :control_plane, ControlPlane.Repo, url: url
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

# Settling a ticket: how many times a cancelled check is re-run, and how many
# revisions a failing pull request gets (none of either unless set).
settle =
  [reruns: System.get_env("HEPH_RERUNS"), revisions: System.get_env("HEPH_REVISIONS")]
  |> Enum.reject(fn {_key, value} -> value in [nil, ""] end)
  |> Enum.map(fn {key, value} -> {key, String.to_integer(value)} end)

if settle != [] do
  config :control_plane, :settle, settle
end
