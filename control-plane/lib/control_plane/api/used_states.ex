defmodule ControlPlane.Api.UsedStates do
  @moduledoc """
  The login states already spent. A state is good for one callback: the cookie that carries it
  is still in the browser after the callback, so what makes a replay fail is this memory. A
  state is only remembered for as long as it could have been valid, so the memory stays small.
  """

  use Agent

  def start_link(_opts), do: Agent.start_link(fn -> %{} end, name: __MODULE__)

  @doc "Spend a state: `:ok` the first time, `:used` after. `expires_at` is in unix seconds."
  @spec consume(String.t(), integer()) :: :ok | :used
  def consume(nonce, expires_at) do
    now = System.system_time(:second)

    Agent.get_and_update(__MODULE__, fn used ->
      used = Map.filter(used, fn {_, expiry} -> expiry > now end)

      if Map.has_key?(used, nonce),
        do: {:used, used},
        else: {:ok, Map.put(used, nonce, expires_at)}
    end)
  end
end
