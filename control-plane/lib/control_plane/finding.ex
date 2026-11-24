defmodule ControlPlane.Finding do
  @moduledoc """
  What a review finding says about where it is, as the review contract has it: an object
  with a path and a line. A number out of a protobuf Struct is a float, so a whole line is
  written without its fraction. A location that is a plain string is written as it is, and
  anything else is left out: what a reviewer said about where must never stop a ticket
  being settled, or a comment being written.
  """

  @doc "The line that says where, or nil."
  @spec location_line(term()) :: String.t() | nil
  def location_line(%{"path" => path, "line" => line})
      when is_binary(path) and path != "" and is_number(line),
      do: "Location: #{path}:#{line_number(line)}"

  def location_line(location) when is_binary(location) and location != "",
    do: "Location: #{location}"

  def location_line(_), do: nil

  @doc """
  Where a finding is, for the API: a path and a line, either of which is nil when the
  lens did not give it. A location that is a plain string is the path alone.
  """
  @spec place(term()) :: %{path: String.t() | nil, line: integer() | nil} | nil
  def place(%{"path" => path} = location) when is_binary(path) and path != "" do
    line = location["line"]
    %{path: path, line: if(is_number(line), do: line_number(line) |> trunc(), else: nil)}
  end

  def place(location) when is_binary(location) and location != "",
    do: %{path: location, line: nil}

  def place(_), do: nil

  defp line_number(line) when is_float(line) and line == trunc(line), do: trunc(line)
  defp line_number(line), do: line
end
