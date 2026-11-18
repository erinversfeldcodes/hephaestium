import Config

# A database of its own, so that a test run never touches anything else on the
# machine's Postgres. CI gives the connection through DATABASE_URL.
config :control_plane, ControlPlane.Repo,
  database: "hephaestium_test",
  hostname: "localhost",
  pool: Ecto.Adapters.SQL.Sandbox,
  pool_size: 10

config :logger, level: :warning
