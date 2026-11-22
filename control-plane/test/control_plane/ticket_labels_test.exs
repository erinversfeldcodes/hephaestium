defmodule ControlPlane.TicketLabelsTest do
  @moduledoc """
  A repository the control plane serves gets the labels it uses, created when missing and
  never changed when present. These run against the stand-in `gh` and the stand-in tracker,
  so they prove the control plane's logic and the requests it makes; they do not prove that
  GitHub accepts those requests.
  """

  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import ExUnit.CaptureLog

  alias ControlPlane.Jobs.Intake
  alias ControlPlane.{StandInGitHub, StandInSystems, TicketLabels, Tracker}

  @repo "erinversfeldcodes/gazebolang"
  @labels "repos/#{@repo}/labels"
  @all ["heph:ready", "heph:running", "heph:done", "heph:failed", "heph:blocked"]

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-labels-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)

    Application.put_env(:control_plane, :intake,
      gh: gh.gh,
      heph: gh.heph,
      repositories: [@repo]
    )

    TicketLabels.forget(@repo)

    # The suite logs only errors; a warning is what is being looked for here.
    level = Logger.level()
    Logger.configure(level: :warning)

    on_exit(fn ->
      Logger.configure(level: level)
      Application.put_env(:control_plane, :intake, previous)
      TicketLabels.forget(@repo)
      File.rm_rf(dir)
    end)

    StandInGitHub.respond(
      gh,
      "repos/#{@repo}/issues?labels=heph:ready&state=open&per_page=100",
      []
    )

    %{gh: gh}
  end

  defp exist(gh, names),
    do: StandInGitHub.respond(gh, @labels <> "?per_page=100", for(n <- names, do: %{"name" => n}))

  defp created(gh) do
    for {"POST", @labels, ["name=" <> name | rest]} <- StandInGitHub.writes(gh),
        do: {name, rest}
  end

  test "a repository with none of the labels gets all five, with the colour and description the others use",
       %{gh: gh} do
    exist(gh, ["bug", "enhancement"])

    assert :ok = perform_job(Intake, %{})

    assert created(gh) == [
             {"heph:ready", ["color=a1b4a9", "description=Approved for an agent run"]},
             {"heph:running", ["color=ededed", "description="]},
             {"heph:done", ["color=ededed", "description="]},
             {"heph:failed", ["color=ededed", "description="]},
             {"heph:blocked", ["color=ededed", "description="]}
           ]
  end

  test "a repository with some of the labels gets only the rest, and the ones it has are not written to",
       %{gh: gh} do
    exist(gh, ["heph:running", "heph:failed", "bug"])

    assert :ok = perform_job(Intake, %{})

    assert Enum.map(created(gh), &elem(&1, 0)) == ["heph:ready", "heph:done", "heph:blocked"]
    assert length(StandInGitHub.writes(gh)) == 3
  end

  test "a repository with all five labels gets no write at all", %{gh: gh} do
    exist(gh, @all)

    assert :ok = perform_job(Intake, %{})

    assert StandInGitHub.writes(gh) == []
  end

  test "a repository is checked once, not on every pass", %{gh: gh} do
    exist(gh, @all)

    assert :ok = perform_job(Intake, %{})
    assert :ok = perform_job(Intake, %{})

    reads = Enum.filter(StandInGitHub.gh_calls(gh), &String.contains?(&1, "/labels?"))
    assert length(reads) == 1
  end

  test "a label that cannot be created is logged once for the repository, and intake carries on",
       %{gh: gh} do
    exist(gh, ["heph:running"])
    StandInGitHub.fail_write(gh, "POST", @labels, "HTTP 403: Resource not accessible")

    log =
      capture_log(fn ->
        assert :ok = perform_job(Intake, %{})
        assert :ok = perform_job(Intake, %{})
      end)

    assert length(String.split(log, "could not ensure the labels of #{@repo}")) == 2
    assert log =~ "heph:ready: HTTP 403"
    assert log =~ "heph:blocked: HTTP 403"

    approved = Enum.filter(StandInGitHub.gh_calls(gh), &String.contains?(&1, "labels=heph:ready"))
    assert length(approved) == 2
  end

  test "a repository whose labels cannot be read is logged once, and intake carries on", %{gh: gh} do
    log =
      capture_log(fn ->
        assert :ok = perform_job(Intake, %{})
      end)

    assert log =~ "could not ensure the labels of #{@repo}"
    assert StandInGitHub.writes(gh) == []
    assert Enum.any?(StandInGitHub.gh_calls(gh), &String.contains?(&1, "labels=heph:ready"))
  end

  describe "through the tracker boundary" do
    setup do
      previous = Application.get_env(:control_plane, :systems)

      Application.put_env(:control_plane, :systems, %{
        @repo => %{tracker: {ControlPlane.StandInTracker, "TRACK"}}
      })

      start_supervised!(StandInSystems)
      StandInSystems.answer(:tracker, :ensure_labels, :ok)
      StandInSystems.answer(:tracker, :approved_tickets, {:ok, []})

      on_exit(fn ->
        if previous,
          do: Application.put_env(:control_plane, :systems, previous),
          else: Application.delete_env(:control_plane, :systems)
      end)
    end

    test "the labels are asked of the repository's own tracker, under its own name", %{gh: gh} do
      assert :ok = perform_job(Intake, %{})

      assert [{:tracker, :ensure_labels, ["TRACK", labels]}, {:tracker, :approved_tickets, _}] =
               StandInSystems.calls()

      assert Enum.map(labels, & &1.name) == @all
      assert StandInGitHub.gh_calls(gh) == []
    end

    test "the operation is the tracker's own" do
      assert :ok = Tracker.ensure_labels(@repo, TicketLabels.all())
    end
  end
end
