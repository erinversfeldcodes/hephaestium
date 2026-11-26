defmodule ControlPlane.ExecutorLinkTest do
  @moduledoc """
  The server side of the executor link: workers on developers' machines claim the workloads
  their person approved, as leases, and stream what they do back. A stand-in worker speaks the
  protocol over a real listener; nothing here reaches GitHub or a real executor.
  """

  use ControlPlane.DataCase, async: false
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query
  import ExUnit.CaptureLog
  import Plug.Conn
  import Plug.Test
  import ControlPlane.Fixtures

  alias ControlPlane.Api.{Csrf, Router, Session, Settings, UsedStates}

  alias ControlPlane.{
    DeviceTokens,
    Events,
    Leases,
    Runner,
    SpendEntry,
    StandIn,
    StandInWorker,
    Workload,
    WorkloadLease,
    Workloads
  }

  @secret String.duplicate("k", 64)
  @person "erinversfeldcodes"
  @other "hubot"

  defp settings(overrides \\ []) do
    [
      secret_key_base: @secret,
      client_id: "id",
      client_secret: "secret",
      base_url: "https://heph.test",
      allowed_logins: [@person, @other],
      github: ControlPlane.StandInGitHubLogin,
      session_ttl: 3600,
      poll_ms: 10
    ]
    |> Keyword.merge(overrides)
    |> Settings.build!()
  end

  setup do
    previous = Application.get_env(:control_plane, :executor_link)

    Application.put_env(:control_plane, :executor_link,
      enabled: true,
      lease_seconds: 120,
      claim_wait_ms: 60,
      reap_ms: 60_000
    )

    on_exit(fn ->
      if previous,
        do: Application.put_env(:control_plane, :executor_link, previous),
        else: Application.delete_env(:control_plane, :executor_link)
    end)

    start_supervised!(UsedStates)

    bandit =
      start_supervised!(
        {Bandit, plug: {Router, settings: settings()}, port: 0, startup_log: false}
      )

    {:ok, {_, port}} = ThousandIsland.listener_info(bandit)
    {:ok, %{token: mine, record: mine_record}} = DeviceTokens.issue(@person, "laptop")
    {:ok, %{token: theirs}} = DeviceTokens.issue(@other, "their laptop")
    %{port: port, mine: mine, mine_record: mine_record, theirs: theirs}
  end

  # A queued workload approved by `approver`.
  defp queued(approver \\ @person) do
    {:ok, created} =
      workload()
      |> put_in(["approval", "approvedBy"], approver)
      |> Workloads.create()

    created.id
  end

  # A queued review of a pull request, approved by `approver`, put on the queue after what is
  # queued before it.
  defp queued_review(approver \\ @person) do
    {:ok, created} =
      workload()
      |> put_in(["approval", "approvedBy"], approver)
      |> Map.put("review", %{"pullRequestNumber" => "7", "lens" => "security"})
      |> Workloads.create()

    created.id
  end

  defp stream(id, from, kinds),
    do:
      kinds
      |> Enum.with_index(from)
      |> Enum.map(fn {kind, sequence} -> event(id, sequence, kind) end)

  defp beginning(id), do: stream(id, 1, [started(), workspace_ready()])

  defp drain_queries(acc) do
    receive do
      {:query, query} -> drain_queries([query | acc])
    after
      0 -> Enum.reverse(acc)
    end
  end

  defp lease(id), do: Repo.get(WorkloadLease, id)

  # The lease ended a minute ago, as a laptop that closed would leave it.
  defp lapse(id) do
    past = DateTime.add(DateTime.utc_now(), -60, :second)

    Repo.update_all(from(l in WorkloadLease, where: l.workload_id == ^id),
      set: [expires_at: past]
    )
  end

  describe "a worker with a valid token" do
    test "claims a workload and streams events that are kept in order", ctx do
      id = queued()

      assert %{status: 200, body: %{"workload" => claimed, "lease" => %{"seconds" => 120}}} =
               StandInWorker.claim(ctx.port, ctx.mine)

      assert claimed == workload(id) |> put_in(["approval", "approvedBy"], @person)
      assert Workloads.get(id).state == "preparing"

      sent = stream(id, 1, [started(), workspace_ready(), agent_message("hi"), gate_started()])

      assert %{status: 200, body: %{"accepted" => 4}} =
               StandInWorker.events(ctx.port, ctx.mine, id, sent)

      assert Enum.map(Events.list(id), & &1.event) == sent
      assert Workloads.get(id).state == "gating"

      more = stream(id, 5, [gate_finished(), finished()])
      assert %{status: 200} = StandInWorker.events(ctx.port, ctx.mine, id, more)
      assert Workloads.get(id).state == "completed"

      record = %{"workloadId" => id, "outcome" => "OUTCOME_COMPLETED"}
      assert %{status: 200} = StandInWorker.record(ctx.port, ctx.mine, id, record)
      assert Workloads.get(id).record == record
      refute lease(id).active, "the lease is let go when the record is kept"
    end

    test "locks a workload for an event without reading its documents", ctx do
      id = queued()
      assert %{status: 200} = StandInWorker.claim(ctx.port, ctx.mine)

      test_pid = self()
      handler = "lock-query-#{System.unique_integer()}"

      :telemetry.attach(
        handler,
        [:control_plane, :repo, :query],
        fn _event, _measure, %{query: query}, _config ->
          if self() == test_pid, do: send(test_pid, {:query, query})
        end,
        nil
      )

      on_exit(fn -> :telemetry.detach(handler) end)

      assert {:ok, 1, _} = Leases.ingest(ctx.mine_record, id, [event(id, 1, started())])
      :telemetry.detach(handler)

      queries = drain_queries([])
      # The first lock is the lease's; the append takes its own, which reads the row it advances.
      [lock | _] = Enum.filter(queries, &(&1 =~ ~r/FOR UPDATE/))

      assert lock =~ ~r/^SELECT w0\."id" FROM "workloads"/
      refute lock =~ "workload\""
      refute lock =~ "record"
    end

    test "is given a queued review before an implementation that was queued earlier", ctx do
      implementation = queued()
      Process.sleep(5)
      review = queued_review()

      assert %{status: 200, body: %{"workload" => %{"id" => ^review}}} =
               StandInWorker.claim(ctx.port, ctx.mine)

      assert %{status: 200, body: %{"workload" => %{"id" => ^implementation}}} =
               StandInWorker.claim(ctx.port, ctx.mine)
    end

    test "holds several workloads at once, each with a lease of its own", ctx do
      first = queued()
      second = queued()

      assert %{status: 200} = StandInWorker.claim(ctx.port, ctx.mine, %{slots: 2})
      assert %{status: 200} = StandInWorker.claim(ctx.port, ctx.mine, %{slots: 2})
      assert lease(first).active and lease(second).active
      assert DeviceTokens.running(ctx.mine_record) == 2
      assert Repo.reload!(ctx.mine_record).slots == 2

      # One finishing, or being given back, leaves the other held.
      assert %{status: 200} = StandInWorker.release(ctx.port, ctx.mine, first)
      refute lease(first).active
      assert lease(second).active
      assert %{status: 200} = StandInWorker.heartbeat(ctx.port, ctx.mine, second)
      assert DeviceTokens.running(ctx.mine_record) == 1
    end

    test "a claim that says nothing, or something absurd, is an executor of one slot", ctx do
      queued()
      assert %{status: 200} = StandInWorker.claim(ctx.port, ctx.mine)
      assert Repo.reload!(ctx.mine_record).slots == 1

      assert %{status: 204} = StandInWorker.claim(ctx.port, ctx.mine, %{slots: 5000})
      assert Repo.reload!(ctx.mine_record).slots == 1
    end

    test "is connected for as long as it sends anything, not only when it claims", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)

      long_ago = DateTime.add(DateTime.utc_now(), -3600, :second)
      Repo.update_all(from(t in ControlPlane.DeviceToken), set: [last_seen_at: long_ago])
      refute DeviceTokens.connected?(@person)

      StandInWorker.heartbeat(ctx.port, ctx.mine, id)
      assert DeviceTokens.connected?(@person)
    end

    test "waits for a workload when there is none, then answers that there is none", ctx do
      assert %{status: 204, body: nil} = StandInWorker.claim(ctx.port, ctx.mine)
    end

    test "has an event that breaks the stream's rules refused, and its workload failed", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)

      sent = [event(id, 1, started()), event(id, 3, agent_message())]

      assert %{status: 422, body: %{"error" => "gap_in_sequence", "accepted" => 1}} =
               StandInWorker.events(ctx.port, ctx.mine, id, sent)

      failed = Workloads.get(id)
      assert failed.state == "failed"
      assert failed.reason =~ "the stream broke a rule (gap_in_sequence)"
      assert Enum.map(Events.list(id), & &1.sequence) == [1]
      refute lease(id).active
    end

    test "may send a batch again after the network cut it, and nothing is kept twice", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      sent = beginning(id)

      assert %{status: 200, body: %{"accepted" => 2}} =
               StandInWorker.events(ctx.port, ctx.mine, id, sent)

      assert %{status: 200, body: %{"accepted" => 2}} =
               StandInWorker.events(ctx.port, ctx.mine, id, sent)

      assert Enum.map(Events.list(id), & &1.sequence) == [1, 2]

      # An event that is a different one under a sequence already kept is a broken rule.
      changed = [
        event(id, 2, workspace_ready()) |> put_in(["workspaceReady", "path"], "/elsewhere")
      ]

      assert %{status: 422, body: %{"error" => "repeated_sequence"}} =
               StandInWorker.events(ctx.port, ctx.mine, id, changed)
    end

    test "that ends without saying how is failed, as when the local executor exits", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      StandInWorker.events(ctx.port, ctx.mine, id, beginning(id))
      assert %{status: 200} = StandInWorker.record(ctx.port, ctx.mine, id, %{"workloadId" => id})
      assert Workloads.get(id).state == "failed"
    end

    test "with an event of the kind that says delivery began, has it kept", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)

      sent = beginning(id) ++ [event(id, 3, %{"deliveryBegan" => %{}})]

      assert %{status: 200, body: %{"accepted" => 3}} =
               StandInWorker.events(ctx.port, ctx.mine, id, sent)

      assert List.last(Events.list(id)).kind == "deliveryBegan"
    end
  end

  describe "a token that is not good" do
    test "unknown, revoked, malformed or absent is refused, and says nothing about why", ctx do
      id = queued()
      {:ok, %{token: gone, record: gone_record}} = DeviceTokens.issue(@person, "sold")
      assert :ok = DeviceTokens.revoke(@person, gone_record.id)

      for token <- [nil, "hdt_unknown", "unknown", gone] do
        assert %{status: 401, body: %{"error" => "not authorized"}} =
                 StandInWorker.claim(ctx.port, token)

        assert %{status: 401} = StandInWorker.events(ctx.port, token, id, beginning(id))
        assert %{status: 401} = StandInWorker.heartbeat(ctx.port, token, id)
        assert %{status: 401} = StandInWorker.record(ctx.port, token, id, %{})
      end

      assert Workloads.get(id).state == "queued"
    end

    test "of a login that has left the allowlist is refused", ctx do
      id = queued()
      narrow = settings(allowed_logins: [@other])

      conn =
        conn(:post, "/worker/claim")
        |> put_req_header("authorization", "Bearer " <> ctx.mine)
        |> Router.call(Router.init(settings: narrow))

      assert conn.status == 401
      assert Workloads.get(id).state == "queued"
    end

    test "that was revoked while its worker held a workload loses it", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      assert :ok = DeviceTokens.revoke(@person, ctx.mine_record.id)

      assert %{status: 401} = StandInWorker.events(ctx.port, ctx.mine, id, beginning(id))
      assert [%{result: :requeued}] = Leases.reap()
      assert Workloads.get(id).state == "queued"
    end

    test "of another person claims nothing of a person's, and sends nothing for it", ctx do
      id = queued()
      assert %{status: 204} = StandInWorker.claim(ctx.port, ctx.theirs)
      assert Workloads.get(id).state == "queued"

      assert %{status: 200} = StandInWorker.claim(ctx.port, ctx.mine)
      assert %{status: 409} = StandInWorker.events(ctx.port, ctx.theirs, id, beginning(id))
      assert %{status: 409} = StandInWorker.heartbeat(ctx.port, ctx.theirs, id)
      assert %{status: 409} = StandInWorker.record(ctx.port, ctx.theirs, id, %{})
      assert Events.list(id) == []
    end

    test "is refused for a workload that is not its own even when it names a real one", ctx do
      id = queued(@other)
      assert %{status: 409} = StandInWorker.events(ctx.port, ctx.mine, id, beginning(id))
      assert %{status: 409} = StandInWorker.events(ctx.port, ctx.mine, Ecto.UUID.generate(), [])
    end
  end

  describe "a lease that lapses" do
    test "puts the workload back once, and the old worker's later events are refused", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      StandInWorker.events(ctx.port, ctx.mine, id, beginning(id))
      assert length(Events.list(id)) == 2

      lapse(id)
      assert [%{workload_id: ^id, result: :requeued}] = Leases.reap()

      back = Workloads.get(id)
      assert back.state == "queued"
      assert length(Events.list(id)) == 2, "what the lost run said stays kept"
      assert lease(id).requeues == 1

      # The laptop opens again and goes on as if nothing had happened.
      later = [event(id, 3, agent_message("still here"))]
      assert %{status: 409} = StandInWorker.events(ctx.port, ctx.mine, id, later)
      assert %{status: 409} = StandInWorker.heartbeat(ctx.port, ctx.mine, id)
      assert %{status: 409} = StandInWorker.record(ctx.port, ctx.mine, id, %{"x" => 1})
      assert length(Events.list(id)) == 2

      # And a second sweep does nothing more to it.
      assert Leases.reap() == []
      assert lease(id).requeues == 1
    end

    test "is refused at once, before any sweep has noticed it", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      lapse(id)

      assert %{status: 409} = StandInWorker.events(ctx.port, ctx.mine, id, beginning(id))
      assert Events.list(id) == []
      assert Workloads.get(id).state == "preparing", "not yet swept"
    end

    test "gives the workload to the next claim, which runs it from the start", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      StandInWorker.events(ctx.port, ctx.mine, id, beginning(id))
      lapse(id)

      # The claim looks for lapsed leases itself, so no timer is needed for the work to move.
      {:ok, %{token: second}} = DeviceTokens.issue(@person, "desktop")
      assert %{status: 200} = StandInWorker.claim(ctx.port, second)

      assert %{status: 200, body: %{"accepted" => 2}} =
               StandInWorker.events(ctx.port, second, id, beginning(id))

      assert Enum.map(Events.list(id), & &1.sequence) == [1, 2]
      assert lease(id).token_id != ctx.mine_record.id
    end

    test "after delivery began fails the workload instead of putting it back", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)

      sent = beginning(id) ++ [event(id, 3, %{"deliveryBegan" => %{}})]
      StandInWorker.events(ctx.port, ctx.mine, id, sent)

      lapse(id)
      assert [%{workload_id: ^id, result: :failed}] = Leases.reap()

      failed = Workloads.get(id)
      assert failed.state == "failed"
      assert failed.reason =~ "may have pushed"
      assert length(Events.list(id)) == 3, "what it said is kept as evidence"
      assert lease(id).requeues == 0
    end

    test "is put back at most twice, and the third loss fails it", ctx do
      id = queued()

      for round <- 1..2 do
        assert %{status: 200} = StandInWorker.claim(ctx.port, ctx.mine)
        lapse(id)
        assert [%{result: :requeued}] = Leases.reap()
        assert lease(id).requeues == round
      end

      assert %{status: 200} = StandInWorker.claim(ctx.port, ctx.mine)
      lapse(id)
      assert [%{result: :failed}] = Leases.reap()

      failed = Workloads.get(id)
      assert failed.state == "failed"
      assert failed.reason =~ "put back 2 times already"
      assert %{status: 204} = StandInWorker.claim(ctx.port, ctx.mine)
    end

    test "that said how it ended, and lost only its record, ends as it said", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      StandInWorker.events(ctx.port, ctx.mine, id, beginning(id) ++ [event(id, 3, finished())])
      assert Workloads.get(id).state == "completed"

      lapse(id)
      assert [%{result: :ended}] = Leases.reap()
      assert Workloads.get(id).state == "completed"
      assert length(Events.list(id)) == 3
    end

    test "is held off by every event and every heartbeat", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)

      near = DateTime.add(DateTime.utc_now(), 2, :second)

      set = fn ->
        Repo.update_all(from(l in WorkloadLease, where: l.workload_id == ^id),
          set: [expires_at: near]
        )
      end

      set.()
      StandInWorker.events(ctx.port, ctx.mine, id, beginning(id))
      assert DateTime.diff(lease(id).expires_at, DateTime.utc_now()) > 100

      set.()
      assert %{status: 200} = StandInWorker.heartbeat(ctx.port, ctx.mine, id)
      assert DateTime.diff(lease(id).expires_at, DateTime.utc_now()) > 100
      assert Leases.reap() == []
    end
  end

  describe "a workload its operator stopped" do
    test "is put back and claimed again, and the numbering goes on", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      StandInWorker.events(ctx.port, ctx.mine, id, beginning(id))

      assert %{status: 200, body: %{"result" => "requeued"}} =
               StandInWorker.release(ctx.port, ctx.mine, id)

      assert Workloads.get(id).state == "queued"
      assert lease(id).requeues == 1
      refute lease(id).active
      assert length(Events.list(id)) == 2, "what the stopped run said stays kept"

      assert %{status: 200, body: %{"nextSequence" => 3}} =
               StandInWorker.claim(ctx.port, ctx.mine)

      assert Workloads.get(id).state == "preparing"

      assert %{status: 200, body: %{"accepted" => 1}} =
               StandInWorker.events(ctx.port, ctx.mine, id, [event(id, 3, started())])

      assert Enum.map(Events.list(id), & &1.sequence) == [1, 2, 3]
    end

    test "after delivery began fails the workload", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      sent = beginning(id) ++ [event(id, 3, %{"deliveryBegan" => %{}})]
      StandInWorker.events(ctx.port, ctx.mine, id, sent)

      assert %{status: 200, body: %{"result" => "failed"}} =
               StandInWorker.release(ctx.port, ctx.mine, id)

      failed = Workloads.get(id)
      assert failed.state == "failed"
      assert failed.reason =~ "may have pushed"
      assert %{status: 204} = StandInWorker.claim(ctx.port, ctx.mine)
    end

    test "that has been put back as often as the link allows is failed", ctx do
      id = queued()

      for _ <- 1..2 do
        StandInWorker.claim(ctx.port, ctx.mine)
        assert %{status: 200} = StandInWorker.release(ctx.port, ctx.mine, id)
      end

      StandInWorker.claim(ctx.port, ctx.mine)

      assert %{status: 200, body: %{"result" => "failed"}} =
               StandInWorker.release(ctx.port, ctx.mine, id)

      assert Workloads.get(id).state == "failed"
    end

    test "is refused for a token that does not hold it", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      assert %{status: 409} = StandInWorker.release(ctx.port, ctx.theirs, id)
      assert Workloads.get(id).state == "preparing"
    end

    test "is not given back when the control plane cancelled it: that ends cancelled", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      sent = beginning(id) ++ [event(id, 3, finished("OUTCOME_CANCELLED", "cancelled"))]
      StandInWorker.events(ctx.port, ctx.mine, id, sent)
      record = %{"workloadId" => id, "outcome" => "OUTCOME_CANCELLED"}
      assert %{status: 200} = StandInWorker.record(ctx.port, ctx.mine, id, record)
      assert Workloads.get(id).state == "cancelled"
      assert %{status: 204} = StandInWorker.claim(ctx.port, ctx.mine)
    end
  end

  describe "a workload's way to an executor" do
    test "goes to its approver's worker only, and waits while that worker is not connected",
         ctx do
      id = queued(@person)

      # Someone else's worker is connected and busy asking; it is not given this one.
      for _ <- 1..3, do: assert(%{status: 204} = StandInWorker.claim(ctx.port, ctx.theirs))
      assert DeviceTokens.connected?(@other)
      refute DeviceTokens.connected?(@person), "its approver's worker has not been there"
      assert Workloads.get(id).state == "queued"
      assert lease(id) == nil

      # A workload does not time out for waiting.
      past = DateTime.add(DateTime.utc_now(), -30 * 86_400, :second)

      Repo.update_all(from(w in Workload, where: w.id == ^id),
        set: [inserted_at: past, updated_at: past]
      )

      assert [] = Leases.reap()
      assert Workloads.get(id).state == "queued"

      # Its approver's worker comes, and is given it.
      assert %{status: 200, body: %{"workload" => %{"id" => ^id}}} =
               StandInWorker.claim(ctx.port, ctx.mine)

      assert DeviceTokens.connected?(@person)
      assert Workloads.get(id).state == "preparing"
    end

    test "is matched on the approver's login without regard to case", ctx do
      {:ok, created} =
        workload()
        |> put_in(["approval", "approvedBy"], "ErinVersfeldCodes")
        |> Workloads.create()

      assert %{status: 200, body: %{"workload" => %{"id" => id}}} =
               StandInWorker.claim(ctx.port, ctx.mine)

      assert id == created.id
    end

    test "is the oldest first, and the one claimed is not offered again", ctx do
      first = queued()
      second = queued()

      Repo.update_all(from(w in Workload, where: w.id == ^first),
        set: [inserted_at: ~U[2026-11-01 00:00:00.000000Z]]
      )

      assert %{body: %{"workload" => %{"id" => ^first}}} = StandInWorker.claim(ctx.port, ctx.mine)

      assert %{body: %{"workload" => %{"id" => ^second}}} =
               StandInWorker.claim(ctx.port, ctx.mine)

      assert %{status: 204} = StandInWorker.claim(ctx.port, ctx.mine)
    end

    test "is never run by two workers: a workload held is not claimed again", ctx do
      id = queued()
      {:ok, %{token: second}} = DeviceTokens.issue(@person, "desktop")

      assert %{status: 200, body: %{"workload" => %{"id" => ^id}}} =
               StandInWorker.claim(ctx.port, ctx.mine)

      assert %{status: 204} = StandInWorker.claim(ctx.port, second)
      assert %{status: 204} = StandInWorker.claim(ctx.port, ctx.mine)
      assert lease(id).token_id == ctx.mine_record.id
    end

    test "is never run by two workers that claim at the same moment", ctx do
      ids = for _ <- 1..3, do: queued()

      tokens =
        for n <- 1..8 do
          {:ok, %{token: token}} = DeviceTokens.issue(@person, "worker #{n}")
          token
        end

      answers =
        tokens
        |> Task.async_stream(
          fn token -> StandInWorker.claim(ctx.port, token) end,
          max_concurrency: 8,
          timeout: 30_000
        )
        |> Enum.map(fn {:ok, answer} -> answer end)

      given = for %{status: 200, body: %{"workload" => %{"id" => id}}} <- answers, do: id
      assert length(given) == 3, "each of the three is given out"
      assert Enum.sort(given) == Enum.sort(ids), "and each only once"
      assert Enum.count(answers, &(&1.status == 204)) == 5
    end
  end

  describe "a workload that comes in over the link" do
    test "is stored as a local run of the same lines is: events, record, state and ledger", ctx do
      local = queued()
      remote = queued()

      costs = [
        %{"unit" => "COST_UNIT_USD", "source" => "SOURCE_REPORTED_BY_RUNTIME", "amount" => 1.25},
        %{
          "unit" => "COST_UNIT_TOKENS",
          "source" => "SOURCE_REPORTED_BY_RUNTIME",
          "amount" => 1000
        }
      ]

      kinds = [
        started(),
        workspace_ready(),
        agent_message("a binary file: ab" <> <<0>> <> "cd"),
        gate_started(),
        gate_finished(),
        %{"cost" => hd(costs)},
        finished()
      ]

      record = fn id ->
        %{
          "workloadId" => id,
          "outcome" => "OUTCOME_COMPLETED",
          "finishedAt" => "2026-11-22T07:00:00Z",
          "costs" => costs
        }
      end

      # The local run: the executor's lines come through its standard output.
      dir = Path.join(System.tmp_dir!(), "cp-link-#{System.unique_integer([:positive])}")
      on_exit(fn -> File.rm_rf(dir) end)
      lines = for e <- stream(local, 1, kinds), do: e

      Application.put_env(:control_plane, :executor_link, enabled: false)

      Application.put_env(:control_plane, :runner,
        executor:
          StandIn.executor(Path.join(dir, "stand-in"), events: lines, record: record.(local)),
        runs_dir: Path.join(dir, "runs"),
        silence_limit_ms: 5_000
      )

      on_exit(fn -> Application.delete_env(:control_plane, :runner) end)
      assert :ok = perform_job(ControlPlane.Jobs.RunWorkload, %{"workload_id" => local}, [])

      Application.put_env(:control_plane, :executor_link,
        enabled: true,
        claim_wait_ms: 60,
        lease_seconds: 120,
        reap_ms: 60_000
      )

      # The remote run: the same lines, over the link, in two requests.
      Repo.update_all(from(w in Workload, where: w.id == ^local),
        set: [inserted_at: ~U[2026-01-01 00:00:00.000000Z]]
      )

      Repo.update_all(from(w in Workload, where: w.id == ^remote),
        set: [inserted_at: DateTime.utc_now()]
      )

      assert %{body: %{"workload" => %{"id" => ^remote}}} =
               StandInWorker.claim(ctx.port, ctx.mine)

      sent = stream(remote, 1, kinds)
      {first, rest} = Enum.split(sent, 3)
      assert %{status: 200} = StandInWorker.events(ctx.port, ctx.mine, remote, first)
      assert %{status: 200} = StandInWorker.events(ctx.port, ctx.mine, remote, rest)
      assert %{status: 200} = StandInWorker.record(ctx.port, ctx.mine, remote, record.(remote))

      same_id = fn value ->
        value |> Jason.encode!() |> String.replace(remote, local) |> Jason.decode!()
      end

      stored = fn id ->
        for e <- Events.list(id), do: {e.sequence, e.time, e.kind, same_id.(e.event)}
      end

      assert stored.(remote) == stored.(local)
      assert length(stored.(remote)) == 7
      assert same_id.(Workloads.get(remote).record) == Workloads.get(local).record
      assert Workloads.get(remote).state == Workloads.get(local).state
      assert Workloads.get(remote).reason == Workloads.get(local).reason

      ledger = fn id ->
        Repo.all(
          from e in SpendEntry,
            where: e.workload_id == ^id,
            order_by: [e.step, e.unit],
            select: {e.step, e.kind, e.unit, e.source, e.amount, e.repository, e.ticket_number}
        )
      end

      assert ledger.(remote) != []
      assert ledger.(remote) == ledger.(local)
    end

    test "has no local job: with the link on, enqueueing leaves the workload for an executor",
         _ctx do
      {:ok, created} = Workloads.create(workload())
      assert {:ok, :waiting_for_executor} = Runner.enqueue(created.id)
      refute_enqueued(worker: ControlPlane.Jobs.RunWorkload)

      Application.put_env(:control_plane, :executor_link, enabled: false)
      assert {:ok, %Oban.Job{}} = Runner.enqueue(created.id)
      assert_enqueued(worker: ControlPlane.Jobs.RunWorkload)
    end

    test "is not started locally by a job left from before the link was turned on", _ctx do
      {:ok, created} = Workloads.create(workload())

      assert {:discard, _} =
               perform_job(ControlPlane.Jobs.RunWorkload, %{"workload_id" => created.id}, [])

      assert Workloads.get(created.id).state == "queued"
    end

    test "is not failed by a control plane that restarted while a worker held it", ctx do
      id = queued()
      StandInWorker.claim(ctx.port, ctx.mine)
      StandInWorker.events(ctx.port, ctx.mine, id, beginning(id))

      assert [] = ControlPlane.Recovery.run(DateTime.add(DateTime.utc_now(), 60, :second))
      assert Workloads.get(id).state == "running"
    end
  end

  describe "device tokens" do
    test "are kept as a hash, shown once, and never as themselves" do
      {:ok, %{token: token, record: record}} = DeviceTokens.issue(@person, "laptop")

      assert String.starts_with?(token, "hdt_")
      assert record.token_hash == :crypto.hash(:sha256, token) |> Base.encode16(case: :lower)

      {:ok, %{rows: rows}} = Ecto.Adapters.SQL.query(Repo, "SELECT * FROM device_tokens", [])
      refute inspect(rows) =~ token

      refute inspect(record) =~ record.token_hash,
             "and it does not appear when a record is printed"
    end

    test "are revoked only by their own login" do
      {:ok, %{record: record}} = DeviceTokens.issue(@person, "laptop")
      assert DeviceTokens.revoke(@other, record.id) == :not_found
      assert DeviceTokens.revoke(@person, "not a uuid") == :not_found
      assert DeviceTokens.revoke(@person, record.id) == :ok
      assert DeviceTokens.revoke(@person, record.id) == :not_found
    end

    test "never reach a log, whether they are made, used, refused or revoked", ctx do
      previous = Logger.level()
      Logger.configure(level: :debug)
      on_exit(fn -> Logger.configure(level: previous) end)

      log =
        capture_log([level: :debug], fn ->
          {:ok, %{token: fresh, record: record}} = DeviceTokens.issue(@person, "laptop")
          id = queued()
          StandInWorker.claim(ctx.port, fresh)
          StandInWorker.events(ctx.port, fresh, id, beginning(id))
          StandInWorker.claim(ctx.port, fresh <> "x")
          DeviceTokens.revoke(@person, record.id)
          StandInWorker.claim(ctx.port, fresh)
          send(self(), {:fresh, fresh})
        end)

      assert_received {:fresh, fresh}
      refute log =~ fresh
      refute log =~ ctx.mine
      refute log =~ "hdt_"
    end
  end

  describe "the routes a browser session writes with" do
    defp as(login), do: "heph_session=" <> Session.token(login, settings())

    defp browser(method, path, opts \\ []) do
      conn(method, path, opts[:body] || "")
      |> then(fn conn ->
        if c = opts[:cookie], do: put_req_header(conn, "cookie", c), else: conn
      end)
      |> then(fn conn ->
        if t = opts[:csrf], do: put_req_header(conn, "x-csrf-token", t), else: conn
      end)
      |> then(fn conn ->
        if t = opts[:bearer],
          do: put_req_header(conn, "authorization", "Bearer " <> t),
          else: conn
      end)
      |> then(fn conn ->
        if o = opts[:origin], do: put_req_header(conn, "origin", o), else: conn
      end)
      |> Router.call(Router.init(settings: settings()))
    end

    defp csrf_for(login), do: Csrf.token(settings(), login)
    defp body(conn), do: Jason.decode!(conn.resp_body)

    test "make a token with a session and a CSRF token, and show it once" do
      conn =
        browser(:post, "/api/device-tokens",
          cookie: as(@person),
          csrf: csrf_for(@person),
          body: ~s({"name":"laptop"})
        )

      assert conn.status == 201
      assert %{"token" => "hdt_" <> _ = token, "name" => "laptop", "id" => id} = body(conn)
      assert get_resp_header(conn, "cache-control") == ["no-store"]

      listed = browser(:get, "/api/device-tokens", cookie: as(@person))
      assert id in Enum.map(body(listed)["tokens"], & &1["id"])
      refute listed.resp_body =~ token

      # Nothing is held and the worker has said nothing, so it may run one and runs none.
      assert %{"slots" => 1, "running" => 0} =
               Enum.find(body(listed)["tokens"], &(&1["id"] == id))

      assert {:ok, _} = DeviceTokens.authenticate(token, settings())
    end

    test "refuse to make a token without a CSRF token" do
      conn =
        browser(:post, "/api/device-tokens", cookie: as(@person), body: ~s({"name":"laptop"}))

      assert conn.status == 403
      assert length(DeviceTokens.list(@person)) == 1, "only the one the test began with"
    end

    test "refuse a CSRF token that is another person's, forged, or from another origin" do
      for csrf <- [csrf_for(@other), "forged", csrf_for(@person) <> "x"] do
        conn =
          browser(:post, "/api/device-tokens",
            cookie: as(@person),
            csrf: csrf,
            body: ~s({"name":"laptop"})
          )

        assert conn.status == 403
      end

      conn =
        browser(:post, "/api/device-tokens",
          cookie: as(@person),
          csrf: csrf_for(@person),
          origin: "https://evil.test",
          body: ~s({"name":"laptop"})
        )

      assert conn.status == 403

      conn =
        browser(:post, "/api/device-tokens",
          cookie: as(@person),
          csrf: csrf_for(@person),
          origin: "https://heph.test",
          body: ~s({"name":"laptop"})
        )

      assert conn.status == 201
      assert DeviceTokens.list(@person) |> length() == 2
    end

    test "refuse to revoke without a CSRF token, and revoke with one" do
      {:ok, %{token: token, record: record}} = DeviceTokens.issue(@person, "laptop")

      refused = browser(:post, "/api/device-tokens/#{record.id}/revoke", cookie: as(@person))
      assert refused.status == 403
      assert {:ok, _} = DeviceTokens.authenticate(token, settings())

      done =
        browser(:post, "/api/device-tokens/#{record.id}/revoke",
          cookie: as(@person),
          csrf: csrf_for(@person)
        )

      assert done.status == 200
      assert DeviceTokens.authenticate(token, settings()) == :error
    end

    test "refuse a bearer token in place of a session" do
      {:ok, %{token: token}} = DeviceTokens.issue(@person, "laptop")

      for {method, path, body} <- [
            {:post, "/api/device-tokens", ~s({"name":"x"})},
            {:post, "/api/device-tokens/#{Ecto.UUID.generate()}/revoke", ""},
            {:get, "/api/device-tokens", ""}
          ] do
        conn = browser(method, path, bearer: token, csrf: csrf_for(@person), body: body)
        assert conn.status == 401, "#{method} #{path} gave #{conn.status}"
      end

      assert length(DeviceTokens.list(@person)) == 2, "the one the test began with, and this one"
    end

    test "do not let a person revoke another's token" do
      {:ok, %{record: record}} = DeviceTokens.issue(@person, "laptop")

      conn =
        browser(:post, "/api/device-tokens/#{record.id}/revoke",
          cookie: as(@other),
          csrf: csrf_for(@other)
        )

      assert conn.status == 404
    end
  end

  describe "the routes a worker writes with" do
    test "refuse a browser session cookie, with or without a CSRF token" do
      id = queued()

      for {path, body} <- [
            {"/worker/claim", ""},
            {"/worker/workloads/#{id}/events", ~s({"events":[]})},
            {"/worker/workloads/#{id}/heartbeat", ""},
            {"/worker/workloads/#{id}/record", "{}"}
          ] do
        conn =
          conn(:post, path, body)
          |> put_req_header("cookie", "heph_session=" <> Session.token(@person, settings()))
          |> put_req_header("x-csrf-token", Csrf.token(settings(), @person))
          |> Router.call(Router.init(settings: settings()))

        assert conn.status == 401, "#{path} gave #{conn.status}"
      end

      assert Workloads.get(id).state == "queued"
    end
  end
end
