defmodule ControlPlane.HealthTest do
  @moduledoc """
  The answer the platform's health check gets: 200 while the database answers a query, 503
  while it does not. The down case is a real Repo pointed at a port nothing listens on,
  not a stand-in for the check.
  """

  use ExUnit.Case, async: false

  # An unreachable database is logged at error level by the driver, as it should be; the
  # tests that make one do not need it on the screen.
  @moduletag :capture_log
  import Plug.Test
  import Plug.Conn

  alias ControlPlane.{Health, ScratchRepo}

  defp get(path, repo) do
    conn(:get, path) |> ControlPlane.Health.Plug.call(ControlPlane.Health.Plug.init(repo: repo))
  end

  defp stop_repo(pid) do
    ref = Process.monitor(pid)
    Process.exit(pid, :shutdown)
    assert_receive {:DOWN, ^ref, _, _, _}
  end

  setup do
    Ecto.Adapters.SQL.Sandbox.checkout(ControlPlane.Repo)
    Ecto.Adapters.SQL.Sandbox.mode(ControlPlane.Repo, {:shared, self()})
    on_exit(fn -> Ecto.Adapters.SQL.Sandbox.mode(ControlPlane.Repo, :manual) end)
    :ok
  end

  defp start_unreachable do
    Application.put_env(:control_plane, ScratchRepo,
      hostname: "127.0.0.1",
      port: 1,
      database: "nowhere",
      username: "nobody",
      pool_size: 1
    )

    {:ok, pid} = ScratchRepo.start_link()
    on_exit(fn -> if Process.alive?(pid), do: stop_repo(pid) end)
    pid
  end

  test "with the database answering the check passes" do
    assert Health.check(ControlPlane.Repo) == :ok
  end

  test "GET /health is 200 and says ok while the database answers" do
    conn = get("/health", ControlPlane.Repo)

    assert conn.status == 200
    assert Jason.decode!(conn.resp_body) == %{"status" => "ok"}
    assert get_resp_header(conn, "content-type") == ["application/json; charset=utf-8"]
  end

  test "GET /health is 503 and says why while the database does not answer" do
    start_unreachable()

    conn = get("/health", ScratchRepo)

    assert conn.status == 503
    assert %{"status" => "unavailable", "reason" => reason} = Jason.decode!(conn.resp_body)
    assert reason =~ "database"
  end

  test "the check itself returns the error for an unreachable database" do
    start_unreachable()

    assert {:error, reason} = Health.check(ScratchRepo)
    assert reason =~ "database"
  end

  test "any other path is 404" do
    assert get("/", ControlPlane.Repo).status == 404
  end

  test "only GET is answered" do
    conn =
      conn(:post, "/health")
      |> ControlPlane.Health.Plug.call(ControlPlane.Health.Plug.init(repo: ControlPlane.Repo))

    assert conn.status == 404
  end
end
