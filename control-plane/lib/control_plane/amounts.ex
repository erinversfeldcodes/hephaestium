defmodule ControlPlane.Amounts do
  @moduledoc """
  How an amount spent is written for a person: dollars with three decimals, tokens as a whole
  number, credits as they are. Each unit is on its own, and nothing converts one to another.
  """

  # The units, in the order a person reads them.
  @units ["usd", "tokens", "credits"]

  @doc "One amount in a unit (`usd`, `tokens` or `credits`), or nil for a unit not known."
  @spec format(String.t(), number()) :: String.t() | nil
  def format("usd", amount), do: "$" <> :erlang.float_to_binary(amount * 1.0, decimals: 3)
  def format("tokens", amount), do: "#{trunc(amount)} tokens"
  def format("credits", amount), do: "#{amount} credits"
  def format(_unit, _amount), do: nil

  @doc "Every unit of a map of totals, dollars first, joined by `separator`."
  @spec describe(%{String.t() => number()}, String.t()) :: String.t()
  def describe(totals, separator \\ ", ") do
    totals
    |> Enum.sort_by(fn {unit, _} -> Enum.find_index(@units, &(&1 == unit)) || length(@units) end)
    |> Enum.map(fn {unit, amount} -> format(unit, amount) end)
    |> Enum.reject(&is_nil/1)
    |> Enum.join(separator)
  end
end
