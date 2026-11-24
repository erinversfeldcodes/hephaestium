defmodule ControlPlane.WebhookFixtures do
  @moduledoc """
  Webhook deliveries as GitHub's documentation shows them, reduced to the fields the receiver
  reads, with a line of repository text in each body that must never reach a job. These are
  synthetic: they prove the receiver against the documented shape, not against a delivery
  GitHub really sent.
  """

  @repository "erinversfeldcodes/gazebolang"
  @secret "a-webhook-secret-for-tests-only"
  # Text a repository holds, which is not the control plane's to keep.
  @text "REPOSITORY-TEXT-THAT-MUST-NOT-BE-KEPT"

  def repository, do: @repository
  def secret, do: @secret
  def text, do: @text

  @doc "The signature header's value for a body, as GitHub makes it."
  def sign(body, secret \\ @secret),
    do: "sha256=" <> (:crypto.mac(:hmac, :sha256, secret, body) |> Base.encode16(case: :lower))

  defp base(extra) do
    Map.merge(
      %{
        "repository" => %{"full_name" => @repository, "default_branch" => "main"},
        "sender" => %{"login" => "maint"}
      },
      extra
    )
  end

  def issues(action, number \\ 12, extra \\ %{}) do
    base(
      Map.merge(
        %{
          "action" => action,
          "issue" => %{"number" => number, "state" => "open", "body" => @text}
        },
        extra
      )
    )
  end

  def labeled(label \\ "heph:ready"),
    do: issues("labeled", 12, %{"label" => %{"name" => label}})

  def unlabeled(label \\ "heph:ready"),
    do: issues("unlabeled", 12, %{"label" => %{"name" => label}})

  def issue_comment(on \\ :ticket) do
    issue = %{"number" => 12, "body" => @text}
    issue = if on == :change, do: Map.put(issue, "pull_request", %{"url" => "x"}), else: issue

    base(%{"action" => "created", "issue" => issue, "comment" => %{"body" => @text}})
  end

  def pull_request(action, merged \\ false) do
    base(%{
      "action" => action,
      "number" => 7,
      "pull_request" => %{
        "number" => 7,
        "merged" => merged,
        "body" => @text,
        "head" => %{"sha" => "abc123", "ref" => "heph/12-x"}
      }
    })
  end

  def review(action) do
    base(%{
      "action" => action,
      "review" => %{"body" => @text},
      "pull_request" => %{"number" => 7, "head" => %{"sha" => "abc123"}}
    })
  end

  def check_run(action \\ "completed", pulls \\ [%{"number" => 7}]) do
    base(%{
      "action" => action,
      "check_run" => %{"head_sha" => "abc123", "name" => @text, "pull_requests" => pulls}
    })
  end

  def check_suite(action \\ "completed", pulls \\ [%{"number" => 7}]) do
    base(%{
      "action" => action,
      "check_suite" => %{"head_sha" => "abc123", "pull_requests" => pulls}
    })
  end

  # A commit status names a commit and no change, and has no action.
  def status(state \\ "success"),
    do: base(%{"sha" => "abc123", "state" => state, "description" => @text})

  def push(ref \\ "refs/heads/main"),
    do: base(%{"ref" => ref, "after" => "def456", "head_commit" => %{"message" => @text}})

  @doc "A delivery: `{headers, body}`, signed unless `signature:` says otherwise."
  def delivery(event, payload, opts \\ []) do
    body = Keyword.get(opts, :raw) || Jason.encode!(payload)

    id =
      Keyword.get(opts, :id, "delivery-" <> Integer.to_string(System.unique_integer([:positive])))

    headers =
      [{"x-github-event", event}, {"x-github-delivery", id}, {"content-type", "application/json"}] ++
        case Keyword.get(opts, :signature, :valid) do
          :valid -> [{"x-hub-signature-256", sign(body, Keyword.get(opts, :secret, @secret))}]
          :none -> []
          other -> [{"x-hub-signature-256", other}]
        end

    {Enum.reject(headers, fn {_, v} -> is_nil(v) end), body}
  end
end
