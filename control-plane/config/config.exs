import Config

config :control_plane, ecto_repos: [ControlPlane.Repo]

config :control_plane, ControlPlane.Repo,
  migration_primary_key: [type: :uuid],
  migration_timestamps: [type: :utc_datetime_usec]

config :control_plane, Oban,
  repo: ControlPlane.Repo,
  queues: [workloads: 5, intake: 1],
  plugins: [
    {Oban.Plugins.Cron,
     crontab: [
       {"* * * * *", ControlPlane.Jobs.Intake},
       {"* * * * *", ControlPlane.Jobs.SettleTickets},
       {"7 * * * *", ControlPlane.Jobs.RecheckOwed},
       {"*/10 * * * *", ControlPlane.Jobs.EvaluateMerged},
       {"*/10 * * * *", ControlPlane.Jobs.RollUpEpics},
       {"*/10 * * * *", ControlPlane.Jobs.KeepCurrent},
       {"*/5 * * * *", ControlPlane.Jobs.AnswerReplies}
     ]}
  ]

# The repositories whose tickets are taken in, and the programs intake reads
# GitHub and builds workloads with: `gh` and `heph` on the path unless this says.
config :control_plane, :intake, repositories: []

# How the executor is run. The executor is the program on the path unless this
# says which; the runs directory is where each run keeps its files.
config :control_plane, :runner,
  silence_limit_ms: 120_000,
  kill_grace_ms: 10_000

import_config "#{config_env()}.exs"
