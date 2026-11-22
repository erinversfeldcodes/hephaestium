defmodule ControlPlane.StandInSystems do
  @moduledoc """
  A tracker and a code host that are not GitHub, to prove that the control plane reaches
  them only through the two behaviours: each operation records what it was asked, with the
  name it was given, and answers what the test said, or an error.

  This proves the logic is written against the boundary and not against GitHub; it does not
  prove that any real second system fits the boundary.
  """

  use Agent

  def start_link(_), do: Agent.start_link(fn -> %{answers: %{}, calls: []} end, name: __MODULE__)

  @doc "Make an operation of `:tracker` or `:code_host` answer this."
  def answer(side, operation, value),
    do: Agent.update(__MODULE__, &put_in(&1.answers[{side, operation}], value))

  @doc "Every call made, oldest first, as `{side, operation, arguments}`."
  def calls, do: Agent.get(__MODULE__, &Enum.reverse(&1.calls))

  @doc false
  def ask(side, operation, arguments) do
    Agent.get_and_update(__MODULE__, fn state ->
      answer = Map.get(state.answers, {side, operation}, {:error, "no answer for #{operation}"})
      {answer, %{state | calls: [{side, operation, arguments} | state.calls]}}
    end)
  end
end

defmodule ControlPlane.StandInTracker do
  @moduledoc "The stand-in tracker. See `ControlPlane.StandInSystems`."
  @behaviour ControlPlane.Tracker

  alias ControlPlane.StandInSystems, as: S

  @impl true
  def capabilities, do: MapSet.new()
  @impl true
  def approved_tickets(p, label), do: S.ask(:tracker, :approved_tickets, [p, label])
  @impl true
  def ticket(p, k), do: S.ask(:tracker, :ticket, [p, k])
  @impl true
  def ticket_history(p, k, o), do: S.ask(:tracker, :ticket_history, [p, k, o])
  @impl true
  def ticket_edits(p, k), do: S.ask(:tracker, :ticket_edits, [p, k])
  @impl true
  def open_epics(p), do: S.ask(:tracker, :open_epics, [p])
  @impl true
  def sub_tickets(p, k), do: S.ask(:tracker, :sub_tickets, [p, k])
  @impl true
  def comments(p, k, o), do: S.ask(:tracker, :comments, [p, k, o])
  @impl true
  def create_ticket(p, t, b), do: S.ask(:tracker, :create_ticket, [p, t, b])
  @impl true
  def link_sub_ticket(p, k, c), do: S.ask(:tracker, :link_sub_ticket, [p, k, c])
  @impl true
  def block_ticket(p, k, b), do: S.ask(:tracker, :block_ticket, [p, k, b])
  @impl true
  def add_label(p, k, l), do: S.ask(:tracker, :add_label, [p, k, l])
  @impl true
  def remove_label(p, k, l), do: S.ask(:tracker, :remove_label, [p, k, l])
  @impl true
  def comment_on_ticket(p, k, b), do: S.ask(:tracker, :comment_on_ticket, [p, k, b])
  @impl true
  def close_ticket(p, k), do: S.ask(:tracker, :close_ticket, [p, k])
end

defmodule ControlPlane.StandInCodeHost do
  @moduledoc "The stand-in code host. See `ControlPlane.StandInSystems`."
  @behaviour ControlPlane.CodeHost

  alias ControlPlane.StandInSystems, as: S

  @impl true
  def capabilities, do: MapSet.new()
  @impl true
  def change(r, n), do: S.ask(:code_host, :change, [r, n])
  @impl true
  def change_files(r, n, o), do: S.ask(:code_host, :change_files, [r, n, o])
  @impl true
  def compare(r, f, t), do: S.ask(:code_host, :compare, [r, f, t])
  @impl true
  def comment_on_change(r, n, b), do: S.ask(:code_host, :comment_on_change, [r, n, b])
  @impl true
  def merge(r, n, c), do: S.ask(:code_host, :merge, [r, n, c])
  @impl true
  def reviews(r, n), do: S.ask(:code_host, :reviews, [r, n])
  @impl true
  def approve(r, n, c, b), do: S.ask(:code_host, :approve, [r, n, c, b])
  @impl true
  def required_checks(r, b), do: S.ask(:code_host, :required_checks, [r, b])
  @impl true
  def checks(r, c), do: S.ask(:code_host, :checks, [r, c])
  @impl true
  def check_log(r, i), do: S.ask(:code_host, :check_log, [r, i])
  @impl true
  def check_annotations(r, i), do: S.ask(:code_host, :check_annotations, [r, i])
  @impl true
  def rerun_check(r, i), do: S.ask(:code_host, :rerun_check, [r, i])
  @impl true
  def file(r, p, ref), do: S.ask(:code_host, :file, [r, p, ref])
  @impl true
  def directory(r, p, ref), do: S.ask(:code_host, :directory, [r, p, ref])
  @impl true
  def permission(r, l), do: S.ask(:code_host, :permission, [r, l])
end
