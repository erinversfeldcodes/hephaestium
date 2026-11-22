defmodule ControlPlane.VerdictsTest do
  @moduledoc """
  What the verdicts of the reviews of a pull request mean for its ticket, and for
  merging it (ADR 0026).
  """

  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query
  import ControlPlane.Fixtures

  alias ControlPlane.{Events, Reviews, Settle, StandInGitHub, Workloads}
  alias ControlPlane.Workload

  @repo "erinversfeldcodes/gazebolang"
  @athenaeum "erinversfeldcodes/athenaeum"
  @ref "v0.17.0"
  @issue "repos/#{@repo}/issues/12"
  @pr_url "https://github.com/#{@repo}/pull/52"
  @sha "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5"
  @pull "repos/#{@repo}/pulls/52"
  @rules "repos/#{@repo}/rules/branches/main"
  @runs "repos/#{@repo}/commits/#{@sha}/check-runs?per_page=100"
  @files "repos/#{@repo}/pulls/52/files"
  @manifest "repos/#{@repo}/contents/.athenaeum/manifest.toml?ref=#{@sha}"
  @plan %{
    "lenses" => [
      %{"pack" => "elixir", "components" => ["web"]},
      %{"pack" => "rust", "components" => ["core"]}
    ],
    "unpacked" => [],
    "unowned" => []
  }

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-verdicts-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)
    previous_settle = Application.get_env(:control_plane, :settle)

    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])
    settings(revisions: 2)

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      Application.put_env(:control_plane, :settle, previous_settle)
      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())
    id = record.id
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

    Workloads.keep_record(id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => @pr_url}
    })

    StandInGitHub.respond(gh, @pull, pull())
    StandInGitHub.respond(gh, @rules, [])
    StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "completed", "success")]))

    StandInGitHub.respond(gh, @files, [
      %{"filename" => "web/lib/a.ex"},
      %{"filename" => "core/src/lib.rs"}
    ])

    StandInGitHub.respond_text(gh, @manifest, "schema_version = 0\n")
    StandInGitHub.heph_lenses(gh, @plan)
    lens(gh, "rust", "---\nname: rust\nblocking: true\n---\nthe rust lens")
    lens(gh, "elixir", "---\nname: elixir\nblocking: true\n---\nthe elixir lens")

    StandInGitHub.respond(gh, "repos/#{@athenaeum}/contents/packs?ref=#{@ref}", [
      %{"name" => "elixir", "type" => "dir"},
      %{"name" => "rust", "type" => "dir"}
    ])

    %{gh: gh, id: id}
  end

  defp settings(extra),
    do:
      Application.put_env(
        :control_plane,
        :settle,
        Keyword.merge([reviews: [@repo], athenaeum_ref: @ref], extra)
      )

  defp lens(gh, pack, text),
    do:
      StandInGitHub.respond_text(
        gh,
        "repos/#{@athenaeum}/contents/packs/#{pack}/review.md?ref=#{@ref}",
        text
      )

  defp pull(overrides \\ %{}),
    do:
      Map.merge(
        %{
          "state" => "open",
          "merged" => false,
          "head" => %{"sha" => @sha, "ref" => "heph/12-x", "repo" => %{"full_name" => @repo}},
          "base" => %{"ref" => "main"}
        },
        overrides
      )

  defp run(id, name, status, conclusion),
    do: %{"id" => id, "name" => name, "status" => status, "conclusion" => conclusion}

  defp runs(list), do: %{"total_count" => length(list), "check_runs" => list}

  defp result(id), do: Enum.find(Settle.run(), &(&1.workload_id == id)).result

  defp labels(gh),
    do: for({"POST", @issue <> "/labels", [field]} <- StandInGitHub.writes(gh), do: field)

  defp comments(gh),
    do: for({"POST", @issue <> "/comments", [body]} <- StandInGitHub.writes(gh), do: body)

  defp puts(gh), do: for({"PUT", endpoint, _} <- StandInGitHub.writes(gh), do: endpoint)

  defp reviews_of(id, sha) do
    Repo.all(
      from(w in Workload,
        where:
          w.parent_id == ^id and fragment("?->'review' IS NOT NULL", w.workload) and
            fragment("?->'repository'->>'baseCommit' = ?", w.workload, ^sha)
      )
    )
  end

  defp review(id, lens, sha \\ @sha),
    do: Enum.find(reviews_of(id, sha), &(&1.workload["review"]["lens"] == lens))

  # Ask for the reviews, as settling does when the checks are green.
  defp asked(id) do
    {:ok, _} = Reviews.request(Workloads.get(id), pull()["head"]["sha"], "52")
    id
  end

  # A review ends as `state`, with this verdict in its record.
  defp ended(id, lens, state, verdict \\ nil, sha \\ @sha) do
    review = review(id, lens, sha)
    record = if verdict, do: %{"review" => verdict}, else: %{}

    Repo.update_all(from(w in Workload, where: w.id == ^review.id),
      set: [state: state, record: record]
    )
  end

  defp verdict(lens, kind, findings \\ [], sha \\ @sha),
    do: %{
      "schema_version" => 0.0,
      "lens" => lens,
      "commit" => sha,
      "verdict" => kind,
      "findings" => findings,
      "notes" => ["a note"]
    }

  defp blocking(title),
    do: %{
      "severity" => "P1",
      "blocking" => true,
      "title" => title,
      "evidence" => "src/lib.rs:12 unwraps a Result",
      "revision" => "Return the error instead of unwrapping it.",
      # As the review contract has it, and as a protobuf Struct keeps a number: a float.
      "location" => %{"path" => "src/lib.rs", "line" => 12.0}
    }

  defp approve_all(id) do
    ended(id, "rust", "completed", verdict("rust", "approved"))
    ended(id, "elixir", "completed", verdict("elixir", "approved"))
  end

  defp revisions_of(id) do
    Repo.all(
      from(w in Workload,
        where: w.parent_id == ^id and fragment("?->'revision' IS NOT NULL", w.workload)
      )
    )
  end

  test "a review still running: the ticket waits, naming it, and nothing is written", %{
    gh: gh,
    id: id
  } do
    asked(id)
    ended(id, "rust", "completed", verdict("rust", "approved"))

    assert {:waiting, message} = result(id)

    assert message =~ "elixir"
    refute message =~ "rust"
    assert StandInGitHub.writes(gh) == []
    refute Workloads.get(id).settled_at
  end

  test "every lens approved: done, and the ticket says which lenses approved", %{gh: gh, id: id} do
    asked(id)
    approve_all(id)

    assert result(id) == :done

    assert labels(gh) == ["labels[]=heph:done"]
    comment = Enum.join(comments(gh))
    assert comment =~ "elixir"
    assert comment =~ "rust"
    assert comment =~ "approved"
    assert Workloads.get(id).settled_at
  end

  test "a blocking verdict starts a revision told each blocking finding", %{gh: gh, id: id} do
    asked(id)
    ended(id, "elixir", "completed", verdict("elixir", "approved"))

    ended(
      id,
      "rust",
      "completed",
      verdict("rust", "needs_revision", [blocking("Unwrap on a Result")])
    )

    assert result(id) == {:revising, 1}

    assert [revision] = revisions_of(id)
    assert revision.workload["revision"]["attempt"] == 1
    assert revision.workload["revision"]["branch"] == "heph/12-x"
    assert revision.workload["repository"]["baseCommit"] == @sha
    refute Map.has_key?(revision.workload, "review")
    assert [failure] = revision.workload["revision"]["failures"]
    assert failure["name"] =~ "rust"
    assert failure["name"] =~ "Unwrap on a Result"
    assert failure["excerpt"] =~ "P1"
    assert failure["excerpt"] =~ "src/lib.rs:12 unwraps a Result"
    assert failure["excerpt"] =~ "Return the error instead of unwrapping it."
    assert failure["excerpt"] =~ "Location: src/lib.rs:12\n"
    refute failure["excerpt"] =~ "12.0"
    assert Workloads.get(id).settled_at
    assert hd(comments(gh)) =~ "review"
    assert labels(gh) == []
  end

  # The locations a verdict may carry: the contract's object, and what it must survive.
  defp location_case(name) do
    %{
      "a string" => {"src/lib.rs:12", "Location: src/lib.rs:12\n"},
      "missing" => {:none, nil},
      "null" => {nil, nil},
      "a number" => {12, nil},
      "a list" => {["src/lib.rs", 12], nil},
      "an object without a line" => {%{"path" => "src/lib.rs"}, nil},
      "an object with a non-number line" => {%{"path" => "src/lib.rs", "line" => "twelve"}, nil},
      "an object with no path" => {%{"line" => 12.0}, nil},
      "an object with an empty path" => {%{"path" => "", "line" => 12.0}, nil}
    }
    |> Map.fetch!(name)
  end

  for name <- [
        "a string",
        "missing",
        "null",
        "a number",
        "a list",
        "an object without a line",
        "an object with a non-number line",
        "an object with no path",
        "an object with an empty path"
      ] do
    test "a finding whose location is #{name} still starts a revision, and tells it only if usable",
         %{id: id} do
      {location, expected} = location_case(unquote(name))
      finding = blocking("A finding")

      finding =
        case location do
          :none -> Map.delete(finding, "location")
          other -> Map.put(finding, "location", other)
        end

      asked(id)
      ended(id, "rust", "completed", verdict("rust", "needs_revision", [finding]))

      assert result(id) == {:revising, 1}
      assert [revision] = revisions_of(id)
      assert [%{"excerpt" => excerpt}] = revision.workload["revision"]["failures"]

      if expected,
        do: assert(excerpt =~ expected),
        else: refute(excerpt =~ "Location:")
    end
  end

  test "a location with a fractional line is not rounded to something it is not", %{id: id} do
    asked(id)
    finding = Map.put(blocking("A finding"), "location", %{"path" => "a.rs", "line" => 12.5})
    ended(id, "rust", "completed", verdict("rust", "needs_revision", [finding]))

    assert result(id) == {:revising, 1}
    assert [revision] = revisions_of(id)
    assert [%{"excerpt" => excerpt}] = revision.workload["revision"]["failures"]
    assert excerpt =~ "Location: a.rs:12.5\n"
  end

  test "a finding that is not blocking is not sent to the revision", %{id: id} do
    asked(id)
    ended(id, "elixir", "completed", verdict("elixir", "approved"))

    ended(
      id,
      "rust",
      "completed",
      verdict("rust", "needs_revision", [
        blocking("Unwrap on a Result"),
        %{
          "severity" => "P3",
          "blocking" => false,
          "title" => "A name",
          "evidence" => "src/lib.rs:1"
        }
      ])
    )

    assert result(id) == {:revising, 1}

    [revision] = revisions_of(id)
    assert [%{"name" => name}] = revision.workload["revision"]["failures"]
    refute name =~ "A name"
  end

  test "a blocking verdict is acted on while another review is still running", %{id: id} do
    asked(id)

    ended(
      id,
      "rust",
      "completed",
      verdict("rust", "needs_revision", [blocking("Unwrap on a Result")])
    )

    assert result(id) == {:revising, 1}
  end

  test "a blocking verdict with revisions off fails the ticket, saying what was found", %{
    gh: gh,
    id: id
  } do
    settings([])
    asked(id)
    approve_all(id)

    ended(
      id,
      "rust",
      "completed",
      verdict("rust", "needs_revision", [blocking("Unwrap on a Result")])
    )

    assert {:failed, names, _} = result(id)

    assert names == ["rust"]
    assert labels(gh) == ["labels[]=heph:failed"]
    comment = Enum.join(comments(gh))
    assert comment =~ "Unwrap on a Result"
    assert comment =~ "not revised"
    assert revisions_of(id) == []
  end

  test "a blocking verdict with the revisions used up fails the ticket", %{gh: gh, id: id} do
    settings(revisions: 1)
    # This workload is itself the first revision.
    Repo.update_all(from(w in Workload, where: w.id == ^id),
      set: [
        workload:
          Map.put(Workloads.get(id).workload, "revision", %{
            "attempt" => 1,
            "branch" => "heph/12-x"
          })
      ]
    )

    asked(id)
    approve_all(id)

    ended(
      id,
      "rust",
      "completed",
      verdict("rust", "needs_revision", [blocking("Unwrap on a Result")])
    )

    assert {:failed, ["rust"], _} = result(id)

    assert labels(gh) == ["labels[]=heph:failed"]
    assert revisions_of(id) == []
    assert Enum.join(comments(gh)) =~ "Unwrap on a Result"
  end

  test "a review that could not run is never an approval", %{gh: gh, id: id} do
    for state <- ["failed", "timed_out", "cancelled", "blocked"] do
      Repo.delete_all(from(w in Workload, where: w.parent_id == ^id))
      Repo.update_all(from(w in Workload, where: w.id == ^id), set: [settled_at: nil])
      asked(id)
      ended(id, "elixir", "completed", verdict("elixir", "approved"))
      ended(id, "rust", state)

      assert {:failed, ["rust"], extra} = result(id)
      assert extra =~ "rust"
      assert extra =~ state

      refute "labels[]=heph:done" in labels(gh)
    end
  end

  test "a review that completed with no verdict in its record is not an approval", %{
    gh: gh,
    id: id
  } do
    asked(id)
    ended(id, "elixir", "completed", verdict("elixir", "approved"))
    ended(id, "rust", "completed")

    assert {:failed, ["rust"], extra} = result(id)

    assert extra =~ "no verdict"
    assert labels(gh) == ["labels[]=heph:failed"]
  end

  test "a lens whose text is not in the release blocks the ticket", %{gh: gh, id: id} do
    StandInGitHub.fail(
      gh,
      "repos/#{@athenaeum}/contents/packs/elixir/review.md?ref=#{@ref}",
      "gh: Not Found (HTTP 404)"
    )

    asked(id)
    ended(id, "rust", "completed", verdict("rust", "approved"))

    assert {:failed, ["elixir"], extra} = result(id)

    assert extra =~ "elixir"
    assert extra =~ "no lens"
    assert labels(gh) == ["labels[]=heph:failed"]
  end

  test "a lens that is advisory does not stop the change", %{gh: gh, id: id} do
    lens(gh, "elixir", "---\nname: elixir\nblocking: false\n---\nthe elixir lens")
    asked(id)
    ended(id, "rust", "completed", verdict("rust", "approved"))

    ended(
      id,
      "elixir",
      "completed",
      verdict("elixir", "needs_revision", [blocking("Unwrap on a Result")])
    )

    assert result(id) == :done

    assert revisions_of(id) == []
    comment = Enum.join(comments(gh))
    assert comment =~ "elixir"
    assert comment =~ "advisory"
  end

  test "an advisory lens that never ran does not stop the change either", %{gh: gh, id: id} do
    lens(gh, "elixir", "---\nname: elixir\nblocking: false\n---\nthe elixir lens")
    asked(id)
    ended(id, "rust", "completed", verdict("rust", "approved"))
    ended(id, "elixir", "failed")

    assert result(id) == :done
  end

  test "a verdict for an earlier head commit does not count for the new one", %{gh: gh, id: id} do
    asked(id)
    approve_all(id)

    other = String.duplicate("b", 40)
    StandInGitHub.respond(gh, @pull, pull(%{"head" => %{"sha" => other, "ref" => "heph/12-x"}}))

    StandInGitHub.respond(
      gh,
      "repos/#{@repo}/commits/#{other}/check-runs?per_page=100",
      runs([run(1, "ci", "completed", "success")])
    )

    StandInGitHub.respond_text(
      gh,
      "repos/#{@repo}/contents/.athenaeum/manifest.toml?ref=#{other}",
      "schema_version = 0\n"
    )

    assert {:waiting, message} = result(id)

    assert message =~ "elixir" and message =~ "rust"
    refute Workloads.get(id).settled_at
    assert length(reviews_of(id, other)) == 2
  end

  test "a verdict in a record that names another commit is not an approval", %{id: id} do
    asked(id)
    ended(id, "elixir", "completed", verdict("elixir", "approved"))
    ended(id, "rust", "completed", verdict("rust", "approved", [], String.duplicate("c", 40)))

    assert {:failed, ["rust"], extra} = result(id)

    assert extra =~ "no verdict"
  end

  test "a verdict in a record that names another lens is not an approval", %{id: id} do
    asked(id)
    ended(id, "elixir", "completed", verdict("elixir", "approved"))
    ended(id, "rust", "completed", verdict("elixir", "approved"))

    assert {:failed, ["rust"], extra} = result(id)

    assert extra =~ "no verdict"
  end

  test "a verdict that is not one of the contract's three is not an approval", %{id: id} do
    asked(id)
    ended(id, "elixir", "completed", verdict("elixir", "approved"))
    ended(id, "rust", "completed", verdict("rust", "looks fine to me"))

    assert {:failed, ["rust"], extra} = result(id)

    assert extra =~ "no verdict"
  end

  test "a blocking finding is revised even when another lens could not review", %{id: id} do
    asked(id)
    ended(id, "elixir", "failed")

    ended(
      id,
      "rust",
      "completed",
      verdict("rust", "needs_revision", [blocking("Unwrap on a Result")])
    )

    assert result(id) == {:revising, 1}
  end

  test "a lens that could not review fails the ticket without waiting for the others", %{id: id} do
    asked(id)
    ended(id, "rust", "failed")

    assert {:failed, ["rust"], _} = result(id)
  end

  test "an advisory lens that is still running is waited for", %{gh: gh, id: id} do
    lens(gh, "elixir", "---\nname: elixir\nblocking: false\n---\nthe elixir lens")
    asked(id)
    ended(id, "rust", "completed", verdict("rust", "approved"))

    assert {:waiting, message} = result(id)

    assert message =~ "elixir"
  end

  test "a review of another pull request at the same commit is not this one's", %{id: id} do
    asked(id)
    approve_all(id)
    other = review(id, "rust")

    Repo.update_all(from(w in Workload, where: w.id == ^other.id),
      set: [workload: put_in(other.workload, ["review", "pullRequestNumber"], "53")]
    )

    assert {:waiting, message} = result(id)

    assert message =~ "rust"
    refute message =~ "elixir"
  end

  # A review of this lens that is newer than the real one, but of another pull request or
  # head commit, and failed. If it were read, the ticket would not be done.
  defp newer_failed_review(id, change) do
    real = review(id, "rust")

    {:ok, copy} =
      Workloads.create(change.(%{real.workload | "id" => Ecto.UUID.generate()}), parent: id)

    Repo.update_all(from(w in Workload, where: w.id == ^copy.id),
      set: [state: "failed", inserted_at: NaiveDateTime.add(real.inserted_at, 60)]
    )
  end

  test "a newer review of another pull request at the same commit is not read", %{id: id} do
    asked(id)
    approve_all(id)
    newer_failed_review(id, &put_in(&1, ["review", "pullRequestNumber"], "53"))

    assert result(id) == :done
  end

  test "a newer review of another head commit is not read", %{id: id} do
    asked(id)
    approve_all(id)
    newer_failed_review(id, &put_in(&1, ["repository", "baseCommit"], String.duplicate("d", 40)))

    assert result(id) == :done
  end

  test "a change that needs no lens is done", %{gh: gh, id: id} do
    StandInGitHub.heph_lenses(gh, %{"lenses" => [], "unpacked" => ["docs"], "unowned" => []})

    assert result(id) == :done

    assert labels(gh) == ["labels[]=heph:done"]
    assert Enum.join(comments(gh)) =~ "docs"
  end

  describe "merging" do
    setup %{gh: gh} do
      settings(automerge: [@repo], revisions: 2)
      StandInGitHub.respond(gh, @pull, pull(%{"mergeable_state" => "clean"}))
      :ok
    end

    test "waits for the reviews: nothing is merged while one is pending", %{gh: gh, id: id} do
      asked(id)
      ended(id, "rust", "completed", verdict("rust", "approved"))

      assert {:waiting, _} = result(id)

      assert puts(gh) == []
    end

    test "is held by a blocking verdict", %{gh: gh, id: id} do
      asked(id)
      approve_all(id)

      ended(
        id,
        "rust",
        "completed",
        verdict("rust", "needs_revision", [blocking("Unwrap on a Result")])
      )

      assert result(id) == {:revising, 1}

      assert puts(gh) == []
    end

    test "is held by a review that could not run", %{gh: gh, id: id} do
      asked(id)
      ended(id, "elixir", "completed", verdict("elixir", "approved"))
      ended(id, "rust", "failed")

      assert {:failed, ["rust"], _} = result(id)

      assert puts(gh) == []
    end

    test "happens once every lens approved, pinned to the head that was reviewed", %{
      gh: gh,
      id: id
    } do
      asked(id)
      approve_all(id)

      assert result(id) == {:done, :merged}

      assert puts(gh) == ["repos/#{@repo}/pulls/52/merge"]
    end
  end
end
