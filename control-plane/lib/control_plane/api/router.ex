defmodule ControlPlane.Api.Router do
  @moduledoc """
  Every route the API has, in one table, and the one place a request is let through.

  A route is either `:public` (the login's three, and nothing else) or `:session`: it runs
  only for a request whose cookie is a good, unexpired session of a login still on the
  allowlist, and any other request, including one for a path that does not exist, is 401. A
  route cannot be reached around the session because there is no other way to dispatch to it.

  The API reads: only GET and HEAD are answered, and every other method is 405 before
  anything else is looked at, which is also what makes cross-site request forgery nothing to
  forge. The exceptions are the routes below whose method is `POST`, and nothing else. They
  exist only while the executor link is on, and are of two kinds that never stand in for each
  other:

    * `:session_write`: a person's own device tokens, and their reply to an agent's question
      (made as them, with the user token their login gave). These need the session cookie and a
      CSRF token, and ignore any bearer token.
    * `:worker`: an executor's. These need a device token as a bearer token, and ignore any
      cookie, so a browser can never be made to act as an executor and a token is no way to
      act as a person.

  A route that changes state must say which of the two it is, or be the webhook route:

    * `:webhook`: a code host's delivery. It needs no session and no token, because the
      signature over the body is its authentication, checked before anything is read from it.
      It exists only while webhooks are on.

  There is no other way in for a write.
  """

  @behaviour Plug

  import Plug.Conn

  alias ControlPlane.Api.{
    Auth,
    Csrf,
    DeviceTokens,
    Endpoints,
    Reply,
    Session,
    Settings,
    Static,
    Webhook,
    Worker
  }

  alias ControlPlane.{ExecutorLink, Webhooks}

  # The first segments the API owns. Under them nothing is a file: a path with no route stays
  # what it was (401 or 404), never the dashboard's page.
  @reserved ~w(api auth worker webhooks)

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
      path: ~w(api workloads :id gates),
      access: :session,
      handler: {Endpoints, :gates}
    },
    %{
      method: "GET",
      path: ~w(api tickets :owner :name :number verdicts),
      access: :session,
      handler: {Endpoints, :verdicts}
    },
    %{
      method: "GET",
      path: ~w(api tickets :owner :name :number review-effort),
      access: :session,
      handler: {Endpoints, :review_effort}
    },
    %{
      method: "GET",
      path: ~w(api workloads :id account),
      access: :session,
      handler: {Endpoints, :account}
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
    %{method: "GET", path: ~w(api questions), access: :session, handler: {Endpoints, :questions}},
    %{
      method: "POST",
      path: ~w(api tickets :owner :name :number reply),
      access: :session_write,
      handler: {Reply, :create}
    },

    # The executor link. Present only while it is on.
    %{
      method: "GET",
      path: ~w(api csrf),
      access: :session,
      handler: {DeviceTokens, :csrf}
    },
    %{
      method: "GET",
      path: ~w(api device-tokens),
      access: :session,
      link: true,
      handler: {DeviceTokens, :index}
    },
    %{
      method: "POST",
      path: ~w(api device-tokens),
      access: :session_write,
      link: true,
      handler: {DeviceTokens, :issue}
    },
    %{
      method: "POST",
      path: ~w(api device-tokens :id revoke),
      access: :session_write,
      link: true,
      handler: {DeviceTokens, :revoke}
    },
    %{
      method: "POST",
      path: ~w(worker claim),
      access: :worker,
      link: true,
      handler: {Worker, :claim}
    },
    %{
      method: "POST",
      path: ~w(worker workloads :id events),
      access: :worker,
      link: true,
      handler: {Worker, :events}
    },
    %{
      method: "POST",
      path: ~w(worker workloads :id heartbeat),
      access: :worker,
      link: true,
      handler: {Worker, :heartbeat}
    },
    %{
      method: "POST",
      path: ~w(worker workloads :id record),
      access: :worker,
      link: true,
      handler: {Worker, :record}
    },
    %{
      method: "POST",
      path: ~w(worker workloads :id release),
      access: :worker,
      link: true,
      handler: {Worker, :release}
    },

    # A code host's webhooks. Present only while they are on.
    %{
      method: "POST",
      path: ~w(webhooks :source),
      access: :webhook,
      webhooks: true,
      handler: {Webhook, :receive}
    }
  ]

  @doc "Every route: its method, its path as segments (`:name` is a parameter), its access."
  @spec routes() :: [map()]
  def routes, do: @routes

  @doc "The routes that change something: the only ones that answer a method other than GET."
  @spec write_routes() :: [map()]
  def write_routes, do: Enum.filter(@routes, &(&1.method != "GET"))

  @impl true
  def init(opts), do: opts

  @impl true
  def call(conn, opts) do
    settings = opts[:settings] || Settings.build!(Application.fetch_env!(:control_plane, :api))
    method = if conn.method == "HEAD", do: "GET", else: conn.method

    case find(method, conn.path_info) do
      nil when method != "GET" ->
        conn
        |> put_resp_header("allow", allowed(conn.path_info))
        |> json(405, %{error: "this API only reads, except where a route says otherwise"})

      nil
      when method == "GET" and is_binary(settings.static_dir) and
             (conn.path_info == [] or hd(conn.path_info) not in @reserved) ->
        Static.serve(conn, settings.static_dir)

      %{access: :public, handler: handler} ->
        dispatch(conn, handler, route_params(method, conn.path_info), settings, nil)

      %{access: :webhook} = route ->
        # The signature, checked by the receiver, and nothing else: no cookie or token is looked at.
        dispatch(conn, route.handler, route_params(method, conn.path_info), settings, nil)

      %{access: :worker} = route ->
        # A device token and nothing else: the cookie is never looked at.
        with {:ok, presented} <- bearer(conn),
             {:ok, token} <- ControlPlane.DeviceTokens.authenticate(presented, settings) do
          dispatch(conn, route.handler, route_params(method, conn.path_info), settings, token)
        else
          _ -> json(conn, 401, %{error: "not authorized"})
        end

      %{access: :session_write} = route ->
        # The session and a CSRF token and nothing else: a bearer token is never looked at.
        with {:ok, login} <- Session.login(conn, settings),
             {:csrf, true} <- {:csrf, Csrf.valid?(conn, settings, login)} do
          dispatch(conn, route.handler, route_params(method, conn.path_info), settings, login)
        else
          {:csrf, false} -> json(conn, 403, %{error: "the CSRF token is missing or not valid"})
          :error -> json(conn, 401, %{error: "not signed in"})
        end

      route ->
        case Session.login(conn, settings) do
          {:ok, login} when route != nil ->
            dispatch(conn, route.handler, route_params(method, conn.path_info), settings, login)

          {:ok, _login} ->
            json(conn, 404, %{error: "not found"})

          :error ->
            json(conn, 401, %{error: "not signed in"})
        end
    end
  end

  defp bearer(conn) do
    case get_req_header(conn, "authorization") do
      ["Bearer " <> token] when token != "" -> {:ok, token}
      _ -> :error
    end
  end

  # The methods a path answers, for the `allow` header: what is served there, and HEAD with GET.
  defp allowed(segments) do
    methods =
      for route <- available(), matches?(route.path, segments), uniq: true, do: route.method

    methods
    |> Enum.flat_map(fn
      "GET" -> ["GET", "HEAD"]
      other -> [other]
    end)
    |> case do
      [] -> ["GET", "HEAD"]
      methods -> methods
    end
    |> Enum.join(", ")
  end

  defp dispatch(conn, {module, function}, params, settings, login),
    do: apply(module, function, [conn, params, settings, login])

  defp find(method, segments),
    do: Enum.find(available(), &(&1.method == method and matches?(&1.path, segments)))

  # The link's routes are there only while the link is on, and the webhook's only while webhooks
  # are; off, the API is as it was without them.
  defp available do
    @routes
    |> then(&if(ExecutorLink.enabled?(), do: &1, else: Enum.reject(&1, fn r -> r[:link] end)))
    |> then(&if(Webhooks.enabled?(), do: &1, else: Enum.reject(&1, fn r -> r[:webhooks] end)))
  end

  defp matches?(pattern, segments) when length(pattern) == length(segments),
    do: Enum.zip(pattern, segments) |> Enum.all?(&segment?/1)

  defp matches?(_, _), do: false

  defp segment?({":" <> _, _}), do: true
  defp segment?({same, same}), do: true
  defp segment?(_), do: false

  # The parameters of the route a request matched.
  defp route_params(method, segments) do
    %{path: pattern} = find(method, segments)

    for {":" <> name, value} <- Enum.zip(pattern, segments), into: %{}, do: {name, value}
  end

  defp json(conn, status, body) do
    conn |> put_resp_content_type("application/json") |> send_resp(status, Jason.encode!(body))
  end
end
