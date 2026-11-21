defmodule ControlPlane.WorkspacesTest do
  @moduledoc """
  A finished run's checkout is deleted so unattended runs do not fill the disk. What the
  run said (its workload, events, log and record) stays, and a run that did not complete
  keeps its checkout for a while so someone can look at it.
  """

  use ControlPlane.DataCase

  import Ecto.Query
  import ControlPlane.Fixtures

  alias ControlPlane.{Repo, Runner, Workload, Workloads, Workspaces}

  @now ~U[2026-11-21 12:00:00Z]

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-spaces-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)

    Application.put_env(:control_plane, :runner,
      runs_dir: Path.join(dir, "runs"),
      workspace_keep_hours: 72
    )

    on_exit(fn ->
      Application.delete_env(:control_plane, :runner)
      File.rm_rf(dir)
    end)

    %{dir: dir}
  end

  # A workload in the given state that ended the given number of hours before @now, with a
  # run directory holding a checkout and the files the run keeps.
  defp run(state, hours_ago \\ 0) do
    {:ok, record} = Workloads.create(workload())
    finished = DateTime.add(@now, -hours_ago * 3600, :second)

    Repo.update_all(from(w in Workload, where: w.id == ^record.id),
      set: [state: state, finished_at: finished, updated_at: finished]
    )

    directory = run_directory(record)
    File.mkdir_p!(Path.join(directory, "work/repo/.git"))
    File.write!(Path.join(directory, "work/repo/.git/HEAD"), "ref: refs/heads/main")

    for file <- ~w(workload.json events.jsonl executor.log record.json),
        do: File.write!(Path.join(directory, file), "kept")

    {record.id, directory}
  end

  defp run_directory(record) do
    runs = Runner.config(:runs_dir)
    Path.join([runs, "erinversfeldcodes-gazebolang", "#{record.ticket_number}-#{record.id}"])
  end

  defp kept?(directory),
    do:
      Enum.all?(
        ~w(workload.json events.jsonl executor.log record.json),
        &File.exists?(Path.join(directory, &1))
      )

  test "a completed run's checkout is deleted and what the run said is kept" do
    {_, directory} = run("completed")

    Workspaces.sweep(@now)

    refute File.exists?(Path.join(directory, "work"))
    assert kept?(directory)
  end

  test "a failed run keeps its checkout inside the window and loses it after" do
    {_, recent} = run("failed", 71)
    {_, old} = run("failed", 73)

    Workspaces.sweep(@now)

    assert File.exists?(Path.join(recent, "work/repo/.git/HEAD"))
    refute File.exists?(Path.join(old, "work"))
    assert kept?(recent) and kept?(old)
  end

  test "every end but completed is held for the window" do
    held = for state <- ~w(blocked timed_out cancelled), do: run(state, 1)

    Workspaces.sweep(@now)

    for {_, directory} <- held, do: assert(File.exists?(Path.join(directory, "work")))
  end

  test "a run that is still going is never touched, however old" do
    runs = for state <- ~w(queued preparing running gating), do: run(state, 500)

    assert Workspaces.sweep(@now) == 0

    for {_, directory} <- runs, do: assert(File.exists?(Path.join(directory, "work")))
  end

  test "a directory that belongs to no workload is left alone" do
    stray = Path.join(Runner.config(:runs_dir), "erinversfeldcodes-gazebolang/12-not-a-workload")
    File.mkdir_p!(Path.join(stray, "work"))
    unnamed = Path.join(Runner.config(:runs_dir), "erinversfeldcodes-gazebolang/notes")
    File.mkdir_p!(Path.join(unnamed, "work"))

    assert Workspaces.sweep(@now) == 0

    assert File.exists?(Path.join(stray, "work"))
    assert File.exists?(Path.join(unnamed, "work"))
  end

  test "a symlink is not followed out of the runs directory", %{dir: dir} do
    outside = Path.join(dir, "outside")
    File.mkdir_p!(outside)
    File.write!(Path.join(outside, "precious"), "keep me")

    {_, directory} = run("completed")
    File.rm_rf!(Path.join(directory, "work"))
    File.ln_s!(outside, Path.join(directory, "work"))

    Workspaces.sweep(@now)

    assert File.read!(Path.join(outside, "precious")) == "keep me"
    refute File.exists?(Path.join(directory, "work"))
  end

  test "a run directory that is itself a symlink is not entered", %{dir: dir} do
    outside = Path.join(dir, "outside-run")
    File.mkdir_p!(Path.join(outside, "work"))

    {id, directory} = run("completed")
    File.rm_rf!(directory)
    File.ln_s!(outside, directory)

    assert Workspaces.sweep(@now) == 0

    assert File.exists?(Path.join(outside, "work"))
    assert Workloads.get(id).state == "completed"
  end

  test "a deletion that cannot be done is not an error and leaves the workload as it was" do
    {id, directory} = run("completed")
    File.chmod!(Path.join(directory, "work/repo/.git"), 0o500)
    on_exit(fn -> File.chmod(Path.join(directory, "work/repo/.git"), 0o700) end)

    assert is_integer(Workspaces.sweep(@now))

    assert Workloads.get(id).state == "completed"
  end

  test "sweeping again, with nothing left, deletes nothing" do
    run("completed")
    assert Workspaces.sweep(@now) == 1
    assert Workspaces.sweep(@now) == 0
  end

  test "with no runs directory yet there is nothing to sweep" do
    assert Workspaces.sweep(@now) == 0
  end

  test "the window is the one configured" do
    Application.put_env(:control_plane, :runner,
      runs_dir: Runner.config(:runs_dir),
      workspace_keep_hours: 1
    )

    {_, directory} = run("failed", 2)

    Workspaces.sweep(@now)

    refute File.exists?(Path.join(directory, "work"))
  end
end
