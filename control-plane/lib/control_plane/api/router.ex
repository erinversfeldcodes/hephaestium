defmodule ControlPlane.Api.Router do
  @moduledoc """
  Every route the API has, in one table, and the one place a request is let through.

  A route is either `:public` (the login's three, and nothing else) or `:session`: it runs
  only for a request whose cookie is a good, unexpired session of a login still on the
  allowlist, and any other request, including one for a path that does not exist, is 401. A
  route cannot be reached around the session because there is no other way to dispatch to it.

  Only GET and HEAD are answered. Nothing the API serves writes, so every other method is 405
  before anything else is looked at, which is also what makes cross-site request forgery
  nothing to forge: there is no state-changing request to send. A route that changes state
  must add a CSRF check when it is added.
  """

  @behaviour Plug

  import Plug.Conn

  alias ControlPlane.Api.{Auth, Endpoints, Session, Settings}

  @routes [
    %{method: "GET", path: ~w(auth login), access: :public, handler: {Auth, :login}},
    %{method: "GET", path: ~w(auth callback), access: :public, handler: {Auth, :callback}},
    %{method: "GET", path: ~w(auth logout), access: :public, handler: {Auth, :logout}},
    %{method: "GET", path: ~w(api session), access: :session, handler: {Endpoints, :session}},
    %{method: "GET", path: ~w(api tickets), access: :session, handler: {Endpoints, :tickets}},
    %{
      method: "GET",
      path: ~w(api tickets :owner :name :number),
      access: :session,
      handler: {Endpoints, :ticket}
    },
    %{
      method: "GET",
      path: ~w(api workloads :id),
      access: :session,
      handler: {Endpoints, :workload}
    },
    %{
      method: "GET",
      path: ~w(api workloads :id events),
      access: :session,
      handler: {Endpoints, :events}
    },
    %{
      method: "GET",
      path: ~w(api workloads :id events stream),
      access: :session,
      handler: {Endpoints, :stream}
    },
    %{
      method: "GET",
      path: ~w(api workloads :id steps),
      access: :session,
      handler: {Endpoints, :steps}
    },
    %{
      method: "GET",
      path: ~w(api spend tickets :owner :name :number),
      access: :session,
      handler: {Endpoints, :spend_ticket}
    },
    %{
      method: "GET",
      path: ~w(api spend epics :owner :name :number),
      access: :session,
      handler: {Endpoints, :spend_epic}
    },
    %{
      method: "GET",
      path: ~w(api spend branch),
      access: :session,
      handler: {Endpoints, :spend_branch}
    },
    %{
      method: "GET",
      path: ~w(api spend days),
      access: :session,
      handler: {Endpoints, :spend_days}
    },
    %{method: "GET", path: ~w(api questions), access: :session, handler: {Endpoints, :questions}}
  ]

  @doc "Every route: its method, its path as segments (`:name` is a parameter), its access."
  @spec routes() :: [map()]
  def routes, do: @routes

  @impl true
  def init(opts), do: opts

  @impl true
  def call(%Plug.Conn{method: method} = conn, _opts) when method not in ["GET", "HEAD"] do
    conn
    |> put_resp_header("allow", "GET, HEAD")
    |> json(405, %{error: "this API only reads"})
  end

  def call(conn, opts) do
    settings = opts[:settings] || Settings.build!(Application.fetch_env!(:control_plane, :api))

    case find(conn.path_info) do
      %{access: :public, handler: handler} = _route ->
        dispatch(conn, handler, route_params(conn.path_info), settings, nil)

      route ->
        case Session.login(conn, settings) do
          {:ok, login} when route != nil ->
            dispatch(conn, route.handler, route_params(conn.path_info), settings, login)

          {:ok, _login} ->
            json(conn, 404, %{error: "not found"})

          :error ->
            json(conn, 401, %{error: "not signed in"})
        end
    end
  end

  defp dispatch(conn, {module, function}, params, settings, login),
    do: apply(module, function, [conn, params, settings, login])

  defp find(segments), do: Enum.find(@routes, &matches?(&1.path, segments))

  defp matches?(pattern, segments) when length(pattern) == length(segments),
    do: Enum.zip(pattern, segments) |> Enum.all?(&segment?/1)

  defp matches?(_, _), do: false

  defp segment?({":" <> _, _}), do: true
  defp segment?({same, same}), do: true
  defp segment?(_), do: false

  # The parameters of the route a request matched.
  defp route_params(segments) do
    %{path: pattern} = find(segments)

    for {":" <> name, value} <- Enum.zip(pattern, segments), into: %{}, do: {name, value}
  end

  defp json(conn, status, body) do
    conn |> put_resp_content_type("application/json") |> send_resp(status, Jason.encode!(body))
  end
end
