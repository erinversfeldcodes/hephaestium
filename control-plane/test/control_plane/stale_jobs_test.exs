defmodule ControlPlane.StaleJobsTest do
  @moduledoc """
  A job a killed control plane left `executing` is ended when the next control plane starts,
  so it does not stay `executing` for ever: nothing else ever ends it, and it is what would
  stop its workload being queued again.
  """

  use ControlPlane.DataCase

  import Ecto.Query

  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.RunWorkload
  alias ControlPlane.{Recovery, Workloads}

  # A job for a new workload, in this state, last attempted this long before now.
  defp job(state, seconds_ago, queue \\ "workloads") do
    {:ok, record} = Workloads.create(workload())

    {:ok, job} =
      %{"workload_id" => record.id}
      |> RunWorkload.new(queue: queue)
      |> Oban.insert()

    attempted = DateTime.add(DateTime.utc_now(), -seconds_ago, :second)

    Repo.update_all(from(j in Oban.Job, where: j.id == ^job.id),
      set: [state: state, attempted_at: attempted]
    )

    job.id
  end

  defp state(id), do: Repo.get!(Oban.Job, id).state

  defp boot, do: DateTime.utc_now()

  test "a job an earlier control plane left executing is ended" do
    stale = job("executing", 600)

    assert Recovery.end_stale_jobs(boot()) == 1

    assert state(stale) == "cancelled"
  end

  test "a job started since this control plane began is left alone" do
    started = boot()
    running = job("executing", -5)

    assert Recovery.end_stale_jobs(started) == 0

    assert state(running) == "executing"
  end

  test "a job still waiting in the queue is left alone, whatever its age" do
    waiting = for state <- ["available", "scheduled", "retryable"], do: job(state, 100_000)

    assert Recovery.end_stale_jobs(boot()) == 0

    assert Enum.map(waiting, &state/1) == ["available", "scheduled", "retryable"]
  end

  test "a job in another queue is left alone" do
    other = job("executing", 600, "intake")

    assert Recovery.end_stale_jobs(boot()) == 0

    assert state(other) == "executing"
  end

  test "a job that has already ended is not touched again" do
    done = job("completed", 600)

    assert Recovery.end_stale_jobs(boot()) == 0

    assert state(done) == "completed"
  end

  test "with nothing to end nothing is ended" do
    assert Recovery.end_stale_jobs(boot()) == 0
  end
end
