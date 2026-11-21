defmodule ControlPlane.RunnerTest do
  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.RunWorkload
  alias ControlPlane.{Events, Runner, StandIn, Workloads}

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-run-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)

    on_exit(fn ->
      Application.delete_env(:control_plane, :runner)
      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())
    %{dir: dir, id: record.id}
  end

  defp configure(dir, executor, extra \\ []) do
    Application.put_env(
      :control_plane,
      :runner,
      Keyword.merge(
        [
          executor: executor,
          runs_dir: Path.join(dir, "runs"),
          silence_limit_ms: 5_000,
          kill_grace_ms: 2_000
        ],
        extra
      )
    )
  end

  defp run(id, opts \\ []), do: perform_job(RunWorkload, %{"workload_id" => id}, opts)

  defp stream(id, outcome \\ "OUTCOME_COMPLETED", reason \\ "") do
    [
      event(id, 1, started()),
      event(id, 2, workspace_ready()),
      event(id, 3, agent_message()),
      event(id, 4, finished(outcome, reason))
    ]
  end

  defp run_dir(dir, id), do: Path.join(dir, "runs/erinversfeldcodes-gazebolang/12-#{id}")

  defp wait_until(what, condition) do
    deadline = System.monotonic_time(:millisecond) + 10_000

    Stream.repeatedly(fn ->
      if condition.(), do: :done, else: Process.sleep(50) && :waiting
    end)
    |> Enum.find(fn state ->
      state == :done or
        (System.monotonic_time(:millisecond) > deadline && flunk("timed out waiting for #{what}"))
    end)
  end

  describe "a workload that runs" do
    test "has its events kept in order, its state and record from the executor, and its lines kept",
         %{dir: dir, id: id} do
      events = stream(id)
      record = %{"workloadId" => id, "outcome" => "OUTCOME_COMPLETED", "eventCount" => "4"}
      configure(dir, StandIn.executor(Path.join(dir, "stand-in"), events: events, record: record))

      assert :ok = run(id)

      done = Workloads.get(id)
      assert done.state == "completed"
      assert done.record == record
      assert Enum.map(Events.list(id), & &1.event) == events

      directory = run_dir(dir, id)
      assert File.read!(Path.join(directory, "workload.json")) |> Jason.decode!() == workload(id)

      assert File.read!(Path.join(directory, "events.jsonl")) ==
               Enum.map_join(events, &(Jason.encode!(&1) <> "\n")),
             "the lines are kept as they came"

      args = File.read!(Path.join(dir, "stand-in/args.txt")) |> String.split("\n", trim: true)
      assert Enum.at(args, 0) == "run"
      assert ["--workload", Path.join(directory, "workload.json")] -- args == []
      assert ["--work-dir", Path.join(directory, "work")] -- args == []
      assert ["--record", Path.join(directory, "record.json")] -- args == []
      assert "--deliver" in args
    end

    test "that completed has its checkout deleted and what it said kept; one that failed keeps both",
         %{dir: dir, id: id} do
      configure(dir, StandIn.executor(Path.join(dir, "stand-in"), events: stream(id)))
      File.mkdir_p!(Path.join(run_dir(dir, id), "work/repo"))
      File.write!(Path.join(run_dir(dir, id), "work/repo/file"), "checkout")

      assert :ok = run(id)

      refute File.exists?(Path.join(run_dir(dir, id), "work"))
      assert File.exists?(Path.join(run_dir(dir, id), "events.jsonl"))

      {:ok, failing} = Workloads.create(workload())
      # The end is stamped when the executor says it, which is now; the fixtures' own
      # times are days ago and would put the end outside the window.
      events =
        failing.id
        |> stream("OUTCOME_FAILED", "gate test failed")
        |> List.replace_at(
          3,
          event(failing.id, 4, finished("OUTCOME_FAILED", "gate test failed"), DateTime.utc_now())
        )

      configure(dir, StandIn.executor(Path.join(dir, "stand-in-2"), events: events))
      File.mkdir_p!(Path.join(run_dir(dir, failing.id), "work/repo"))

      assert :ok = run(failing.id)

      assert File.exists?(Path.join(run_dir(dir, failing.id), "work/repo"))
    end

    test "that failed ends failed, with the reason the executor gave", %{dir: dir, id: id} do
      events = stream(id, "OUTCOME_FAILED", "gate test failed")
      configure(dir, StandIn.executor(Path.join(dir, "stand-in"), events: events))

      assert :ok = run(id)

      done = Workloads.get(id)
      assert done.state == "failed"
      assert done.reason == "gate test failed"
    end

    test "when the executor exits without saying how it ended ends failed with its exit status",
         %{dir: dir, id: id} do
      events = Enum.take(stream(id), 3)
      configure(dir, StandIn.executor(Path.join(dir, "stand-in"), events: events, exit: 3))

      assert :ok = run(id)

      done = Workloads.get(id)
      assert done.state == "failed"
      assert done.reason =~ "without saying how it ended"
      assert done.reason =~ "3"
      assert length(Events.list(id)) == 3
    end
  end

  describe "a stream that cannot be trusted" do
    test "ends the workload failed naming the rule, stops the executor and keeps nothing after it",
         %{dir: dir, id: id} do
      events = [event(id, 1, started()), event(id, 3, agent_message())]
      configure(dir, StandIn.executor(Path.join(dir, "stand-in"), events: events, then: :hang))

      assert :ok = run(id)

      done = Workloads.get(id)
      assert done.state == "failed"
      assert done.reason =~ "gap_in_sequence"
      assert length(Events.list(id)) == 1

      wait_until("the executor to stop", fn ->
        not StandIn.running?(Path.join(dir, "stand-in"))
      end)
    end

    test "includes a line that is not an event", %{dir: dir, id: id} do
      events = [event(id, 1, started()), "this is not json"]
      configure(dir, StandIn.executor(Path.join(dir, "stand-in"), events: events, then: :hang))

      assert :ok = run(id)

      done = Workloads.get(id)
      assert done.state == "failed"
      assert done.reason =~ "not_an_event"

      wait_until("the executor to stop", fn ->
        not StandIn.running?(Path.join(dir, "stand-in"))
      end)
    end
  end

  test "an executor that goes silent ends the workload failed and is stopped", %{dir: dir, id: id} do
    events = [event(id, 1, started())]

    configure(dir, StandIn.executor(Path.join(dir, "stand-in"), events: events, then: :hang),
      silence_limit_ms: 1_500
    )

    assert :ok = run(id)

    done = Workloads.get(id)
    assert done.state == "failed"
    assert done.reason =~ "went silent"
    wait_until("the executor to stop", fn -> not StandIn.running?(Path.join(dir, "stand-in")) end)
  end

  describe "an executor that will not stop" do
    test "is killed at the end of the grace period, and the workload ends failed",
         %{dir: dir, id: id} do
      events = [event(id, 1, started())]
      stand_in = Path.join(dir, "stand-in")

      configure(dir, StandIn.executor(stand_in, events: events, then: :stubborn),
        silence_limit_ms: 1_500,
        kill_grace_ms: 300
      )

      task = Task.async(fn -> run(id) end)

      assert {:ok, :ok} = Task.yield(task, 10_000) || Task.shutdown(task, :brutal_kill),
             "the job is still waiting on an executor that ignores being told to stop"

      done = Workloads.get(id)
      assert done.state == "failed"
      assert done.reason =~ "went silent"
      wait_until("the executor to be gone", fn -> not StandIn.running?(stand_in) end)
    end

    test "that stops when told to is not killed", %{dir: dir, id: id} do
      events = [event(id, 1, started())]
      stand_in = Path.join(dir, "stand-in")

      configure(dir, StandIn.executor(stand_in, events: events, then: :hang),
        silence_limit_ms: 1_500,
        kill_grace_ms: 300
      )

      assert :ok = run(id)

      # Left alone past its grace period, the run has ended as it did before, by its own stop.
      Process.sleep(600)
      assert Workloads.get(id).reason =~ "went silent"
      refute StandIn.running?(stand_in)
    end
  end

  describe "cancelling" do
    test "a running workload ends cancelled, as the executor says", %{dir: dir, id: id} do
      running = [event(id, 1, started()), event(id, 2, workspace_ready())]

      after_stop = [
        event(id, 3, finished("OUTCOME_CANCELLED", "cancelled by a signal"))
      ]

      record = %{"workloadId" => id, "outcome" => "OUTCOME_CANCELLED"}

      configure(
        dir,
        StandIn.executor(Path.join(dir, "stand-in"),
          events: running,
          then: {:stop_with, after_stop, record}
        )
      )

      job = Task.async(fn -> run(id) end)
      wait_until("the workload to be running", fn -> Workloads.get(id).state == "running" end)

      assert :ok = Runner.cancel(id)
      assert :ok = Task.await(job, 15_000)

      done = Workloads.get(id)
      assert done.state == "cancelled"
      assert done.reason == "cancelled by a signal"
      assert done.record == record
    end

    test "a workload that is not running cannot be cancelled", %{id: id} do
      assert {:error, :not_running} = Runner.cancel(id)
    end
  end

  describe "an executor that cannot be started" do
    test "is retried, and on the last attempt ends the workload failed", %{dir: dir, id: id} do
      configure(dir, Path.join(dir, "nowhere/executor"))

      assert {:error, _} = run(id, attempt: 1, max_attempts: 3)
      assert Workloads.get(id).state == "queued", "it is tried again, so it is not failed yet"

      assert {:error, _} = run(id, attempt: 3, max_attempts: 3)
      done = Workloads.get(id)
      assert done.state == "failed"
      assert done.reason =~ "could not start the executor"
    end
  end

  test "a job that runs twice runs the workload once", %{dir: dir, id: id} do
    events = stream(id)
    configure(dir, StandIn.executor(Path.join(dir, "stand-in"), events: events))

    assert :ok = run(id)
    assert :ok = run(id)

    calls = File.read!(Path.join(dir, "stand-in/calls.log")) |> String.split("\n", trim: true)
    assert length(calls) == 1
    assert length(Events.list(id)) == 4
  end

  test "a workload is put on the queue once, however often it is asked", %{id: id} do
    assert {:ok, _} = Runner.enqueue(id)
    assert {:ok, _} = Runner.enqueue(id)

    assert [_one] = all_enqueued(worker: RunWorkload)
  end
end
