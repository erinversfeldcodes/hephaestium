defmodule ControlPlane.Api.Session do
  @moduledoc """
  A session is a cookie the control plane signed: the login and when it was signed, nothing
  else, with no store behind it. It is secure, http-only and same-site, it expires, and a
  login that has left the allowlist stops reading with it.

  The login flow's own state travels the same way, in a cookie of its own that lives ten
  minutes.
  """

  import Plug.Conn

  alias ControlPlane.Api.Settings

  @session "heph_session"
  @state "heph_oauth_state"
  @state_ttl 600

  @session_salt "heph api session"
  @state_salt "heph api login state"

  @doc "A signed session token for a login. `signed_at` (unix seconds) is for tests."
  @spec token(String.t(), Settings.t(), keyword()) :: String.t()
  def token(login, settings, opts \\ []) do
    sign(settings, @session_salt, %{login: login}, opts)
  end

  @doc "Start a session for a login: the response carries its cookie."
  @spec start(Plug.Conn.t(), String.t(), Settings.t()) :: Plug.Conn.t()
  def start(conn, login, settings) do
    put_resp_cookie(
      conn,
      @session,
      token(login, settings),
      cookie_options(settings, settings.session_ttl, "/")
    )
  end

  @doc "End the session: the browser is told to drop the cookie."
  @spec stop(Plug.Conn.t(), Settings.t()) :: Plug.Conn.t()
  def stop(conn, settings) do
    put_resp_cookie(conn, @session, "", cookie_options(settings, 0, "/"))
  end

  @doc "The login of a request's session, if it has a good one for a login still allowed."
  @spec login(Plug.Conn.t(), Settings.t()) :: {:ok, String.t()} | :error
  def login(conn, settings) do
    conn = fetch_cookies(conn)

    with token when is_binary(token) <- conn.req_cookies[@session],
         {:ok, %{login: login}} <- verify(settings, @session_salt, token, settings.session_ttl),
         true <- Settings.allowed?(settings, login) do
      {:ok, login}
    else
      _ -> :error
    end
  end

  @doc "Begin a login: a new state, set in a short-lived cookie. Returns the conn and the state."
  @spec begin_login(Plug.Conn.t(), Settings.t()) :: {Plug.Conn.t(), String.t()}
  def begin_login(conn, settings) do
    nonce = Base.url_encode64(:crypto.strong_rand_bytes(32), padding: false)

    conn =
      put_resp_cookie(
        conn,
        @state,
        sign(settings, @state_salt, %{nonce: nonce}, []),
        cookie_options(settings, @state_ttl, "/auth")
      )

    {conn, nonce}
  end

  @doc """
  The state a callback arrives with is good when it is the one in the cookie, signed by us,
  not yet expired, and not spent. Spending it is what makes a replay fail.
  """
  @spec check_state(Plug.Conn.t(), Settings.t(), term()) :: :ok | :error
  def check_state(conn, settings, state) do
    conn = fetch_cookies(conn)

    with true <- is_binary(state) and state != "",
         token when is_binary(token) <- conn.req_cookies[@state],
         {:ok, %{nonce: nonce}} <- verify(settings, @state_salt, token, @state_ttl),
         true <- Plug.Crypto.secure_compare(nonce, state),
         :ok <-
           ControlPlane.Api.UsedStates.consume(nonce, System.system_time(:second) + @state_ttl) do
      :ok
    else
      _ -> :error
    end
  end

  @doc "The state cookie is dropped whatever the callback came to."
  @spec drop_state(Plug.Conn.t(), Settings.t()) :: Plug.Conn.t()
  def drop_state(conn, settings),
    do: put_resp_cookie(conn, @state, "", cookie_options(settings, 0, "/auth"))

  defp cookie_options(settings, max_age, path) do
    [
      http_only: true,
      secure: settings.secure,
      same_site: "Lax",
      max_age: max_age,
      path: path
    ]
  end

  defp sign(settings, salt, data, opts) do
    sign_opts = if opts[:signed_at], do: [signed_at: opts[:signed_at]], else: []
    Plug.Crypto.sign(settings.secret_key_base, salt, data, sign_opts)
  end

  defp verify(settings, salt, token, max_age) do
    Plug.Crypto.verify(settings.secret_key_base, salt, token, max_age: max_age)
  end
end
