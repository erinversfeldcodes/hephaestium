defmodule ControlPlane.ReviewApproval do
  @moduledoc """
  The approving review on GitHub: submitted by the review App, for one commit, when the
  reviews of that commit say every lens that blocks approved it (`ControlPlane.Reviews`
  decides that, and this is asked only then).

  The code host is asked first whether the App has already approved this commit, so a pull
  request is approved once for a head commit however often it is settled; an approval of
  another commit is not this one's. Nothing here is ever a request for changes: a finding
  is a comment and a revision, and a new push dismisses an approval, so no approval stands
  for a commit no lens read.
  """

  alias ControlPlane.{Account, CodeHost}

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
    body =
      Account.clean(
        "Hephaestium: every lens that blocks approved this commit.\n\n" <> Enum.join(lines, "\n")
      )

    case CodeHost.approve(repository, number, sha, body) do
      :unsupported -> {:error, "the code host cannot approve a change"}
      result -> result
    end
  end
end
