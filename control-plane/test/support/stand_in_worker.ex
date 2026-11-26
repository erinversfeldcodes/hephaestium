defmodule ControlPlane.StandInWorker do
  @moduledoc """
  A stand-in for the executor's side of the link, for tests: it speaks the protocol over a real
  listener with plain HTTP requests, as the executor will, and holds nothing but a token. It
  never reaches a real network.
  """

  @doc "A request. Returns the status and the decoded JSON body (or the text if it is not JSON)."
  def request(port, method, path, opts \\ []) do
    headers =
      for {name, value} <- bearer(opts[:token]) ++ Keyword.get(opts, :headers, []),
          do: {String.to_charlist(name), String.to_charlist(value)}

    url = String.to_charlist("http://127.0.0.1:#{port}#{path}")

    request =
      case method do
        :get -> {url, headers}
        :post -> {url, headers, ~c"application/json", body(opts[:body])}
      end

    {:ok, {{_, status, _}, _headers, answer}} =
      :httpc.request(method, request, [timeout: 15_000], body_format: :binary)

    decoded =
      if answer == "",
        do: nil,
        else:
          (case Jason.decode(answer) do
             {:ok, json} -> json
             _ -> answer
           end)

    %{status: status, body: decoded}
  end

  defp bearer(nil), do: []
  defp bearer(token), do: [{"authorization", "Bearer " <> token}]

  defp body(nil), do: ""
  defp body(map), do: Jason.encode!(map)

  def claim(port, token, body \\ nil),
    do: request(port, :post, "/worker/claim", token: token, body: body)

  def events(port, token, id, events),
    do:
      request(port, :post, "/worker/workloads/#{id}/events",
        token: token,
        body: %{events: events}
      )

  def heartbeat(port, token, id),
    do: request(port, :post, "/worker/workloads/#{id}/heartbeat", token: token)

  def record(port, token, id, record),
    do: request(port, :post, "/worker/workloads/#{id}/record", token: token, body: record)

  def held(port, token, ids),
    do: request(port, :post, "/worker/workloads/held", token: token, body: %{ids: ids})

  def release(port, token, id),
    do: request(port, :post, "/worker/workloads/#{id}/release", token: token)
end
