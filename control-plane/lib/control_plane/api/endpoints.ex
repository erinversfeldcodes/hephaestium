defmodule ControlPlane.Api.Endpoints do
  @moduledoc """
  The read-only endpoints. Each takes the connection, the path's parameters, and the signed-in
  login, and answers JSON. A parameter that is not what it should be is a 400 or a 404, never
  an error that reaches the person.
  """

  import Plug.Conn

  alias ControlPlane.Api.Read
  alias ControlPlane.{Spend, Workloads}

  @default_page 100
  @keepalive_ms 15_000

  def session(conn, _params, _settings, login), do: json(conn, 200, %{login: login})

  def tickets(conn, _params, _settings, _login) do
    with {:ok, limit} <- int(conn, "limit", 50, 1..200),
         {:ok, offset} <- int(conn, "offset", 0, 0..1_000_000) do
      json(conn, 200, %{tickets: Read.tickets(limit, offset)})
    else
      :error -> bad(conn, "limit and offset must be whole numbers in range")
    end
  end

  def ticket(conn, params, _settings, _login) do
    with {:ok, repository, number} <- ticket_params(params),
         %{} = ticket <- Read.ticket(repository, number) do
      json(conn, 200, ticket)
    else
      _ -> not_found(conn)
    end
  end

  def workload(conn, %{"id" => id}, _settings, _login) do
    case find(id) do
      nil -> not_found(conn)
      workload -> json(conn, 200, Read.workload(workload))
    end
  end

  def events(conn, %{"id" => id}, _settings, _login) do
    with %{} <- find(id),
         {:ok, after_sequence} <- int(conn, "after", 0, 0..1_000_000_000),
         {:ok, limit} <- int(conn, "limit", @default_page, 1..Read.max_page()) do
      json(conn, 200, Read.events(id, after_sequence, limit))
    else
      nil -> not_found(conn)
      :error -> bad(conn, "after and limit must be whole numbers in range")
    end
  end

  def steps(conn, %{"id" => id}, _settings, _login) do
    case find(id) do
      nil -> not_found(conn)
      workload -> json(conn, 200, %{steps: Read.steps(workload)})
    end
  end

  def gates(conn, %{"id" => id}, _settings, _login) do
    case find(id) do
      nil -> not_found(conn)
      workload -> json(conn, 200, %{gates: Read.gates(workload)})
    end
  end

  def verdicts(conn, params, _settings, _login) do
    with {:ok, repository, number} <- ticket_params(params) do
      json(conn, 200, %{verdicts: Read.verdicts(repository, number)})
    else
      _ -> not_found(conn)
    end
  end

  def review_effort(conn, params, _settings, _login) do
    with {:ok, repository, number} <- ticket_params(params) do
      json(conn, 200, Read.review_effort(repository, number))
    else
      _ -> not_found(conn)
    end
  end

  def account(conn, %{"id" => id}, _settings, _login) do
    case find(id) do
      nil -> not_found(conn)
      workload -> json(conn, 200, Read.agent_text(workload))
    end
  end

  def spend_ticket(conn, params, _settings, _login) do
    with {:ok, repository, number} <- ticket_params(params) do
      json(conn, 200, Spend.ticket(repository, number))
    else
      _ -> not_found(conn)
    end
  end

  def spend_epic(conn, params, _settings, _login) do
    with {:ok, repository, number} <- ticket_params(params) do
      json(conn, 200, Spend.epic(repository, number))
    else
      _ -> not_found(conn)
    end
  end

  def spend_branch(conn, _params, _settings, _login) do
    conn = fetch_query_params(conn)

    case {conn.query_params["repository"], conn.query_params["branch"]} do
      {repository, branch} when is_binary(repository) and is_binary(branch) and branch != "" ->
        json(conn, 200, Spend.branch(repository, branch))

      _ ->
        bad(conn, "repository and branch are needed")
    end
  end

  def spend_days(conn, _params, _settings, _login) do
    conn = fetch_query_params(conn)

    with repository when is_binary(repository) and repository != "" <-
           conn.query_params["repository"],
         {:ok, days} <- int(conn, "days", 7, 1..366) do
      json(conn, 200, %{days: Spend.by_day(repository, days)})
    else
      _ -> bad(conn, "repository is needed, and days must be a whole number from 1 to 366")
    end
  end

  def questions(conn, _params, _settings, _login),
    do: json(conn, 200, %{questions: Read.questions()})

  @doc """
  A stream of a workload's events as server-sent events: those after the cursor (the `after`
  parameter, or the `Last-Event-ID` a reconnecting browser sends) and then each event as it is
  appended, in order, until the one that finishes the workload. The database is the only
  source: the stream looks for new events at a short interval, so nothing has to be told that
  an event was kept. It ends when the session stops being good.
  """
  def stream(%Plug.Conn{method: "HEAD"} = conn, _params, _settings, _login),
    do: send_resp(conn, 200, "")

  def stream(conn, %{"id" => id}, settings, _login) do
    with %{} <- find(id),
         {:ok, cursor} <- cursor(conn) do
      conn =
        conn
        |> put_resp_content_type("text/event-stream")
        |> put_resp_header("cache-control", "no-cache")
        |> put_resp_header("x-accel-buffering", "no")
        |> send_chunked(200)

      follow(conn, id, cursor, settings, System.monotonic_time(:millisecond))
    else
      nil -> not_found(conn)
      :error -> bad(conn, "after must be a whole number")
    end
  end

  defp cursor(conn) do
    conn = fetch_query_params(conn)

    case conn.query_params["after"] || List.first(get_req_header(conn, "last-event-id")) do
      nil -> {:ok, 0}
      value -> parse_int(value, 0..1_000_000_000)
    end
  end

  defp follow(conn, id, cursor, settings, last_sent) do
    case Read.events_after(id, cursor, Read.max_page()) do
      [] ->
        Process.sleep(settings.poll_ms)
        idle(conn, id, cursor, settings, last_sent)

      events ->
        send_events(conn, events, id, settings)
    end
  end

  # Nothing new: stop if the session is no longer good, say the connection is alive now and
  # then so a proxy does not close it, and look again.
  defp idle(conn, id, cursor, settings, last_sent) do
    now = System.monotonic_time(:millisecond)

    with {:ok, _login} <- ControlPlane.Api.Session.login(conn, settings),
         {:ok, conn, last_sent} <- keepalive(conn, now, last_sent) do
      follow(conn, id, cursor, settings, last_sent)
    else
      _ -> conn
    end
  end

  defp keepalive(conn, now, last_sent) when now - last_sent >= @keepalive_ms do
    case chunk(conn, ": keepalive\n\n") do
      {:ok, conn} -> {:ok, conn, now}
      {:error, _} -> :closed
    end
  end

  defp keepalive(conn, _now, last_sent), do: {:ok, conn, last_sent}

  defp send_events(conn, [event | rest], id, settings) do
    case chunk(conn, frame(event)) do
      {:ok, conn} when event.kind == "finished" ->
        conn

      {:ok, conn} when rest == [] ->
        follow(conn, id, event.sequence, settings, System.monotonic_time(:millisecond))

      {:ok, conn} ->
        send_events(conn, rest, id, settings)

      {:error, _closed} ->
        conn
    end
  end

  defp frame(event) do
    data = Jason.encode!(Read.event(event))
    "id: #{event.sequence}\nevent: #{event.kind}\ndata: #{data}\n\n"
  end

  defp find(id) do
    case Ecto.UUID.cast(id) do
      {:ok, id} -> Workloads.get(id)
      :error -> nil
    end
  end

  defp ticket_params(%{"owner" => owner, "name" => name, "number" => number}) do
    case parse_int(number, 1..1_000_000_000) do
      {:ok, number} -> {:ok, "#{owner}/#{name}", number}
      :error -> :error
    end
  end

  defp int(conn, name, default, range) do
    conn = fetch_query_params(conn)

    case conn.query_params[name] do
      nil -> {:ok, default}
      value -> parse_int(value, range)
    end
  end

  defp parse_int(value, range) when is_binary(value) do
    case Integer.parse(value) do
      {number, ""} -> if number in range, do: {:ok, number}, else: :error
      _ -> :error
    end
  end

  defp parse_int(_, _), do: :error

  defp json(conn, status, body) do
    conn |> put_resp_content_type("application/json") |> send_resp(status, Jason.encode!(body))
  end

  defp bad(conn, message), do: json(conn, 400, %{error: message})
  defp not_found(conn), do: json(conn, 404, %{error: "not found"})
end
