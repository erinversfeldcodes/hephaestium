import Config

config :control_plane, ecto_repos: [ControlPlane.Repo]

config :control_plane, ControlPlane.Repo,
  migration_primary_key: [type: :uuid],
  migration_timestamps: [type: :utc_datetime_usec]

import_config "#{config_env()}.exs"
