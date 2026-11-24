defmodule ControlPlane.Api.DeviceTokens do
  @moduledoc """
  The routes a signed-in person manages their device tokens with: get a token for CSRF, make a
  token, list one's own, revoke one. Making and revoking write, so they are behind the session
  and the CSRF token, and take no bearer token. A token is shown in the answer to its making
  and is not kept anywhere it can be read from.
  """

  import Plug.Conn

  alias ControlPlane.{DeviceToken, DeviceTokens}
  alias ControlPlane.Api.Csrf

  @max_body 4_096

  def csrf(conn, _params, settings, login),
    do: json(conn, 200, %{csrfToken: Csrf.token(settings, login)})

  def index(conn, _params, _settings, login) do
    tokens =
      for token <- DeviceTokens.list(login), do: view(token, DeviceTokens.recently_seen?(token))

    json(conn, 200, %{tokens: tokens, connected: DeviceTokens.connected?(login)})
  end

  def issue(conn, _params, _settings, login) do
    with {:ok, body, conn} <- read_body(conn, length: @max_body),
         {:ok, %{"name" => name}} <- Jason.decode(body),
         {:ok, %{token: token, record: record}} <- DeviceTokens.issue(login, name) do
      conn
      |> put_resp_header("cache-control", "no-store")
      |> json(201, Map.put(view(record, false), :token, token))
    else
      _ -> json(conn, 400, %{error: "a name of one to 80 characters is needed"})
    end
  end

  def revoke(conn, %{"id" => id}, _settings, login) do
    case DeviceTokens.revoke(login, id) do
      :ok -> json(conn, 200, %{revoked: true})
      :not_found -> json(conn, 404, %{error: "not found"})
    end
  end

  defp view(%DeviceToken{} = token, connected) do
    %{
      id: token.id,
      name: token.name,
      createdAt: token.inserted_at,
      lastSeenAt: token.last_seen_at,
      revokedAt: token.revoked_at,
      connected: connected,
      slots: token.slots,
      running: DeviceTokens.running(token)
    }
  end

  defp json(conn, status, body) do
    conn |> put_resp_content_type("application/json") |> send_resp(status, Jason.encode!(body))
  end
end
