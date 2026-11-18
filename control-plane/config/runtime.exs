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
