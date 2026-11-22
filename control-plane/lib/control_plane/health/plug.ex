defmodule ControlPlane.Health.Plug do
  @moduledoc """
  The one route the platform's health check calls: `GET /health`, 200 while the database
  answers and 503, with the reason, while it does not. Nothing else is served, and nothing in
  the answer comes from a request.
  """

  @behaviour Plug
  import Plug.Conn

  @impl true
  def init(opts), do: Keyword.get(opts, :repo, ControlPlane.Repo)

  @impl true
  def call(%Plug.Conn{method: "GET", request_path: "/health"} = conn, repo) do
    case ControlPlane.Health.check(repo) do
      :ok -> respond(conn, 200, %{status: "ok"})
      {:error, reason} -> respond(conn, 503, %{status: "unavailable", reason: reason})
    end
  end

  def call(conn, _repo), do: send_resp(conn, 404, "")

  defp respond(conn, status, body) do
    conn |> put_resp_content_type("application/json") |> send_resp(status, Jason.encode!(body))
  end
end
