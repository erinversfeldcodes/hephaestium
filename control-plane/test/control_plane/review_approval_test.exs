defmodule ControlPlane.ReviewApprovalTest do
  @moduledoc """
  When a pull request is approved on GitHub by the review App: only when every lens that
  blocks has approved the head commit, once for a head commit, and never on a partial
  yes. GitHub is the stand-in `gh`; the App's token is a fixed one, so what is proven
  here is when an approval is asked for and how, and not that GitHub counts it (the
  review App test and a real pull request show that).
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
  @other "0000000000000000000000000000000000000001"
  @pull "repos/#{@repo}/pulls/52"
  @reviews "repos/#{@repo}/pulls/52/reviews"
  @rules "repos/#{@repo}/rules/branches/main"
  @runs "repos/#{@repo}/commits/#{@sha}/check-runs?per_page=100"
  @files "repos/#{@repo}/pulls/52/files"
  @manifest "repos/#{@repo}/contents/.athenaeum/manifest.toml?ref=#{@sha}"
  @bot "hephaestium-reviewer[bot]"
  @token "fake-installation-token-1234"
  @packs ["elixir", "rust", "python", "sql", "typescript"]

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-approval-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)
    previous_settle = Application.get_env(:control_plane, :settle)
    previous_app = Application.get_env(:control_plane, :review_app)

    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])

    Application.put_env(:control_plane, :settle,
      reviews: [@repo],
      athenaeum_ref: @ref,
      revisions: 2
    )

    app(true)

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      Application.put_env(:control_plane, :settle, previous_settle)
      Application.put_env(:control_plane, :review_app, previous_app)
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
    StandInGitHub.respond(gh, @files, Enum.map(@packs, &%{"filename" => "#{&1}/a"}))
    StandInGitHub.respond_text(gh, @manifest, "schema_version = 0\n")

    StandInGitHub.heph_lenses(gh, %{
      "lenses" => Enum.map(@packs, &%{"pack" => &1, "components" => [&1]}),
      "unpacked" => [],
      "unowned" => []
    })

    for pack <- @packs, do: lens(gh, pack, "---\nname: #{pack}\nblocking: true\n---\nthe lens")

    StandInGitHub.respond(
      gh,
      "repos/#{@athenaeum}/contents/packs?ref=#{@ref}",
      Enum.map(@packs, &%{"name" => &1, "type" => "dir"})
    )

    StandInGitHub.respond(gh, @reviews, [])
    {:ok, _} = Reviews.request(Workloads.get(id), pull_map()["head"]["sha"], "52")
    %{gh: gh, id: id}
  end

  # The App is on (with a token that is always the same), or not configured.
  defp app(true),
    do:
      Application.put_env(:control_plane, :review_app,
        app_id: "5862313",
        key_file: "/nonexistent/key.pem",
        token: fn _repository -> {:ok, %{token: @token, login: @bot}} end
      )

  defp app(false), do: Application.put_env(:control_plane, :review_app, [])

  defp lens(gh, pack, text),
    do:
      StandInGitHub.respond_text(
        gh,
        "repos/#{@athenaeum}/contents/packs/#{pack}/review.md?ref=#{@ref}",
        text
      )

  defp pull_map,
    do: %{
      "state" => "open",
      "merged" => false,
      "head" => %{"sha" => @sha, "ref" => "heph/12-x", "repo" => %{"full_name" => @repo}},
      "base" => %{"ref" => "main"}
    }

  defp pull, do: pull_map()

  defp run(id, name, status, conclusion),
    do: %{"id" => id, "name" => name, "status" => status, "conclusion" => conclusion}

  defp runs(list), do: %{"total_count" => length(list), "check_runs" => list}

  defp result(id), do: Enum.find(Settle.run(), &(&1.workload_id == id)).result

  defp approvals(gh),
    do: for({"POST", @reviews, fields} <- StandInGitHub.writes(gh), do: fields)

  defp labels(gh),
    do: for({"POST", @issue <> "/labels", [field]} <- StandInGitHub.writes(gh), do: field)

  defp review(id, lens) do
    Repo.one!(
      from(w in Workload,
        where:
          w.parent_id == ^id and fragment("?->'review'->>'lens' = ?", w.workload, ^lens) and
            fragment("?->'repository'->>'baseCommit' = ?", w.workload, ^@sha)
      )
    )
  end

  defp ended(id, lens, state, verdict \\ nil) do
    record = if verdict, do: %{"review" => verdict}, else: %{}

    Repo.update_all(from(w in Workload, where: w.id == ^review(id, lens).id),
      set: [state: state, record: record]
    )
  end

  defp said(lens, kind, findings \\ [], sha \\ @sha),
    do: %{
      "schema_version" => 0.0,
      "lens" => lens,
      "commit" => sha,
      "verdict" => kind,
      "findings" => findings,
      "notes" => []
    }

  defp approve(id, packs),
    do: for(pack <- packs, do: ended(id, pack, "completed", said(pack, "approved")))

  defp finding(blocking?),
    do: %{
      "severity" => if(blocking?, do: "P1", else: "P3"),
      "blocking" => blocking?,
      "title" => "A finding",
      "evidence" => "src/lib.rs:12",
      "revision" => "Fix it."
    }

  defp approved_by_the_app(sha \\ @sha),
    do: [
      %{"user" => %{"login" => @bot, "type" => "Bot"}, "state" => "APPROVED", "commit_id" => sha}
    ]

  describe "every lens that blocks approved the head commit" do
    test "the review App approves that commit, once, and the pass waits for GitHub to count it",
         %{gh: gh, id: id} do
      approve(id, @packs)

      assert {:waiting, message} = result(id)
      assert message =~ "approv"

      assert [fields] = approvals(gh)
      assert "event=APPROVE" in fields
      assert "commit_id=#{@sha}" in fields
      body = Enum.find(fields, &String.starts_with?(&1, "body="))
      for pack <- @packs, do: assert(body =~ pack)

      assert labels(gh) == []
      refute Workloads.get(id).settled_at
    end

    test "the next pass sees the approval, asks for nothing more, and the ticket is done", %{
      gh: gh,
      id: id
    } do
      approve(id, @packs)
      assert {:waiting, _} = result(id)

      StandInGitHub.respond(gh, @reviews, approved_by_the_app())

      assert result(id) == :done
      assert length(approvals(gh)) == 1
      assert labels(gh) == ["labels[]=heph:done"]
      assert Workloads.get(id).settled_at
    end

    test "settling again and again does not approve again", %{gh: gh, id: id} do
      approve(id, @packs)
      StandInGitHub.respond(gh, @reviews, approved_by_the_app())

      for _ <- 1..3, do: Settle.run()

      assert approvals(gh) == []
    end

    test "an approval of an earlier head commit is not an approval of this one", %{gh: gh, id: id} do
      approve(id, @packs)
      StandInGitHub.respond(gh, @reviews, approved_by_the_app(@other))

      assert {:waiting, _} = result(id)
      assert [_] = approvals(gh)
    end

    test "an approval by anyone else is not the App's", %{gh: gh, id: id} do
      approve(id, @packs)

      StandInGitHub.respond(gh, @reviews, [
        %{
          "user" => %{"login" => "a-person", "type" => "User"},
          "state" => "APPROVED",
          "commit_id" => @sha
        }
      ])

      assert {:waiting, _} = result(id)
      assert [_] = approvals(gh)
    end

    test "a review of the App that is not an approval is not an approval", %{gh: gh, id: id} do
      approve(id, @packs)

      StandInGitHub.respond(gh, @reviews, [
        %{
          "user" => %{"login" => @bot, "type" => "Bot"},
          "state" => "COMMENTED",
          "commit_id" => @sha
        }
      ])

      assert {:waiting, _} = result(id)
      assert [_] = approvals(gh)
    end

    test "the token reaches gh through its environment, never its arguments", %{gh: gh, id: id} do
      approve(id, @packs)
      result(id)

      calls = File.read!(Path.join(gh.dir, "gh-calls.log"))
      refute calls =~ @token

      tokens = File.read!(Path.join(gh.dir, "gh-tokens.log"))
      assert tokens =~ "POST #{@reviews} #{@token}"
    end

    test "the token is used for the approval and for no other call", %{gh: gh, id: id} do
      approve(id, @packs)
      result(id)

      lines =
        gh.dir |> Path.join("gh-tokens.log") |> File.read!() |> String.split("\n", trim: true)

      assert Enum.all?(lines, &(&1 =~ @reviews))
    end

    test "a failure to approve waits, is not an approval, and is tried again", %{gh: gh, id: id} do
      approve(id, @packs)
      StandInGitHub.fail_write(gh, "POST", @reviews, "gh: Server Error (HTTP 502)")

      assert {:waiting, message} = result(id)
      assert message =~ "502"
      assert labels(gh) == []
      refute Workloads.get(id).settled_at

      StandInGitHub.clear_write_failure(gh, "POST", @reviews)
      assert {:waiting, _} = result(id)
      assert length(approvals(gh)) == 2
    end

    test "a token that cannot be had waits, and nothing is submitted", %{gh: gh, id: id} do
      Application.put_env(:control_plane, :review_app,
        app_id: "5862313",
        key_file: "/nonexistent/key.pem",
        token: fn _repository -> {:error, "the App is not installed on the repository"} end
      )

      approve(id, @packs)

      assert {:waiting, message} = result(id)
      assert message =~ "not installed"
      assert approvals(gh) == []
      refute Workloads.get(id).settled_at
    end

    test "with no App configured nothing is submitted and the ticket is done as before", %{
      gh: gh,
      id: id
    } do
      app(false)
      approve(id, @packs)

      assert result(id) == :done
      assert approvals(gh) == []
    end

    test "the approval is of the commit that was reviewed, even if the pull request moved", %{
      gh: gh,
      id: id
    } do
      approve(id, @packs)
      StandInGitHub.respond(gh, @pull, put_in(pull(), ["head", "sha"], @other))

      StandInGitHub.respond(
        gh,
        "repos/#{@repo}/commits/#{@other}/check-runs?per_page=100",
        runs([run(1, "ci", "completed", "success")])
      )

      result(id)

      assert approvals(gh) == []
    end
  end

  describe "one lens that blocks has not approved, so nothing is approved" do
    test "four of five approved and one found something that blocks", %{gh: gh, id: id} do
      approve(id, Enum.take(@packs, 4))
      ended(id, "typescript", "completed", said("typescript", "needs_revision", [finding(true)]))

      assert {:revising, 1} = result(id)
      assert approvals(gh) == []
    end

    test "four of five approved and one is still running", %{gh: gh, id: id} do
      approve(id, Enum.take(@packs, 4))

      assert {:waiting, message} = result(id)
      assert message =~ "typescript"
      assert approvals(gh) == []
    end

    test "four of five approved and one could not review", %{gh: gh, id: id} do
      approve(id, Enum.take(@packs, 4))
      ended(id, "typescript", "failed")

      assert {:failed, ["typescript"], _} = result(id)
      assert approvals(gh) == []
    end

    test "four of five approved and one left no verdict", %{gh: gh, id: id} do
      approve(id, Enum.take(@packs, 4))
      ended(id, "typescript", "completed")

      assert {:failed, ["typescript"], _} = result(id)
      assert approvals(gh) == []
    end

    test "four of five approved and one lens has no text in the release", %{gh: gh, id: id} do
      StandInGitHub.fail(
        gh,
        "repos/#{@athenaeum}/contents/packs/typescript/review.md?ref=#{@ref}",
        "gh: Not Found (HTTP 404)"
      )

      Repo.delete_all(from(w in Workload, where: w.parent_id == ^id))

      assert {:ok, %{missing: ["typescript"]}} =
               Reviews.request(Workloads.get(id), pull()["head"]["sha"], "52")

      approve(id, Enum.take(@packs, 4))

      assert {:failed, ["typescript"], _} = result(id)
      assert approvals(gh) == []
    end

    test "four of five approved and one said failed with no blocking finding", %{gh: gh, id: id} do
      approve(id, Enum.take(@packs, 4))
      ended(id, "typescript", "completed", said("typescript", "failed"))

      assert {:failed, ["typescript"], message} = result(id)
      assert message =~ "failed"
      assert approvals(gh) == []
    end

    test "four of five approved and one asked for a revision naming nothing that blocks", %{
      gh: gh,
      id: id
    } do
      approve(id, Enum.take(@packs, 4))
      ended(id, "typescript", "completed", said("typescript", "needs_revision", [finding(false)]))

      assert {:failed, ["typescript"], message} = result(id)
      assert message =~ "needs_revision"
      assert approvals(gh) == []
    end

    test "four of five approved and the fifth's approval is of another commit", %{gh: gh, id: id} do
      approve(id, Enum.take(@packs, 4))
      ended(id, "typescript", "completed", said("typescript", "approved", [], @other))

      assert {:failed, ["typescript"], _} = result(id)
      assert approvals(gh) == []
    end

    test "an approval stays out when the blocked lens is the only one that is not advisory", %{
      gh: gh,
      id: id
    } do
      for pack <- Enum.take(@packs, 4),
          do: lens(gh, pack, "---\nname: #{pack}\nblocking: false\n---\nthe lens")

      Repo.delete_all(from(w in Workload, where: w.parent_id == ^id))
      {:ok, _} = Reviews.request(Workloads.get(id), pull()["head"]["sha"], "52")
      ended(id, "typescript", "completed", said("typescript", "needs_revision", [finding(true)]))

      assert {:revising, 1} = result(id)
      assert approvals(gh) == []
    end
  end

  describe "an advisory lens" do
    test "does not stop the approval whatever it said", %{gh: gh, id: id} do
      lens(gh, "sql", "---\nname: sql\nblocking: false\n---\nthe lens")
      Repo.delete_all(from(w in Workload, where: w.parent_id == ^id))
      {:ok, _} = Reviews.request(Workloads.get(id), pull()["head"]["sha"], "52")

      approve(id, Enum.reject(@packs, &(&1 == "sql")))
      ended(id, "sql", "completed", said("sql", "needs_revision", [finding(true)]))

      assert {:waiting, _} = result(id)
      assert [_] = approvals(gh)
    end
  end
end
