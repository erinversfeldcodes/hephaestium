defmodule ControlPlane.Programs do
  @moduledoc """
  A guard on the programs the control plane runs on a person's machine: `gh`,
  `heph` and the executor act on real repositories with the person's own
  credentials.

  In the tests the configuration sets `refuse_real_programs`, and then only a
  program that fails (`false`) or one made in the machine's temporary directory,
  which is how a test stands in for one, may be run. A test, or a change to a
  test's configuration, that points at the real program is refused, because the
  alternative is a test that writes to a real repository.
  """

  @doc "Whether `command` may be run."
  @spec allowed?(String.t()) :: boolean()
  def allowed?(command) do
    if Application.get_env(:control_plane, :refuse_real_programs, false) do
      command == "false" or
        (Path.type(command) == :absolute and
           String.starts_with?(Path.expand(command), Path.expand(System.tmp_dir!())))
    else
      true
    end
  end

  @doc "The message for a program that is not allowed."
  def refusal(command),
    do:
      "refusing to run #{command}: the tests may only run a stand-in made in the temporary directory"
end
