defmodule ControlPlane.ReviewApproval do
  @moduledoc """
  The approving review on GitHub: submitted by the review App, for one commit, when the
  reviews of that commit say every lens that blocks approved it (`ControlPlane.Reviews`
  decides that, and this is asked only then).

  GitHub is asked first whether the App has already approved this commit, so a pull
  request is approved once for a head commit however often it is settled; an approval of
  another commit is not this one's. Nothing here is ever a request for changes: a finding
  is a comment and a revision, and a new push dismisses an approval, so no approval stands
  for a commit no lens read.
  """

  alias ControlPlane.{Account, GitHub, ReviewApp}

  @doc """
  Make sure the pull request is approved at `sha`:

    * `:disabled`: no review App is configured, and nothing was asked of GitHub;
    * `:approved`: the App had already approved this commit;
    * `:submitted`: the approval was made just now;
    * `{:error, why}`: there is no approval, and it may be tried again.
  """
  @spec ensure(String.t(), String.t(), String.t(), [String.t()]) ::
          :disabled | :approved | :submitted | {:error, String.t()}
  def ensure(repository, number, sha, lines) do
    case ReviewApp.token(repository) do
      :disabled ->
        :disabled

      {:error, message} ->
        {:error, message}

      {:ok, %{token: token, login: login}} ->
        with {:ok, reviews} <- GitHub.get_all("repos/#{repository}/pulls/#{number}/reviews") do
          if approved?(reviews, login, sha),
            do: :approved,
            else: submit(repository, number, sha, lines, token)
        end
    end
  end

  defp approved?(reviews, login, sha) do
    Enum.any?(reviews, fn review ->
      get_in(review, ["user", "login"]) == login and review["state"] == "APPROVED" and
        review["commit_id"] == sha
    end)
  end

  defp submit(repository, number, sha, lines, token) do
    body =
      Account.clean(
        "Hephaestium: every lens that blocks approved this commit.\n\n" <> Enum.join(lines, "\n")
      )

    case GitHub.write(
           "POST",
           "repos/#{repository}/pulls/#{number}/reviews",
           ["event=APPROVE", "commit_id=#{sha}", "body=#{body}"],
           token: token
         ) do
      {:ok, _} -> :submitted
      {:error, message} -> {:error, message}
    end
  end
end
