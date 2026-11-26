defmodule ControlPlane.ConcurrencyTest do
  @moduledoc """
  Writers that really are concurrent. The usual test shares one connection among
  its processes, which makes them take turns, so a race could not show; here every
  process has a connection of its own, and what is written is truncated afterwards.
  """

  use ExUnit.Case, async: false

  import ControlPlane.Fixtures

  alias ControlPlane.{DeviceTokens, Events, Leases, Repo, Workload, Workloads}
  alias Ecto.Adapters.SQL.Sandbox

  setup do
    Sandbox.mode(Repo, :auto)

    on_exit(fn ->
      Repo.query!("TRUNCATE events, workloads, workload_leases, device_tokens CASCADE")
      Sandbox.mode(Repo, :manual)
    end)
  end

  test "two appends of the same sequence at once: exactly one wins" do
    id = Ecto.UUID.generate()
    {:ok, _} = Workloads.create(workload(id))
    {:ok, _} = Events.append(id, event(id, 1, started()))

    results =
      1..8
      |> Enum.map(fn n ->
        Task.async(fn -> Events.append(id, event(id, 2, agent_message("writer #{n}"))) end)
      end)
      |> Task.await_many()

    assert Enum.count(results, &match?({:ok, _}, &1)) == 1
    assert Enum.count(results, &(&1 == {:error, :repeated_sequence})) == 7
    assert length(Events.list(id)) == 2
  end

  test "two executors claiming at once for one ticket: only one is given a workload" do
    {:ok, %{record: token}} = DeviceTokens.issue("erinversfeldcodes", "laptop")

    for _ <- 1..2 do
      {:ok, _} =
        workload()
        |> put_in(["approval", "approvedBy"], "erinversfeldcodes")
        |> Workloads.create()
    end

    results =
      1..8
      |> Enum.map(fn _ -> Task.async(fn -> Leases.claim(token) end) end)
      |> Task.await_many()

    assert Enum.count(results, &match?({:ok, %Workload{}}, &1)) == 1
    assert Enum.count(results, &(&1 == :none)) == 7
  end
end
