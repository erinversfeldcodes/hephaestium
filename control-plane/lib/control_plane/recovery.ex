defmodule ControlPlane.Recovery do
  @moduledoc """
  What becomes of the workloads a stopped control plane left running (#133).

  A workload that was preparing, running or gating when the control plane stopped has no
  one reading its executor any more: the executor exits on its broken pipe, and nothing
  says so. Without this the workload stays `running` for ever and its ticket
  `heph:running`. When the control plane starts, each such workload is failed, saying the
  control plane stopped and the run was lost, and its ticket is told as it is for any
  failed workload: the failed label, the running one removed, a comment.

  Only a workload last updated before this control plane began is recovered, so one that a
  job has started since is never failed. A `queued` workload is left alone, because its
  job is still in the queue. A workload that was lost is not re-run: a person re-approves
  the ticket, or resumes its pull request.
  """

  import Ecto.Query

  require Logger

  alias ControlPlane.{Repo, Tickets, Workload, Workloads}

  @in_flight ["preparing", "running", "gating"]
  @queue "workloads"
  @reason "the control plane stopped while this ran, so the run was lost"

  @doc """
  Fail every workload that was in flight before `started_at`, and tell its ticket.
  Returns what became of each. A write to GitHub that fails is that workload's result and
  does not stop the others.
  """
  @spec run(DateTime.t()) :: [%{workload_id: String.t(), result: :ok | {:error, String.t()}}]
  def run(started_at \\ DateTime.utc_now()) do
    lost =
      Repo.all(
        from w in Workload,
          where: w.state in ^@in_flight and w.updated_at < ^started_at,
          # An executor on a developer's machine still holds this one, and goes on holding it
          # across a restart of the control plane: its lease lapsing is what loses it.
          where:
            w.id not in subquery(
              from l in ControlPlane.WorkloadLease, where: l.active, select: l.workload_id
            ),
          order_by: w.inserted_at
      )

    for workload <- lost do
      Workloads.fail(workload.id, @reason)
      result = Tickets.ended(Workloads.get(workload.id))

      if match?({:error, _}, result),
        do:
          Logger.warning(
            "recovered #{workload.id}, but could not tell its ticket: #{inspect(result)}"
          )

      %{workload_id: workload.id, result: result}
    end
  end

  @doc """
  End the jobs an earlier control plane left `executing`. A control plane that was killed
  leaves its running jobs as they were, and nothing else ends them: they stay `executing`
  for ever, and a job for a workload that is stuck there is what would stop it being queued
  again. Only a job last attempted before `started_at` is ended, so one this control plane
  has started is never touched; a job still waiting in the queue is left to run. Returns how
  many were ended.
  """
  @spec end_stale_jobs(DateTime.t()) :: non_neg_integer()
  def end_stale_jobs(started_at \\ DateTime.utc_now()) do
    {:ok, ended} =
      Oban.cancel_all_jobs(
        from j in Oban.Job,
          where: j.queue == ^@queue and j.state == "executing" and j.attempted_at < ^started_at
      )

    if ended > 0,
      do:
        Logger.warning(
          "ended #{ended} job(s) the control plane that ran them left executing when it stopped"
        )

    ended
  end
end
