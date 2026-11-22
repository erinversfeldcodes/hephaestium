defmodule ControlPlane.ApiStaticTest do
  @moduledoc """
  The API serves the dashboard's built files from a directory it is told about, on the same
  listener and origin as the API, and changes nothing about itself when it is told of none.
  """

  use ExUnit.Case, async: true

  import Plug.Test
  import Plug.Conn

  alias ControlPlane.Api.{Router, Settings}

  @secret String.duplicate("k", 64)

  @moduletag :tmp_dir

  defp settings(overrides) do
    [
      secret_key_base: @secret,
      client_id: "the-client-id",
      client_secret: "the-client-secret",
      base_url: "https://heph.test",
      allowed_logins: ["octocat"]
    ]
    |> Keyword.merge(overrides)
    |> Settings.build!()
  end

  # A built dashboard in `root/site`, and a file beside it that must never be served.
  defp build_site(root) do
    site = Path.join(root, "site")
    File.mkdir_p!(Path.join(site, "assets"))
    File.mkdir_p!(Path.join(site, "api"))
    File.write!(Path.join(site, "index.html"), "<!doctype html><title>the dashboard</title>")
    File.write!(Path.join(site, "assets/app-1a2b3c.js"), "console.log('app')")
    File.write!(Path.join(site, "assets/style-1a2b3c.css"), "body{}")
    File.write!(Path.join(site, "api/session"), "a file that must not shadow the API")
    File.write!(Path.join(root, "secret.txt"), "outside the served directory")
    site
  end

  defp get(path, settings, method \\ :get) do
    conn(method, path) |> Router.call(Router.init(settings: settings))
  end

  defp with_segments(settings, segments) do
    conn(:get, "/")
    |> Map.put(:path_info, segments)
    |> Router.call(Router.init(settings: settings))
  end

  describe "with a directory set" do
    setup %{tmp_dir: root}, do: %{settings: settings(static_dir: build_site(root))}

    test "the root is the page, with no session", %{settings: settings} do
      conn = get("/", settings)

      assert conn.status == 200
      assert conn.resp_body =~ "the dashboard"
      assert get_resp_header(conn, "content-type") |> hd() =~ "text/html"
      assert get_resp_header(conn, "cache-control") == ["no-cache"]
      assert get_resp_header(conn, "x-content-type-options") == ["nosniff"]
      assert get_resp_header(conn, "content-security-policy") |> hd() =~ "default-src 'self'"
    end

    test "a file is served with its type, and a built asset may be kept for good", %{
      settings: settings
    } do
      js = get("/assets/app-1a2b3c.js", settings)
      assert js.status == 200
      assert js.resp_body == "console.log('app')"
      assert get_resp_header(js, "content-type") |> hd() =~ "javascript"
      assert get_resp_header(js, "cache-control") |> hd() =~ "immutable"

      css = get("/assets/style-1a2b3c.css", settings)
      assert get_resp_header(css, "content-type") |> hd() =~ "text/css"
    end

    test "a page's address the server has no file for gets the page, so a reload works", %{
      settings: settings
    } do
      for path <- ["/tickets", "/tickets/erinversfeldcodes/gazebolang/12", "/spend"] do
        conn = get(path, settings)
        assert conn.status == 200, path
        assert conn.resp_body =~ "the dashboard", path
      end
    end

    test "a file that is not there is 404, not the page", %{settings: settings} do
      conn = get("/assets/missing-9z.js", settings)

      assert conn.status == 404
      refute conn.resp_body =~ "the dashboard"
    end

    test "HEAD is answered and has no body", %{settings: settings} do
      conn = get("/", settings, :head)

      assert conn.status == 200
      assert conn.resp_body == ""
    end

    test "only GET and HEAD are answered, as everywhere else", %{settings: settings} do
      for method <- [:post, :put, :patch, :delete] do
        assert get("/", settings, method).status == 405
        assert get("/assets/app-1a2b3c.js", settings, method).status == 405
      end
    end

    test "a path that climbs out of the directory is never served", %{
      settings: settings,
      tmp_dir: root
    } do
      secret = File.read!(Path.join(root, "secret.txt"))

      for segments <- [
            ["..", "secret.txt"],
            ["assets", "..", "..", "secret.txt"],
            ["%2e%2e", "secret.txt"],
            ["..%2fsecret.txt"],
            ["assets", "..%2f..%2fsecret.txt"],
            ["..\\secret.txt"],
            [".", ".", "secret.txt"],
            ["assets", "app-1a2b3c.js\0"],
            ["/etc/passwd"]
          ] do
        conn = with_segments(settings, segments)
        refute conn.resp_body =~ secret, inspect(segments)
        assert conn.status in [400, 404], inspect(segments)
      end

      conn = get("/..%2fsecret.txt", settings)
      refute conn.resp_body =~ secret
    end

    test "the API keeps its paths: a file cannot stand in for a route, and a missing one is not the page",
         %{settings: settings} do
      session = get("/api/session", settings)
      assert session.status == 401
      assert Jason.decode!(session.resp_body) == %{"error" => "not signed in"}

      assert get("/api/nothing-here", settings).status == 401
      assert get("/auth/nothing-here", settings).status == 401
      assert get("/worker/claim", settings).status == 401
      assert get("/api", settings).status == 401
      refute get("/api/nothing-here", settings).resp_body =~ "the dashboard"
    end

    test "the login routes still log in", %{settings: settings} do
      conn = get("/auth/login", settings)

      assert conn.status == 302
      assert get_resp_header(conn, "location") |> hd() =~ "github.com/login/oauth/authorize"
    end
  end

  describe "with no directory set" do
    test "the API is as it was: the root is 401 and no file is looked for" do
      for path <- ["/", "/tickets", "/index.html", "/assets/app-1a2b3c.js"] do
        conn = get(path, settings([]))

        assert conn.status == 401, path
        assert Jason.decode!(conn.resp_body) == %{"error" => "not signed in"}
      end
    end

    test "an empty setting is no setting" do
      assert get("/", settings(static_dir: "")).status == 401
      assert get("/", settings(static_dir: nil)).status == 401
    end
  end

  describe "the setting" do
    test "a directory with no page in it is refused when the API is built, naming the setting",
         %{tmp_dir: root} do
      empty = Path.join(root, "empty")
      File.mkdir_p!(empty)

      error = assert_raise ArgumentError, fn -> settings(static_dir: empty) end
      assert error.message =~ "HEPH_API_STATIC_DIR"

      assert_raise ArgumentError, ~r/HEPH_API_STATIC_DIR/, fn ->
        settings(static_dir: Path.join(root, "missing"))
      end
    end
  end
end
