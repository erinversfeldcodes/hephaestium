defmodule ControlPlane.Jobs.Intake do
  @moduledoc """
  Run intake for each repository the control plane manages. It runs on a schedule,
  and never overlaps itself.
  """

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 50]

  require Logger

  alias ControlPlane.{Intake, TicketLabels}

  @impl Oban.Worker
  def perform(_job) do
    repositories =
      Application.get_env(:control_plane, :intake, []) |> Keyword.get(:repositories, [])

    for repository <- repositories do
      TicketLabels.ensure_once(repository)

      case Intake.run(repository) do
        {:error, message} ->
          Logger.warning("intake of #{repository}: #{message}")

        results ->
          for %{ticket: ticket, result: result} <- results do
            Logger.info("intake of #{repository} ##{ticket}: #{inspect(result)}")
          end
      end
    end

    :ok
  end
end
