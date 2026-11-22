defmodule ControlPlane.Api.Auth do
  @moduledoc """
  The login with GitHub's user authorization (the web flow), the only routes that need no
  session. The person is sent to GitHub with a state that is kept in a cookie; GitHub sends
  them back with a code and the same state; the state is checked and spent, the code is
  exchanged for a user token, the token is used to read the login and dropped, and the login
  must be on the allowlist for a session to be made. A login that is not on it is refused and
  nothing is made.
  """

  import Plug.Conn

  alias ControlPlane.Api.{Session, Settings}

  @authorize "https://github.com/login/oauth/authorize"

  @doc "Send the person to GitHub to authorize."
  def login(conn, _params, settings, _login) do
    {conn, state} = Session.begin_login(conn, settings)

    query =
      URI.encode_query(%{
        client_id: settings.client_id,
        redirect_uri: redirect_uri(settings),
        state: state,
        allow_signup: "false"
      })

    redirect(conn, @authorize <> "?" <> query)
  end

  @doc "Where GitHub sends the person back to."
  def callback(conn, _params, settings, _login) do
    conn = fetch_query_params(conn)

    %{"code" => code, "state" => state} =
      Map.merge(%{"code" => nil, "state" => nil}, conn.query_params)

    state_ok = Session.check_state(conn, settings, state)
    conn = Session.drop_state(conn, settings)

    with :ok <- state_ok,
         true <- is_binary(code) and code != "" do
      sign_in(conn, code, settings)
    else
      _ -> refuse(conn, 400, "the sign-in was not started here, or was already used")
    end
  end

  defp sign_in(conn, code, settings) do
    github = settings.github

    with {:ok, token} <- github.exchange_code(code, redirect_uri(settings), settings),
         {:ok, login} <- github.login(token) do
      if Settings.allowed?(settings, login) do
        conn |> Session.start(login, settings) |> redirect("/")
      else
        refuse(conn, 403, "this login is not allowed to sign in")
      end
    else
      {:error, _reason} -> refuse(conn, 401, "GitHub did not sign this person in")
    end
  end

  @doc "End the session."
  def logout(conn, _params, settings, _login) do
    conn |> Session.stop(settings) |> redirect("/")
  end

  defp redirect(conn, location) do
    conn |> put_resp_header("location", location) |> send_resp(302, "")
  end

  defp refuse(conn, status, message) do
    conn
    |> put_resp_content_type("application/json")
    |> send_resp(status, Jason.encode!(%{error: message}))
  end

  defp redirect_uri(settings), do: settings.base_url <> "/auth/callback"
end
