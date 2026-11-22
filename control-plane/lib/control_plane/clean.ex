defmodule ControlPlane.Clean do
  @moduledoc """
  Text from an agent or a tool is kept as it came, except for what Postgres cannot hold: a NUL
  byte cannot be in text or in jsonb, and one in an agent's output would raise in the process
  that keeps it. Each is replaced by U+FFFD, the character that stands for one that could not be
  read, so nothing raises and a stream has no gap where an event would have been refused. The
  raw line stays in the run's own events file.
  """

  @nul <<0>>
  @replacement "\u{FFFD}"

  @doc """
  The term with every string in it cleaned: in a map's keys and values and in a list. What is
  not a string, and a struct (a value, not a map of strings), is left as it is.
  """
  @spec deep(term()) :: term()
  def deep(%{__struct__: _} = struct), do: struct
  def deep(%{} = map), do: Map.new(map, fn {key, value} -> {deep(key), deep(value)} end)
  def deep(list) when is_list(list), do: Enum.map(list, &deep/1)
  def deep(text) when is_binary(text), do: String.replace(text, @nul, @replacement)
  def deep(other), do: other
end
