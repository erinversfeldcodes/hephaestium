defmodule ControlPlane.Api.Worker do
  @moduledoc """
  The routes an executor on a developer's machine uses, all opened by the executor, all behind
  a device token and never a session.

  The transport is a long poll with plain requests: a claim waits for a workload and answers
  when there is one or when the wait ends, and everything the executor says afterwards is a
  `POST`. Nothing is held open for long and nothing is pushed to the machine, so it needs no
  inbound port, and every request is one a proxy or a firewall treats as ordinary web
  traffic. A dropped connection costs one request, which the executor makes again.

  | Route | |
  | --- | --- |
  | `POST /worker/claim` | `{"slots": n}`, how many workloads the executor may run at once (one when it says nothing). The workload to run, as a lease, and the number its first event takes, or 204 when there is none. |
  | `POST /worker/workloads/:id/events` | `{"events": [...]}`: kept in order by the stream's rules. |
  | `POST /worker/workloads/:id/heartbeat` | Keeps the lease while nothing else is sent. |
  | `POST /worker/workloads/:id/record` | The run record, at the end. |

  | `POST /worker/workloads/held` | `{"ids": [...]}`: which of those workloads still need the workspace the executor made for them, as `{"held": [...]}`. Those not ended, and those that completed with a pull request not yet settled. The executor clears the others' workspaces after its retention period. |
  | `POST /worker/workloads/:id/release` | The operator stopped the executor: the workload is given back, to be queued again or failed as a lapse would decide. |

  A request about a workload the token does not hold, because the lease lapsed, was let go or
  was never its, is 409.
  """

  import Plug.Conn

  alias ControlPlane.{DeviceTokens, ExecutorLink, Leases, Workloads}

  @max_body 32_000_000

  def claim(conn, _params, settings, token) do
    DeviceTokens.seen(token)
    {conn, slots} = slots(conn)
    DeviceTokens.slots(token, slots)
    deadline = System.monotonic_time(:millisecond) + ExecutorLink.config(:claim_wait_ms)
    wait(conn, token, settings.poll_ms, deadline)
  end

  # How many workloads the executor says it may run at once. A claim with no body, or one that
  # does not say, is an executor that runs one at a time.
  defp slots(conn) do
    case body(conn) do
      {:ok, %{"slots" => slots}, conn} -> {conn, slots}
      {:ok, _, conn} -> {conn, 1}
      :error -> {conn, 1}
    end
  end

  defp wait(conn, token, poll_ms, deadline) do
    cond do
      # Revoked while it waited: it is answered as any request with a revoked token is.
      not DeviceTokens.active?(token.id) ->
        json(conn, 401, %{error: "not authorized"})

      true ->
        case Leases.claim(token) do
          {:ok, workload} ->
            json(conn, 200, %{
              workload: workload.workload,
              lease: %{seconds: ExecutorLink.config(:lease_seconds)},
              # 1 for a workload that has not run; after a lost run the numbering goes on, so
              # what the lost run said stays kept beside what the next one says.
              nextSequence: Leases.next_sequence(workload.id)
            })

          :none ->
            if System.monotonic_time(:millisecond) + poll_ms >= deadline do
              DeviceTokens.seen(token)
              send_resp(conn, 204, "")
            else
              Process.sleep(poll_ms)
              wait(conn, token, poll_ms, deadline)
            end
        end
    end
  end

  def events(conn, %{"id" => id}, _settings, token) do
    DeviceTokens.seen(token)

    with {:ok, id} <- cast(id),
         {:ok, %{"events" => events}, conn} when is_list(events) <- body(conn) do
      case Leases.ingest(token, id, events) do
        {:ok, count, expires_at} ->
          json(conn, 200, %{accepted: count, leaseEndsAt: expires_at})

        {:error, :not_held} ->
          not_held(conn)

        {:error, {:rule, rule, accepted}} ->
          json(conn, 422, %{error: Atom.to_string(rule), accepted: accepted, failed: true})
      end
    else
      _ -> json(conn, 400, %{error: ~s(a body of {"events": [...]} is needed)})
    end
  end

  def heartbeat(conn, %{"id" => id}, _settings, token) do
    DeviceTokens.seen(token)

    with {:ok, id} <- cast(id),
         {:ok, expires_at} <- Leases.heartbeat(token, id) do
      json(conn, 200, %{leaseEndsAt: expires_at})
    else
      :error -> json(conn, 400, %{error: "not a workload id"})
      {:error, :not_held} -> not_held(conn)
    end
  end

  def record(conn, %{"id" => id}, _settings, token) do
    DeviceTokens.seen(token)

    with {:ok, id} <- cast(id),
         {:ok, %{} = record, conn} <- body(conn) do
      case Leases.finish(token, id, record) do
        :ok -> json(conn, 200, %{kept: true})
        {:error, :not_held} -> not_held(conn)
      end
    else
      _ -> json(conn, 400, %{error: "the record must be a JSON object"})
    end
  end

  def held(conn, _params, _settings, token) do
    DeviceTokens.seen(token)

    with {:ok, %{"ids" => ids}, conn} when is_list(ids) <- body(conn),
         {:ok, ids} <- cast_all(ids) do
      json(conn, 200, %{held: Workloads.held(ids)})
    else
      _ -> json(conn, 400, %{error: ~s(a body of {"ids": [...]} of workload ids is needed)})
    end
  end

  defp cast_all(ids) do
    Enum.reduce_while(ids, {:ok, []}, fn id, {:ok, cast} ->
      case cast(id) do
        {:ok, id} -> {:cont, {:ok, [id | cast]}}
        :error -> {:halt, :error}
      end
    end)
  end

  def release(conn, %{"id" => id}, _settings, token) do
    DeviceTokens.seen(token)

    with {:ok, id} <- cast(id),
         {:ok, result} <- Leases.give_back(token, id) do
      json(conn, 200, %{result: Atom.to_string(result)})
    else
      :error -> json(conn, 400, %{error: "not a workload id"})
      {:error, :not_held} -> not_held(conn)
    end
  end

  defp body(conn) do
    with {:ok, text, conn} <- read_body(conn, length: @max_body),
         {:ok, decoded} <- Jason.decode(text) do
      {:ok, decoded, conn}
    else
      _ -> :error
    end
  end

  defp cast(id) do
    case Ecto.UUID.cast(id) do
      {:ok, id} -> {:ok, id}
      :error -> :error
    end
  end

  defp not_held(conn),
    do: json(conn, 409, %{error: "this token does not hold the workload"})

  defp json(conn, status, body) do
    conn |> put_resp_content_type("application/json") |> send_resp(status, Jason.encode!(body))
  end
end
