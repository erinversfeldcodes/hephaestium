defmodule ControlPlane.StandIn do
  @moduledoc """
  A stand-in for the executor: a program that records how it was started, prints a
  stream of events, writes a run record where it was told to, and then exits, or
  hangs, or waits to be told to stop.
  """

  @doc """
  Make a stand-in executor in `dir`, and return its path.

  Options:
    * `:events` - the lines it prints, as maps (encoded as JSON) or strings
    * `:record` - the run record it writes, as a map, if any
    * `:exit` - its exit status, default 0
    * `:then` - `:exit` (default), `:hang` (go quiet, and stop when told to),
      `:stubborn` (go quiet and ignore being told to stop, so only a kill ends it) or
      `{:stop_with, lines, record}` (wait, and when told to stop print `lines`,
      write `record` and exit)
  """
  def executor(dir, opts) do
    File.mkdir_p!(dir)
    lines = opts |> Keyword.get(:events, []) |> Enum.map(&encode/1)
    File.write!(Path.join(dir, "events.canned"), Enum.map_join(lines, &(&1 <> "\n")))

    if record = opts[:record] do
      File.write!(Path.join(dir, "record.canned"), Jason.encode!(record))
    end

    {then_script, stop_lines} =
      case Keyword.get(opts, :then, :exit) do
        :exit ->
          {"exit #{Keyword.get(opts, :exit, 0)}", nil}

        :hang ->
          {~S"""
           trap 'exit 143' TERM
           while :; do sleep 0.2; done
           """, nil}

        :stubborn ->
          {~S"""
           trap '' TERM
           while :; do sleep 0.2; done
           """, nil}

        {:stop_with, stop_lines, stop_record} ->
          File.write!(
            Path.join(dir, "stop.canned"),
            Enum.map_join(Enum.map(stop_lines, &encode/1), &(&1 <> "\n"))
          )

          File.write!(Path.join(dir, "stop-record.canned"), Jason.encode!(stop_record))

          {~S"""
           stop() { cat $d/stop.canned; cp $d/stop-record.canned "$rec"; exit 0; }
           trap stop TERM
           while :; do sleep 0.2; done
           """, :stop}
      end

    copy_record =
      if opts[:record] && stop_lines == nil,
        do: ~S|[ -n "$rec" ] && cp $d/record.canned "$rec"|,
        else: ":"

    script = """
    #!/bin/sh
    d=#{dir}
    echo $$ > $d/pid.txt
    echo started >> $d/calls.log
    printf '%s\\n' "$@" > $d/args.txt
    prev=''
    for a in "$@"; do [ "$prev" = '--record' ] && rec="$a"; prev="$a"; done
    cat $d/events.canned
    #{copy_record}
    #{then_script}
    """

    path = Path.join(dir, "executor")
    File.write!(path, script)
    File.chmod!(path, 0o755)
    path
  end

  defp encode(line) when is_binary(line), do: line
  defp encode(line), do: Jason.encode!(line)

  @doc """
  Whether the stand-in's process is still running. The process has to be the stand-in:
  a number that is another program's by now, and a process that has ended and not yet been
  collected, are not it.
  """
  def running?(dir) do
    pid = dir |> Path.join("pid.txt") |> File.read!() |> String.trim()

    case System.cmd("ps", ["-o", "command=", "-p", pid], stderr_to_stdout: true) do
      {command, 0} -> String.contains?(command, Path.join(dir, "executor"))
      _ -> false
    end
  end
end
