defmodule ControlPlane.LowDiskTest do
  @moduledoc """
  A run is not started on a disk that is nearly full: the workload stays queued and is
  tried again later, so a run is not begun that the disk cannot hold.
  """

  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import ExUnit.CaptureLog
  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.RunWorkload
  alias ControlPlane.{Disk, StandIn, Workloads}

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-disk-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)

    # The suite logs only errors; these tests read warnings.
    level = Logger.level()
    Logger.configure(level: :warning)

    on_exit(fn ->
      Logger.configure(level: level)
      Application.delete_env(:control_plane, :runner)
      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())
    %{dir: dir, id: record.id}
  end

  # A `df` that reports this many megabytes free, as POSIX `df -Pk` lays it out. It records
  # the path it was asked about.
  defp df(dir, free_mb) do
    path = Path.join(dir, "df")

    File.write!(path, """
    #!/bin/sh
    echo "$@" >> "#{dir}/df-args.txt"
    echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
    echo "/dev/disk3s1 481036337 100 #{free_mb * 1024} 14% /"
    """)

    File.chmod!(path, 0o755)
    path
  end

  defp broken_df(dir, body) do
    path = Path.join(dir, "df-broken")
    File.write!(path, "#!/bin/sh\n#{body}\n")
    File.chmod!(path, 0o755)
    path
  end

  defp configure(dir, id, extra) do
    executor = StandIn.executor(Path.join(dir, "stand-in"), events: stream(id))

    Application.put_env(
      :control_plane,
      :runner,
      Keyword.merge(
        [
          executor: executor,
          runs_dir: Path.join(dir, "runs"),
          silence_limit_ms: 5_000,
          kill_grace_ms: 2_000
        ],
        extra
      )
    )
  end

  defp stream(id) do
    [
      event(id, 1, started()),
      event(id, 2, workspace_ready()),
      event(id, 3, finished())
    ]
  end

  defp run(id), do: perform_job(RunWorkload, %{"workload_id" => id})

  # The run's result, with the log it wrote kept out of the test output.
  defp quietly(id) do
    capture_log(fn -> send(self(), {:quietly, run(id)}) end)
    assert_received {:quietly, result}
    result
  end

  defp started?(dir), do: File.exists?(Path.join(dir, "stand-in/args.txt"))

  describe "the free space" do
    test "is read from the filesystem that holds the runs directory", %{dir: dir} do
      configure(dir, "x", df: df(dir, 4096), min_free_mb: 1)

      assert Disk.free_mb() == {:ok, 4096}

      # Not yet made, so it is asked about the nearest directory that is.
      [asked] = File.read!(Path.join(dir, "df-args.txt")) |> String.split("\n", trim: true)
      assert asked == "-Pk #{dir}"
    end

    test "is an error, not a number, when df cannot say", %{dir: dir} do
      configure(dir, "x", df: broken_df(dir, "exit 1"))
      assert {:error, _} = Disk.free_mb()

      configure(dir, "x", df: broken_df(dir, "echo nonsense"))
      assert {:error, _} = Disk.free_mb()

      configure(dir, "x", df: Path.join(dir, "no-such-df"))
      assert {:error, _} = Disk.free_mb()
    end
  end

  describe "a workload" do
    test "is not started below the limit: it stays queued, the executor is not run, and the log says why",
         %{dir: dir, id: id} do
      configure(dir, id, df: df(dir, 500), min_free_mb: 1024)

      log = capture_log(fn -> send(self(), {:result, run(id)}) end)

      assert_received {:result, {:snooze, seconds}}
      assert seconds > 0
      assert Workloads.get(id).state == "queued"
      refute started?(dir)
      assert log =~ "500"
      assert log =~ "1024"
      assert log =~ "HEPH_MIN_FREE_MB"
    end

    test "waiting does not use up an attempt", %{dir: dir, id: id} do
      configure(dir, id, df: df(dir, 500), min_free_mb: 1024)

      assert {:snooze, _} = quietly(id)
      assert {:snooze, _} = quietly(id)

      assert Workloads.get(id).state == "queued"
    end

    test "is started at the limit and above it", %{dir: dir, id: id} do
      configure(dir, id, df: df(dir, 1024), min_free_mb: 1024)

      assert :ok = quietly(id)

      assert started?(dir)
      assert Workloads.get(id).state == "completed"
    end

    test "is started whatever the disk says when the limit is 0", %{dir: dir, id: id} do
      configure(dir, id, df: df(dir, 0), min_free_mb: 0)

      assert :ok = quietly(id)

      assert started?(dir)
    end

    test "is started when the free space cannot be read, and the log says so", %{dir: dir, id: id} do
      configure(dir, id, df: broken_df(dir, "exit 1"), min_free_mb: 1024)

      log = capture_log(fn -> send(self(), {:result, run(id)}) end)
      assert_received {:result, :ok}

      assert started?(dir)
      assert log =~ "could not read the free disk space"
    end

    test "is checked against ten gigabytes when no limit is set", %{dir: dir, id: id} do
      configure(dir, id, df: df(dir, 10_239))
      assert {:snooze, _} = quietly(id)

      configure(dir, id, df: df(dir, 10_240))
      assert :ok = quietly(id)
    end
  end
end
