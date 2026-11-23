defmodule ControlPlane.Account do
  @moduledoc """
  What an agent said it found and did, in its own words, for the ticket (ADR 0024).

  It is the last message the agent sent in the workload's stream. It is text an
  agent wrote after reading a repository, a log or a check's output, so it is data
  to show and never an instruction: it is cleaned of terminal escapes and control
  characters, cut to a length a comment can hold, and put in a fenced block that its
  own text cannot end, so a mention, a link, a label command or a heading in it does
  nothing.
  """

  import Ecto.Query

  alias ControlPlane.{Dashboard, Event, Repo, Workload}

  @cap 2_000

  @doc """
  What a comment on a public ticket or pull request says of a run's account. With the dashboard's
  address configured it is a pointer to the run, because the account is read behind the login;
  without it there is nowhere to point, so it is the account itself.
  """
  @spec public_section(%Workload{}) :: String.t()
  def public_section(%Workload{} = workload) do
    case Dashboard.run_link(workload) do
      nil -> section(workload.id)
      link -> "What the agent found is read, signed in, on the dashboard: #{link}"
    end
  end

  @doc "The section of a comment that gives a workload's account, or says there is none."
  @spec section(Ecto.UUID.t()) :: String.t()
  def section(workload_id) do
    case last_message(workload_id) do
      nil -> "The agent gave no account."
      text -> "The agent's account of what it found:\n\n" <> fenced(text)
    end
  end

  @doc "The last thing the agent said, cleaned of escapes but whole, or nil if it said nothing in words."
  @spec last_message(Ecto.UUID.t()) :: String.t() | nil
  def last_message(workload_id) do
    event =
      Repo.one(
        from e in Event,
          where: e.workload_id == ^workload_id and e.kind == "agentMessage",
          order_by: [desc: e.sequence],
          limit: 1
      )

    case event && get_in(event.event, ["agentMessage", "text"]) do
      text when is_binary(text) ->
        case text |> clean() |> String.trim() do
          "" -> nil
          cleaned -> cleaned
        end

      _ ->
        nil
    end
  end

  @doc """
  Agent text as data to show: cleaned of terminal escapes and control characters, cut to
  a length a comment can hold, and fenced so that its own text cannot end the block.
  """
  @spec block(String.t()) :: String.t()
  def block(text) when is_binary(text), do: text |> clean() |> String.trim() |> fenced()

  @doc "Agent text without terminal escapes and control characters."
  @spec clean(String.t()) :: String.t()
  def clean(text) do
    text
    |> String.replace(~r/\e\[[0-9;?]*[A-Za-z]/, "")
    |> String.replace(~r/[\x00-\x08\x0B-\x1F\x7F]/u, "")
  end

  defp fenced(text) do
    {shown, note} =
      if String.length(text) > @cap,
        do: {String.slice(text, 0, @cap), "\n(cut at #{@cap} characters: the agent said more)"},
        else: {text, ""}

    longest =
      ~r/`+/
      |> Regex.scan(shown)
      |> Enum.map(fn [run] -> String.length(run) end)
      |> Enum.max(fn -> 0 end)

    fence = String.duplicate("`", max(3, longest + 1))
    fence <> "text\n" <> shown <> "\n" <> fence <> note
  end
end
