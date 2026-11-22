defmodule ControlPlane.ReviewAppTest do
  @moduledoc """
  The review App's side of an approval: the signed assertion, and the installation token
  it is exchanged for. The HTTP exchange is proven against a local server that answers
  in the shapes GitHub gives (read from the real one); it does not prove that GitHub
  accepts this particular key.
  """

  use ExUnit.Case, async: false

  alias ControlPlane.ReviewApp

  @repo "erinversfeldcodes/gazebolang"

  setup do
    key = :public_key.generate_key({:rsa, 2048, 65_537})
    dir = Path.join(System.tmp_dir!(), "cp-review-app-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    path = Path.join(dir, "app.pem")
    pem = :public_key.pem_encode([:public_key.pem_entry_encode(:RSAPrivateKey, key)])
    File.write!(path, pem)

    previous = Application.get_env(:control_plane, :review_app)
    ReviewApp.forget()

    on_exit(fn ->
      Application.put_env(:control_plane, :review_app, previous)
      ReviewApp.forget()
      File.rm_rf(dir)
    end)

    {:RSAPrivateKey, _, modulus, exponent, _, _, _, _, _, _, _} = key
    %{path: path, public: {:RSAPublicKey, modulus, exponent}, pem: pem}
  end

  defp configure(settings), do: Application.put_env(:control_plane, :review_app, settings)

  # A local server that answers the three requests a token needs, and tells the test
  # what it was asked, one request per connection.
  defp serve(answers) do
    {:ok, listener} = :gen_tcp.listen(0, [:binary, active: false, reuseaddr: true])
    {:ok, port} = :inet.port(listener)
    test = self()

    spawn_link(fn -> accept(listener, test, answers) end)
    "http://127.0.0.1:#{port}"
  end

  defp accept(listener, test, answers) do
    case :gen_tcp.accept(listener, 5_000) do
      {:ok, socket} ->
        {:ok, request} = :gen_tcp.recv(socket, 0, 5_000)
        [head | _] = String.split(request, "\r\n\r\n", parts: 2)
        [line | headers] = String.split(head, "\r\n")
        [method, path | _] = String.split(line, " ")
        authorization = Enum.find_value(headers, &header(&1, "authorization"))
        send(test, {:asked, method, path, authorization})

        {status, body} = Map.get(answers, {method, path}, {404, ~s({"message":"Not Found"})})

        :gen_tcp.send(
          socket,
          "HTTP/1.1 #{status} X\r\ncontent-type: application/json\r\ncontent-length: #{byte_size(body)}\r\nconnection: close\r\n\r\n#{body}"
        )

        :gen_tcp.close(socket)
        accept(listener, test, answers)

      {:error, _} ->
        :ok
    end
  end

  defp header(line, name) do
    case String.split(line, ": ", parts: 2) do
      [key, value] -> if String.downcase(key) == name, do: value
      _ -> nil
    end
  end

  defp answers(token \\ "ghs_a_token", expires \\ 3_600) do
    expires_at =
      DateTime.utc_now()
      |> DateTime.add(expires)
      |> DateTime.truncate(:second)
      |> DateTime.to_iso8601()

    %{
      {"GET", "/app"} => {200, ~s({"slug":"hephaestium-reviewer"})},
      {"GET", "/repos/#{@repo}/installation"} => {200, ~s({"id":184350266})},
      {"POST", "/app/installations/184350266/access_tokens"} =>
        {201, Jason.encode!(%{"token" => token, "expires_at" => expires_at})}
    }
  end

  defp asked do
    receive do
      {:asked, method, path, authorization} -> [{method, path, authorization} | asked()]
    after
      50 -> []
    end
  end

  describe "the signed assertion" do
    test "names the App, is short-lived, and its signature is the key's", %{
      path: path,
      public: public
    } do
      configure(app_id: "5862313", key_file: path)

      assert {:ok, jwt} = ReviewApp.assertion(1_700_000_000)
      [header, claims, signature] = String.split(jwt, ".")

      assert Jason.decode!(Base.url_decode64!(header, padding: false)) ==
               %{"alg" => "RS256", "typ" => "JWT"}

      claims = Jason.decode!(Base.url_decode64!(claims, padding: false))
      assert claims["iss"] == "5862313"
      assert claims["iat"] < 1_700_000_000
      assert claims["exp"] > 1_700_000_000
      assert claims["exp"] - claims["iat"] <= 600

      assert :public_key.verify(
               header <> "." <> hd(tl(String.split(jwt, "."))),
               :sha256,
               Base.url_decode64!(signature, padding: false),
               public
             )
    end

    test "a signature over other text is not the key's", %{path: path, public: public} do
      configure(app_id: "5862313", key_file: path)
      assert {:ok, jwt} = ReviewApp.assertion(1_700_000_000)
      [header, _claims, signature] = String.split(jwt, ".")

      refute :public_key.verify(
               header <> ".other",
               :sha256,
               Base.url_decode64!(signature, padding: false),
               public
             )
    end
  end

  describe "a key held in the environment rather than a file" do
    test "signs as the key does, and the key is the one named by the variable", %{
      pem: pem,
      public: public
    } do
      configure(app_id: "5862313", key: pem)

      assert ReviewApp.enabled?()
      assert {:ok, jwt} = ReviewApp.assertion(1_700_000_000)
      [header, claims, signature] = String.split(jwt, ".")

      assert :public_key.verify(
               header <> "." <> claims,
               :sha256,
               Base.url_decode64!(signature, padding: false),
               public
             )
    end

    test "a value that is not a key is refused without repeating it" do
      configure(app_id: "5862313", key: "not a key, secret-looking-text")

      assert {:error, message} = ReviewApp.assertion(1_700_000_000)
      refute message =~ "secret-looking-text"
    end

    test "when both are given the key in the environment is used", %{pem: pem} do
      configure(app_id: "5862313", key: pem, key_file: "/nonexistent/app.pem")

      assert {:ok, _jwt} = ReviewApp.assertion(1_700_000_000)
    end
  end

  describe "enabled?" do
    test "needs both the App and its key" do
      configure([])
      refute ReviewApp.enabled?()
      configure(app_id: "5862313")
      refute ReviewApp.enabled?()
      configure(key_file: "/tmp/x.pem")
      refute ReviewApp.enabled?()
      configure(app_id: "", key_file: "/tmp/x.pem")
      refute ReviewApp.enabled?()
      configure(app_id: "5862313", key_file: "/tmp/x.pem")
      assert ReviewApp.enabled?()
    end

    test "is not enabled when nothing is configured" do
      Application.delete_env(:control_plane, :review_app)
      refute ReviewApp.enabled?()
      assert ReviewApp.token(@repo) == :disabled
    end
  end

  describe "the installation token" do
    test "is exchanged for the repository's installation, and says whose it is", %{path: path} do
      url = serve(answers())
      configure(app_id: "5862313", key_file: path, api_url: url)

      assert {:ok, %{token: "ghs_a_token", login: "hephaestium-reviewer[bot]"}} =
               ReviewApp.token(@repo)

      requests = asked()

      assert Enum.map(requests, fn {method, path, _} -> {method, path} end) |> Enum.sort() ==
               [
                 {"GET", "/app"},
                 {"GET", "/repos/#{@repo}/installation"},
                 {"POST", "/app/installations/184350266/access_tokens"}
               ]

      for {_, _, authorization} <- requests do
        assert authorization =~ ~r/\ABearer [\w-]+\.[\w-]+\.[\w-]+\z/
      end
    end

    test "is kept until shortly before it expires, and not asked for again", %{path: path} do
      url = serve(answers())
      configure(app_id: "5862313", key_file: path, api_url: url)

      assert {:ok, first} = ReviewApp.token(@repo)
      assert length(asked()) == 3
      assert {:ok, ^first} = ReviewApp.token(@repo)
      assert asked() == []
    end

    test "one about to expire is asked for again", %{path: path} do
      url = serve(answers("ghs_short", 30))
      configure(app_id: "5862313", key_file: path, api_url: url)

      assert {:ok, %{token: "ghs_short"}} = ReviewApp.token(@repo)
      assert length(asked()) == 3
      assert {:ok, %{token: "ghs_short"}} = ReviewApp.token(@repo)
      assert length(asked()) == 3
    end

    test "a repository the App is not installed on has no token", %{path: path} do
      url =
        serve(%{
          {"GET", "/app"} => {200, ~s({"slug":"hephaestium-reviewer"})},
          {"GET", "/repos/#{@repo}/installation"} => {404, ~s({"message":"Not Found"})}
        })

      configure(app_id: "5862313", key_file: path, api_url: url)

      assert {:error, message} = ReviewApp.token(@repo)
      assert message =~ "installation"
      assert message =~ "404"
    end

    test "GitHub refusing the exchange is an error, and nothing is kept", %{path: path} do
      url =
        serve(%{
          {"GET", "/app"} => {200, ~s({"slug":"hephaestium-reviewer"})},
          {"GET", "/repos/#{@repo}/installation"} => {200, ~s({"id":184350266})},
          {"POST", "/app/installations/184350266/access_tokens"} =>
            {401, ~s({"message":"Bad credentials"})}
        })

      configure(app_id: "5862313", key_file: path, api_url: url)

      assert {:error, message} = ReviewApp.token(@repo)
      assert message =~ "401"
      assert {:error, _} = ReviewApp.token(@repo)
    end

    test "a key file that is not there is an error that names the file, not a crash" do
      configure(app_id: "5862313", key_file: "/nonexistent/review-app.pem")

      assert {:error, message} = ReviewApp.token(@repo)
      assert message =~ "/nonexistent/review-app.pem"
    end

    test "a file that is not a key is an error, and never repeats what was in it", %{path: path} do
      File.write!(path, "this is not a key but is secret-looking text")
      configure(app_id: "5862313", key_file: path)

      assert {:error, message} = ReviewApp.token(@repo)
      assert message =~ "key"
      refute message =~ "secret-looking"
    end

    test "an unreachable GitHub is an error, not a crash", %{path: path} do
      {:ok, listener} = :gen_tcp.listen(0, [:binary, active: false])
      {:ok, port} = :inet.port(listener)
      :gen_tcp.close(listener)
      configure(app_id: "5862313", key_file: path, api_url: "http://127.0.0.1:#{port}")

      assert {:error, message} = ReviewApp.token(@repo)
      assert is_binary(message)
    end

    test "the key's text and the token never appear in an error", %{path: path, pem: pem} do
      url =
        serve(%{
          {"GET", "/app"} => {200, ~s({"slug":"hephaestium-reviewer"})},
          {"GET", "/repos/#{@repo}/installation"} => {200, ~s({"id":184350266})},
          {"POST", "/app/installations/184350266/access_tokens"} => {500, ~s({"message":"boom"})}
        })

      configure(app_id: "5862313", key_file: path, api_url: url)

      assert {:error, message} = ReviewApp.token(@repo)
      refute message =~ "PRIVATE KEY"
      refute message =~ String.slice(pem, 40, 30)
    end
  end
end
