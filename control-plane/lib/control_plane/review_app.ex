defmodule ControlPlane.ReviewApp do
  @moduledoc """
  The GitHub App that approves a pull request when every lens that blocks has approved
  its head commit, and what it takes to act as it: a token for the repository.

  The App has a private key in a file the operator names (`HEPH_REVIEW_APP_KEY_FILE`), or
  held in a variable itself (`HEPH_REVIEW_APP_KEY`) where the platform keeps secrets as
  variables, and an id (`HEPH_REVIEW_APP_ID`). The variable wins when both are set. The key signs a short-lived assertion that names the App;
  GitHub exchanges the assertion for a token that works on one repository for about an
  hour. The token is kept in memory until shortly before it ends and is handed to `gh`
  through its environment, so it is in no argument and no log. The key is read each time a
  token is needed, and nothing here ever repeats what was in it.

  `gh` cannot send the assertion (it sends a token's scheme, which GitHub refuses for it),
  so these few requests are made directly, over TLS with the system's roots.
  """

  @default_api "https://api.github.com"
  # An assertion may last ten minutes at most; this one lasts nine and is back-dated a
  # minute for a clock that is a little behind.
  @lasts 540
  @back_dated 60
  # A token is asked for again once it has less than this left.
  @margin 300
  @timeout 10_000

  @doc "Whether an approval can be submitted at all: the App and its key are both named."
  @spec enabled?() :: boolean()
  def enabled? do
    present?(settings()[:app_id]) and
      (present?(settings()[:key]) or present?(settings()[:key_file]))
  end

  @doc """
  A token for this repository, and the login the App's reviews carry, or why there is none.
  `:disabled` when the App is not configured.
  """
  @spec token(String.t()) ::
          {:ok, %{token: String.t(), login: String.t()}} | {:error, String.t()} | :disabled
  def token(repository) do
    cond do
      is_function(settings()[:token], 1) -> settings()[:token].(repository)
      not enabled?() -> :disabled
      true -> cached(repository)
    end
  end

  @doc "Forget every kept token."
  @spec forget() :: :ok
  def forget do
    for {{__MODULE__, _} = key, _} <- :persistent_term.get(), do: :persistent_term.erase(key)
    :ok
  end

  @doc "The signed assertion that names the App, valid from `now` (seconds)."
  @spec assertion(integer()) :: {:ok, String.t()} | {:error, String.t()}
  def assertion(now \\ System.os_time(:second)) do
    with {:ok, key} <- read_key() do
      header = encode(%{"alg" => "RS256", "typ" => "JWT"})

      claims =
        encode(%{
          "iat" => now - @back_dated,
          "exp" => now + @lasts,
          "iss" => to_string(settings()[:app_id])
        })

      signature =
        (header <> "." <> claims)
        |> :public_key.sign(:sha256, key)
        |> Base.url_encode64(padding: false)

      {:ok, header <> "." <> claims <> "." <> signature}
    end
  end

  defp cached(repository) do
    key = {__MODULE__, repository}
    now = System.os_time(:second)

    case :persistent_term.get(key, nil) do
      %{expires_at: expires} = kept when expires - now > @margin ->
        {:ok, Map.take(kept, [:token, :login])}

      _ ->
        with {:ok, fresh} <- mint(repository) do
          :persistent_term.put(key, fresh)
          {:ok, Map.take(fresh, [:token, :login])}
        end
    end
  end

  defp mint(repository) do
    with {:ok, jwt} <- assertion(),
         {:ok, %{"slug" => slug}} <- request(:get, "/app", jwt, nil, "the App"),
         {:ok, %{"id" => installation}} <-
           request(:get, "/repos/#{repository}/installation", jwt, nil, "the App's installation"),
         {:ok, %{"token" => token, "expires_at" => expires_at}} <-
           request(
             :post,
             "/app/installations/#{installation}/access_tokens",
             jwt,
             %{"repositories" => [repository |> String.split("/") |> List.last()]},
             "a token for the installation"
           ),
         {:ok, expires, _} <- DateTime.from_iso8601(expires_at) do
      {:ok, %{token: token, login: slug <> "[bot]", expires_at: DateTime.to_unix(expires)}}
    else
      {:error, _} = error -> error
      _ -> {:error, "GitHub did not give a token in the shape expected"}
    end
  end

  defp request(method, path, jwt, body, what) do
    url = String.to_charlist((settings()[:api_url] || @default_api) <> path)

    headers = [
      {~c"authorization", String.to_charlist("Bearer " <> jwt)},
      {~c"accept", ~c"application/vnd.github+json"},
      {~c"user-agent", ~c"hephaestium-control-plane"}
    ]

    request =
      if method == :get,
        do: {url, headers},
        else: {url, headers, ~c"application/json", Jason.encode!(body)}

    case :httpc.request(method, request, http_options(url), body_format: :binary) do
      {:ok, {{_, status, _}, _, answer}} when status in 200..299 ->
        case Jason.decode(answer) do
          {:ok, decoded} -> {:ok, decoded}
          _ -> {:error, "GitHub's answer for #{what} was not JSON"}
        end

      {:ok, {{_, status, _}, _, _}} ->
        {:error, "GitHub refused #{what} (HTTP #{status})"}

      {:error, reason} ->
        {:error, "could not ask GitHub for #{what}: #{inspect_reason(reason)}"}
    end
  end

  defp http_options(url) do
    base = [timeout: @timeout, connect_timeout: @timeout]

    if List.starts_with?(url, ~c"https") do
      [
        ssl: [
          verify: :verify_peer,
          cacerts: :public_key.cacerts_get(),
          customize_hostname_check: [
            match_fun: :public_key.pkix_verify_hostname_match_fun(:https)
          ]
        ]
      ] ++ base
    else
      base
    end
  end

  # What went wrong, without anything that was sent.
  defp inspect_reason({:failed_connect, _}), do: "the connection failed"
  defp inspect_reason(:timeout), do: "it timed out"
  defp inspect_reason(_), do: "the request failed"

  defp read_key do
    if present?(settings()[:key]) do
      parse_key(settings()[:key], "the review App's key in HEPH_REVIEW_APP_KEY")
    else
      read_key_file(settings()[:key_file])
    end
  end

  defp read_key_file(file) do
    case File.read(file) do
      {:ok, pem} ->
        parse_key(pem, "the file #{file}")

      {:error, reason} ->
        {:error, "the review App's key file #{file} could not be read (#{reason})"}
    end
  end

  # What was read is never repeated, only where it came from.
  defp parse_key(pem, source) do
    with [entry | _] <- :public_key.pem_decode(pem),
         key when is_tuple(key) <- decode(entry) do
      {:ok, key}
    else
      _ -> {:error, "#{source} does not hold a private key the review App can sign with"}
    end
  end

  defp decode(entry) do
    :public_key.pem_entry_decode(entry)
  rescue
    _ -> nil
  end

  defp encode(map), do: map |> Jason.encode!() |> Base.url_encode64(padding: false)

  defp present?(value), do: is_binary(value) and value != ""

  defp settings, do: Application.get_env(:control_plane, :review_app) || []
end
