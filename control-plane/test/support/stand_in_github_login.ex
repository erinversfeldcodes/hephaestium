defmodule ControlPlane.StandInGitHubLogin do
  @moduledoc """
  A stand-in for GitHub's sign-in, for tests: the codes a test lists as valid (code to login)
  are the only ones it accepts. It never reaches the network.
  """

  @behaviour ControlPlane.Api.GitHubLogin

  @doc "The codes this stand-in accepts, and the login each one signs in as."
  def accept(codes), do: Application.put_env(:control_plane, :stand_in_logins, codes)

  @impl true
  def exchange_code(code, _redirect_uri, _settings) do
    if Map.has_key?(codes(), code),
      do: {:ok, "token-for-" <> code},
      else: {:error, "GitHub refused the code"}
  end

  @impl true
  def login("token-for-" <> code) do
    case Map.fetch(codes(), code) do
      {:ok, login} -> {:ok, login}
      :error -> {:error, "no such token"}
    end
  end

  def login(_), do: {:error, "no such token"}

  defp codes, do: Application.get_env(:control_plane, :stand_in_logins, %{})
end
