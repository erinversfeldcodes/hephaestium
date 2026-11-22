defmodule ControlPlane.UniversalLensesTest do
  @moduledoc """
  A lens whose own front matter says `applies: always` is asked for on every change that is
  reviewed, not only when a component names its pack (#135). The spec lens (does the change
  do what the ticket asks) cannot be named by a component, so nothing else would ask for it.
  """

  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query
  import ControlPlane.Fixtures

  alias ControlPlane.{Events, Reviews, StandInGitHub, Workloads}
  alias ControlPlane.Workload

  @repo "erinversfeldcodes/gazebolang"
  @athenaeum "erinversfeldcodes/athenaeum"
  @ref "v0.20.0"
  @pr_url "https://github.com/#{@repo}/pull/52"
  @sha "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5"
  @files "repos/#{@repo}/pulls/52/files"
  @manifest "repos/#{@repo}/contents/.athenaeum/manifest.toml?ref=#{@sha}"
  @packs "repos/#{@athenaeum}/contents/packs?ref=#{@ref}"
  @plan %{
    "lenses" => [%{"pack" => "rust", "components" => ["core"]}],
    "unpacked" => [],
    "unowned" => []
  }
  @always "---\nname: spec\ndescription: x\nblocking: true\napplies: always\n---\nthe spec lens"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-universal-#{System.unique_integer([:positive])}")
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

    StandInGitHub.respond(gh, @files, [%{"filename" => "core/src/lib.rs"}])
    StandInGitHub.respond_text(gh, @manifest, "schema_version = 0\n")
    StandInGitHub.heph_lenses(gh, @plan)
    packs(gh, ["rust", "spec"])
    lens(gh, "rust", "the rust lens")
    lens(gh, "spec", @always)

    %{gh: gh, id: id}
  end

  defp packs(gh, names),
    do: StandInGitHub.respond(gh, @packs, Enum.map(names, &%{"name" => &1, "type" => "dir"}))

  defp lens(gh, pack, text),
    do:
      StandInGitHub.respond_text(
        gh,
        "repos/#{@athenaeum}/contents/packs/#{pack}/review.md?ref=#{@ref}",
        text
      )

  defp pull,
    do: %{
      "state" => "open",
      "merged" => false,
      "head" => %{"sha" => @sha, "ref" => "heph/12-x"},
      "base" => %{"ref" => "main"}
    }

  defp request(id), do: Reviews.request(Workloads.get(id), pull()["head"]["sha"], "52")

  defp reviews_of(id) do
    Repo.all(
      from(w in Workload,
        where: w.parent_id == ^id and fragment("?->'review' IS NOT NULL", w.workload),
        order_by: [asc: w.inserted_at]
      )
    )
  end

  defp lenses_of(list), do: list |> Enum.map(& &1.workload["review"]["lens"]) |> Enum.sort()

  test "a lens that applies always is asked for although no component names it", %{id: id} do
    assert {:ok, %{requested: ["rust", "spec"]}} = request(id)

    reviews = reviews_of(id)
    assert lenses_of(reviews) == ["rust", "spec"]
    spec = Enum.find(reviews, &(&1.workload["review"]["lens"] == "spec"))
    assert spec.workload["review"]["lensDocument"] == @always
    assert spec.workload["review"]["pullRequestNumber"] == "52"
    assert spec.workload["repository"]["baseCommit"] == @sha
    assert spec.parent_id == id
    assert spec.state == "queued"
  end

  test "a lens without the flag is not asked for", %{gh: gh, id: id} do
    packs(gh, ["rust", "spec", "system"])
    lens(gh, "system", "---\nname: system\nblocking: true\n---\nthe system lens")

    assert {:ok, %{requested: ["rust", "spec"]}} = request(id)
    assert lenses_of(reviews_of(id)) == ["rust", "spec"]
  end

  test "a flag that is not in the front matter does not count", %{gh: gh, id: id} do
    packs(gh, ["rust", "prose"])
    lens(gh, "prose", "---\nname: prose\nblocking: true\n---\napplies: always\nin the body")

    assert {:ok, %{requested: ["rust"]}} = request(id)
  end

  test "the flag is read whatever spaces are around it", %{gh: gh, id: id} do
    packs(gh, ["rust", "spec"])
    lens(gh, "spec", "---\nname: spec\napplies:    always   \nblocking: true\n---\nx")

    assert {:ok, %{requested: ["rust", "spec"]}} = request(id)
  end

  test "any other value of the flag does not count", %{gh: gh, id: id} do
    for value <- ["never", "sometimes", "always-not", ""] do
      packs(gh, ["rust", "spec"])
      lens(gh, "spec", "---\nname: spec\napplies: #{value}\nblocking: true\n---\nx")

      # Whichever pass it is, `rust` is the only lens: spec was not asked for.
      assert {:ok, %{requested: requested, existing: existing}} = request(id), value
      assert Enum.sort(requested ++ existing) == ["rust"], value
    end
  end

  test "a lens the diff selected that also applies always is asked for once", %{gh: gh, id: id} do
    lens(gh, "rust", "---\nname: rust\napplies: always\nblocking: true\n---\nthe rust lens")

    assert {:ok, %{requested: ["rust", "spec"]}} = request(id)
    assert lenses_of(reviews_of(id)) == ["rust", "spec"]
  end

  test "asking again at the same head asks for nothing more", %{id: id} do
    {:ok, _} = request(id)
    assert {:ok, second} = request(id)

    assert second.requested == []
    assert second.existing == ["rust", "spec"]
    assert length(reviews_of(id)) == 2
  end

  test "an unreadable listing asks for nothing, and says why", %{gh: gh, id: id} do
    StandInGitHub.fail(gh, @packs, "gh: HTTP 502")

    assert {:error, message} = request(id)

    assert message =~ "lenses"
    assert message =~ "502"
    assert reviews_of(id) == []
  end

  test "a listing that is not a list of packs asks for nothing, and says so", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @packs, %{"message" => "Not Found"})

    assert {:error, message} = request(id)

    assert message =~ "not a list of packs"
    assert reviews_of(id) == []
  end

  test "a lens that cannot be read asks for nothing, and says why", %{gh: gh, id: id} do
    StandInGitHub.fail(
      gh,
      "repos/#{@athenaeum}/contents/packs/spec/review.md?ref=#{@ref}",
      "gh: HTTP 502"
    )

    assert {:error, message} = request(id)

    assert message =~ "spec"
    assert reviews_of(id) == []
  end

  test "a pack with no review.md is not a lens, and is ignored", %{gh: gh, id: id} do
    packs(gh, ["rust", "spec", "notes"])

    StandInGitHub.fail(
      gh,
      "repos/#{@athenaeum}/contents/packs/notes/review.md?ref=#{@ref}",
      "gh: Not Found (HTTP 404)"
    )

    assert {:ok, %{requested: ["rust", "spec"]}} = request(id)
  end

  test "an entry that is not a directory, or has an odd name, is not read", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @packs, [
      %{"name" => "rust", "type" => "dir"},
      %{"name" => "spec", "type" => "dir"},
      %{"name" => "README.md", "type" => "file"},
      %{"name" => "stray", "type" => "file"},
      %{"name" => "../../etc", "type" => "dir"},
      %{"name" => "Bad Name", "type" => "dir"}
    ])

    assert {:ok, %{requested: ["rust", "spec"]}} = request(id)

    for call <- StandInGitHub.gh_calls(gh) do
      refute call =~ "etc", call
      refute call =~ "README", call
      refute call =~ "stray", call
    end
  end

  test "a change that selects no lens still gets the lenses that apply to every change", %{
    gh: gh,
    id: id
  } do
    # The files belong to a component that names no pack: heph says no lens is selected.
    StandInGitHub.respond(gh, @files, [%{"filename" => ".github/workflows/ci.yml"}])

    StandInGitHub.heph_lenses(gh, %{
      "lenses" => [],
      "unpacked" => ["platform"],
      "unowned" => []
    })

    assert {:ok, %{requested: ["spec"], unpacked: ["platform"]}} = request(id)
    assert lenses_of(reviews_of(id)) == ["spec"]
  end

  test "a change whose files belong to no component at all gets them too", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @files, [%{"filename" => "NOTES.md"}])

    StandInGitHub.heph_lenses(gh, %{"lenses" => [], "unpacked" => [], "unowned" => ["NOTES.md"]})

    assert {:ok, %{requested: ["spec"], unowned: ["NOTES.md"]}} = request(id)
  end

  test "a change with no changed files asks for nothing", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @files, [])

    assert {:ok, %{requested: []}} = request(id)
    assert reviews_of(id) == []
  end

  test "its blocking finding is a revision, and its approval is in the approved lines", %{id: id} do
    {:ok, _} = request(id)
    workload = Workloads.get(id)
    asked = %{requested: [], existing: ["rust", "spec"], missing: [], unpacked: [], unowned: []}

    end_review = fn pack, verdict ->
      review = Enum.find(reviews_of(id), &(&1.workload["review"]["lens"] == pack))
      {:ok, _} = Events.append(review.id, event(review.id, 1, started()))
      {:ok, _} = Events.append(review.id, event(review.id, 2, finished("OUTCOME_COMPLETED", "")))

      Workloads.keep_record(review.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "review" =>
          Map.merge(
            %{"schema_version" => 0.0, "lens" => pack, "commit" => @sha, "notes" => []},
            verdict
          )
      })
    end

    end_review.("rust", %{"verdict" => "approved", "findings" => []})

    end_review.("spec", %{
      "verdict" => "needs_revision",
      "findings" => [
        %{
          "severity" => "P1",
          "blocking" => true,
          "title" => "The ticket asked for X",
          "evidence" => "no X",
          "revision" => "Add X."
        }
      ]
    })

    assert {:revise, [%{lens: "spec"}]} = Reviews.assess(workload, "52", @sha, asked)
  end

  test "an approving universal lens is in the lines the ticket is told", %{id: id} do
    {:ok, _} = request(id)
    workload = Workloads.get(id)
    asked = %{requested: [], existing: ["rust", "spec"], missing: [], unpacked: [], unowned: []}

    for pack <- ["rust", "spec"] do
      review = Enum.find(reviews_of(id), &(&1.workload["review"]["lens"] == pack))
      {:ok, _} = Events.append(review.id, event(review.id, 1, started()))
      {:ok, _} = Events.append(review.id, event(review.id, 2, finished("OUTCOME_COMPLETED", "")))

      Workloads.keep_record(review.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "review" => %{
          "schema_version" => 0.0,
          "lens" => pack,
          "commit" => @sha,
          "verdict" => "approved",
          "findings" => [],
          "notes" => []
        }
      })
    end

    assert {:approved, lines} = Reviews.assess(workload, "52", @sha, asked)
    assert "- rust: approved" in lines
    assert "- spec: approved" in lines
  end
end
