defmodule ControlPlane.StepCostsTest do
  @moduledoc """
  What a ticket's status comment says a run spent. A run that read its own change under
  lenses lists each agent it started, so what a ticket spent on reading can be told apart
  from what it spent on the work. Dollars and tokens are different things and stay apart.
  """

  use ExUnit.Case, async: true

  alias ControlPlane.Tickets

  defp usd(amount), do: %{"unit" => "COST_UNIT_USD", "amount" => amount}
  defp tokens(amount), do: %{"unit" => "COST_UNIT_TOKENS", "amount" => amount}

  defp step(kind, costs, extra \\ %{}),
    do: Map.merge(%{"kind" => kind, "round" => 0, "costs" => costs}, extra)

  test "a run with no steps says what it spent as it always did" do
    record = %{"costs" => [usd(0.8)]}

    assert Tickets.cost_section(record) == "Cost, as the runtime reported it: $0.800"
  end

  test "a run that read its change says what each kind of agent spent" do
    record = %{
      "costs" => [usd(0.8)],
      "steps" => [
        step("KIND_IMPLEMENTATION", [usd(0.25)]),
        step("KIND_REVIEW", [usd(0.1)], %{"lens" => "rust", "round" => 1}),
        step("KIND_REVIEW", [usd(0.05)], %{"lens" => "spec", "round" => 1}),
        step("KIND_REVISION", [usd(0.4)], %{"round" => 1})
      ]
    }

    assert Tickets.cost_section(record) ==
             "Cost, as the runtime reported it: $0.800\n" <>
               "Of that: the implementation $0.250, reading under lenses $0.150, fixing what they found $0.400"
  end

  test "a kind of agent that spent nothing is not listed" do
    record = %{
      "costs" => [usd(0.35)],
      "steps" => [
        step("KIND_IMPLEMENTATION", [usd(0.25)]),
        step("KIND_REVIEW", [usd(0.1)], %{"lens" => "rust", "round" => 1})
      ]
    }

    section = Tickets.cost_section(record)

    assert section =~ "the implementation $0.250, reading under lenses $0.100"
    refute section =~ "fixing"
  end

  test "dollars and tokens are never added to each other" do
    record = %{
      "costs" => [usd(0.25), tokens(1_200)],
      "steps" => [
        step("KIND_IMPLEMENTATION", [usd(0.25), tokens(1_000)]),
        step("KIND_REVIEW", [tokens(200)], %{"lens" => "rust", "round" => 1})
      ]
    }

    section = Tickets.cost_section(record)

    assert section =~ "the implementation $0.250 and 1000 tokens"
    assert section =~ "reading under lenses 200 tokens"
  end

  test "steps with nothing spent give no breakdown at all" do
    record = %{
      "costs" => [usd(0.25)],
      "steps" => [step("KIND_IMPLEMENTATION", [])]
    }

    assert Tickets.cost_section(record) == "Cost, as the runtime reported it: $0.250"
  end

  test "a run that reported no cost says nothing about cost" do
    assert Tickets.cost_section(%{"steps" => [step("KIND_IMPLEMENTATION", [usd(0.25)])]}) == nil
    assert Tickets.cost_section(%{}) == nil
  end
end
