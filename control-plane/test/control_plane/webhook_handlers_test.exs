defmodule ControlPlane.WebhookHandlersTest do
  @moduledoc """
  What each event does once its job runs. A delivery is a hint to look: the handler matches it
  against the database first and reads the host only for what belongs to a workload or a
  ticket the control plane has met, then calls what the poll calls. The host is a stand-in
  that records what it was asked, so these prove which reads are made and which are not; that
  GitHub answers in these shapes is not proved here.
  """

  use ControlPlane.DataCase, async: false
  use Oban.Testing, repo: ControlPlane.Repo

  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.{HandleEvent, SettleWorkload}
  alias ControlPlane.{Events, StandInCodeHost, StandInGitHub, StandInSystems, StandInTracker}
  alias ControlPlane.{Ticket, TicketEvent, Workloads}
  alias ControlPlane.Webhooks.{Event, Handle}
  alias ControlPlane.WebhookFixtures, as: W

  @repo W.repository()
  @approved_at "2026-11-18T07:00:00Z"
  @now ~U[2026-11-22 09:00:00Z]

  setup do
    start_supervised!(StandInSystems)

    dir = Path.join(System.tmp_dir!(), "cp-handlers-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)

    saved =
      for key <- [:systems, :intake, :settle],
          do: {key, Application.fetch_env(:control_plane, key)}

    Application.put_env(:control_plane, :systems, %{
      @repo => %{tracker: {StandInTracker, @repo}, code_host: {StandInCodeHost, @repo}}
    })

    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [@repo])
    Application.put_env(:control_plane, :settle, completeness: [@repo], closures: [@repo])

    on_exit(fn ->
      for {key, saved} <- saved do
        case saved do
          {:ok, value} -> Application.put_env(:control_plane, key, value)
          :error -> Application.delete_env(:control_plane, key)
        end
      end

      File.rm_rf(dir)
    end)

    %{gh: gh}
  end

  defp event(kind, fields \\ []) do
    struct!(
      %Event{
        kind: kind,
        source: "github",
        repository: @repo,
        number: 12,
        actor: "maint",
        at: @now,
        delivery_id: "d-1"
      },
      fields
    )
  end

  defp host_calls, do: StandInSystems.calls()

  # A workload that completed with a pull request, not yet settled.
  defp awaiting(number \\ 52) do
    {:ok, record} = Workloads.create(workload())
    id = record.id
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

    Workloads.keep_record(id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => "https://github.com/#{@repo}/pull/#{number}"}
    })

    Workloads.get(id)
  end

  defp settled(number) do
    w = awaiting(number)
    Workloads.settle(w.id)
    Workloads.get(w.id)
  end

  defp stand_in_event(id, sequence, kind), do: event(id, sequence, kind)

  # Each read of a change is for settling or keeping current, and one more is for the record of
  # the pull request.
  defp changes(calls),
    do: for({:code_host, :change, [_, number]} <- calls, do: to_string(number))

  # What was asked of the host other than the change itself, which the record of the pull
  # request reads whatever else is done.
  defp beyond_the_change(calls), do: Enum.reject(calls, &match?({:code_host, :change, _}, &1))

  describe "a ticket approved (what Intake does for each ticket)" do
    setup %{gh: gh} do
      StandInSystems.answer(
        :tracker,
        :ticket,
        {:ok, %Ticket{key: 12, state: :open, labels: ["heph:ready"]}}
      )

      StandInSystems.answer(
        :tracker,
        :ticket_history,
        {:ok,
         [
           %TicketEvent{
             kind: :labeled,
             label: "heph:ready",
             actor: "maint",
             at: ~U[2026-11-18 07:00:00Z],
             at_text: @approved_at
           }
         ]}
      )

      StandInSystems.answer(:tracker, :ticket_edits, {:ok, []})
      StandInSystems.answer(:code_host, :permission, {:ok, :write})

      built =
        workload()
        |> put_in(["ticket", "number"], "12")
        |> put_in(["approval"], %{"approvedBy" => "maint", "approvedAt" => @approved_at})

      StandInGitHub.heph_prints(gh, built)
      :ok
    end

    test "starts the workload the poll would have started, reading the one ticket" do
      assert Handle.run(event(:ticket_approved)) == :started

      assert [record] = Repo.all(ControlPlane.Workload)
      assert record.ticket_number == 12
      assert record.state == "queued"

      assert record.workload["approval"] == %{
               "approvedBy" => "maint",
               "approvedAt" => @approved_at
             }

      assert [_] = all_enqueued(worker: ControlPlane.Jobs.RunWorkload)

      refute Enum.any?(host_calls(), &match?({:tracker, :approved_tickets, _}, &1))
      assert {:tracker, :ticket, [@repo, 12]} in host_calls()
    end

    test "a second delivery for the same approval starts nothing more" do
      Handle.run(event(:ticket_approved))
      Handle.run(event(:ticket_approved, delivery_id: "d-2"))

      assert length(Repo.all(ControlPlane.Workload)) == 1
    end

    test "does not approve on an event's say-so: an approver who cannot write is refused" do
      StandInSystems.answer(:code_host, :permission, {:ok, :read})

      Handle.run(event(:ticket_approved, actor: "maint"))

      assert Repo.all(ControlPlane.Workload) == []
    end

    test "for a repository that is not configured reads nothing" do
      Handle.run(event(:ticket_approved, repository: "someone/else"))

      assert host_calls() == []
    end
  end

  describe "a ticket edited" do
    test "that the control plane has never met reads nothing" do
      Handle.run(event(:ticket_edited, cause: :edited))
      Handle.run(event(:ticket_edited, cause: :unapproved))

      assert host_calls() == []
    end

    test "that it has met is looked at again, as intake would" do
      awaiting()
      StandInSystems.answer(:tracker, :ticket, {:ok, %Ticket{key: 12, state: :open, labels: []}})
      StandInSystems.answer(:tracker, :ticket_history, {:ok, []})

      Handle.run(event(:ticket_edited, cause: :edited))

      assert {:tracker, :ticket, [@repo, 12]} in host_calls()
    end
  end

  describe "a ticket closed" do
    test "in a repository that does not judge completeness reads nothing" do
      Application.put_env(:control_plane, :settle, completeness: [])

      Handle.run(event(:ticket_closed))

      assert host_calls() == []
    end

    test "asks the tracker for its parent, and stops when there is none" do
      StandInSystems.answer(:tracker, :parent, {:ok, nil})

      Handle.run(event(:ticket_closed))

      assert host_calls() == [{:tracker, :parent, [@repo, 12]}]
    end

    test "rolls up the parent when its sub-tickets are all closed, as the poll's epic pass does" do
      StandInSystems.answer(
        :tracker,
        :parent,
        {:ok, %Ticket{key: 5, state: :open, progress: %{completed: 2, total: 2}}}
      )

      StandInSystems.answer(:tracker, :sub_tickets, {:ok, [%Ticket{key: 12, state: :closed}]})

      Handle.run(event(:ticket_closed))

      assert {:tracker, :sub_tickets, [@repo, 5]} in host_calls()
    end

    test "leaves the parent alone while a sub-ticket is open" do
      StandInSystems.answer(
        :tracker,
        :parent,
        {:ok, %Ticket{key: 5, state: :open, progress: %{completed: 1, total: 2}}}
      )

      Handle.run(event(:ticket_closed))

      assert host_calls() == [{:tracker, :parent, [@repo, 12]}]
    end
  end

  describe "a ticket reopened" do
    test "reads nothing when no ticket was closed on evidence" do
      Handle.run(event(:ticket_reopened))

      assert host_calls() == []
    end

    test "reads that ticket's history when it was, as the poll does" do
      {:ok, record} =
        Workloads.create(Map.put(workload(), "completeness", %{"skillDocument" => "x"}))

      {:ok, _} = Events.append(record.id, stand_in_event(record.id, 1, started()))

      {:ok, _} =
        Events.append(record.id, stand_in_event(record.id, 2, finished("OUTCOME_COMPLETED", "")))

      Workloads.keep_record(record.id, %{"completeness" => closing_verdict()})
      StandInSystems.answer(:tracker, :ticket_history, {:ok, []})

      Handle.run(event(:ticket_reopened))

      assert host_calls() == [{:tracker, :ticket_history, [@repo, 12, []]}]
    end

    defp closing_verdict do
      %{
        "verdict" => "met",
        "decides" => "evidence",
        "epic" => false,
        "requirements" => [
          %{
            "text" => "r",
            "settled_by" => "evidence",
            "status" => "met",
            "evidence" => %{"kind" => "test", "where" => "w"}
          }
        ],
        "owed" => [],
        "could_not_check" => []
      }
    end
  end

  describe "a comment" do
    test "on a change is dropped" do
      Handle.run(event(:comment_added, on: :change))

      assert host_calls() == []
    end

    test "on a ticket whose last step does not wait on a person reads nothing" do
      Handle.run(event(:comment_added, on: :ticket))

      assert host_calls() == []
    end

    test "on a ticket whose last step waits on a person reads its comments" do
      {:ok, step} = Workloads.create(Map.put(workload(), "gap", %{"step" => "question"}))
      {:ok, _} = Events.append(step.id, stand_in_event(step.id, 1, started()))
      Workloads.fail(step.id, "waiting")
      StandInSystems.answer(:tracker, :comments, {:ok, []})

      Handle.run(event(:comment_added, on: :ticket))

      assert host_calls() == [{:tracker, :comments, [@repo, 12, []]}]
    end

    test "on another ticket than the one waiting reads nothing" do
      {:ok, step} = Workloads.create(Map.put(workload(), "gap", %{"step" => "question"}))
      Workloads.fail(step.id, "waiting")

      Handle.run(event(:comment_added, on: :ticket, number: 99))

      assert host_calls() == []
    end
  end

  describe "a change event for a workload awaiting settlement" do
    for kind <- [:checks_completed, :change_closed, :change_merged, :change_updated] do
      test "#{kind} settles that workload, reading its change" do
        awaiting(52)
        StandInSystems.answer(:code_host, :change, {:error, "stand-in"})

        Handle.run(event(unquote(kind), number: 52, head: "abc"))

        assert changes(host_calls()) == ["52", "52"]
      end
    end

    test "for a change that is no workload's reads nothing" do
      awaiting(52)

      for kind <- [:checks_completed, :change_closed, :change_merged, :change_updated] do
        Handle.run(event(kind, number: 99, head: "abc"))
      end

      assert host_calls() == []
    end

    test "for a repository that is not configured reads nothing" do
      awaiting(52)

      Handle.run(event(:change_merged, repository: "someone/else", number: 52))

      assert host_calls() == []
    end

    test "checks that name no change settle each workload awaiting in that repository" do
      awaiting(52)
      awaiting(53)
      StandInSystems.answer(:code_host, :change, {:error, "stand-in"})

      Handle.run(event(:checks_completed, number: nil, head: "abc"))

      assert Enum.sort(changes(host_calls())) == ["52", "53"]
    end

    test "a delivery for one pull request does not load another ticket's workload" do
      mine = awaiting(52)
      other = awaiting(53)

      handler = "workloads-query-#{System.unique_integer([:positive])}"
      test_pid = self()

      :telemetry.attach(
        handler,
        [:control_plane, :repo, :query],
        fn _event, _measurements, metadata, _config ->
          send(test_pid, {:query, metadata.query, metadata.params})
        end,
        nil
      )

      on_exit(fn -> :telemetry.detach(handler) end)

      assert [^mine] = Workloads.for_change(@repo, 52)
      refute other.id in Enum.map(Workloads.for_change(@repo, 52), & &1.id)

      # The number is part of the query, so the other pull request's row never leaves the database.
      assert_received {:query, _query, params}
      assert "https://github.com/#{@repo}/pull/52" in params
    end

    test "checks that name no change load only that repository's workloads" do
      awaiting(52)
      assert [only] = Workloads.awaiting_settlement(@repo)
      assert only.repository == @repo
      assert Workloads.awaiting_settlement("someone/else") == []
    end

    test "a workload that is already settled is not settled again by a late repeat" do
      settled(52)

      Handle.run(event(:checks_completed, number: 52))
      Handle.run(event(:change_closed, number: 52))

      assert beyond_the_change(host_calls()) == []
    end
  end

  describe "a change event for a workload already settled" do
    test "a merge asks the judgement the poll asks, reading the change" do
      settled(52)

      StandInSystems.answer(
        :code_host,
        :change,
        {:ok, %ControlPlane.Change{merged: false, state: :open}}
      )

      Handle.run(event(:change_merged, number: 52))

      assert changes(host_calls()) == ["52", "52"]
    end

    test "a change updated is looked at as the poll keeps a pull request current" do
      settled(52)

      StandInSystems.answer(
        :code_host,
        :change,
        {:ok, %ControlPlane.Change{state: :open, mergeability: :clean}}
      )

      Handle.run(event(:change_updated, number: 52))

      assert changes(host_calls()) == ["52", "52"]
    end

    test "a change closed without a merge is nothing to do" do
      settled(52)

      Handle.run(event(:change_closed, number: 52))

      assert beyond_the_change(host_calls()) == []
    end
  end

  describe "the base moved" do
    test "reads nothing when no pull request of the repository is settled and open" do
      awaiting(52)

      Handle.run(event(:base_moved, number: nil, head: "def"))

      assert host_calls() == []
    end

    test "looks at each settled pull request of that repository, and only that repository" do
      settled(52)

      StandInSystems.answer(
        :code_host,
        :change,
        {:ok, %ControlPlane.Change{state: :open, mergeability: :clean}}
      )

      Handle.run(event(:base_moved, number: nil, head: "def"))
      assert changes(host_calls()) == ["52"]

      Handle.run(event(:base_moved, repository: "someone/else", number: nil))
      assert changes(host_calls()) == ["52"]
    end
  end

  describe "the job" do
    test "runs the handler for the event in its arguments" do
      awaiting(52)
      StandInSystems.answer(:code_host, :change, {:error, "stand-in"})

      args = event(:change_closed, number: 52) |> Event.to_args()
      assert :ok = perform_job(HandleEvent, args)

      assert changes(host_calls()) == ["52", "52"]
    end

    test "is one job for a delivery id, even if it is inserted twice" do
      args = event(:change_closed, number: 52) |> Event.to_args()

      {:ok, _} = args |> HandleEvent.new() |> Oban.insert()
      {:ok, _} = args |> HandleEvent.new() |> Oban.insert()

      assert length(all_enqueued(worker: HandleEvent)) == 1
    end

    test "arguments that are no event are discarded and read nothing" do
      assert {:cancel, _} = perform_job(HandleEvent, %{"kind" => "star"})
      assert host_calls() == []
    end

    test "a host that cannot be read is an error, so the job is tried again" do
      StandInSystems.answer(:tracker, :ticket, {:error, "the host is down"})

      assert {:error, _} =
               perform_job(HandleEvent, event(:ticket_approved) |> Event.to_args())
    end
  end

  describe "a workload that ends" do
    setup do
      Application.put_env(:control_plane, :webhooks, secret_file: "/nonexistent")
      on_exit(fn -> Application.delete_env(:control_plane, :webhooks) end)
    end

    test "enqueues its own settling, because no webhook says it ended" do
      w = awaiting(52)

      Workloads.settle(w.id) |> then(fn _ -> :ok end)
      Workloads.unsettle(w.id)

      ControlPlane.Webhooks.wake_settling(Workloads.get(w.id))

      assert [%{args: %{"workload_id" => id}}] = all_enqueued(worker: SettleWorkload)
      assert id == w.id
    end

    test "a review that ends enqueues the settling of the workload it reviewed" do
      parent = awaiting(52)

      {:ok, review} =
        Workloads.create(Map.put(workload(), "review", %{"lens" => "x"}), parent: parent.id)

      Workloads.keep_record(review.id, %{"outcome" => "OUTCOME_COMPLETED"})

      ControlPlane.Webhooks.wake_settling(Workloads.get(review.id))

      assert [%{args: %{"workload_id" => id}}] = all_enqueued(worker: SettleWorkload)
      assert id == parent.id
    end

    test "a workload without a pull request enqueues nothing" do
      {:ok, record} = Workloads.create(workload())

      ControlPlane.Webhooks.wake_settling(Workloads.get(record.id))

      assert all_enqueued(worker: SettleWorkload) == []
    end

    test "with webhooks off nothing is enqueued: the poll is what settles" do
      Application.delete_env(:control_plane, :webhooks)
      w = awaiting(52)

      ControlPlane.Webhooks.wake_settling(w)

      assert all_enqueued(worker: SettleWorkload) == []
    end

    test "the job settles the workload, once" do
      w = awaiting(52)
      StandInSystems.answer(:code_host, :change, {:error, "stand-in"})

      assert :ok = perform_job(SettleWorkload, %{"workload_id" => w.id})
      assert changes(host_calls()) == ["52", "52"]

      # Settled now: it is not settled again, and only the record of the pull request is read.
      Workloads.settle(w.id)
      assert :ok = perform_job(SettleWorkload, %{"workload_id" => w.id})
      assert changes(host_calls()) == ["52", "52", "52"]
    end
  end

  describe "the backstop" do
    import ExUnit.CaptureLog

    setup do
      level = Logger.level()
      Logger.configure(level: :warning)
      on_exit(fn -> Logger.configure(level: level) end)
    end

    defp approvable(gh) do
      StandInSystems.answer(
        :tracker,
        :approved_tickets,
        {:ok, [%Ticket{key: 12, state: :open, labels: ["heph:ready"]}]}
      )

      StandInSystems.answer(:tracker, :ensure_labels, :ok)

      StandInSystems.answer(
        :tracker,
        :ticket,
        {:ok, %Ticket{key: 12, state: :open, labels: ["heph:ready"]}}
      )

      StandInSystems.answer(
        :tracker,
        :ticket_history,
        {:ok,
         [
           %TicketEvent{
             kind: :labeled,
             label: "heph:ready",
             actor: "maint",
             at: ~U[2026-11-18 07:00:00Z],
             at_text: @approved_at
           }
         ]}
      )

      StandInSystems.answer(:tracker, :ticket_edits, {:ok, []})
      StandInSystems.answer(:code_host, :permission, {:ok, :write})

      built =
        workload()
        |> put_in(["ticket", "number"], "12")
        |> put_in(["approval"], %{"approvedBy" => "maint", "approvedAt" => @approved_at})

      StandInGitHub.heph_prints(gh, built)
    end

    test "finds an approval whose delivery was dropped, and says a delivery was missed", %{gh: gh} do
      approvable(gh)
      Application.put_env(:control_plane, :webhooks, secret_file: "/x")
      on_exit(fn -> Application.delete_env(:control_plane, :webhooks) end)

      log = capture_log(fn -> assert :ok = perform_job(ControlPlane.Jobs.Intake, %{}) end)

      assert [%{ticket_number: 12}] = Repo.all(ControlPlane.Workload)
      assert log =~ "missed delivery"
      assert log =~ "ticket_approved"
    end

    test "a pass that finds nothing to do says nothing", %{gh: gh} do
      approvable(gh)
      StandInSystems.answer(:tracker, :approved_tickets, {:ok, []})
      Application.put_env(:control_plane, :webhooks, secret_file: "/x")
      on_exit(fn -> Application.delete_env(:control_plane, :webhooks) end)

      log = capture_log(fn -> assert :ok = perform_job(ControlPlane.Jobs.Intake, %{}) end)

      refute log =~ "missed delivery"
    end

    test "with webhooks off the poll is not called a miss", %{gh: gh} do
      approvable(gh)

      log = capture_log(fn -> assert :ok = perform_job(ControlPlane.Jobs.Intake, %{}) end)

      assert [%{ticket_number: 12}] = Repo.all(ControlPlane.Workload)
      refute log =~ "missed delivery"
    end

    test "finds a settlement whose delivery was dropped" do
      awaiting(52)
      StandInSystems.answer(:code_host, :change, {:error, "stand-in"})

      assert :ok = perform_job(ControlPlane.Jobs.SettleTickets, %{})

      assert changes(host_calls()) == ["52"]
    end
  end
end
