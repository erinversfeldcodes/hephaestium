defmodule ControlPlane.Ticket do
  @moduledoc """
  A ticket as the control plane reads it, whichever tracker it lives in.

  `key` is what names it in its project (a number on GitHub). `id` is the tracker's own
  handle for linking it to another ticket, when that differs from the key. `progress` is
  how many of its sub-tickets are done, when it has any. `closed_at` is when it closed.
  """

  defstruct [:key, :id, :state, :closed_at, :progress, labels: []]

  @type t :: %__MODULE__{
          key: term(),
          id: term(),
          state: :open | :closed,
          closed_at: DateTime.t() | nil,
          progress: %{completed: term(), total: integer()} | nil,
          labels: [String.t()]
        }
end

defmodule ControlPlane.TicketEvent do
  @moduledoc """
  One thing that happened to a ticket, with who did it and when. `kind` is `:labeled`,
  `:unlabeled`, `:renamed`, `:closed`, `:reopened` or `:other`. `at` is nil when the tracker
  gave no time or one that cannot be read, and `at_text` is what it gave.
  """

  defstruct [:kind, :label, :actor, :at, :at_text]

  @type t :: %__MODULE__{
          kind: atom(),
          label: String.t() | nil,
          actor: String.t() | nil,
          at: DateTime.t() | nil,
          at_text: String.t() | nil
        }
end

defmodule ControlPlane.TicketEdit do
  @moduledoc "An edit of a ticket's own text: who made it, if the tracker says, and when."

  defstruct [:editor, :at]

  @type t :: %__MODULE__{editor: String.t() | nil, at: DateTime.t() | nil}
end

defmodule ControlPlane.TicketComment do
  @moduledoc "A comment on a ticket: its text and when it was made."

  defstruct [:body, :at]

  @type t :: %__MODULE__{body: String.t() | nil, at: DateTime.t() | nil}
end

defmodule ControlPlane.Change do
  @moduledoc """
  A change (a pull request or merge request) as the control plane reads it.

  `mergeability` is `:clean`, `:behind`, `:blocked`, `:conflicted`, `:draft` or `:unknown`
  (the code host has not worked it out), or `{:other, text}` for a state the code host names
  that none of those covers.
  """

  defstruct [
    :number,
    :url,
    :state,
    :head_sha,
    :head_ref,
    :head_repository,
    :base_ref,
    merged: false,
    mergeability: :unknown
  ]

  @type t :: %__MODULE__{
          number: term(),
          url: String.t() | nil,
          state: :open | :closed | :other,
          merged: boolean(),
          head_sha: String.t() | nil,
          head_ref: String.t() | nil,
          head_repository: String.t() | nil,
          base_ref: String.t() | nil,
          mergeability:
            :clean | :behind | :blocked | :conflicted | :draft | :unknown | {:other, String.t()}
        }
end

defmodule ControlPlane.Check do
  @moduledoc """
  One run of a check on a commit. `state` is `:completed` or `:running`. `conclusion` is the
  code host's word for how it ended (`"success"`, `"failure"`, `"cancelled"`, ...) and nil
  while it runs. `title` and `summary` are what the check said of itself.
  """

  defstruct [:id, :name, :state, :conclusion, :title, :summary]

  @type t :: %__MODULE__{
          id: term(),
          name: String.t(),
          state: :completed | :running,
          conclusion: String.t() | nil,
          title: String.t() | nil,
          summary: String.t() | nil
        }
end

defmodule ControlPlane.ChangeReview do
  @moduledoc "A review on a change: who gave it, its state, and the commit it was of."

  defstruct [:author, :state, :commit]

  @type t :: %__MODULE__{
          author: String.t() | nil,
          state: String.t() | nil,
          commit: String.t() | nil
        }
end
