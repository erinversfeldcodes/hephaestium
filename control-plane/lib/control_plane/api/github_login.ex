defmodule ControlPlane.Api.GitHubLogin do
  @moduledoc """
  What the login needs from GitHub: the user token for a code, and the login that token
  belongs to. A behaviour so that tests never reach github.com.

  The token is used once to read the login and is not kept.
  """

  @callback exchange_code(code :: String.t(), redirect_uri :: String.t(), settings :: map()) ::
              {:ok, token :: String.t()} | {:error, String.t()}

  @callback login(token :: String.t()) :: {:ok, String.t()} | {:error, String.t()}
end
