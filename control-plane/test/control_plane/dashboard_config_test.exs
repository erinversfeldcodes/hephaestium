defmodule ControlPlane.DashboardConfigTest do
  @moduledoc """
  The dashboard's public address is what lets the ticket carry a pointer instead of the
  agent's words, so it is off unless set, and a value that is not an address is refused at
  start rather than producing links that go nowhere.
  """

  use ExUnit.Case, async: true

  @runtime Path.expand("../../config/runtime.exs", __DIR__)

  defp configured(value) do
    previous = System.get_env("HEPH_DASHBOARD_URL")

    try do
      if value,
        do: System.put_env("HEPH_DASHBOARD_URL", value),
        else: System.delete_env("HEPH_DASHBOARD_URL")

      config = Config.Reader.read!(@runtime, env: :test, target: :host)
      get_in(config, [:control_plane, :dashboard])
    after
      if previous,
        do: System.put_env("HEPH_DASHBOARD_URL", previous),
        else: System.delete_env("HEPH_DASHBOARD_URL")
    end
  end

  test "is off unless set" do
    assert configured(nil) == nil
    assert configured("") == nil
  end

  test "is the address given" do
    assert configured("https://heph.example") == [url: "https://heph.example"]
  end

  test "is refused when it is not an http or https address" do
    for bad <- ["heph.example", "ftp://heph.example", "https://"] do
      assert_raise RuntimeError, ~r/HEPH_DASHBOARD_URL must be/, fn -> configured(bad) end
    end
  end
end
