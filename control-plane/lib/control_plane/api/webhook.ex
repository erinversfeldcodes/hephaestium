defmodule ControlPlane.Api.Webhook do
  @moduledoc """
  The one route a code host calls. It has no session, no cookie and no bearer token: the
  signature over the body is the only authentication, and a request that does not carry a good
  one is refused before anything is read from it. It answers after one insert and does nothing
  else in the request.
  """

  import Plug.Conn

  alias ControlPlane.Webhooks

  @doc "Take in a delivery from the source the path names."
  def receive(conn, %{"source" => source}, _settings, _login) do
    limit = Webhooks.max_body_bytes()

    # The raw bytes are what was signed, so they are read here and never parsed on the way.
    case read_body(conn, length: limit, read_length: limit) do
      {:ok, body, conn} -> answer(conn, Webhooks.receive(source, conn.req_headers, body))
      {:more, _partial, conn} -> json(conn, 413, %{error: "the body is too large"})
      {:error, _} -> json(conn, 400, %{error: "the body could not be read"})
    end
  end

  defp answer(conn, :accepted), do: json(conn, 202, %{status: "accepted"})
  defp answer(conn, :ignored), do: json(conn, 202, %{status: "ignored"})
  defp answer(conn, :duplicate), do: json(conn, 200, %{status: "duplicate"})
  defp answer(conn, {:error, :not_found}), do: json(conn, 404, %{error: "not found"})
  defp answer(conn, {:error, :unauthorized}), do: json(conn, 401, %{error: "not authorized"})
  defp answer(conn, {:error, :bad_request}), do: json(conn, 400, %{error: "not a delivery"})
  defp answer(conn, {:error, :unavailable}), do: json(conn, 503, %{error: "not available"})
  defp answer(conn, {:error, :failed}), do: json(conn, 500, %{error: "not taken in"})

  defp json(conn, status, body) do
    conn |> put_resp_content_type("application/json") |> send_resp(status, Jason.encode!(body))
  end
end
