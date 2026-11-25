defmodule ControlPlane.Answers do
  @moduledoc """
  What a person answered to the questions agents asked on a ticket.

  An answer is kept on the workload that asked, when the person gives it on the dashboard, and
  is part of the request of every later workload on the ticket: the agent that is run again
  after a failed gate is not asked to guess what the person already settled. An answer written
  as a plain comment on the ticket is not kept here, and no run reads it.
  """

  import Ecto.Query

  alias ControlPlane.{Repo, Workload, Workloads}

  @doc "Keep a person's answer on the workload that asked the question."
  @spec keep(Ecto.UUID.t(), String.t()) :: :ok
  def keep(workload_id, answer) do
    {1, _} =
      Repo.update_all(from(w in Workload, where: w.id == ^workload_id), set: [answer: answer])

    :ok
  end

  @doc """
  Every question of the ticket that has an answer, oldest first, each with that answer. A
  question answered twice is listed once, with the later answer.
  """
  @spec for_ticket(String.t(), integer()) :: [%{String.t() => String.t()}]
  def for_ticket(repository, number) do
    repository
    |> Workloads.for_ticket(number)
    |> Enum.flat_map(fn
      %Workload{answer: answer, record: %{"question" => question}} ->
        answered(question, answer)

      _ ->
        []
    end)
    |> latest_per_question()
  end

  @doc """
  The answers of several tickets, read in one query that leaves out the workloads' large
  requests and records: a map from `{repository, number}` to what `for_ticket/2` lists.
  """
  @spec for_tickets([{String.t(), integer()}]) :: %{{String.t(), integer()} => [map()]}
  def for_tickets([]), do: %{}

  def for_tickets(tickets) do
    wanted = MapSet.new(tickets)
    repositories = tickets |> Enum.map(&elem(&1, 0)) |> Enum.uniq()
    numbers = tickets |> Enum.map(&elem(&1, 1)) |> Enum.uniq()

    Repo.all(
      from w in Workload,
        where: w.repository in ^repositories and w.ticket_number in ^numbers,
        where: not is_nil(w.answer) and w.answer != "",
        order_by: [w.inserted_at, w.id],
        select: {w.repository, w.ticket_number, fragment("?->>'question'", w.record), w.answer}
    )
    |> Enum.filter(fn {repository, number, _, _} ->
      MapSet.member?(wanted, {repository, number})
    end)
    |> Enum.group_by(
      fn {repository, number, _, _} -> {repository, number} end,
      fn {_, _, question, answer} -> answered(question, answer) end
    )
    |> Map.new(fn {key, lists} -> {key, lists |> List.flatten() |> latest_per_question()} end)
  end

  defp answered(question, answer)
       when is_binary(answer) and answer != "" and is_binary(question),
       do: [%{"question" => question, "answer" => answer}]

  defp answered(_, _), do: []

  defp latest_per_question(answers) do
    answers
    |> Enum.reverse()
    |> Enum.uniq_by(&normalise(&1["question"]))
    |> Enum.reverse()
  end

  @doc "Put the ticket's answers in the request, when it has any."
  @spec add(map(), String.t(), integer()) :: map()
  def add(workload, repository, number) do
    case for_ticket(repository, number) do
      [] -> workload
      answers -> Map.put(workload, "answers", answers)
    end
  end

  @doc "The answer already given to this question among a ticket's answers, or nil."
  @spec earlier([map()], String.t()) :: String.t() | nil
  def earlier(answers, question) do
    wanted = normalise(question)

    Enum.find_value(answers, fn %{"question" => asked, "answer" => answer} ->
      if normalise(asked) == wanted, do: answer
    end)
  end

  # The same question asked again is not worded the same twice.
  defp normalise(text) do
    text |> String.downcase() |> String.split() |> Enum.join(" ")
  end
end
