defmodule ControlPlane.ReviewsTest do
  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query
  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.RunWorkload
  alias ControlPlane.{Events, Reviews, Settle, StandInGitHub, Tickets, Workloads}
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
  @manifest_text "schema_version = 0\n\n[components.core]\nroot = \".\"\npaths = [\"**\"]\npack = \"rust\"\n"
  @plan %{
    "lenses" => [
      %{"pack" => "elixir", "components" => ["web"]},
      %{"pack" => "rust", "components" => ["core"]}
    ],
    "unpacked" => [],
    "unowned" => []
  }

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-reviews-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)
    previous_settle = Application.get_env(:control_plane, :settle)

    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])
    Application.put_env(:control_plane, :settle, reviews: [@repo], athenaeum_ref: @ref)

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

    StandInGitHub.respond_text(gh, @manifest, @manifest_text)
    StandInGitHub.heph_lenses(gh, @plan)
    lens(gh, "rust", "the rust lens")
    lens(gh, "elixir", "the elixir lens")

    %{gh: gh, id: id}
  end

  defp lens(gh, pack, text),
    do:
      StandInGitHub.respond_text(
        gh,
        "repos/#{@athenaeum}/contents/packs/#{pack}/review.md?ref=#{@ref}",
        text
      )

  defp pull(sha \\ @sha),
    do: %{
      "state" => "open",
      "merged" => false,
      "head" => %{"sha" => sha, "ref" => "heph/12-x"},
      "base" => %{"ref" => "main"}
    }

  defp run(id, name, status, conclusion),
    do: %{"id" => id, "name" => name, "status" => status, "conclusion" => conclusion}

  defp runs(list), do: %{"total_count" => length(list), "check_runs" => list}

  defp reviews_of(id) do
    Repo.all(
      from(w in Workload,
        where: w.parent_id == ^id and fragment("? ? 'review'", w.workload, ^"review") == true,
        order_by: [asc: w.inserted_at]
      )
    )
  end

  defp lenses_of(list), do: list |> Enum.map(& &1.workload["review"]["lens"]) |> Enum.sort()

  defp request(id) do
    workload = Workloads.get(id)
    Reviews.request(workload, pull(), "52")
  end

  test "each lens the change needs gets a review workload at the head commit", %{id: id} do
    assert {:ok, %{requested: ["elixir", "rust"]}} = request(id)

    reviews = reviews_of(id)
    assert lenses_of(reviews) == ["elixir", "rust"]

    rust = Enum.find(reviews, &(&1.workload["review"]["lens"] == "rust"))
    assert rust.workload["review"]["lensDocument"] == "the rust lens"
    assert rust.workload["review"]["pullRequestNumber"] == "52"
    assert rust.workload["review"]["pullRequestUrl"] == @pr_url
    assert rust.workload["repository"]["baseCommit"] == @sha
    refute Map.has_key?(rust.workload, "revision")
    assert rust.workload["id"] == rust.id
    assert rust.id != id
    assert rust.parent_id == id
    assert rust.state == "queued"
    assert_enqueued(worker: RunWorkload, args: %{workload_id: rust.id})
  end

  test "the changed files and the manifest at the head commit are what heph is asked about", %{
    gh: gh,
    id: id
  } do
    {:ok, _} = request(id)

    assert [call] = StandInGitHub.heph_calls(gh)
    assert call =~ "lenses --manifest "
    assert call =~ "--changed web/lib/a.ex"
    assert call =~ "--changed core/src/lib.rs"
    assert StandInGitHub.manifest_seen(gh) == {:ok, @manifest_text}
  end

  test "a lens is read from the pinned Athenaeum release, as raw text", %{gh: gh, id: id} do
    {:ok, _} = request(id)

    calls = StandInGitHub.gh_calls(gh)

    assert Enum.any?(
             calls,
             &(&1 =~ "repos/#{@athenaeum}/contents/packs/rust/review.md?ref=#{@ref}")
           )

    assert {:ok, headers} = File.read(Path.join(gh.dir, "gh-headers.log"))
    assert headers =~ "application/vnd.github.raw"
  end

  test "asking again at the same head asks for nothing more", %{id: id} do
    {:ok, %{requested: first}} = request(id)
    {:ok, second} = request(id)

    assert length(first) == 2
    assert second.requested == []
    assert second.existing == ["elixir", "rust"]
    assert length(reviews_of(id)) == 2
  end

  test "a new head commit is a new review, and the old ones stay", %{gh: gh, id: id} do
    {:ok, _} = request(id)
    other = String.duplicate("b", 40)

    StandInGitHub.respond(
      gh,
      "repos/#{@repo}/contents/.athenaeum/manifest.toml?ref=#{other}",
      @manifest_text
    )

    assert {:ok, %{requested: ["elixir", "rust"]}} =
             Reviews.request(Workloads.get(id), pull(other), "52")

    reviews = reviews_of(id)
    assert length(reviews) == 4

    assert reviews
           |> Enum.map(& &1.workload["repository"]["baseCommit"])
           |> Enum.uniq()
           |> length() == 2
  end

  test "a lens that cannot be found is said, and the others are still asked for", %{
    gh: gh,
    id: id
  } do
    StandInGitHub.fail(
      gh,
      "repos/#{@athenaeum}/contents/packs/elixir/review.md?ref=#{@ref}",
      "gh: Not Found (HTTP 404)"
    )

    assert {:ok, result} = request(id)

    assert result.requested == ["rust"]
    assert result.missing == ["elixir"]
    assert lenses_of(reviews_of(id)) == ["rust"]
  end

  test "what no lens will look at is reported and no review is made for it", %{gh: gh, id: id} do
    StandInGitHub.heph_lenses(gh, %{
      "lenses" => [],
      "unpacked" => ["docs"],
      "unowned" => ["Makefile"]
    })

    assert {:ok, result} = request(id)

    assert result.requested == []
    assert result.unpacked == ["docs"]
    assert result.unowned == ["Makefile"]
    assert reviews_of(id) == []
  end

  test "when heph cannot say which lenses, nothing is asked for and it is an error", %{
    gh: gh,
    id: id
  } do
    StandInGitHub.heph_lenses_fails(gh, "heph: the manifest is not valid")

    assert {:error, message} = request(id)

    assert message =~ "the manifest is not valid"
    assert reviews_of(id) == []
  end

  test "when the changed files cannot be read, nothing is asked for", %{gh: gh, id: id} do
    StandInGitHub.fail(gh, @files, "gh: HTTP 502")

    assert {:error, message} = request(id)

    assert message =~ "502"
    assert reviews_of(id) == []
  end

  test "when the manifest cannot be read, nothing is asked for", %{gh: gh, id: id} do
    StandInGitHub.fail(gh, @manifest, "gh: Not Found (HTTP 404)")

    assert {:error, message} = request(id)

    assert message =~ "manifest"
    assert reviews_of(id) == []
  end

  describe "in settling" do
    defp result(id), do: Enum.find(Settle.run(), &(&1.workload_id == id)).result

    test "a pull request whose checks are green is reviewed, and the ticket is done", %{id: id} do
      assert result(id) == :done

      assert lenses_of(reviews_of(id)) == ["elixir", "rust"]
    end

    test "nothing is asked for when the repository has not opted in", %{gh: gh, id: id} do
      Application.put_env(:control_plane, :settle, athenaeum_ref: @ref)

      assert result(id) == :done

      assert reviews_of(id) == []
      assert StandInGitHub.heph_calls(gh) == []
    end

    test "nothing is asked for when no Athenaeum release is named", %{id: id} do
      Application.put_env(:control_plane, :settle, reviews: [@repo])

      assert {:waiting, message} = result(id)

      assert message =~ "HEPH_ATHENAEUM_REF"
      assert reviews_of(id) == []
      refute Workloads.get(id).settled_at
    end

    test "checks that are not green ask for no review", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "completed", "failure")]))

      assert result(id) == {:failed, ["ci"]}

      assert reviews_of(id) == []
    end

    test "checks still running ask for no review", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @rules, [
        %{
          "type" => "required_status_checks",
          "parameters" => %{"required_status_checks" => [%{"context" => "ci"}]}
        }
      ])

      StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "in_progress", nil)]))

      assert {:waiting, _} = result(id)

      assert reviews_of(id) == []
    end

    test "when the lenses cannot be worked out the ticket waits and is not done", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.heph_lenses_fails(gh, "heph: the manifest is not valid")

      assert {:waiting, message} = result(id)

      assert message =~ "reviews"
      refute Workloads.get(id).settled_at
      assert StandInGitHub.writes(gh) == []
    end
  end

  describe "a review workload and its ticket" do
    setup %{gh: gh, id: id} do
      {:ok, _} = request(id)
      review = hd(reviews_of(id))
      %{review: Workloads.get(review.id), gh: gh}
    end

    test "starting one does not touch the ticket", %{review: review, gh: gh} do
      assert Tickets.started(review) == :ok
      assert StandInGitHub.writes(gh) == []
    end

    test "ending one, however it ended, does not touch the ticket", %{review: review, gh: gh} do
      for state <- ["completed", "failed", "timed_out", "blocked", "cancelled"] do
        assert Tickets.ended(%{review | state: state}) == :ok
      end

      assert StandInGitHub.writes(gh) == []
    end

    test "a review that completed is not a pull request awaiting settlement", %{review: review} do
      Workloads.keep_record(review.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "review" => %{"verdict" => "approved"}
      })

      Repo.update_all(from(w in Workload, where: w.id == ^review.id), set: [state: "completed"])

      refute Enum.any?(Workloads.awaiting_settlement(), &(&1.id == review.id))
    end

    test "requesting reviews does not settle the workload it reviews", %{id: id} do
      refute Workloads.get(id).settled_at
    end
  end
end
