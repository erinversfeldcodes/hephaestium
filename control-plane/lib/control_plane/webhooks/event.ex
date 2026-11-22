defmodule ControlPlane.Webhooks.Event do
  @moduledoc """
  What a code host or a tracker said happened, in the control plane's own words and nothing
  else of what it sent. An event is a hint to look: it never carries a system's state, so a
  handler reads the current state through the behaviours, and an event may be late, repeated
  or out of order without changing what is done.

  `number` is the ticket's key or the change's number and is nil when the sender gave none
  (a check run that belongs to no change); `head` is the commit a check or a change is at;
  `on` says whether a comment is on a `:ticket` or a `:change`; `cause` is the reason a kind
  has more than one (`:unapproved`, `:review`, ...).
  """

  @kinds ~w(ticket_approved ticket_edited ticket_closed ticket_reopened comment_added
            checks_completed change_updated change_merged change_closed base_moved)a

  defstruct [:kind, :source, :repository, :number, :actor, :at, :delivery_id, :cause, :on, :head]

  @type t :: %__MODULE__{}

  @doc "The kinds of event there are."
  def kinds, do: @kinds

  @doc "The event as the arguments of a job: plain strings, so they survive the database."
  @spec to_args(t()) :: map()
  def to_args(%__MODULE__{} = event) do
    event
    |> Map.from_struct()
    |> Map.new(fn
      {key, %DateTime{} = at} ->
        {Atom.to_string(key), DateTime.to_iso8601(at)}

      {key, atom} when is_atom(atom) and not is_nil(atom) ->
        {Atom.to_string(key), Atom.to_string(atom)}

      {key, value} ->
        {Atom.to_string(key), value}
    end)
  end

  @doc "The event from the arguments of a job, or `:error` if they are not one."
  @spec from_args(map()) :: {:ok, t()} | :error
  def from_args(%{"kind" => kind} = args) do
    with {:ok, kind} <- one_of(kind, @kinds),
         {:ok, at} <- time(args["at"]) do
      {:ok,
       %__MODULE__{
         kind: kind,
         source: args["source"],
         repository: args["repository"],
         number: args["number"],
         actor: args["actor"],
         at: at,
         delivery_id: args["delivery_id"],
         cause: atom(args["cause"]),
         on: atom(args["on"]),
         head: args["head"]
       }}
    end
  end

  def from_args(_), do: :error

  # Only atoms the control plane already knows are made from text a job holds.
  @known ~w(unapproved edited review synchronize ticket change)a

  defp atom(nil), do: nil
  defp atom(text) when is_binary(text), do: Enum.find(@known, &(Atom.to_string(&1) == text))
  defp atom(_), do: nil

  defp one_of(text, atoms) when is_binary(text) do
    case Enum.find(atoms, &(Atom.to_string(&1) == text)) do
      nil -> :error
      atom -> {:ok, atom}
    end
  end

  defp one_of(_, _), do: :error

  defp time(text) when is_binary(text) do
    case DateTime.from_iso8601(text) do
      {:ok, at, _offset} -> {:ok, at}
      _ -> :error
    end
  end

  defp time(_), do: :error
end
