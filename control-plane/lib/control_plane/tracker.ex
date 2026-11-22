defmodule ControlPlane.Tracker do
  @moduledoc """
  Where a repository's tickets live. The control plane reads and writes tickets only through
  this boundary, and never sees a system's own payload: every operation answers in the
  control plane's own structures (`ControlPlane.Ticket` and its companions).

  Each operation names the repository the control plane knows the work by and is
  dispatched to the tracker configured for it (`ControlPlane.Systems`); a module that
  implements the behaviour receives the project the repository's tickets are in.
  Failures are `{:error, message}`, and the message is what an operator reads.

  `pages:` on a history or comments read says whether only the first page is read (`:first`,
  the default) or every page (`:all`).
  """

  alias ControlPlane.{Systems, Ticket, TicketComment, TicketEdit, TicketEvent}

  @type project :: String.t()
  @type key :: term()
  @type error :: {:error, String.t()}
  @type capability :: :sub_tickets | :blocking_links | :label_actor_and_time

  @callback capabilities() :: MapSet.t(capability())
  @callback approved_tickets(project, label :: String.t()) :: {:ok, [Ticket.t()]} | error
  @callback ticket(project, key) :: {:ok, Ticket.t()} | error
  @callback parent(project, key) :: {:ok, Ticket.t() | nil} | error
  @callback ticket_history(project, key, keyword()) :: {:ok, [TicketEvent.t()]} | error
  @callback ticket_edits(project, key) :: {:ok, [TicketEdit.t()]} | error
  @callback open_epics(project) :: {:ok, [Ticket.t()]} | error
  @callback sub_tickets(project, key) :: {:ok, [Ticket.t()]} | error
  @callback comments(project, key, keyword()) :: {:ok, [TicketComment.t()]} | error
  @callback create_ticket(project, title :: String.t(), body :: String.t()) ::
              {:ok, Ticket.t()} | error
  @callback link_sub_ticket(project, parent :: key, child :: Ticket.t()) :: :ok | error
  @callback block_ticket(project, key, blocker :: Ticket.t()) :: :ok | error
  @callback ensure_labels(project, labels :: [ControlPlane.TicketLabels.label()]) :: :ok | error
  @callback add_label(project, key, label :: String.t()) :: :ok | error
  @callback remove_label(project, key, label :: String.t()) :: :ok | error
  @callback comment_on_ticket(project, key, body :: String.t()) :: :ok | error
  @callback close_ticket(project, key) :: :ok | error

  @doc "What the repository's tracker supports."
  def capabilities(repository) do
    {module, _project} = Systems.tracker(repository)
    module.capabilities()
  end

  def approved_tickets(repository, label), do: call(repository, :approved_tickets, [label])
  def ticket(repository, key), do: call(repository, :ticket, [key])
  def parent(_repository, _key), do: raise("not built")

  def ticket_history(repository, key, opts \\ []),
    do: call(repository, :ticket_history, [key, opts])

  def ticket_edits(repository, key), do: call(repository, :ticket_edits, [key])
  def open_epics(repository), do: call(repository, :open_epics, [])
  def sub_tickets(repository, key), do: call(repository, :sub_tickets, [key])
  def comments(repository, key, opts \\ []), do: call(repository, :comments, [key, opts])
  def create_ticket(repository, title, body), do: call(repository, :create_ticket, [title, body])

  def link_sub_ticket(repository, parent, child),
    do: call(repository, :link_sub_ticket, [parent, child])

  def block_ticket(repository, key, blocker), do: call(repository, :block_ticket, [key, blocker])

  @doc """
  Make the labels exist in the repository's tracker: read what is there once, create only
  what is missing, and leave every label that exists as it is.
  """
  def ensure_labels(repository, labels), do: call(repository, :ensure_labels, [labels])

  def add_label(repository, key, label), do: call(repository, :add_label, [key, label])
  def remove_label(repository, key, label), do: call(repository, :remove_label, [key, label])

  def comment_on_ticket(repository, key, body),
    do: call(repository, :comment_on_ticket, [key, body])

  def close_ticket(repository, key), do: call(repository, :close_ticket, [key])

  defp call(repository, operation, arguments) do
    {module, project} = Systems.tracker(repository)
    apply(module, operation, [project | arguments])
  end
end
