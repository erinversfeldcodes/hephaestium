import Config

# A database of its own, so that a test run never touches anything else on the
# machine's Postgres. CI gives the connection through DATABASE_URL.
config :control_plane, ControlPlane.Repo,
  database: "hephaestium_test",
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

config :logger, level: :error
