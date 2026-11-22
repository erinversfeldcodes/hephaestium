defmodule ControlPlane.SpendCommentTest do
  @moduledoc """
  What the ticket's status comment says about spend (ADR 0036): beside what the run that just
  ended spent, what the ticket has spent so far over all its workloads and by kind of step, and
  for a ticket under an epic, what the epic has spent over its tickets.
  """

  use ControlPlane.DataCase

  import ControlPlane.Fixtures
  import ControlPlane.SpendFixtures

  alias ControlPlane.{Events, StandInGitHub, Tickets, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @pr "https://github.com/erinversfeldcodes/gazebolang/pull/52"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-spend-comment-#{System.unique_integer([:positive])}")
    stand_in = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)

    Application.put_env(:control_plane, :intake,
      gh: stand_in.gh,
      heph: stand_in.heph,
      repositories: []
    )

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    %{gh: stand_in}
  end

  describe "spend_section/1" do
    test "says what the ticket has spent over all its workloads, and how many, each unit on its own" do
      spent(12, [usd(0.5), tokens(1_000.0)])
      spent(12, [usd(0.25), tokens(500.0)])
      spent(13, [usd(99.0)])

      section = Tickets.spend_section(workload_of(12))

      assert section =~ "Spent on this ticket so far: $0.750, 1500 tokens across 2 workloads."
      refute section =~ "99"
    end

    test "says what each kind of step spent, and leaves out a kind that spent nothing" do
      spent(12, [usd(0.9)],
        steps: [
          %{"kind" => "KIND_IMPLEMENTATION", "costs" => [usd(0.5)]},
          %{"kind" => "KIND_REVIEW", "lens" => "rust", "round" => 1, "costs" => [usd(0.4)]}
        ]
      )

      section = Tickets.spend_section(workload_of(12))

      assert section =~ "By kind of step: the implementation $0.500, reviewing $0.400"
      refute section =~ "fixing"
    end

    test "says nothing when nothing was spent" do
      {:ok, created} = Workloads.create(workload())

      assert Tickets.spend_section(Workloads.get(created.id)) == nil
    end

    test "for a ticket under an epic, says what the epic has spent over its tickets" do
      spent(12, [usd(0.5)], epic: 7)
      spent(13, [usd(2.0)], epic: 7)
      spent(14, [usd(50.0)], epic: 8)

      section = Tickets.spend_section(workload_of(12, epic: 7))

      assert section =~ "Its epic #7 has spent $2.500 across 2 tickets so far."
    end

    test "says nothing of an epic for a ticket that is not under one" do
      spent(12, [usd(0.5)])

      refute Tickets.spend_section(workload_of(12)) =~ "epic"
    end
  end

  describe "the status comment of a run that ended" do
    defp comment(gh) do
      {"POST", @issue <> "/comments", ["body=" <> body]} =
        Enum.find(StandInGitHub.writes(gh), fn {method, endpoint, _} ->
          method == "POST" and endpoint == @issue <> "/comments"
        end)

      body
    end

    test "says what the run spent, and then what the ticket has spent so far including it", %{
      gh: gh
    } do
      # An earlier workload of the ticket.
      spent(12, [usd(1.0)], epic: 7)
      spent(13, [usd(2.0)], epic: 7)

      {:ok, record} = Workloads.create(put_in(workload(), ["ticket", "parent"], "7"))
      id = record.id
      {:ok, _} = Events.append(id, event(id, 1, started()))
      {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

      Workloads.keep_record(id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => @pr, "number" => "52"},
        "costs" => [usd(0.5)]
      })

      assert :ok = Tickets.ended(Workloads.get(id))

      body = comment(gh)
      assert body =~ "Cost, as the runtime reported it: $0.500"
      assert body =~ "Spent on this ticket so far: $1.500 across 2 workloads."
      assert body =~ "Its epic #7 has spent $3.500 across 2 tickets so far."
    end
  end

  # The workload of a ticket, as the comment is written about it.
  defp workload_of(ticket, opts \\ []) do
    {:ok, created} =
      workload()
      |> put_in(["ticket", "number"], Integer.to_string(ticket))
      |> then(fn workload ->
        case opts[:epic] do
          nil -> workload
          epic -> put_in(workload, ["ticket", "parent"], Integer.to_string(epic))
        end
      end)
      |> Workloads.create()

    Workloads.get(created.id)
  end
end
