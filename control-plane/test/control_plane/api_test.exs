defmodule ControlPlane.ApiTest do
  @moduledoc """
  The control plane's HTTP surface: a login with GitHub, and read-only endpoints behind the
  session it makes. The tests call the router as a plug, and the stream test runs a real
  listener. GitHub itself is a stand-in: no test here reaches github.com.
  """

  use ControlPlane.DataCase, async: false

  import Plug.Test
  import Plug.Conn

  alias ControlPlane.Api.{Router, Session, Settings, UsedStates}
  alias ControlPlane.{Events, Fixtures, StandInGitHubLogin, Workloads}

  @secret String.duplicate("k", 64)

  defp settings(overrides \\ []) do
    [
      secret_key_base: @secret,
      client_id: "the-client-id",
      client_secret: "the-client-secret",
      base_url: "https://heph.test",
      allowed_logins: ["octocat"],
      github: StandInGitHubLogin,
      session_ttl: 3600,
      poll_ms: 20
    ]
    |> Keyword.merge(overrides)
    |> Settings.build!()
  end

  setup do
    start_supervised!(UsedStates)
    StandInGitHubLogin.accept(%{"good" => "octocat", "stranger" => "mallory"})
    on_exit(fn -> Application.delete_env(:control_plane, :stand_in_logins) end)
    :ok
  end

  defp request(method, path, opts \\ []) do
    settings = opts[:settings] || settings()

    conn(method, path)
    |> then(fn conn ->
      case opts[:cookie] do
        nil -> conn
        cookie -> put_req_header(conn, "cookie", cookie)
      end
    end)
    |> Router.call(Router.init(settings: settings))
  end

  defp cookie_for(login, opts \\ []) do
    "heph_session=" <> Session.token(login, opts[:settings] || settings(), opts)
  end

  defp get_as(path, login \\ "octocat"), do: request(:get, path, cookie: cookie_for(login))
  defp json(conn), do: Jason.decode!(conn.resp_body)

  defp set_cookie(conn, name) do
    conn.resp_headers
    |> Enum.filter(fn {key, _} -> key == "set-cookie" end)
    |> Enum.map(&elem(&1, 1))
    |> Enum.find(&String.starts_with?(&1, name <> "="))
  end

  defp cookie_value(set_cookie), do: set_cookie |> String.split(";") |> hd()

  # A workload with events, kept in the database.
  defp seed(number \\ 12, state_events \\ [Fixtures.started(), Fixtures.workspace_ready()]) do
    id = Ecto.UUID.generate()

    {:ok, _} =
      Fixtures.workload(id)
      |> put_in(["ticket", "number"], Integer.to_string(number))
      |> Workloads.create()

    state_events
    |> Enum.with_index(1)
    |> Enum.each(fn {kind, sequence} ->
      {:ok, _} = Events.append(id, Fixtures.event(id, sequence, kind))
    end)

    id
  end

  @repository "erinversfeldcodes/gazebolang"

  # Workloads made in the same second cannot be told apart by when they were made.
  defp backdate(id) do
    import Ecto.Query
    past = DateTime.add(DateTime.utc_now(), -3600, :second) |> DateTime.truncate(:second)

    Repo.update_all(from(w in ControlPlane.Workload, where: w.id == ^id),
      set: [inserted_at: past]
    )
  end

  describe "every endpoint without a session" do
    test "is refused with 401, and says nothing about the data" do
      id = seed()

      for path <- [
            "/api/session",
            "/api/tickets",
            "/api/tickets/erinversfeldcodes/gazebolang/12",
            "/api/workloads/#{id}",
            "/api/workloads/#{id}/events",
            "/api/workloads/#{id}/events/stream",
            "/api/workloads/#{id}/steps",
            "/api/workloads/#{id}/account",
            "/api/spend/tickets/erinversfeldcodes/gazebolang/12",
            "/api/spend/epics/erinversfeldcodes/gazebolang/12",
            "/api/spend/branch?repository=#{@repository}&branch=x",
            "/api/spend/days?repository=#{@repository}",
            "/api/questions",
            "/api/nothing-here"
          ] do
        conn = request(:get, path)
        assert conn.status == 401, "#{path} gave #{conn.status}"
        assert json(conn) == %{"error" => "not signed in"}
      end
    end

    test "is refused with a session that is not on the allowlist any more" do
      conn = request(:get, "/api/session", cookie: cookie_for("mallory"))
      assert conn.status == 401
    end
  end

  describe "the login" do
    test "starting it sends the person to GitHub with a state, and keeps the state in a cookie" do
      conn = request(:get, "/auth/login")

      assert conn.status == 302
      [location] = get_resp_header(conn, "location")
      uri = URI.parse(location)
      query = URI.decode_query(uri.query)
      assert uri.host == "github.com"
      assert query["client_id"] == "the-client-id"
      assert query["redirect_uri"] == "https://heph.test/auth/callback"
      assert byte_size(query["state"]) >= 32
      refute location =~ "the-client-secret"
      assert set_cookie(conn, "heph_oauth_state")
    end

    defp begin_login do
      conn = request(:get, "/auth/login")
      [location] = get_resp_header(conn, "location")
      state = URI.decode_query(URI.parse(location).query)["state"]
      {state, cookie_value(set_cookie(conn, "heph_oauth_state"))}
    end

    test "a login on the allowlist gets a session that reads who they are" do
      {state, state_cookie} = begin_login()

      conn =
        request(:get, "/auth/callback?code=good&state=#{state}", cookie: state_cookie)

      assert conn.status == 302
      session = set_cookie(conn, "heph_session")
      assert session

      me = request(:get, "/api/session", cookie: cookie_value(session))
      assert me.status == 200
      assert json(me)["login"] == "octocat"
    end

    test "the session cookie is secure, http-only, same-site and expires" do
      {state, state_cookie} = begin_login()
      conn = request(:get, "/auth/callback?code=good&state=#{state}", cookie: state_cookie)
      session = set_cookie(conn, "heph_session")

      assert session =~ "secure"
      assert session =~ "HttpOnly"
      assert session =~ "SameSite=Lax"
      assert session =~ "max-age=3600"
    end

    test "a login not on the allowlist is refused and no session is made" do
      {state, state_cookie} = begin_login()
      conn = request(:get, "/auth/callback?code=stranger&state=#{state}", cookie: state_cookie)

      assert conn.status == 403
      refute set_cookie(conn, "heph_session")
    end

    test "an empty allowlist lets no one in" do
      settings = settings(allowed_logins: [])
      {state, state_cookie} = begin_login()

      conn =
        request(:get, "/auth/callback?code=good&state=#{state}",
          cookie: state_cookie,
          settings: settings
        )

      assert conn.status == 403
      refute set_cookie(conn, "heph_session")
    end

    test "the allowlist ignores the case of a login" do
      StandInGitHubLogin.accept(%{"good" => "OctoCat"})
      {state, state_cookie} = begin_login()
      conn = request(:get, "/auth/callback?code=good&state=#{state}", cookie: state_cookie)
      assert conn.status == 302
      assert set_cookie(conn, "heph_session")
    end

    test "a state that is not the one in the cookie is refused" do
      {_state, state_cookie} = begin_login()
      conn = request(:get, "/auth/callback?code=good&state=forged", cookie: state_cookie)

      assert conn.status == 400
      refute set_cookie(conn, "heph_session")
    end

    test "a callback with no state cookie is refused" do
      {state, _cookie} = begin_login()
      conn = request(:get, "/auth/callback?code=good&state=#{state}")

      assert conn.status == 400
      refute set_cookie(conn, "heph_session")
    end

    test "a state is good once: a second callback with it is refused" do
      {state, state_cookie} = begin_login()
      path = "/auth/callback?code=good&state=#{state}"

      assert request(:get, path, cookie: state_cookie).status == 302
      second = request(:get, path, cookie: state_cookie)

      assert second.status == 400
      refute set_cookie(second, "heph_session")
    end

    test "a code GitHub refuses makes no session" do
      {state, state_cookie} = begin_login()
      conn = request(:get, "/auth/callback?code=bad&state=#{state}", cookie: state_cookie)

      assert conn.status == 401
      refute set_cookie(conn, "heph_session")
    end

    test "logging out clears the session cookie" do
      conn = request(:get, "/auth/logout", cookie: cookie_for("octocat"))
      assert conn.status == 302
      assert set_cookie(conn, "heph_session") =~ "max-age=0"
    end
  end

  describe "a session" do
    test "that has expired is refused" do
      old = System.system_time(:second) - 7200
      conn = request(:get, "/api/session", cookie: cookie_for("octocat", signed_at: old))
      assert conn.status == 401
    end

    test "that was tampered with is refused" do
      cookie = cookie_for("octocat")
      tampered = String.slice(cookie, 0..-3//1) <> "xx"
      assert request(:get, "/api/session", cookie: tampered).status == 401
    end

    test "signed with another key is refused" do
      other = settings(secret_key_base: String.duplicate("z", 64))
      cookie = cookie_for("octocat", settings: other)
      assert request(:get, "/api/session", cookie: cookie).status == 401
    end
  end

  describe "a method that changes something" do
    test "is refused with 405 on every route and on a route that does not exist" do
      paths =
        for route <- Router.routes() do
          route.path
          |> Enum.map(&if(String.starts_with?(&1, ":"), do: "1", else: &1))
          |> then(&("/" <> Enum.join(&1, "/")))
        end

      assert paths != []

      for method <- [:post, :put, :patch, :delete, :options],
          path <- ["/api/nothing-here" | paths] do
        for cookie <- [nil, cookie_for("octocat")] do
          conn = request(method, path, cookie: cookie)
          assert conn.status == 405, "#{method} #{path} gave #{conn.status}"
          assert get_resp_header(conn, "allow") == ["GET, HEAD"]
        end
      end
    end
  end

  describe "the methods that change something, with the executor link on" do
    # Every route that is not a read, and the only ones. A route added to the table that
    # writes fails the first test until it is named here, on purpose: a write is decided by a
    # person reading this list, not by whoever added the route.
    @allowed_writes [
      {"POST", ["api", "device-tokens"]},
      {"POST", ["api", "device-tokens", ":id", "revoke"]},
      {"POST", ["worker", "claim"]},
      {"POST", ["worker", "workloads", ":id", "events"]},
      {"POST", ["worker", "workloads", ":id", "heartbeat"]},
      {"POST", ["worker", "workloads", ":id", "record"]},
      {"POST", ["webhooks", ":source"]}
    ]

    setup do
      Application.put_env(:control_plane, :executor_link, enabled: true)
      Application.put_env(:control_plane, :webhooks, secret_file: "/nonexistent")

      on_exit(fn ->
        Application.delete_env(:control_plane, :executor_link)
        Application.delete_env(:control_plane, :webhooks)
      end)
    end

    defp concrete(path) do
      "/" <> Enum.map_join(path, "/", &if(String.starts_with?(&1, ":"), do: "1", else: &1))
    end

    test "are exactly the routes named here" do
      assert Router.write_routes() |> Enum.map(&{&1.method, &1.path}) |> Enum.sort() ==
               Enum.sort(@allowed_writes)

      assert Router.routes()
             |> Enum.reject(&(&1.method == "GET"))
             |> Enum.all?(&(&1.access in [:session_write, :worker, :webhook]))
    end

    test "every other method on every route, and on one that does not exist, is still 405" do
      allowed = MapSet.new(@allowed_writes)

      paths =
        [["api", "nothing-here"], ["worker", "nothing-here"]] ++
          Enum.map(Router.routes(), & &1.path)

      for method <- ["POST", "PUT", "PATCH", "DELETE", "OPTIONS"],
          path <- paths,
          not MapSet.member?(allowed, {method, path}),
          cookie <- [nil, cookie_for("octocat")] do
        conn =
          request(method |> String.downcase() |> String.to_atom(), concrete(path), cookie: cookie)

        assert conn.status == 405, "#{method} #{concrete(path)} gave #{conn.status}"
        assert [allow] = get_resp_header(conn, "allow")
        refute allow =~ method, "#{method} #{concrete(path)} says it allows #{allow}"
      end
    end

    test "a write route is 405 for every method but its own, and says which is its own" do
      conn = request(:put, "/api/device-tokens", cookie: cookie_for("octocat"))
      assert conn.status == 405
      assert get_resp_header(conn, "allow") == ["GET, HEAD, POST"]

      assert request(:get, "/worker/claim", cookie: cookie_for("octocat")).status == 404
      assert request(:get, "/worker/claim").status == 401
    end
  end

  describe "the routes" do
    @public [
      {"GET", ["auth", "login"]},
      {"GET", ["auth", "callback"]},
      {"GET", ["auth", "logout"]}
    ]

    test "each is a login route or is behind the session" do
      for route <- Router.routes() do
        case route.access do
          :public ->
            assert {route.method, route.path} in @public,
                   "#{inspect(route.path)} is reachable without a session"

          :session ->
            assert route.method == "GET"

          # Writes: named in the list of the ones allowed, and each is a person with a CSRF
          # token or an executor with a device token, never a bare cookie.
          access when access in [:session_write, :worker, :webhook] ->
            assert {route.method, route.path} in @allowed_writes

          other ->
            flunk("#{inspect(route.path)} has the access #{inspect(other)}")
        end
      end
    end

    test "each route behind the session answers 401 without one" do
      for route <- Router.routes(), route.access == :session do
        path =
          route.path
          |> Enum.map(&if(String.starts_with?(&1, ":"), do: "1", else: &1))
          |> then(&("/" <> Enum.join(&1, "/")))

        assert request(:get, path).status == 401, "#{path} is reachable without a session"
      end
    end
  end

  describe "an allowed session reads" do
    test "who it is" do
      conn = get_as("/api/session")
      assert conn.status == 200
      assert json(conn) == %{"login" => "octocat"}
    end

    test "tickets with their latest workload and how many there were" do
      id = seed(12)
      _other = seed(13)

      conn = get_as("/api/tickets")
      assert conn.status == 200
      %{"tickets" => tickets} = json(conn)
      assert length(tickets) == 2
      ticket = Enum.find(tickets, &(&1["number"] == 12))
      assert ticket["repository"] == @repository
      assert ticket["title"] == "Make the reset spec wait for hydration"
      assert ticket["workloads"] == 1
      assert ticket["latest"]["id"] == id
      assert ticket["latest"]["state"] == "running"
    end

    test "a ticket with its workloads and what it spent" do
      id = seed(12)
      conn = get_as("/api/tickets/#{@repository}/12")
      assert conn.status == 200
      body = json(conn)

      assert [%{"id" => ^id, "kind" => "implementation", "state" => "running"}] =
               body["workloads"]

      assert body["number"] == 12

      assert get_as("/api/tickets/#{@repository}/99").status == 404
      assert get_as("/api/tickets/#{@repository}/abc").status == 404
    end

    test "a workload" do
      id = seed()
      conn = get_as("/api/workloads/#{id}")
      assert conn.status == 200
      assert %{"id" => ^id, "state" => "running", "repository" => @repository} = json(conn)

      assert get_as("/api/workloads/#{Ecto.UUID.generate()}").status == 404
      assert get_as("/api/workloads/not-a-uuid").status == 404
    end

    test "a page of a workload's events, in order, from a cursor" do
      id =
        seed(12, [
          Fixtures.started(),
          Fixtures.workspace_ready(),
          Fixtures.agent_message("a"),
          Fixtures.agent_message("b")
        ])

      all = get_as("/api/workloads/#{id}/events") |> json()
      assert Enum.map(all["events"], & &1["sequence"]) == [1, 2, 3, 4]
      assert hd(all["events"])["kind"] == "started"
      assert Enum.at(all["events"], 2)["event"]["agentMessage"]["text"] == "a"

      page = get_as("/api/workloads/#{id}/events?after=1&limit=2") |> json()
      assert Enum.map(page["events"], & &1["sequence"]) == [2, 3]
      assert page["next"] == 3

      assert get_as("/api/workloads/#{id}/events?limit=abc").status == 400
    end

    test "a workload's steps from its record" do
      id = seed()

      steps = [
        %{
          "kind" => "STEP_KIND_IMPLEMENTATION",
          "costs" => [%{"unit" => "COST_UNIT_USD", "amount" => 0.5}]
        },
        %{"kind" => "STEP_KIND_REVIEW", "lens" => "security", "round" => 1}
      ]

      :ok = Workloads.keep_record(id, %{"outcome" => "OUTCOME_COMPLETED", "steps" => steps})

      body = get_as("/api/workloads/#{id}/steps") |> json()
      assert body["steps"] == steps

      empty = seed(14)
      assert get_as("/api/workloads/#{empty}/steps") |> json() == %{"steps" => []}
    end

    test "the spend roll-ups" do
      import ControlPlane.SpendFixtures

      spent(12, [usd(0.5), tokens(1_000.0)],
        epic: 5,
        branch: "heph/x",
        at: DateTime.to_iso8601(DateTime.utc_now())
      )

      ticket = get_as("/api/spend/tickets/#{@repository}/12") |> json()
      assert ticket["totals"]["usd"] == 0.5
      assert ticket["workloads"] == 1

      epic = get_as("/api/spend/epics/#{@repository}/5") |> json()
      assert epic["totals"]["usd"] == 0.5
      assert [%{"ticket" => 12}] = epic["tickets"]

      branch = get_as("/api/spend/branch?repository=#{@repository}&branch=heph/x") |> json()
      assert branch["totals"]["tokens"] == 1000.0

      days = get_as("/api/spend/days?repository=#{@repository}&days=3") |> json()
      assert [%{"totals" => %{"usd" => 0.5}}] = days["days"]

      assert get_as("/api/spend/branch").status == 400
      assert get_as("/api/spend/days?repository=#{@repository}&days=x").status == 400
    end

    test "a run's account and question are read behind the login, cleaned, and as the executor redacted them" do
      marker = "[redacted: CLAUDE_CODE_OAUTH_TOKEN]"

      id =
        seed(30, [
          Fixtures.started(),
          Fixtures.agent_message("looking"),
          Fixtures.agent_message("\e[31mThe cause is in lib/a.ex.\e[0m TOKEN=#{marker}"),
          Fixtures.finished("OUTCOME_BLOCKED", "needs an answer")
        ])

      :ok =
        Workloads.keep_record(id, %{"outcome" => "OUTCOME_BLOCKED", "question" => "Which one?"})

      body = get_as("/api/workloads/#{id}/account") |> json()

      assert body == %{
               "workload" => id,
               "account" => "The cause is in lib/a.ex. TOKEN=#{marker}",
               "question" => "Which one?"
             }
    end

    test "a run that said nothing and asked nothing has neither, and an unknown run is 404" do
      id = seed(31)

      assert get_as("/api/workloads/#{id}/account") |> json() ==
               %{"workload" => id, "account" => nil, "question" => nil}

      assert get_as("/api/workloads/#{Ecto.UUID.generate()}/account").status == 404
      assert get_as("/api/workloads/not-a-uuid/account").status == 404
    end

    test "the questions waiting for a person, and not one that a later workload answered" do
      asked =
        seed(20, [Fixtures.started(), Fixtures.finished("OUTCOME_BLOCKED", "needs an answer")])

      :ok =
        Workloads.keep_record(asked, %{
          "outcome" => "OUTCOME_BLOCKED",
          "question" => "Which database?"
        })

      # Blocked with no question is not a question.
      silent = seed(21, [Fixtures.started(), Fixtures.finished("OUTCOME_BLOCKED", "stuck")])
      :ok = Workloads.keep_record(silent, %{"outcome" => "OUTCOME_BLOCKED"})

      # Asked, and then a new workload for the ticket began: it was answered.
      answered = seed(22, [Fixtures.started(), Fixtures.finished("OUTCOME_BLOCKED", "q")])

      :ok =
        Workloads.keep_record(answered, %{"outcome" => "OUTCOME_BLOCKED", "question" => "Old?"})

      backdate(answered)
      _later = seed(22)

      body = get_as("/api/questions") |> json()

      assert [%{"workload" => ^asked, "ticket" => 20, "question" => "Which database?"}] =
               body["questions"]
    end
  end

  describe "the stream of a workload's events" do
    setup do
      bandit =
        start_supervised!(
          {Bandit, plug: {Router, settings: settings()}, port: 0, startup_log: false}
        )

      {:ok, {_, port}} = ThousandIsland.listener_info(bandit)
      %{port: port}
    end

    defp connect(port, path, cookie) do
      {:ok, socket} = :gen_tcp.connect(~c"127.0.0.1", port, [:binary, active: false])

      :ok =
        :gen_tcp.send(
          socket,
          "GET #{path} HTTP/1.1\r\nhost: localhost\r\ncookie: #{cookie}\r\n\r\n"
        )

      socket
    end

    # Read until the text has arrived (or the socket closes).
    defp read_until(socket, buffer, pattern) do
      if buffer =~ pattern do
        buffer
      else
        case :gen_tcp.recv(socket, 0, 3000) do
          {:ok, data} -> read_until(socket, buffer <> data, pattern)
          {:error, _} -> buffer
        end
      end
    end

    test "delivers the events appended after it connected, in order, and ends with the last", %{
      port: port
    } do
      id = seed(12, [Fixtures.started()])
      socket = connect(port, "/api/workloads/#{id}/events/stream", cookie_for("octocat"))

      # What was already there arrives first.
      buffer = read_until(socket, "", "id: 1\n")
      assert buffer =~ "HTTP/1.1 200"
      assert buffer =~ "text/event-stream"
      refute buffer =~ "id: 2\n"

      for {kind, sequence} <- [
            {Fixtures.agent_message("one"), 2},
            {Fixtures.agent_message("two"), 3},
            {Fixtures.finished(), 4}
          ] do
        {:ok, _} = Events.append(id, Fixtures.event(id, sequence, kind))
      end

      buffer = read_until(socket, buffer, "id: 4\n")
      ids = Regex.scan(~r/^id: (\d+)$/m, buffer) |> Enum.map(&String.to_integer(List.last(&1)))
      assert ids == [1, 2, 3, 4]
      assert buffer =~ "event: agentMessage"
      assert buffer =~ "\"text\":\"two\""

      # The stream ends once the workload has finished: the response is completed.
      assert read_until(socket, buffer, "\r\n0\r\n\r\n") =~ "event: finished"
    end

    test "starts after the event a reconnecting browser last saw", %{port: port} do
      id = seed(12, [Fixtures.started(), Fixtures.workspace_ready(), Fixtures.finished()])

      socket =
        connect(port, "/api/workloads/#{id}/events/stream?after=1", cookie_for("octocat"))

      buffer = read_until(socket, "", "\r\n0\r\n\r\n")
      ids = Regex.scan(~r/^id: (\d+)$/m, buffer) |> Enum.map(&String.to_integer(List.last(&1)))
      assert ids == [2, 3]
    end

    test "is refused without a session", %{port: port} do
      id = seed()
      {:ok, socket} = :gen_tcp.connect(~c"127.0.0.1", port, [:binary, active: false])

      :ok =
        :gen_tcp.send(
          socket,
          "GET /api/workloads/#{id}/events/stream HTTP/1.1\r\nhost: x\r\n\r\n"
        )

      assert read_until(socket, "", "not signed in") =~ "HTTP/1.1 401"
    end
  end

  describe "the API's start" do
    test "needs its key: settings without one are refused, and so is a short one" do
      for bad <- [nil, "", "tiny-secret"] do
        error =
          assert_raise ArgumentError, fn ->
            Settings.build!(
              secret_key_base: bad,
              client_id: "a",
              client_secret: "b",
              base_url: "https://x.test",
              allowed_logins: []
            )
          end

        assert error.message =~ "HEPH_API_SECRET_KEY_BASE"
        refute error.message =~ "tiny-secret"
      end
    end

    test "the application builds no listener unless a port is set, and refuses to build one without the key" do
      assert ControlPlane.Api.children(nil) == []

      assert_raise ArgumentError, ~r/HEPH_API_SECRET_KEY_BASE/, fn ->
        ControlPlane.Api.children(
          port: 4000,
          client_id: "a",
          client_secret: "b",
          base_url: "https://x.test"
        )
      end

      children = ControlPlane.Api.children([port: 4000] ++ Enum.to_list(settings_list()))
      assert length(children) == 2
    end

    defp settings_list do
      [
        secret_key_base: @secret,
        client_id: "a",
        client_secret: "b",
        base_url: "https://x.test",
        allowed_logins: ["octocat"]
      ]
    end
  end
end
