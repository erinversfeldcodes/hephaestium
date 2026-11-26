defmodule ControlPlane.OneWorkloadPerTicketTest do
  @moduledoc """
  Two workloads for one ticket would push to the one branch, and the second to push is refused.
  A workload that changes the ticket's branch is not given while another that does is under way
  for that ticket; it stays queued, and is given when the first ends.
  """

  use ControlPlane.DataCase

  import Ecto.Query
  import ControlPlane.Fixtures

  alias ControlPlane.{DeviceTokens, Leases, Repo, Workload, Workloads}

  setup do
    {:ok, %{record: token}} = DeviceTokens.issue("erinversfeldcodes", "laptop")
    %{token: token}
  end

  defp queue(edit \\ & &1) do
    {:ok, record} =
      workload()
      |> put_in(["approval", "approvedBy"], "erinversfeldcodes")
      |> edit.()
      |> Workloads.create()

    record.id
  end

  test "a second workload of a ticket waits while the first is under way, then is given", %{
    token: token
  } do
    ids = [queue(), queue()]

    assert {:ok, %Workload{id: first}} = Leases.claim(token)
    [second] = ids -- [first]
    assert :none = Leases.claim(token)
    assert Workloads.get(second).state == "queued"

    Repo.update_all(from(w in Workload, where: w.id == ^first), set: [state: "completed"])

    assert {:ok, %Workload{id: ^second}} = Leases.claim(token)
  end

  test "a review of the ticket is not held up by the workload that changes its branch", %{
    token: token
  } do
    first = queue()
    review = queue(&put_in(&1, ["review"], %{"pullRequestUrl" => "", "attempt" => 1}))

    assert {:ok, %Workload{id: ^review}} = Leases.claim(token)
    assert {:ok, %Workload{id: ^first}} = Leases.claim(token)
  end

  test "the local runner's claim does not take a workload while another of the ticket is under way" do
    first = queue()
    second = queue()

    assert {:ok, %Workload{state: "preparing"}} = Workloads.claim(first)
    assert :ticket_busy = Workloads.claim(second)
    assert Workloads.get(second).state == "queued"

    Repo.update_all(from(w in Workload, where: w.id == ^first), set: [state: "completed"])

    assert {:ok, %Workload{id: ^second}} = Workloads.claim(second)
  end
end
