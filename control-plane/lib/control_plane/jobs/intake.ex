defmodule ControlPlane.Jobs.Intake do
  @moduledoc """
  Run intake for each repository the control plane manages. It runs on a schedule,
  and never overlaps itself. With webhooks on it is the backstop for an approval whose
  delivery was dropped, and a ticket it starts is logged as a missed delivery.
  """

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 50]

  require Logger

  alias ControlPlane.{Intake, TicketLabels, Webhooks}

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

          Webhooks.missed(:ticket_approved, Enum.count(results, &acted?(&1.result)))
      end
    end

    :ok
  end

  defp acted?(result), do: result in [:started, :resumed] or match?({:evaluating, _}, result)
end
