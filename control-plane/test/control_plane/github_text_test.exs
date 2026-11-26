defmodule ControlPlane.GitHubTextTest do
  @moduledoc """
  A job's log read through stand-ins for `gh`. The real one refuses a response holding terminal
  escape sequences unless told `--allow-escape-sequences`; that is from its reported message, and
  the stand-ins only prove what the control plane passes and what it returns.
  """

  use ExUnit.Case, async: false

  alias ControlPlane.GitHub

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-text-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    previous = Application.get_env(:control_plane, :intake)

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    {:ok, dir: dir}
  end

  defp stand_in(dir, script) do
    gh = Path.join(dir, "gh")
    File.write!(gh, "#!/bin/sh\n" <> script)
    File.chmod!(gh, 0o755)
    Application.put_env(:control_plane, :intake, gh: gh)
  end

  test "reads a log from a gh that fails without the flag", %{dir: dir} do
    stand_in(dir, """
    allow=0
    for a in "$@"; do [ "$a" = "--allow-escape-sequences" ] && allow=1; done
    if [ $allow = 0 ]; then
      echo "it could not be fetched: the response contains terminal escape sequences; pass --allow-escape-sequences to output it anyway" >&2
      exit 1
    fi
    printf 'plain log\\n'
    """)

    assert {:ok, "plain log\n"} = GitHub.text("repos/o/r/actions/jobs/1/logs")
  end

  test "returns a log holding colour codes without them", %{dir: dir} do
    stand_in(dir, """
    printf '\\033[1mBuild\\033[0m\\n\\033[31merror: boom\\033[0m\\033[0K\\nplain\\n'
    """)

    assert {:ok, "Build\nerror: boom\nplain\n"} = GitHub.text("repos/o/r/actions/jobs/1/logs")
  end
end
