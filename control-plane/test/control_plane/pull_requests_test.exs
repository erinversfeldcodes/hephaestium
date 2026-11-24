defmodule ControlPlane.PullRequestsTest do
  @moduledoc """
  The record of a ticket's pull request: stored from the webhooks that name it, repaired by the
  hourly poll, and read by the API from the database alone. The code host is a stand-in that
  records what it was asked, so these prove what is stored and which reads are made; that
  GitHub answers in these shapes is not proved here.
  """

  use ControlPlane.DataCase, async: false

  import Plug.Test
  import Plug.Conn
  import Ecto.Query, only: [from: 2]

  alias ControlPlane.Api.{Router, Session, Settings, UsedStates}
  alias ControlPlane.{Change, Check, Fixtures, PullRequest, PullRequests, Repo}
  alias ControlPlane.{StandInCodeHost, StandInSystems, Workload, Workloads}
  alias ControlPlane.Webhooks.{Event, Handle}
  alias ControlPlane.WebhookFixtures, as: W

  @repo W.repository()
  @url "https://github.com/#{@repo}/pull/52"
  @head "abc123"

  setup do
    start_supervised!(StandInSystems)
    start_supervised!(UsedStates)

    saved =
      for key <- [:systems, :intake], do: {key, Application.fetch_env(:control_plane, key)}

    Application.put_env(:control_plane, :systems, %{
      @repo => %{
        tracker: {ControlPlane.StandInTracker, @repo},
        code_host: {StandInCodeHost, @repo}
      }
    })

    Application.put_env(:control_plane, :intake, repositories: [@repo])

    on_exit(fn ->
      for {key, saved} <- saved do
        case saved do
          {:ok, value} -> Application.put_env(:control_plane, key, value)
          :error -> Application.delete_env(:control_plane, key)
        end
      end
    end)

    :ok
  end

  # A workload that completed with a pull request, for a ticket.
  defp opened(ticket \\ 12, number \\ 52) do
    id = Ecto.UUID.generate()

    {:ok, _} =
      Fixtures.workload(id)
      |> put_in(["ticket", "number"], Integer.to_string(ticket))
      |> Workloads.create()

    Repo.update_all(from(w in Workload, where: w.id == ^id), set: [state: "completed"])

    Workloads.keep_record(id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => "https://github.com/#{@repo}/pull/#{number}"}
    })

    id
  end

  defp host(change, checks \\ []) do
    StandInSystems.answer(:code_host, :change, {:ok, change})
    StandInSystems.answer(:code_host, :checks, {:ok, checks})
  end

  defp change(fields \\ []),
    do: struct!(%Change{number: 52, url: @url, state: :open, head_sha: @head}, fields)

  defp run(check_state, conclusion),
    do: %Check{id: 1, name: "ci", state: check_state, conclusion: conclusion}

  defp event(kind, fields \\ []) do
    struct!(
      %Event{
        kind: kind,
        source: "github",
        repository: @repo,
        number: 52,
        at: ~U[2026-11-24 09:00:00Z],
        delivery_id: "d-1"
      },
      fields
    )
  end

  defp record(number \\ 52), do: Repo.get_by(PullRequest, repository: @repo, number: number)

  describe "stored from the webhook that names it" do
    for {kind, state, merged, expected} <- [
          {:change_updated, :open, false, "open"},
          {:change_closed, :closed, false, "closed"},
          {:change_merged, :closed, true, "merged"}
        ] do
      test "#{kind} stores #{expected}, as the host gives it" do
        opened()
        host(change(state: unquote(state), merged: unquote(merged)))

        Handle.run(event(unquote(kind)))

        assert %PullRequest{state: unquote(expected), url: @url, number: 52} = record()
      end
    end

    for {name, checks, expected} <- [
          {"no checks", [], "none"},
          {"every check passed", [{:completed, "success"}, {:completed, "skipped"}], "passing"},
          {"a check failed", [{:completed, "success"}, {:completed, "failure"}], "failing"},
          {"a check still runs", [{:completed, "success"}, {:running, nil}], "pending"},
          {"a failure with another still running", [{:completed, "failure"}, {:running, nil}],
           "failing"}
        ] do
      test "checks changed with #{name} stores #{expected}" do
        opened()
        host(change(), for({s, c} <- unquote(Macro.escape(checks)), do: run(s, c)))

        Handle.run(event(:checks_changed))

        assert %PullRequest{checks: unquote(expected), state: "open"} = record()
      end
    end

    test "checks completed that name no change are matched by the commit already recorded" do
      opened()
      host(change(), [run(:running, nil)])
      Handle.run(event(:checks_changed))
      assert record().checks == "pending"

      host(change(), [run(:completed, "success")])
      Handle.run(event(:checks_completed, number: nil, head: @head))

      assert record().checks == "passing"
    end

    test "a pull request no workload opened is not looked at" do
      host(change())

      assert PullRequests.refresh(@repo, 99) == :unknown

      Handle.run(event(:change_updated, number: 99))

      assert record(99) == nil
      assert Enum.filter(StandInSystems.calls(), &match?({:code_host, _, [_, 99 | _]}, &1)) == []
    end

    test "a repeated delivery is stored once, and keeps when it last changed" do
      opened()
      host(change(), [run(:completed, "success")])

      Handle.run(event(:checks_completed))
      first = record()
      Handle.run(event(:checks_completed))
      Handle.run(event(:checks_completed))

      assert Repo.aggregate(PullRequest, :count) == 1
      again = record()
      assert again.changed_at == first.changed_at
      assert DateTime.compare(again.refreshed_at, first.refreshed_at) in [:gt, :eq]
    end

    test "a change of checks moves when it last changed" do
      opened()
      host(change(), [run(:running, nil)])
      Handle.run(event(:checks_changed))
      pending = record()

      host(change(), [run(:completed, "failure")])
      Handle.run(event(:checks_completed))

      failing = record()
      assert failing.checks == "failing"
      assert DateTime.compare(failing.changed_at, pending.changed_at) == :gt
    end

    test "a merged pull request keeps the checks it had" do
      opened()
      host(change(), [run(:completed, "success")])
      Handle.run(event(:checks_completed))

      host(change(state: :closed, merged: true))
      Handle.run(event(:change_merged))

      assert %PullRequest{state: "merged", checks: "passing"} = record()
    end
  end

  describe "the hourly poll" do
    test "repairs a delivery that never came" do
      opened()
      host(change(), [run(:running, nil)])
      Handle.run(event(:checks_changed))
      assert record().checks == "pending"

      # The checks finished and the merge happened, and neither delivery arrived.
      Repo.update_all(PullRequest,
        set: [refreshed_at: DateTime.add(DateTime.utc_now(), -2 * 3600, :second)]
      )

      host(change(state: :closed, merged: true))
      PullRequests.refresh_due(DateTime.utc_now())

      assert record().state == "merged"
    end

    test "leaves alone an open pull request confirmed within the hour" do
      opened()
      host(change(), [run(:running, nil)])
      Handle.run(event(:checks_changed))
      before = length(StandInSystems.calls())

      host(change(state: :closed, merged: true))
      PullRequests.refresh_due(DateTime.utc_now())

      assert record().state == "open"
      assert length(StandInSystems.calls()) == before
    end

    test "stores a pull request a workload opened that no delivery named" do
      opened()
      host(change(), [run(:completed, "success")])

      PullRequests.refresh_due(DateTime.utc_now())

      assert %PullRequest{state: "open", checks: "passing"} = record()
    end

    test "is a job that runs once an hour whether or not webhooks are on" do
      for webhooks <- [true, false] do
        assert {_, ControlPlane.Jobs.RefreshPullRequests} =
                 Enum.find(
                   ControlPlane.Schedule.crontab(webhooks),
                   &(elem(&1, 1) == ControlPlane.Jobs.RefreshPullRequests)
                 )
      end
    end
  end

  describe "the API" do
    @secret String.duplicate("k", 64)

    defp settings do
      Settings.build!(
        secret_key_base: @secret,
        client_id: "the-client-id",
        client_secret: "the-client-secret",
        base_url: "https://heph.test",
        allowed_logins: ["octocat"],
        github: ControlPlane.StandInGitHubLogin,
        session_ttl: 3600,
        poll_ms: 20
      )
    end

    defp get_as(path) do
      conn(:get, path)
      |> put_req_header("cookie", "heph_session=" <> Session.token("octocat", settings(), []))
      |> Router.call(Router.init(settings: settings()))
    end

    defp store(state, checks, number \\ 52) do
      Repo.insert!(%PullRequest{
        repository: @repo,
        number: number,
        url: "https://github.com/#{@repo}/pull/#{number}",
        state: state,
        checks: checks,
        head_sha: @head,
        changed_at: DateTime.utc_now(),
        refreshed_at: DateTime.utc_now()
      })
    end

    defp board_entry(ticket) do
      %{"tickets" => tickets} =
        get_as("/api/tickets") |> Map.fetch!(:resp_body) |> Jason.decode!()

      Enum.find(tickets, &(&1["number"] == ticket))
    end

    for state <- ~w(open merged closed), checks <- ~w(passing failing pending none) do
      test "carries a #{state} pull request with #{checks} checks on the board and the ticket" do
        opened()
        store(unquote(state), unquote(checks))

        expected = %{
          "number" => 52,
          "url" => @url,
          "state" => unquote(state),
          "checks" => unquote(checks)
        }

        assert board_entry(12)["pull_request"] == expected

        ticket = get_as("/api/tickets/#{@repo}/12")
        assert ticket.status == 200
        assert Jason.decode!(ticket.resp_body)["pull_request"] == expected
      end
    end

    test "carries null for a ticket with no pull request" do
      id = Ecto.UUID.generate()

      {:ok, _} =
        Fixtures.workload(id) |> put_in(["ticket", "number"], "13") |> Workloads.create()

      assert board_entry(13)["pull_request"] == nil

      assert get_as("/api/tickets/#{@repo}/13").resp_body
             |> Jason.decode!()
             |> Map.fetch!("pull_request") == nil
    end

    test "carries null for a pull request not yet recorded" do
      opened()

      assert board_entry(12)["pull_request"] == nil
    end

    test "reads nothing from the code host" do
      opened()
      store("open", "failing")

      board_entry(12)
      get_as("/api/tickets/#{@repo}/12")

      assert StandInSystems.calls() == []
    end
  end
end
