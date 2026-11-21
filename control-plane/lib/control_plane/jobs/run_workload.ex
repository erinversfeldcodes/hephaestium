defmodule ControlPlane.Jobs.RunWorkload do
  @moduledoc """
  Run one workload: start the executor on it, keep what it says as it says it, and
  end the workload from how the executor ends.

  Oban retries the dispatching of a workload (the executor could not be started),
  never the work inside it: once the executor has started, the job succeeds
  whatever the outcome, because an outcome is a result and not an error, and
  running it again would do the work again.
  """

  use Oban.Worker,
    queue: :workloads,
    max_attempts: 3,
    unique: [
      period: :infinity,
      keys: [:workload_id],
      states: [:available, :scheduled, :executing, :retryable]
    ]

  alias ControlPlane.{Events, Runner, Tickets, Workloads, Workspaces}

  @impl Oban.Worker
  def perform(%Oban.Job{args: %{"workload_id" => id}, attempt: attempt, max_attempts: max}) do
    case Workloads.claim(id) do
      {:ok, workload} -> dispatch(workload, attempt >= max)
      :already_claimed -> :ok
      :unknown -> {:discard, "there is no workload #{id}"}
    end
  end

  defp dispatch(workload, last_attempt?) do
    directory = run_directory(workload)

    with {:ok, executor} <- find_executor(),
         :ok <- File.mkdir_p(directory),
         :ok <-
           File.write(Path.join(directory, "workload.json"), Jason.encode!(workload.workload)) do
      supervise(workload, executor, directory)
    else
      {:error, reason} ->
        message = "could not start the executor: #{inspect(reason)}"

        if last_attempt? do
          Workloads.fail(workload.id, message)
          # It never ran, but the ticket should say what became of it.
          Tickets.started(workload)
          Tickets.ended(Workloads.get(workload.id))
        else
          Workloads.release(workload.id)
        end

        {:error, message}
    end
  end

  defp run_directory(workload) do
    Path.join([
      Runner.runs_dir(),
      String.replace(workload.repository, "/", "-"),
      "#{workload.ticket_number}-#{workload.id}"
    ])
  end

  defp usable(path) do
    cond do
      not ControlPlane.Programs.allowed?(path) ->
        {:error, {:refused, ControlPlane.Programs.refusal(path)}}

      File.regular?(path) ->
        {:ok, path}

      true ->
        {:error, {:no_such_executor, path}}
    end
  end

  defp find_executor do
    case Runner.config(:executor) || System.find_executable("executor") do
      nil -> {:error, :no_executor}
      path -> usable(path)
    end
  end

  # Start the executor, and read what it says until it ends.
  defp supervise(workload, executor, directory) do
    {:ok, _} = Registry.register(ControlPlane.RunnerRegistry, workload.id, nil)
    log = Path.join(directory, "executor.log")

    arguments = [
      "run",
      "--workload",
      Path.join(directory, "workload.json"),
      "--work-dir",
      Path.join(directory, "work"),
      "--record",
      Path.join(directory, "record.json"),
      "--deliver"
    ]

    # The executor's error output goes to its own file, and its standard output,
    # which is its events, comes to this process.
    port =
      Port.open({:spawn_executable, "/bin/sh"}, [
        :binary,
        :exit_status,
        :hide,
        :use_stdio,
        {:line, 1_048_576},
        {:args, ["-c", ~s(log=$1; shift; exec "$@" 2>>"$log"), "sh", log, executor | arguments]}
      ])

    # Read before anything slow: a fast executor may have exited, and then the port
    # no longer knows its process.
    os_pid = Port.info(port, :os_pid) |> elem(1)

    # The executor is running: the ticket stops looking approved.
    Tickets.started(workload)

    state = %{
      workload: workload,
      port: port,
      os_pid: os_pid,
      events_file: File.open!(Path.join(directory, "events.jsonl"), [:append]),
      directory: directory,
      buffer: "",
      stopping: nil,
      silence_limit: Runner.config(:silence_limit_ms, 120_000),
      grace: Runner.config(:kill_grace_ms, 10_000)
    }

    result = read(state)
    File.close(state.events_file)
    result
  end

  defp read(%{port: port, silence_limit: silence} = state) do
    receive do
      {^port, {:data, {:eol, chunk}}} ->
        line = state.buffer <> chunk
        read(line |> keep(%{state | buffer: ""}))

      {^port, {:data, {:noeol, chunk}}} ->
        read(%{state | buffer: state.buffer <> chunk})

      {^port, {:exit_status, status}} ->
        finish(state, status)

      :cancel ->
        read(stop(state, nil))
    after
      silence ->
        read(stop(state, "the executor went silent: nothing for #{silence} milliseconds"))
    end
  end

  # A line from the executor. Once the executor is being stopped because its
  # stream cannot be trusted, what it says is not kept: a stream that broke a rule
  # is not one to go on reading. A cancellation is different: the executor's
  # answer to being told to stop is how the workload ended, and is kept.
  defp keep(line, %{stopping: %{reason: reason}} = state) when reason != nil do
    IO.binwrite(state.events_file, line <> "\n")
    state
  end

  defp keep(line, state) do
    IO.binwrite(state.events_file, line <> "\n")

    with {:ok, event} <- Jason.decode(line),
         {:ok, _} <- Events.append(state.workload.id, event) do
      state
    else
      {:error, %Jason.DecodeError{}} ->
        stop(state, "the executor wrote a line that is not an event (not_an_event)")

      {:error, rule} ->
        stop(state, "the stream broke a rule (#{rule})")
    end
  end

  # Ask the executor to stop, and kill it if it has not by the end of the grace
  # period. `reason` is why the workload is failed, or nil for a cancellation,
  # which the executor says itself.
  defp stop(%{stopping: nil} = state, reason) do
    signal(state, "TERM")
    timer = Process.send_after(self(), {:kill, state.port}, state.grace)
    %{state | stopping: %{reason: reason, timer: timer}}
  end

  defp stop(state, _), do: state

  defp signal(%{os_pid: os_pid}, name),
    do: System.cmd("kill", ["-#{name}", Integer.to_string(os_pid)], stderr_to_stdout: true)

  defp finish(state, status) do
    id = state.workload.id
    record = Path.join(state.directory, "record.json")

    with {:ok, text} <- File.read(record), {:ok, map} <- Jason.decode(text) do
      Workloads.keep_record(id, map)
    end

    cond do
      state.stopping && state.stopping.reason ->
        Workloads.fail(id, state.stopping.reason)

      true ->
        Workloads.fail(id, "the executor exited (status #{status}) without saying how it ended")
    end

    Tickets.ended(Workloads.get(id))
    # Its record is kept; a completed run has no more use for its checkout.
    Workspaces.clean(state.directory)
    :ok
  end
end
