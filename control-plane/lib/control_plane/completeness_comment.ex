defmodule ControlPlane.CompletenessComment do
  @moduledoc """
  The comment a completeness verdict becomes on its ticket.

  A verdict says whether the ticket's goal is met, requirement by requirement, who should
  decide whether to close it, and what is still owed. It is rendered for a person to read.
  Every word in it was written by an agent reading text that other people wrote, so what
  goes into the table and the lists is one line each, with the characters that would break
  a table escaped, and nothing in it can start a heading or a new block.
  """

  @doc "The comment for a verdict, as the run record holds it."
  @spec render(map()) :: String.t()
  def render(verdict) do
    [
      headline(verdict),
      deciding(verdict),
      table(verdict["requirements"] || []),
      goal(verdict["goal"]),
      owed(verdict["owed"] || []),
      list("Not filed", verdict["not_filed"] || []),
      list("Could not check", verdict["could_not_check"] || [])
    ]
    |> Enum.reject(&(&1 in [nil, ""]))
    |> Enum.join("\n\n")
  end

  defp headline(verdict) do
    "**Hephaestium:** judged whether this ticket's goal is met, at `#{short(verdict["commit"])}` of the default branch: **#{words(verdict["verdict"])}**."
  end

  defp deciding(%{"decides" => "evidence"}), do: "The evidence supports closing this ticket."

  defp deciding(verdict) do
    stays =
      if verdict["verdict"] == "owed",
        do: " The ticket stays open until what is owed is settled.",
        else: ""

    "A person decides whether to close this ticket: #{line(verdict["decides_because"])}.#{stays}"
  end

  defp table([]), do: ""

  defp table(requirements) do
    rows =
      requirements
      |> Enum.with_index(1)
      |> Enum.map(fn {requirement, number} ->
        evidence = requirement["evidence"] || %{}

        "| #{number} | #{cell(requirement["text"])} | #{cell(requirement["settled_by"])} | #{cell(words(requirement["status"]))} | #{evidence_cell(evidence)} | #{cell(requirement["gap"])} |"
      end)

    Enum.join(
      [
        "| # | Requirement | Settled by | Status | Evidence | Gap or next step |",
        "|---|---|---|---|---|---|" | rows
      ],
      "\n"
    )
  end

  defp evidence_cell(%{"kind" => kind, "where" => where}), do: cell("#{kind}: #{where}")
  defp evidence_cell(_), do: ""

  defp goal(%{"status" => status, "why" => why}),
    do: "Goal: #{words(status)}. #{line(why)}"

  defp goal(_), do: ""

  defp owed([]), do: ""

  defp owed(items) do
    lines =
      for item <- items do
        "- #{line(item["what"])} (run: `#{code(item["run"])}`; when: #{line(item["when"])}; settled by: #{line(item["settled_by"])})"
      end

    Enum.join(["Owed:" | lines], "\n")
  end

  defp list(_title, []), do: ""

  defp list(title, items),
    do: Enum.join(["#{title}:" | Enum.map(items, &"- #{line(&1)}")], "\n")

  defp short(commit) when is_binary(commit), do: String.slice(commit, 0, 7)
  defp short(_), do: "an unknown commit"

  defp words(nil), do: ""
  defp words(text), do: text |> to_string() |> String.replace("_", " ")

  # One line, with nothing that starts a block.
  defp line(nil), do: ""

  defp line(text) do
    text
    |> to_string()
    |> String.replace(~r/\s+/, " ")
    |> String.trim()
  end

  # One line that is also safe inside a table cell.
  defp cell(text), do: text |> line() |> String.replace("|", "\\|")

  # A command inside backticks must not be able to end them.
  defp code(text), do: text |> line() |> String.replace("`", "'")
end
