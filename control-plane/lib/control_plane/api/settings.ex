defmodule ControlPlane.Api.Settings do
  @moduledoc """
  What the API is configured with, checked once when it is built. Nothing here is printed or
  logged, and an error names the setting, never its value.
  """

  @min_key_bytes 32
  @default_ttl 8 * 3600

  @type t :: %{
          secret_key_base: String.t(),
          client_id: String.t(),
          client_secret: String.t(),
          base_url: String.t(),
          allowed_logins: [String.t()],
          github: module(),
          session_ttl: pos_integer(),
          poll_ms: pos_integer(),
          secure: boolean()
        }

  @doc "The settings from a keyword list (the application's `:api` environment), or it raises."
  @spec build!(keyword()) :: t()
  def build!(config) do
    key = config[:secret_key_base]

    if not is_binary(key) or byte_size(key) < @min_key_bytes do
      raise ArgumentError,
            "HEPH_API_SECRET_KEY_BASE is not set or is shorter than #{@min_key_bytes} bytes: the API will not start without a key to sign sessions with"
    end

    for {name, setting} <- [
          {"HEPH_API_GITHUB_CLIENT_ID", :client_id},
          {"HEPH_API_GITHUB_CLIENT_SECRET", :client_secret},
          {"HEPH_API_BASE_URL", :base_url}
        ] do
      if config[setting] in [nil, ""],
        do: raise(ArgumentError, "#{name} is not set: the API cannot sign anyone in without it")
    end

    base_url = config[:base_url] |> String.trim_trailing("/")

    %{
      secret_key_base: key,
      client_id: config[:client_id],
      client_secret: config[:client_secret],
      base_url: base_url,
      allowed_logins: Enum.map(config[:allowed_logins] || [], &String.downcase/1),
      github: config[:github] || ControlPlane.Api.GitHubLogin.Http,
      session_ttl: config[:session_ttl] || @default_ttl,
      poll_ms: config[:poll_ms] || 1_000,
      # A cookie is secure unless the API is served over plain http, which is only a laptop.
      secure: not String.starts_with?(base_url, "http://")
    }
  end

  @doc "Whether a GitHub login may sign in. An empty allowlist lets no one in."
  @spec allowed?(t(), String.t()) :: boolean()
  def allowed?(%{allowed_logins: allowed}, login) when is_binary(login),
    do: String.downcase(login) in allowed

  def allowed?(_, _), do: false
end
