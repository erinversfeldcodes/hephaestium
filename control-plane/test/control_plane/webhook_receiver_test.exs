defmodule ControlPlane.WebhookReceiverTest do
  @moduledoc """
  The receiver: a signed route on the API's listener, answered after one insert. A delivery
  that does not verify is refused before anything is read from it, stored or enqueued, and a
  redelivery is one job. The requests are made to the router as a plug; no listener is
  started and no code host is reached.
  """

  use ControlPlane.DataCase, async: false
  use Oban.Testing, repo: ControlPlane.Repo

  import Plug.Test
  import Plug.Conn

  alias ControlPlane.Api.{Router, Settings}
  alias ControlPlane.Jobs.HandleEvent
  alias ControlPlane.Webhooks.Delivery
  alias ControlPlane.WebhookFixtures, as: W

  @secret_key String.duplicate("k", 64)

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-webhook-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    secret_file = Path.join(dir, "secret")
    File.write!(secret_file, W.secret() <> "\n")

    saved = for key <- [:webhooks, :intake], do: {key, Application.fetch_env(:control_plane, key)}
    Application.put_env(:control_plane, :webhooks, secret_file: secret_file)
    Application.put_env(:control_plane, :intake, repositories: [W.repository()])

    on_exit(fn ->
      for {key, saved} <- saved do
        case saved do
          {:ok, value} -> Application.put_env(:control_plane, key, value)
          :error -> Application.delete_env(:control_plane, key)
        end
      end

      File.rm_rf(dir)
    end)

    %{secret_file: secret_file}
  end

  defp settings do
    Settings.build!(
      secret_key_base: @secret_key,
      client_id: "id",
      client_secret: "secret",
      base_url: "https://heph.test",
      allowed_logins: ["octocat"]
    )
  end

  defp post(headers_and_body, path \\ "/webhooks/github") do
    {headers, body} = headers_and_body

    Enum.reduce(headers, conn(:post, path, body), fn {name, value}, conn ->
      put_req_header(conn, name, value)
    end)
    |> Router.call(Router.init(settings: settings()))
  end

  defp deliveries, do: Repo.aggregate(Delivery, :count)
  defp jobs, do: all_enqueued(worker: HandleEvent)

  describe "a delivery that does not verify" do
    test "with no signature is 401, and nothing is stored or enqueued" do
      conn = post(W.delivery("issues", W.labeled(), signature: :none))

      assert conn.status == 401
      assert deliveries() == 0
      assert jobs() == []
    end

    test "with a wrong, malformed or other body's signature is 401, and nothing is stored or enqueued" do
      {_, other_body} = W.delivery("issues", W.issues("closed"))

      for signature <- ["sha256=" <> String.duplicate("0", 64), "garbage", "", W.sign(other_body)] do
        conn = post(W.delivery("issues", W.labeled(), signature: signature))
        assert conn.status == 401, "#{inspect(signature)} gave #{conn.status}"
      end

      assert post(W.delivery("issues", W.labeled(), secret: "another-secret")).status == 401
      assert deliveries() == 0
      assert jobs() == []
    end

    test "is refused before its body is looked at, whatever the body is" do
      conn = post(W.delivery("issues", nil, raw: "not json at all", signature: :none))

      assert conn.status == 401
      assert deliveries() == 0
    end

    test "is refused for an event that would be ignored, so a stranger learns nothing" do
      assert post(W.delivery("star", %{}, signature: :none)).status == 401
    end
  end

  describe "a delivery that verifies" do
    test "is accepted at once, with one stored delivery id and one job" do
      conn = post(W.delivery("issues", W.labeled(), id: "d-1"))

      assert conn.status == 202
      assert deliveries() == 1
      assert [job] = jobs()
      assert job.args["kind"] == "ticket_approved"
      assert job.args["delivery_id"] == "d-1"
      assert job.args["repository"] == W.repository()
      assert job.args["number"] == 12
    end

    test "keeps the normalised event and never the code host's payload" do
      post(W.delivery("issues", W.labeled()))

      assert [job] = jobs()
      refute inspect(job.args) =~ W.text()
    end

    test "is checked against the exact bytes it was signed over" do
      payload = W.labeled()
      # The same JSON, written with spaces a re-encoding would not keep.
      raw = Jason.encode!(payload, pretty: true)
      refute raw == Jason.encode!(payload)

      assert post(W.delivery("issues", payload, raw: raw)).status == 202

      {headers, _} = W.delivery("issues", payload)
      assert post({headers, raw}).status == 401
    end

    test "a redelivery of the same delivery id is answered success and is one job" do
      first = W.delivery("issues", W.labeled(), id: "same")
      assert post(first).status == 202
      assert post(first).status == 200
      assert post(first).status == 200

      assert deliveries() == 1
      assert length(jobs()) == 1
    end

    test "two deliveries with different ids are two jobs" do
      post(W.delivery("issues", W.labeled(), id: "a"))
      post(W.delivery("issues", W.labeled(), id: "b"))

      assert length(jobs()) == 2
    end

    test "a delivery whose job could not be inserted leaves no delivery id behind" do
      {headers, body} = W.delivery("issues", W.labeled(), id: "x")
      # One insert in one transaction: a job that cannot be made rolls the delivery back.
      Application.put_env(:control_plane, :webhooks_fail_job, true)
      on_exit(fn -> Application.delete_env(:control_plane, :webhooks_fail_job) end)

      assert post({headers, body}).status == 500
      assert deliveries() == 0
    end

    test "for a repository that is not configured is acknowledged and dropped" do
      payload = put_in(W.labeled(), ["repository", "full_name"], "someone/else")
      conn = post(W.delivery("issues", payload))

      assert conn.status == 202
      assert deliveries() == 0
      assert jobs() == []
    end

    test "of a kind that is not listed is acknowledged and dropped" do
      for event <- ["star", "ping", "installation"] do
        assert post(W.delivery(event, W.labeled())).status == 202
      end

      assert post(W.delivery("issues", W.labeled("bug"))).status == 202
      assert deliveries() == 0
      assert jobs() == []
    end

    test "with no delivery id, or a body that is not an event, is 400" do
      {headers, body} = W.delivery("issues", W.labeled())
      without_id = {List.keydelete(headers, "x-github-delivery", 0), body}
      assert post(without_id).status == 400

      assert post(W.delivery("issues", nil, raw: "not json")).status == 400
      assert deliveries() == 0
      assert jobs() == []
    end
  end

  test "a body over the limit is 413, and is not stored or enqueued" do
    Application.put_env(:control_plane, :webhooks,
      secret_file: Application.get_env(:control_plane, :webhooks)[:secret_file],
      max_body_bytes: 200
    )

    big = Map.put(W.labeled(), "padding", String.duplicate("x", 500))
    assert post(W.delivery("issues", big)).status == 413
    assert deliveries() == 0
    assert jobs() == []
  end

  test "a secret that cannot be read is 503 and nothing is accepted", %{secret_file: file} do
    File.rm!(file)
    assert post(W.delivery("issues", W.labeled())).status == 503

    File.write!(file, "\n")
    assert post(W.delivery("issues", W.labeled())).status == 503
    assert deliveries() == 0
  end

  test "the secret is read from its file each time, so rotating it needs no restart", %{
    secret_file: file
  } do
    File.write!(file, "the-rotated-secret\n")

    assert post(W.delivery("issues", W.labeled())).status == 401
    assert post(W.delivery("issues", W.labeled(), secret: "the-rotated-secret")).status == 202
  end

  describe "when webhooks are off" do
    setup do
      Application.delete_env(:control_plane, :webhooks)
      :ok
    end

    test "there is no route: the API answers as it did without one" do
      conn = post(W.delivery("issues", W.labeled()))

      assert conn.status == 405
      assert deliveries() == 0
      refute ControlPlane.Webhooks.enabled?()
    end
  end

  test "an unknown source is not found" do
    assert post(W.delivery("issues", W.labeled()), "/webhooks/elsewhere").status == 404
  end

  test "a GET of the route is not an answer" do
    conn = conn(:get, "/webhooks/github") |> Router.call(Router.init(settings: settings()))
    assert conn.status in [401, 404, 405]
    assert deliveries() == 0
  end

  test "the route has no session, no cookie and no bearer token to give" do
    {headers, body} = W.delivery("issues", W.labeled(), signature: :none)

    conn =
      post({[{"cookie", "heph_session=anything"}, {"authorization", "Bearer x"} | headers], body})

    assert conn.status == 401
  end

  test "the route is in the table of routes, as a webhook and nothing else" do
    assert %{access: :webhook, method: "POST"} =
             Enum.find(Router.routes(), &(&1.path == ~w(webhooks :source)))
  end
end
