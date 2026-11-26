defmodule ControlPlane.ApiReplyTest do
  @moduledoc """
  A person answers an agent's question from the dashboard. The comment and the approval are
  made with that person's own GitHub user token and never with the App's, nobody who cannot
  write to the repository changes anything, and the approval is then read by intake as the
  person's own. GitHub is a stand-in: `gh` here records what it was asked and with which token,
  which proves the control plane's logic against the shapes GitHub gives, not that GitHub gives
  them.
  """

  use ControlPlane.DataCase, async: false

  import Plug.Test
  import Plug.Conn

  alias ControlPlane.Api.{Csrf, Router, Session, Settings, UsedStates}
  alias ControlPlane.{Fixtures, StandInGitHub, StandInGitHubLogin, Workloads}
  alias ControlPlane.GitHub.{CodeHost, Tracker}
  alias ControlPlane.Intake

  @repo "erinversfeldcodes/gazebolang"
  @path "/api/tickets/#{@repo}/12/reply"
  @secret String.duplicate("k", 64)

  setup do
    start_supervised!(UsedStates)
    StandInGitHubLogin.accept(%{"good" => "octocat", "stranger" => "mallory"})
    dir = Path.join(System.tmp_dir!(), "cp-reply-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [@repo])

    on_exit(fn ->
      Application.delete_env(:control_plane, :stand_in_logins)
      Application.delete_env(:control_plane, :systems)
      File.rm_rf(dir)
    end)

    permission(gh, "octocat", "write")
    permission(gh, "mallory", "read")
    ask(12, "Which database?")
    %{gh: gh}
  end

  defp settings do
    Settings.build!(
      secret_key_base: @secret,
      client_id: "the-client-id",
      client_secret: "the-client-secret",
      base_url: "https://heph.test",
      allowed_logins: ["octocat", "mallory"],
      github: StandInGitHubLogin,
      session_ttl: 3600
    )
  end

  defp permission(gh, login, level),
    do:
      StandInGitHub.respond(gh, CodeHost.permission_endpoint(@repo, login), %{
        "permission" => level,
        "role_name" => level
      })

  # A ticket whose latest run ended blocked with a question.
  defp ask(number, question) do
    id = Ecto.UUID.generate()

    {:ok, _} =
      Fixtures.workload(id)
      |> put_in(["ticket", "number"], Integer.to_string(number))
      |> Workloads.create()

    for {kind, sequence} <-
          Enum.with_index([Fixtures.started(), Fixtures.finished("OUTCOME_BLOCKED", "q")], 1) do
      {:ok, _} = ControlPlane.Events.append(id, Fixtures.event(id, sequence, kind))
    end

    :ok = Workloads.keep_record(id, %{"outcome" => "OUTCOME_BLOCKED", "question" => question})
  end

  defp approve(gh, at) do
    StandInGitHub.respond(gh, Tracker.ready_endpoint(@repo, "heph:ready"), [
      %{
        "number" => 12,
        "state" => "open",
        "title" => "A ticket",
        "labels" => [%{"name" => "heph:ready"}]
      }
    ])

    StandInGitHub.respond(gh, Tracker.events_endpoint(@repo, 12), [
      %{
        "event" => "labeled",
        "created_at" => at,
        "actor" => %{"login" => "octocat"},
        "label" => %{"name" => "heph:ready"}
      }
    ])

    StandInGitHub.respond_graphql(gh, 12, %{
      "data" => %{"repository" => %{"issue" => %{"userContentEdits" => %{"nodes" => []}}}}
    })

    StandInGitHub.heph_prints(
      gh,
      Fixtures.workload()
      |> put_in(["ticket", "number"], "12")
      |> put_in(["approval"], %{"approvedBy" => "octocat", "approvedAt" => at})
    )
  end

  # A later workload on the ticket that ends blocked with a question.
  defp ask_again(number, question) do
    id = Ecto.UUID.generate()

    {:ok, _} =
      Fixtures.workload(id)
      |> put_in(["ticket", "number"], Integer.to_string(number))
      |> Workloads.create()

    # Workloads are stamped to the second, so one made in the same second as the one before it
    # would not count as later: put it a minute on.
    import Ecto.Query

    later = NaiveDateTime.add(NaiveDateTime.utc_now(:second), 60)

    ControlPlane.Repo.update_all(
      from(w in ControlPlane.Workload, where: w.id == ^id),
      set: [inserted_at: later]
    )

    :ok = Workloads.keep_record(id, %{"outcome" => "OUTCOME_BLOCKED", "question" => question})
    {:ok, _} = ControlPlane.Events.append(id, Fixtures.event(id, 1, Fixtures.started()))

    {:ok, _} =
      ControlPlane.Events.append(
        id,
        Fixtures.event(id, 2, Fixtures.finished("OUTCOME_BLOCKED", "q"))
      )
  end

  # The cookies a login makes: the session, and the user token the login was given.
  defp cookies(login, token) do
    conn(:get, "/")
    |> Session.start(login, settings(), token)
    |> then(fn conn ->
      for {name, %{value: value}} <- conn.resp_cookies, do: "#{name}=#{value}"
    end)
    |> Enum.join("; ")
  end

  defp reply(opts) do
    login = Keyword.get(opts, :login, "octocat")
    body = Keyword.get(opts, :body, Jason.encode!(%{body: "Use Postgres."}))

    conn(:post, Keyword.get(opts, :path, @path), body)
    |> put_req_header("content-type", "application/json")
    |> then(fn conn ->
      case Keyword.fetch(opts, :cookie) do
        {:ok, nil} -> conn
        {:ok, cookie} -> put_req_header(conn, "cookie", cookie)
        :error -> put_req_header(conn, "cookie", cookies(login, "token-for-good"))
      end
    end)
    |> then(fn conn ->
      case Keyword.fetch(opts, :csrf) do
        {:ok, nil} -> conn
        {:ok, csrf} -> put_req_header(conn, "x-csrf-token", csrf)
        :error -> put_req_header(conn, "x-csrf-token", Csrf.token(settings(), login))
      end
    end)
    |> Router.call(Router.init(settings: settings()))
  end

  defp json(conn), do: Jason.decode!(conn.resp_body)

  defp tokens(%{dir: dir}) do
    case File.read(Path.join(dir, "gh-tokens.log")) do
      {:ok, text} -> String.split(text, "\n", trim: true)
      _ -> []
    end
  end

  describe "a person with write access answers" do
    test "the comment and then the approval are made with their own token", %{gh: gh} do
      conn = reply([])

      assert conn.status == 200
      assert json(conn) == %{"commented" => true, "approved" => true}

      assert [
               {"POST", "repos/#{@repo}/issues/12/comments", ["body=Use Postgres."]},
               {"POST", "repos/#{@repo}/issues/12/labels", ["labels[]=heph:ready"]}
             ] == StandInGitHub.writes(gh)
    end

    test "both are made as the person, and none with any other token", %{gh: gh} do
      assert reply([]).status == 200

      assert tokens(gh) == [
               "POST repos/#{@repo}/issues/12/comments token-for-good",
               "POST repos/#{@repo}/issues/12/labels token-for-good"
             ]
    end

    test "the approval that follows is read by intake as the person's own", %{gh: gh} do
      assert reply([]).status == 200

      StandInGitHub.respond(gh, Tracker.ready_endpoint(@repo, "heph:ready"), [
        %{
          "number" => 12,
          "state" => "open",
          "title" => "A ticket",
          "labels" => [%{"name" => "heph:ready"}]
        }
      ])

      StandInGitHub.respond(gh, Tracker.events_endpoint(@repo, 12), [
        %{
          "event" => "labeled",
          "created_at" => "2026-11-18T09:00:00Z",
          "actor" => %{"login" => "octocat"},
          "label" => %{"name" => "heph:ready"}
        }
      ])

      StandInGitHub.respond_graphql(gh, 12, %{
        "data" => %{"repository" => %{"issue" => %{"userContentEdits" => %{"nodes" => []}}}}
      })

      StandInGitHub.heph_prints(
        gh,
        Fixtures.workload()
        |> put_in(["ticket", "number"], "12")
        |> put_in(["approval"], %{
          "approvedBy" => "octocat",
          "approvedAt" => "2026-11-18T09:00:00Z"
        })
      )

      assert [%{ticket: 12, result: :started}] = Intake.run(@repo)

      assert ["workload 12 --repo #{@repo} --approver octocat --approved-at 2026-11-18T09:00:00Z"] ==
               StandInGitHub.heph_calls(gh)
    end

    test "the answer is part of the request of the workload that follows, and of the one after it",
         %{gh: gh} do
      assert reply([]).status == 200
      approve(gh, "2026-11-18T09:00:00Z")
      assert [%{ticket: 12, result: :started}] = Intake.run(@repo)

      [asked, second] = Workloads.for_ticket(@repo, 12)

      assert second.workload["answers"] == [
               %{"question" => "Which database?", "answer" => "Use Postgres."}
             ]

      assert asked.answer == "Use Postgres."

      # The second workload fails a gate and the ticket is approved again: the answer is
      # carried although the person was never asked a second time.
      approve(gh, "2026-11-19T09:00:00Z")
      assert [%{ticket: 12, result: :started}] = Intake.run(@repo)

      [_, _, third] = Workloads.for_ticket(@repo, 12)

      assert third.workload["answers"] == [
               %{"question" => "Which database?", "answer" => "Use Postgres."}
             ]
    end

    test "a question asked again is shown with the answer already given" do
      assert reply([]).status == 200
      ask_again(12, "which database?")

      assert [%{earlier_answer: "Use Postgres."}] = ControlPlane.Api.Read.questions()
    end
  end

  describe "nothing is sent" do
    test "by a person without write access", %{gh: gh} do
      conn = reply(login: "mallory")

      assert conn.status == 403
      assert json(conn)["error"] =~ "write access"
      assert StandInGitHub.writes(gh) == []
    end

    test "by someone GitHub does not know as a collaborator", %{gh: gh} do
      StandInGitHub.fail(
        gh,
        CodeHost.permission_endpoint(@repo, "octocat"),
        "gh: Not Found (HTTP 404)"
      )

      assert reply([]).status == 403
      assert StandInGitHub.writes(gh) == []
    end

    test "when the permission cannot be read", %{gh: gh} do
      StandInGitHub.fail(gh, CodeHost.permission_endpoint(@repo, "octocat"), "gh: broken")

      assert reply([]).status == 502
      assert StandInGitHub.writes(gh) == []
    end

    test "when the ticket has no question waiting", %{gh: gh} do
      conn = reply(path: "/api/tickets/#{@repo}/99/reply")

      assert conn.status == 409
      assert StandInGitHub.writes(gh) == []
    end

    test "without a user token with the session", %{gh: gh} do
      session = "heph_session=" <> Session.token("octocat", settings())
      conn = reply(cookie: session)

      assert conn.status == 401
      assert json(conn)["error"] =~ "sign in again"
      assert StandInGitHub.writes(gh) == []
    end

    test "with another login's user token", %{gh: gh} do
      [_, theirs] = String.split(cookies("mallory", "token-for-stranger"), "; ")
      mine = "heph_session=" <> Session.token("octocat", settings())
      conn = reply(cookie: mine <> "; " <> theirs)

      assert conn.status == 401
      assert StandInGitHub.writes(gh) == []
    end

    test "for an empty or an oversized reply", %{gh: gh} do
      for body <- [%{body: "  "}, %{body: String.duplicate("x", 20_000)}, %{}] do
        assert reply(body: Jason.encode!(body)).status == 400
      end

      assert reply(body: "not json").status == 400
      assert StandInGitHub.writes(gh) == []
    end

    test "without the session, without the CSRF token, or with another person's", %{gh: gh} do
      assert reply(cookie: nil).status == 401
      assert reply(csrf: nil).status == 403
      assert reply(csrf: Csrf.token(settings(), "mallory")).status == 403
      assert StandInGitHub.writes(gh) == []
    end

    test "for a tracker that cannot act as a person", %{gh: gh} do
      Application.put_env(:control_plane, :systems, %{
        @repo => %{
          tracker: {ControlPlane.StandInTracker, "p"},
          code_host: {ControlPlane.StandInCodeHost, "p"}
        }
      })

      start_supervised!(ControlPlane.StandInSystems)
      ControlPlane.StandInSystems.answer(:code_host, :permission, {:ok, :write})

      conn = reply([])

      assert conn.status == 501
      assert StandInGitHub.writes(gh) == []
      assert ControlPlane.StandInSystems.calls() == []
    end
  end

  describe "when GitHub refuses" do
    test "the comment, nothing is changed and the page says so", %{gh: gh} do
      StandInGitHub.fail_write(gh, "POST", "repos/#{@repo}/issues/12/comments", "HTTP 403")

      conn = reply([])

      assert conn.status == 403

      assert json(conn) == %{
               "commented" => false,
               "approved" => false,
               "error" => "GitHub refused the comment"
             }

      assert [{"POST", _, _}] = StandInGitHub.writes(gh)
    end

    test "the approval, the page says the comment was made and the approval was not", %{gh: gh} do
      StandInGitHub.fail_write(gh, "POST", "repos/#{@repo}/issues/12/labels", "HTTP 403")

      conn = reply([])

      assert conn.status == 403
      assert %{"commented" => true, "approved" => false} = json(conn)
    end

    test "a lapsed token, the person is told to sign in again", %{gh: gh} do
      StandInGitHub.fail_write(gh, "POST", "repos/#{@repo}/issues/12/comments", "HTTP 401")

      conn = reply([])

      assert conn.status == 401
      assert json(conn)["error"] =~ "sign in again"
    end

    test "whatever it says, the token is not in the answer" do
      conn = reply([])
      refute conn.resp_body =~ "token-for-good"
    end
  end
end
