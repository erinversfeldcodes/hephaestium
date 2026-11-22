defmodule ControlPlane.Api.GitHubLogin.Http do
  @moduledoc """
  The real GitHub: the App's user authorization (the web flow). The code is exchanged at
  `github.com/login/oauth/access_token` with the App's client id and secret, and the login
  is read from `api.github.com/user`. Errors say what failed and never what was sent.
  """

  @behaviour ControlPlane.Api.GitHubLogin

  @timeout 10_000

  @impl true
  def exchange_code(code, redirect_uri, settings) do
    body =
      URI.encode_query(%{
        client_id: settings.client_id,
        client_secret: settings.client_secret,
        code: code,
        redirect_uri: redirect_uri
      })

    request =
      {~c"https://github.com/login/oauth/access_token",
       [{~c"accept", ~c"application/json"}, {~c"user-agent", ~c"hephaestium-control-plane"}],
       ~c"application/x-www-form-urlencoded", body}

    with {:ok, decoded} <- send_request(:post, request, "the token exchange") do
      case decoded do
        %{"access_token" => token} when is_binary(token) and token != "" -> {:ok, token}
        _ -> {:error, "GitHub did not give a token for the code"}
      end
    end
  end

  @impl true
  def login(token) do
    request =
      {~c"https://api.github.com/user",
       [
         {~c"authorization", String.to_charlist("Bearer " <> token)},
         {~c"accept", ~c"application/vnd.github+json"},
         {~c"user-agent", ~c"hephaestium-control-plane"}
       ]}

    with {:ok, decoded} <- send_request(:get, request, "the user") do
      case decoded do
        %{"login" => login} when is_binary(login) and login != "" -> {:ok, login}
        _ -> {:error, "GitHub did not say who the token belongs to"}
      end
    end
  end

  defp send_request(method, request, what) do
    options = [
      timeout: @timeout,
      connect_timeout: @timeout,
      ssl: [
        verify: :verify_peer,
        cacerts: :public_key.cacerts_get(),
        customize_hostname_check: [match_fun: :public_key.pkix_verify_hostname_match_fun(:https)]
      ]
    ]

    case :httpc.request(method, request, options, body_format: :binary) do
      {:ok, {{_, status, _}, _, answer}} when status in 200..299 ->
        case Jason.decode(answer) do
          {:ok, decoded} -> {:ok, decoded}
          _ -> {:error, "GitHub's answer for #{what} was not JSON"}
        end

      {:ok, {{_, status, _}, _, _}} ->
        {:error, "GitHub refused #{what} (HTTP #{status})"}

      {:error, _} ->
        {:error, "could not ask GitHub for #{what}"}
    end
  end
end
