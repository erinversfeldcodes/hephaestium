defmodule ControlPlane.PrepushTest do
  @moduledoc """
  What the control plane gives a run so that it reads its own change before it pushes: the
  lenses of the pinned release, and how many rounds to read them in. A first run's workload
  and every revision's carry them; a review of the pushed commit never does, because it is
  a read and must have nothing of what an earlier round produced.
  """

  use ControlPlane.DataCase
  import Ecto.Query
  import ControlPlane.Fixtures

  alias ControlPlane.{Events, Prepush, Reviews, StandInGitHub, Workload, Workloads}
  alias ControlPlane.Intake, as: Run

  @repo "erinversfeldcodes/gazebolang"
  @athenaeum "erinversfeldcodes/athenaeum"
  @ref "v0.26.1"
  @packs "repos/#{@athenaeum}/contents/packs?ref=#{@ref}"
  @rust "---\nname: rust\nblocking: true\n---\nthe rust lens"
  @spec_lens "---\nname: spec\napplies: always\n---\nthe spec lens"
  @approved_at "2026-11-18T07:00:00Z"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-prepush-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    intake = Application.get_env(:control_plane, :intake)
    settle = Application.get_env(:control_plane, :settle)

    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [@repo])
    Application.put_env(:control_plane, :settle, reviews: [@repo], athenaeum_ref: @ref)
    Prepush.forget()

    on_exit(fn ->
      Prepush.forget()
      Application.put_env(:control_plane, :intake, intake)
      Application.put_env(:control_plane, :settle, settle)
      File.rm_rf(dir)
    end)

    packs(gh, ["rust", "spec", "empty"])
    lens(gh, "rust", @rust)
    lens(gh, "spec", @spec_lens)
    StandInGitHub.fail(gh, lens_endpoint("empty"), "gh: Not Found (HTTP 404)")
    %{gh: gh}
  end

  defp packs(gh, names),
    do: StandInGitHub.respond(gh, @packs, Enum.map(names, &%{"name" => &1, "type" => "dir"}))

  defp lens_endpoint(pack), do: "repos/#{@athenaeum}/contents/packs/#{pack}/review.md?ref=#{@ref}"
  defp lens(gh, pack, text), do: StandInGitHub.respond_text(gh, lens_endpoint(pack), text)

  defp settings(extra) do
    Application.put_env(
      :control_plane,
      :settle,
      Keyword.merge([reviews: [@repo], athenaeum_ref: @ref], extra)
    )
  end

  describe "the lenses and rounds a run is given" do
    test "are every lens of the pinned release that has one, with its text, and two rounds" do
      workload = workload()

      assert {:ok, given} = Prepush.add(workload, @repo)

      assert given["lenses"] == [
               %{"pack" => "rust", "document" => @rust},
               %{"pack" => "spec", "document" => @spec_lens}
             ]

      assert given["prepushRounds"] == 2
      assert Map.drop(given, ["lenses", "prepushRounds"]) == workload
    end

    test "are as many rounds as the setting says" do
      settings(prepush_rounds: 3)

      assert {:ok, %{"prepushRounds" => 3}} = Prepush.add(workload(), @repo)
    end

    test "are none for a repository whose reviews are not enabled, and nothing is asked of GitHub",
         %{gh: gh} do
      base = workload()

      assert {:ok, same} = Prepush.add(base, "someone/else")

      assert same == base
      assert StandInGitHub.gh_calls(gh) == []
    end

    test "are none when the rounds are 0, which turns reading before the push off", %{gh: gh} do
      settings(prepush_rounds: 0)

      base = workload()

      assert {:ok, same} = Prepush.add(base, @repo)

      assert same == base
      assert StandInGitHub.gh_calls(gh) == []
    end
  end

  describe "a library that cannot be read" do
    test "is an error that says so, and not a workload that reads nothing", %{gh: gh} do
      StandInGitHub.fail(gh, @packs, "gh: Server Error (HTTP 500)")

      assert {:error, message} = Prepush.add(workload(), @repo)

      assert message =~ "lens"
      assert message =~ @ref
    end

    test "is an error naming the lens that could not be read", %{gh: gh} do
      StandInGitHub.fail(gh, lens_endpoint("rust"), "gh: Server Error (HTTP 500)")

      assert {:error, message} = Prepush.add(workload(), @repo)

      assert message =~ "rust"
    end

    test "is read once for a release, not once for every workload", %{gh: gh} do
      assert {:ok, _} = Prepush.add(workload(), @repo)
      calls = length(StandInGitHub.gh_calls(gh))

      assert {:ok, _} = Prepush.add(workload(), @repo)

      assert length(StandInGitHub.gh_calls(gh)) == calls
    end

    test "is read again when it could not be, rather than remembered as broken", %{gh: gh} do
      StandInGitHub.fail(gh, @packs, "gh: Server Error (HTTP 500)")
      assert {:error, _} = Prepush.add(workload(), @repo)

      StandInGitHub.clear_failure(gh, @packs)

      assert {:ok, %{"lenses" => [_, _]}} = Prepush.add(workload(), @repo)
    end
  end

  describe "intake" do
    setup %{gh: gh} do
      StandInGitHub.respond(gh, ControlPlane.GitHub.Tracker.ready_endpoint(@repo, "heph:ready"), [
        %{
          "number" => 12,
          "state" => "open",
          "title" => "A ticket",
          "labels" => [%{"name" => "heph:ready"}]
        }
      ])

      StandInGitHub.respond(gh, ControlPlane.GitHub.Tracker.events_endpoint(@repo, 12), [
        %{
          "event" => "labeled",
          "created_at" => @approved_at,
          "actor" => %{"login" => "maint"},
          "label" => %{"name" => "heph:ready"}
        }
      ])

      StandInGitHub.respond(
        gh,
        ControlPlane.GitHub.CodeHost.permission_endpoint(@repo, "maint"),
        %{
          "permission" => "write",
          "role_name" => "write"
        }
      )

      StandInGitHub.respond_graphql(gh, 12, %{
        "data" => %{"repository" => %{"issue" => %{"userContentEdits" => %{"nodes" => []}}}}
      })

      :ok
    end

    defp built(id) do
      workload(id)
      |> Map.put("ticket", Map.merge(workload()["ticket"], %{"number" => "12"}))
      |> Map.put("approval", %{"approvedBy" => "maint", "approvedAt" => @approved_at})
    end

    test "gives a first run's workload the lenses and the rounds", %{gh: gh} do
      StandInGitHub.heph_prints(gh, built(Ecto.UUID.generate()))

      assert [%{ticket: 12, result: :started}] = Run.run(@repo)

      [stored] = Repo.all(from(w in Workload, where: w.ticket_number == 12))
      assert [%{"pack" => "rust"}, %{"pack" => "spec"}] = stored.workload["lenses"]
      assert stored.workload["prepushRounds"] == 2
    end

    test "builds nothing, and says why, when the library cannot be read", %{gh: gh} do
      StandInGitHub.heph_prints(gh, built(Ecto.UUID.generate()))
      StandInGitHub.fail(gh, @packs, "gh: Server Error (HTTP 500)")

      assert [%{ticket: 12, result: {:skipped, {:could_not_build, message}}}] = Run.run(@repo)

      assert message =~ "lens"
      assert Repo.all(from(w in Workload, where: w.ticket_number == 12)) == []
    end
  end

  describe "a review of the pushed commit" do
    @sha "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5"
    @pr_url "https://github.com/#{@repo}/pull/52"

    setup %{gh: gh} do
      {:ok, record} =
        Workloads.create(
          Map.merge(workload(), %{
            "lenses" => [%{"pack" => "rust", "document" => @rust}],
            "prepushRounds" => 2
          })
        )

      id = record.id
      {:ok, _} = Events.append(id, event(id, 1, started()))
      {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

      # What the first round made: it is in the run's record, and must go no further.
      Workloads.keep_record(id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => @pr_url},
        "steps" => [
          %{
            "kind" => "KIND_REVIEW",
            "lens" => "rust",
            "round" => 1,
            "verdict" => "needs_revision"
          }
        ]
      })

      StandInGitHub.respond(gh, "repos/#{@repo}/pulls/52/files", [
        %{"filename" => "core/src/lib.rs"}
      ])

      StandInGitHub.respond_text(
        gh,
        "repos/#{@repo}/contents/.athenaeum/manifest.toml?ref=#{@sha}",
        "schema_version = 0\n"
      )

      StandInGitHub.heph_lenses(gh, %{
        "lenses" => [%{"pack" => "rust", "components" => ["core"]}],
        "unpacked" => [],
        "unowned" => []
      })

      %{id: id}
    end

    defp pull,
      do: %{
        "state" => "open",
        "merged" => false,
        "head" => %{"sha" => @sha, "ref" => "heph/12-x"},
        "base" => %{"ref" => "main"}
      }

    test "carries no lenses and no rounds, and nothing a round made", %{id: id} do
      assert {:ok, _asked} = Reviews.request(Workloads.get(id), pull()["head"]["sha"], "52")

      reviews =
        Repo.all(from(w in Workload, where: fragment("?->'review' IS NOT NULL", w.workload)))

      assert reviews != []

      for review <- reviews do
        refute Map.has_key?(review.workload, "lenses"), "the library is not given to a reader"
        refute Map.has_key?(review.workload, "prepushRounds")
        refute Map.has_key?(review.workload, "revision")
        text = Jason.encode!(review.workload)
        refute text =~ "needs_revision", "what round one found reached round two"
        refute text =~ "KIND_REVIEW"
      end
    end
  end
end
