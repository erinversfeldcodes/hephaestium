defmodule ControlPlane.Schedule do
  @moduledoc """
  When the recurring jobs run. Without webhooks the control plane learns by asking, so each job
  that reads the code host runs every one to ten minutes. With them a delivery wakes the job
  that matters, and the same jobs run once an hour as the backstop for what was dropped, each at
  its own minute so they do not arrive together.
  """

  alias ControlPlane.Jobs

  # What reads no code host does not change with webhooks.
  @unchanged [
    {"7 * * * *", Jobs.RecheckOwed},
    {"17 * * * *", Jobs.SweepWorkspaces}
  ]

  @poll [
    {"* * * * *", Jobs.Intake},
    {"* * * * *", Jobs.SettleTickets},
    {"*/10 * * * *", Jobs.EvaluateMerged},
    {"*/10 * * * *", Jobs.RollUpEpics},
    {"*/10 * * * *", Jobs.KeepCurrent},
    {"*/5 * * * *", Jobs.AnswerReplies}
  ]

  @backstop [
    {"2 * * * *", Jobs.Intake},
    {"12 * * * *", Jobs.SettleTickets},
    {"22 * * * *", Jobs.KeepCurrent},
    {"32 * * * *", Jobs.EvaluateMerged},
    {"42 * * * *", Jobs.RollUpEpics},
    {"52 * * * *", Jobs.AnswerReplies},
    {"27 * * * *", Jobs.PruneDeliveries}
  ]

  @doc "The crontab, for whether the control plane takes webhooks."
  @spec crontab(boolean()) :: [{String.t(), module()}]
  def crontab(false), do: @poll ++ @unchanged
  def crontab(true), do: @backstop ++ @unchanged

  @doc "Oban's configuration with its cron plugin set to this schedule; nothing when plugins are off."
  @spec oban(keyword(), boolean()) :: keyword()
  def oban(config, webhooks?) do
    case Keyword.get(config, :plugins) do
      plugins when is_list(plugins) ->
        scheduled = {Oban.Plugins.Cron, crontab: crontab(webhooks?)}

        Keyword.put(
          config,
          :plugins,
          Enum.map(plugins, fn
            Oban.Plugins.Cron -> scheduled
            {Oban.Plugins.Cron, _} -> scheduled
            other -> other
          end)
        )

      _ ->
        config
    end
  end
end
