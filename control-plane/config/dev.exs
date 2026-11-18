import Config

config :control_plane, ControlPlane.Repo,
  database: "hephaestium_dev",
  hostname: "localhost",
  pool_size: 5
