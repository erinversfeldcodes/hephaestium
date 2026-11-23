defmodule ControlPlane.Webhooks do
  @moduledoc """
  Learning from webhooks. A code host sends a request when something happens; the control
  plane verifies it, keeps the delivery's id and one job in a single transaction, and answers
  at once. The job reads the current state through the behaviours and acts on that, so a
  delivery is only ever a hint to look.

  Off unless a secret file is named. The secret is read from that file each time, so rotating
  it needs no restart, and it is never printed, logged or kept: a message names the setting.
  """

  import Ecto.Query

  alias Ecto.Multi
  alias ControlPlane.Jobs.{HandleEvent, SettleWorkload}
  alias ControlPlane.{Repo, Systems, Workload}
  alias ControlPlane.Webhooks.{Delivery, Event}

  require Logger

  @default_max_body_bytes 1_000_000
  @keep_seconds 7 * 86_400

  # The jobs that read the code host, which a webhook replaces and which stay as the backstop.
  @backstop [
    ControlPlane.Jobs.Intake,
    ControlPlane.Jobs.SettleTickets,
    ControlPlane.Jobs.KeepCurrent,
    ControlPlane.Jobs.EvaluateMerged,
    ControlPlane.Jobs.RollUpEpics,
    ControlPlane.Jobs.AnswerReplies
  ]

  @doc "The jobs that run as the backstop when webhooks are on."
  def backstop, do: @backstop

  @doc "The webhook settings, or nil when webhooks are off."
  def settings, do: Application.get_env(:control_plane, :webhooks)

  @doc "Whether the control plane takes webhooks."
  @spec enabled?() :: boolean()
  def enabled?, do: settings() != nil

  @doc "The most a delivery's body may be, which is less than what the host sends at most."
  @spec max_body_bytes() :: pos_integer()
  def max_body_bytes, do: (settings() || [])[:max_body_bytes] || @default_max_body_bytes

  @doc """
  Check, when the control plane starts, that the secret can be read and is not empty, so that it
  does not start accepting nothing. Raises naming the setting, never the value.
  """
  @spec check!() :: :ok
  def check! do
    case secret() do
      {:ok, _} ->
        :ok

      :error ->
        raise ArgumentError,
              "HEPH_WEBHOOK_SECRET_FILE does not name a readable, non-empty file (or HEPH_WEBHOOK_SECRET is empty): webhooks cannot be verified without a secret"
    end
  end

  defp secret do
    with {:ok, text} <- secret_text(settings() || []),
         secret when secret != "" <- String.trim(text) do
      {:ok, secret}
    else
      _ -> :error
    end
  end

  # Where the secret is: a file, or the variable of a name. It is read on every call, so a
  # rotated file needs no restart.
  defp secret_text(settings) do
    cond do
      is_binary(settings[:secret_file]) -> File.read(settings[:secret_file])
      is_binary(settings[:secret_variable]) -> System.fetch_env(settings[:secret_variable])
      true -> :error
    end
  end

  @doc """
  Take in a delivery from a source. Nothing about a delivery that does not verify is stored,
  enqueued or looked at beyond the signature. `headers` are the request's, lower case.
  """
  @spec receive(String.t(), [{String.t(), String.t()}], binary()) ::
          :accepted
          | :duplicate
          | :ignored
          | {:error, :not_found | :unavailable | :unauthorized | :bad_request | :failed}
  def receive(source, headers, body) do
    with {:ok, module} <- source_module(source),
         {:ok, secret} <- secret_or(:unavailable),
         :ok <- verify(module, secret, headers, body),
         {:ok, id} <- module_result(module.delivery_id(headers)),
         {:ok, event} <- normalise(module, headers, body) do
      if event_wanted?(event), do: enqueue(source, id, event), else: :ignored
    else
      :ignore -> :ignored
      {:error, reason} -> {:error, reason}
    end
  end

  defp source_module(source) do
    case Systems.webhook_source(source) do
      {:ok, module} -> {:ok, module}
      :error -> {:error, :not_found}
    end
  end

  defp secret_or(reason) do
    case secret() do
      {:ok, secret} -> {:ok, secret}
      :error -> {:error, reason}
    end
  end

  defp verify(module, secret, headers, body),
    do: if(module.verified?(secret, headers, body), do: :ok, else: {:error, :unauthorized})

  defp module_result({:ok, id}), do: {:ok, id}
  defp module_result(:error), do: {:error, :bad_request}

  defp normalise(module, headers, body) do
    case module.normalise(headers, body, DateTime.utc_now()) do
      {:ok, event} -> {:ok, event}
      :ignore -> :ignore
      :error -> {:error, :bad_request}
    end
  end

  # Only a repository the control plane serves is of interest.
  defp event_wanted?(%Event{repository: repository}),
    do: repository in (Application.get_env(:control_plane, :intake, [])[:repositories] || [])

  defp enqueue(source, id, event) do
    job = event |> Event.to_args() |> HandleEvent.new()

    result =
      Multi.new()
      |> Multi.insert_all(
        :delivery,
        Delivery,
        [
          %{
            id: Ecto.UUID.generate(),
            source: source,
            delivery_id: id,
            received_at: DateTime.utc_now()
          }
        ],
        on_conflict: :nothing,
        conflict_target: [:source, :delivery_id]
      )
      |> Multi.run(:job, fn _repo, %{delivery: {count, _}} ->
        if count == 1, do: Oban.insert(job), else: {:ok, :duplicate}
      end)
      |> Repo.transaction()

    case result do
      {:ok, %{job: :duplicate}} -> :duplicate
      {:ok, _} -> :accepted
      {:error, _step, reason, _} -> failed(reason)
    end
  end

  defp failed(reason) do
    Logger.error(
      "could not take in a webhook delivery: #{inspect(reason) |> String.slice(0, 200)}"
    )

    {:error, :failed}
  end

  @doc """
  Settle a workload when it ends, so that no hour passes before its pull request is looked at.
  No webhook says a workload ended, because the control plane caused it. A review that ends
  settles the workload it reviewed. Nothing is queued when webhooks are off: the poll is what
  settles then.
  """
  @spec wake_settling(%Workload{}) :: :ok
  def wake_settling(%Workload{} = workload) do
    with true <- enabled?(),
         id when is_binary(id) <- settling_target(workload) do
      %{"workload_id" => id} |> SettleWorkload.new() |> Oban.insert()
    end

    :ok
  end

  defp settling_target(%Workload{workload: %{"review" => _}, parent_id: parent}), do: parent

  defp settling_target(%Workload{record: %{"pullRequest" => %{"url" => url}}, id: id})
       when is_binary(url),
       do: id

  defp settling_target(_), do: nil

  @doc """
  A full pass of every job that reads the code host, once, as the control plane starts: a
  restart is when deliveries are lost, and the host does not send them again by itself.
  """
  @spec start_pass() :: :ok
  def start_pass do
    if enabled?(), do: for(job <- @backstop, do: Oban.insert(job.new(%{})))
    :ok
  end

  @doc """
  Say that the backstop found work a delivery should have woken, with its kind, so that how
  often a delivery is dropped can be seen and the backstop's period changed on evidence.
  """
  @spec missed(atom(), non_neg_integer()) :: :ok
  def missed(kind, count) do
    if enabled?() and count > 0,
      do: Logger.warning("missed delivery of #{kind}: the backstop found #{count} to act on")

    :ok
  end

  @doc "Forget the delivery ids older than a week, and say how many."
  @spec prune(DateTime.t()) :: non_neg_integer()
  def prune(now) do
    {count, _} =
      Repo.delete_all(
        from d in Delivery, where: d.received_at < ^DateTime.add(now, -@keep_seconds)
      )

    count
  end
end
