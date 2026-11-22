defmodule ControlPlane.ExecutorLinkConfigTest do
  @moduledoc """
  How the executor link is switched on at start: off unless asked for, and refused without the
  API that executors connect through.
  """

  use ExUnit.Case, async: false

  @runtime Path.expand("../../config/runtime.exs", __DIR__)
  @names [
    "HEPH_EXECUTOR_LINK",
    "HEPH_API_PORT",
    "HEPH_API_SECRET_KEY_BASE",
    "HEPH_LEASE_SECONDS",
    "HEPH_CLAIM_WAIT_SECONDS"
  ]

  defp link(environment) do
    previous = for name <- @names, do: {name, System.get_env(name)}

    try do
      for name <- @names, do: System.delete_env(name)
      for {name, value} <- environment, do: System.put_env(name, value)

      config = Config.Reader.read!(@runtime, env: :test, target: :host)
      get_in(config, [:control_plane, :executor_link])
    after
      for {name, value} <- previous do
        if value, do: System.put_env(name, value), else: System.delete_env(name)
      end
    end
  end

  @api [{"HEPH_API_PORT", "4000"}, {"HEPH_API_SECRET_KEY_BASE", String.duplicate("k", 40)}]

  test "is off unless it is asked for" do
    assert link([]) == nil
    assert link([{"HEPH_EXECUTOR_LINK", "local"}]) == nil
    assert link([{"HEPH_EXECUTOR_LINK", ""}]) == nil
  end

  test "is on with workers and the API, and takes its lease and wait from the environment" do
    assert link([{"HEPH_EXECUTOR_LINK", "workers"} | @api]) == [enabled: true]

    tuned =
      link(
        [
          {"HEPH_EXECUTOR_LINK", "workers"},
          {"HEPH_LEASE_SECONDS", "90"},
          {"HEPH_CLAIM_WAIT_SECONDS", "20"}
        ] ++ @api
      )

    assert Enum.sort(tuned) == [claim_wait_ms: 20_000, enabled: true, lease_seconds: 90]
  end

  test "is refused without the API, and for a value it does not know" do
    assert_raise RuntimeError, ~r/needs HEPH_API_PORT/, fn ->
      link([{"HEPH_EXECUTOR_LINK", "workers"}])
    end

    assert_raise RuntimeError, ~r/must be local or workers/, fn ->
      link([{"HEPH_EXECUTOR_LINK", "yes"}])
    end

    assert_raise RuntimeError, ~r/HEPH_LEASE_SECONDS/, fn ->
      link([{"HEPH_EXECUTOR_LINK", "workers"}, {"HEPH_LEASE_SECONDS", "1"}] ++ @api)
    end
  end
end
