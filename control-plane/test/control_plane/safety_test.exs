defmodule ControlPlane.SafetyTest do
  @moduledoc """
  The tests must never act on a real repository by accident: unless a test stands in
  for them, `gh` and `heph` are a program that fails.
  """

  use ExUnit.Case, async: false

  test "by default the tests cannot reach the real GitHub or build a workload with the real heph" do
    intake = Application.get_env(:control_plane, :intake)

    assert intake[:gh] == "false"
    assert intake[:heph] == "false"
    assert intake[:repositories] == []
  end

  test "a read or a write of GitHub in the tests fails, whatever it asks" do
    assert {:error, _} = ControlPlane.GitHub.get("repos/erinversfeldcodes/gazebolang")

    assert {:error, _} =
             ControlPlane.GitHub.write(
               "POST",
               "repos/erinversfeldcodes/gazebolang/issues/12/labels",
               []
             )
  end

  test "even a configuration that names the real gh is refused in the tests" do
    previous = Application.get_env(:control_plane, :intake)
    on_exit(fn -> Application.put_env(:control_plane, :intake, previous) end)

    # A repository that does not exist, so that this test cannot do harm if the
    # refusal it is about were missing: GitHub would only say it is not found.
    Application.put_env(:control_plane, :intake, gh: "gh", heph: "heph", repositories: [])

    assert {:error, message} =
             ControlPlane.GitHub.write("POST", "repos/no-such-owner-zzz/none/issues/1/labels", [])

    assert message =~ "refusing to run"

    assert {:error, message} = ControlPlane.GitHub.get("repos/no-such-owner-zzz/none")
    assert message =~ "refusing to run"
  end

  test "the real gh by its full path is refused too" do
    previous = Application.get_env(:control_plane, :intake)
    on_exit(fn -> Application.put_env(:control_plane, :intake, previous) end)

    if real = System.find_executable("gh") do
      Application.put_env(:control_plane, :intake, gh: real, heph: "heph", repositories: [])

      assert {:error, message} = ControlPlane.GitHub.get("repos/no-such-owner-zzz/none")
      assert message =~ "refusing to run"
    end
  end
end
