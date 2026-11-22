defmodule ControlPlane.SystemsTest do
  @moduledoc """
  A repository is configured as a pair: where its tickets live and where its code lives.
  A bare repository name is GitHub for both, so no existing configuration changes meaning.
  """

  use ExUnit.Case, async: false

  alias ControlPlane.Systems

  setup do
    previous = Application.get_env(:control_plane, :systems)

    on_exit(fn ->
      if previous,
        do: Application.put_env(:control_plane, :systems, previous),
        else: Application.delete_env(:control_plane, :systems)
    end)

    Application.delete_env(:control_plane, :systems)
    :ok
  end

  test "a repository that is not configured has GitHub for both, under its own name" do
    assert Systems.tracker("acme/widgets") == {ControlPlane.GitHub.Tracker, "acme/widgets"}
    assert Systems.code_host("acme/widgets") == {ControlPlane.GitHub.CodeHost, "acme/widgets"}
  end

  test "a repository configured with a tracker only keeps GitHub as its code host" do
    Application.put_env(:control_plane, :systems, %{
      "acme/widgets" => %{tracker: {SomeTracker, "WID"}}
    })

    assert Systems.tracker("acme/widgets") == {SomeTracker, "WID"}
    assert Systems.code_host("acme/widgets") == {ControlPlane.GitHub.CodeHost, "acme/widgets"}
  end

  test "a repository configured with both names both, and another repository is unaffected" do
    Application.put_env(:control_plane, :systems, %{
      "acme/widgets" => %{tracker: {SomeTracker, "WID"}, code_host: {SomeHost, "grp/widgets"}}
    })

    assert Systems.tracker("acme/widgets") == {SomeTracker, "WID"}
    assert Systems.code_host("acme/widgets") == {SomeHost, "grp/widgets"}
    assert Systems.tracker("acme/other") == {ControlPlane.GitHub.Tracker, "acme/other"}
  end
end
