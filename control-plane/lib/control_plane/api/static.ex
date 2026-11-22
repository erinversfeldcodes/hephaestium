defmodule ControlPlane.Api.Static do
  @moduledoc """
  Serves the dashboard's built files from one directory, on the API's own listener so the
  page and the API share an origin and need no cross-origin permission.

  A request is answered with the file its path names, if there is one. A path with no file
  extension that names no file is a page of the single-page app, and gets `index.html` so a
  reload or a bookmark works; a path with an extension that names no file is a 404, so a
  missing script is not mistaken for a page. A path that could leave the directory (a `..`
  segment, a separator or a NUL byte, in any encoding the router may hand over) is a 404 and
  the file system is not touched for it.

  Only the router calls this, and only for a path that is not the API's: it never decides what
  an API path means.
  """

  import Plug.Conn

  @csp "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'"

  @doc "The setting's directory, checked: a directory that holds `index.html`, or it raises."
  @spec check!(Path.t()) :: Path.t()
  def check!(dir) do
    if File.regular?(Path.join(dir, "index.html")) do
      Path.expand(dir)
    else
      raise ArgumentError,
            "HEPH_API_STATIC_DIR is not a directory that holds an index.html: build the dashboard first, or unset it"
    end
  end

  @doc "Answer the request from the directory."
  @spec serve(Plug.Conn.t(), Path.t()) :: Plug.Conn.t()
  def serve(conn, dir) do
    case resolve(conn.path_info, dir) do
      {:file, path} -> send_file_with(conn, path)
      :page -> send_file_with(conn, Path.join(dir, "index.html"))
      :not_found -> send_resp(conn, 404, "not found")
    end
  end

  defp resolve([], _dir), do: :page

  defp resolve(segments, dir) do
    with true <- Enum.all?(segments, &safe_segment?/1),
         {:ok, relative} <- Path.safe_relative(Path.join(segments)),
         path = Path.join(dir, relative) do
      cond do
        File.regular?(path) -> {:file, path}
        Path.extname(List.last(segments)) == "" -> :page
        true -> :not_found
      end
    else
      _ -> :not_found
    end
  end

  # A segment is one name. The router may or may not have decoded it, so it is held to the
  # rule in both forms.
  defp safe_segment?(segment) do
    decoded = URI.decode(segment)

    Enum.all?([segment, decoded], fn s ->
      s not in ["", ".", ".."] and not String.contains?(s, ["/", "\\", <<0>>])
    end)
  rescue
    ArgumentError -> false
  end

  defp send_file_with(conn, path) do
    conn
    |> put_resp_header("content-type", content_type(path))
    |> put_resp_header("x-content-type-options", "nosniff")
    |> cache(path)
    |> page_headers(path)
    |> send_file(200, path)
  end

  defp content_type(path) do
    type = MIME.from_path(path)

    if String.starts_with?(type, "text/") or String.contains?(type, ["javascript", "json"]),
      do: type <> "; charset=utf-8",
      else: type
  end

  # Built assets carry a hash of their content in the name, so they may be kept for good; the
  # page names them, so it must be asked for again each time.
  defp cache(conn, path) do
    if Path.basename(Path.dirname(path)) == "assets" do
      put_resp_header(conn, "cache-control", "public, max-age=31536000, immutable")
    else
      put_resp_header(conn, "cache-control", "no-cache")
    end
  end

  defp page_headers(conn, path) do
    if Path.extname(path) == ".html",
      do: put_resp_header(conn, "content-security-policy", @csp),
      else: conn
  end
end
