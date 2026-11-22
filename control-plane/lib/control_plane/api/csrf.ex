defmodule ControlPlane.Api.Csrf do
  @moduledoc """
  Protection of the routes a browser session may write with. A cookie is sent by the browser on
  any request to this address, whoever's page made it, so a write that the cookie alone allowed
  could be made by another site. A write must also carry a token in a header, which a page on
  another site can neither read (the answer that holds it is not readable cross-site) nor send
  (a custom header makes a cross-site request ask permission first, which this API never
  gives). The token is signed, tied to the login of the session it was given to, and expires
  with it. When the request says where it came from, that must be this API's own address.
  """

  import Plug.Conn

  alias ControlPlane.Api.Settings

  @salt "heph api csrf"
  @header "x-csrf-token"

  @doc "The token for a login."
  @spec token(Settings.t(), String.t()) :: String.t()
  def token(settings, login),
    do: Plug.Crypto.sign(settings.secret_key_base, @salt, %{login: login})

  @doc "Whether a request that is signed in as `login` may write."
  @spec valid?(Plug.Conn.t(), Settings.t(), String.t()) :: boolean()
  def valid?(conn, settings, login) do
    with [presented] <- get_req_header(conn, @header),
         {:ok, %{login: ^login}} <-
           Plug.Crypto.verify(settings.secret_key_base, @salt, presented,
             max_age: settings.session_ttl
           ) do
      same_origin?(conn, settings)
    else
      _ -> false
    end
  end

  # A request that names its origin must name this one. A request that names none (a program,
  # not a browser) has nothing to be forged from.
  defp same_origin?(conn, settings) do
    case get_req_header(conn, "origin") do
      [] -> true
      [origin] -> origin == origin_of(settings.base_url)
      _ -> false
    end
  end

  defp origin_of(url) do
    uri = URI.parse(url)
    default = if uri.scheme == "https", do: 443, else: 80
    port = if uri.port in [nil, default], do: "", else: ":#{uri.port}"
    "#{uri.scheme}://#{uri.host}#{port}"
  end
end
