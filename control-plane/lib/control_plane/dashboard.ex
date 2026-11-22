defmodule ControlPlane.Dashboard do
  @moduledoc """
  Where a person reads what an agent wrote. Both repositories are public, so the ticket and the
  pull request carry a link to the run on the dashboard instead of the agent's words, and only
  when that link can be made: with no public address configured there is nowhere to point, so
  the words stay where they were and nothing is lost.
  """

  alias ControlPlane.Workload

  @doc "The dashboard's public address without a trailing slash, or nil when none is configured."
  @spec url() :: String.t() | nil
  def url do
    case Application.get_env(:control_plane, :dashboard, [])[:url] do
      url when is_binary(url) ->
        case String.trim(url) |> String.trim_trailing("/") do
          "" -> nil
          trimmed -> trimmed
        end

      _ ->
        nil
    end
  end

  @doc "The address of a run on its ticket's page, or nil when the dashboard's address is not configured."
  @spec run_link(%Workload{}) :: String.t() | nil
  def run_link(%Workload{} = workload) do
    case url() do
      nil ->
        nil

      base ->
        "#{base}/tickets/#{workload.repository}/#{workload.ticket_number}#run-#{workload.id}"
    end
  end
end
