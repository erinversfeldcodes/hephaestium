defmodule ControlPlane.BoardColumnsTest do
  @moduledoc """
  The board reads only the columns it shows. A workload's `workload` and `record` documents run to
  tens of kilobytes each, so a query that selects either one moves them over the wire for every
  row on every load. These tests count the bytes the database returns for a board of tickets with
  large documents; the data is synthetic, so they prove which columns are selected, not what
  production moves.
  """

  use ControlPlane.DataCase, async: false

  alias ControlPlane.Api.Read
  alias ControlPlane.{PullRequests, Repo, Workload}
  alias ControlPlane.WebhookFixtures, as: W

  @repo W.repository()
  @tickets 20
  @document_bytes 40_000

  setup do
    saved = Application.fetch_env(:control_plane, :intake)
    Application.put_env(:control_plane, :intake, repositories: [@repo])

    on_exit(fn ->
      case saved do
        {:ok, value} -> Application.put_env(:control_plane, :intake, value)
        :error -> Application.delete_env(:control_plane, :intake)
      end
    end)

    now = DateTime.utc_now()
    big = String.duplicate("x", @document_bytes)

    rows =
      for ticket <- 1..@tickets, run <- 1..2 do
        %{
          id: Ecto.UUID.generate(),
          repository: @repo,
          ticket_number: ticket,
          state: "completed",
          started_at: now,
          inserted_at: NaiveDateTime.add(NaiveDateTime.utc_now(:second), run, :second),
          updated_at: NaiveDateTime.utc_now(:second),
          workload: %{"ticket" => %{"title" => "t#{ticket}"}, "pad" => big},
          record: %{
            "pullRequest" => %{"url" => "https://github.com/#{@repo}/pull/#{ticket}"},
            "gates" => [%{"name" => "test", "result" => %{"outcome" => "failed"}}],
            "pad" => big
          }
        }
      end

    Repo.insert_all(Workload, rows)
    :ok
  end

  # The bytes of every row the database returns while `fun` runs.
  defp returned_bytes(fun) do
    test = self()
    id = "returned-bytes-#{System.unique_integer([:positive])}"

    :telemetry.attach(
      id,
      [:control_plane, :repo, :query],
      fn _event, _measure, meta, _ ->
        with {:ok, %{rows: rows}} when is_list(rows) <- meta.result,
             do: send(test, {:rows, :erlang.external_size(rows)})
      end,
      nil
    )

    try do
      fun.()
      collect(0)
    after
      :telemetry.detach(id)
    end
  end

  defp collect(total) do
    receive do
      {:rows, bytes} -> collect(total + bytes)
    after
      0 -> total
    end
  end

  test "the board query selects no whole document" do
    bytes = returned_bytes(fn -> Read.tickets(@tickets, 0) end)

    # Selecting either document of even one row would be 40,000 bytes on its own.
    assert bytes < @document_bytes, "the board returned #{bytes} bytes"
  end

  test "the board still names the failed gate, the kind and the runs of each ticket" do
    [ticket | _] = Read.tickets(1, 0)

    assert [%{kind: "implementation", failed_gate: "test"}, %{failed_gate: "test"}] = ticket.runs
  end

  test "the pull request lookup selects no whole document" do
    keys = for ticket <- 1..@tickets, do: {@repo, ticket}
    bytes = returned_bytes(fn -> PullRequests.for_tickets(keys) end)

    assert bytes < @document_bytes, "the lookup returned #{bytes} bytes"
  end

  describe "kind, lens and failed gate computed in the database" do
    @gate_cases [
      {"timed out", [%{"name" => "a", "result" => %{"outcome" => "timed out"}}], "a"},
      {"timed_out", [%{"name" => "a", "result" => %{"outcome" => "timed_out"}}], "a"},
      {"passed only", [%{"name" => "a", "result" => %{"outcome" => "passed"}}], nil},
      {"gates not an array", %{"name" => "a"}, nil},
      {"nameless first", [%{"result" => %{"outcome" => "failed"}}], nil},
      {"first failed wins",
       [
         %{"name" => "ok", "result" => %{"outcome" => "passed"}},
         %{"name" => "first", "result" => %{"outcome" => "failed"}},
         %{"name" => "second", "result" => %{"outcome" => "failed"}}
       ], "first"}
    ]

    @document_cases [
      %{},
      %{"revision" => %{}},
      %{"review" => %{"lens" => "elixir"}},
      %{"review" => %{}},
      %{"completeness" => %{}},
      %{"gap" => %{}},
      %{"gap" => %{}, "completeness" => %{}},
      %{"gap" => %{}, "completeness" => %{}, "review" => %{"lens" => "spec"}},
      %{"gap" => %{}, "completeness" => %{}, "review" => %{}, "revision" => %{}}
    ]

    defp insert_one(ticket, workload, record) do
      Repo.insert_all(Workload, [
        %{
          id: Ecto.UUID.generate(),
          repository: @repo,
          ticket_number: ticket,
          state: "failed",
          started_at: DateTime.utc_now(),
          inserted_at: NaiveDateTime.utc_now(:second),
          updated_at: NaiveDateTime.utc_now(:second),
          workload: workload,
          record: record
        }
      ])

      [run] =
        Read.tickets(500, 0)
        |> Enum.find(&(&1.number == ticket))
        |> Map.fetch!(:runs)

      run
    end

    test "kind and lens equal what Read.kind/1 and the document give" do
      for {document, index} <- Enum.with_index(@document_cases) do
        run = insert_one(1_000 + index, document, %{})

        assert run.kind == Read.kind(document), "kind of #{inspect(document)}"
        assert run.lens == get_in(document, ["review", "lens"]), "lens of #{inspect(document)}"
      end
    end

    test "the failed gate follows the rule for each shape of gates" do
      for {{label, gates, expected}, index} <- Enum.with_index(@gate_cases) do
        run = insert_one(2_000 + index, %{}, %{"gates" => gates})
        assert run.failed_gate == expected, label
      end
    end
  end
end
