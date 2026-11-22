import Config

# A database of its own, so that a test run never touches anything else on the
# machine's Postgres. CI gives the connection through DATABASE_URL.
config :control_plane, ControlPlane.Repo,
  # MIX_TEST_PARTITION gives a run a database of its own, so that tests run at the same time (two
  # agents, two worktrees) do not delete each other's rows.
  database: "hephaestium_test#{System.get_env("MIX_TEST_PARTITION")}",
  hostname: "localhost",
  pool: Ecto.Adapters.SQL.Sandbox,
  pool_size: 10

# Jobs are run by the tests, one at a time, and not by a queue of their own.
config :control_plane, Oban, testing: :manual, queues: false, plugins: false

# A test that reaches GitHub, or builds a workload with the real `heph`, would act
# on a real repository. By default both are a program that fails; a test that needs
# them stands in for them.
config :control_plane, :intake, gh: "false", heph: "false", repositories: []
config :control_plane, refuse_real_programs: true

# Workloads a stopped control plane left running are failed when one starts; tests make
# their own workloads in every state, so the suite does not.
config :control_plane, recover_on_start: false

config :logger, level: :error
