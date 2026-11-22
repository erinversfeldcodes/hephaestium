defmodule ControlPlane.Api.Reply do
  @moduledoc """
  A person's answer to an agent's question: the dashboard's one write to a repository. It
  needs the session and a CSRF token like every write a person makes, and it is made with the
  user token that person's login gave, so the comment is theirs and the approval that follows is
  theirs, read by intake as any approval is. The control plane's own identity never stands in
  for the person's, and a person who cannot write to the repository changes nothing.

  What is answered says what was done, in two facts (`commented`, `approved`), and an error
  that is fixed wording and never what GitHub said or the token.
  """

  import Plug.Conn

  alias ControlPlane.Api.Session
  alias ControlPlane.Replies

  @max_body 16_384

  def create(conn, %{"owner" => owner, "name" => name, "number" => number}, settings, login) do
    with {:ok, text} <- reply_text(conn),
         {:ok, number} <- ticket_number(number),
         {:ok, token} <- user_token(conn, settings, login) do
      conn = conn |> put_resp_header("cache-control", "no-store")
      answer(conn, Replies.answer("#{owner}/#{name}", number, login, token, text))
    else
      {:error, status, message} -> json(conn, status, %{error: message})
    end
  end

  defp reply_text(conn) do
    with {:ok, body, _conn} <- read_body(conn, length: @max_body),
         {:ok, %{"body" => text}} when is_binary(text) <- Jason.decode(body),
         text = String.trim(text),
         true <- text != "" and String.length(text) <= Replies.max_length() do
      {:ok, text}
    else
      _ -> {:error, 400, "a reply of one to #{Replies.max_length()} characters is needed"}
    end
  end

  defp ticket_number(raw) do
    case Integer.parse(raw) do
      {number, ""} when number > 0 -> {:ok, number}
      _ -> {:error, 404, "not found"}
    end
  end

  defp user_token(conn, settings, login) do
    case Session.user_token(conn, settings, login) do
      {:ok, token} -> {:ok, token}
      :error -> {:error, 401, "sign in again to reply: this session cannot act as you"}
    end
  end

  defp answer(conn, :ok), do: json(conn, 200, %{commented: true, approved: true})

  defp answer(conn, {:error, :not_waiting}),
    do: json(conn, 409, %{error: "no question is waiting on this ticket"})

  defp answer(conn, {:error, :cannot_write}) do
    json(conn, 403, %{
      error: "you need write access to this repository to answer; nothing was sent",
      commented: false,
      approved: false
    })
  end

  defp answer(conn, {:error, :unsupported}) do
    json(conn, 501, %{
      error: "this repository's tracker cannot take a reply as you; nothing was sent",
      commented: false,
      approved: false
    })
  end

  defp answer(conn, {:error, :unavailable}),
    do: json(conn, 502, %{error: "the repository could not be asked; nothing was sent"})

  defp answer(conn, {:error, {step, reason}}) do
    json(conn, status(reason), %{
      error: message(step, reason),
      commented: step == :approval,
      approved: false
    })
  end

  defp status(:unauthorized), do: 401
  defp status(:refused), do: 403
  defp status(_), do: 502

  defp message(_, :unauthorized), do: "GitHub no longer accepts your sign-in: sign in again"
  defp message(:comment, :refused), do: "GitHub refused the comment"

  defp message(:approval, :refused),
    do: "GitHub refused the approval: your comment was made but the ticket is not approved"

  defp message(:comment, _), do: "GitHub could not take the comment; nothing was sent"

  defp message(:approval, _),
    do: "GitHub could not take the approval: your comment was made but the ticket is not approved"

  defp json(conn, status, body) do
    conn |> put_resp_content_type("application/json") |> send_resp(status, Jason.encode!(body))
  end
end
