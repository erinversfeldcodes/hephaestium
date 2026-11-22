defmodule ControlPlane.BoundaryGuardTest do
  @moduledoc """
  Tickets and changes are reached only through `ControlPlane.Tracker` and
  `ControlPlane.CodeHost`. If any other module of the control plane names the GitHub client,
  runs `gh`, or builds a `repos/` endpoint, the logic is written in one system's words again
  and a second system would have to be taught to every module.

  The files that may are the GitHub implementation (`github.ex` and `github/`), the review
  App's authentication that only that implementation uses (`review_app.ex`), and the
  configuration that names GitHub as the default for a repository (`systems.ex`).
  """

  use ExUnit.Case, async: true

  @lib Path.expand("../../lib", __DIR__)

  @allowed [
    "control_plane/github.ex",
    "control_plane/review_app.ex",
    "control_plane/systems.ex"
  ]

  @patterns [
    {~r/ControlPlane\.GitHub\b/, "names ControlPlane.GitHub"},
    {~r/alias ControlPlane\.\{[^}]*\bGitHub\b/, "aliases GitHub"},
    {~r/\bGitHub\.[a-z_]+\(/, "calls the GitHub client"},
    {~r/\bgh\b/, "mentions gh"},
    {~r"\brepos/", "builds a repos/ endpoint"}
  ]

  @doc false
  def violations(files) do
    for {path, source} <- files,
        not allowed?(path),
        {pattern, what} <- @patterns,
        Regex.match?(pattern, source),
        do: "#{path} #{what}"
  end

  defp allowed?(path),
    do: path in @allowed or String.starts_with?(path, "control_plane/github/")

  defp sources do
    for path <- Path.wildcard(Path.join(@lib, "**/*.ex")) do
      {Path.relative_to(path, @lib), File.read!(path)}
    end
  end

  test "no module outside the GitHub implementation reaches GitHub" do
    assert violations(sources()) == []
  end

  test "the guard sees each kind of violation, and lets the implementation be" do
    planted = [
      {"control_plane/settle.ex", "GitHub.get(endpoint)"},
      {"control_plane/intake.ex", "alias ControlPlane.{Runner, GitHub}"},
      {"control_plane/gaps.ex", "ControlPlane.GitHub.write(a, b, c)"},
      {"control_plane/reviews.ex", "System.cmd(\"gh\", [])"},
      {"control_plane/tickets.ex", ~s|"repos/\#{repo}/issues"|},
      {"control_plane/github/tracker.ex", "GitHub.get(endpoint) gh repos/"}
    ]

    assert Enum.map(violations(planted), &(&1 |> String.split(" ", parts: 2) |> hd())) ==
             [
               "control_plane/settle.ex",
               "control_plane/intake.ex",
               "control_plane/gaps.ex",
               "control_plane/gaps.ex",
               "control_plane/reviews.ex",
               "control_plane/tickets.ex"
             ]
  end
end
