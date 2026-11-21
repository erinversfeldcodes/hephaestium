defmodule ControlPlane.GitHub do
  @moduledoc """
  Reading GitHub by running `gh`, which holds the credentials: the control plane
  holds no token. Everything it reads, it reads through here.
  """

  alias ControlPlane.Programs

  @doc "The command that is run: `gh`, or what the configuration says."
  def command, do: Application.get_env(:control_plane, :intake, []) |> Keyword.get(:gh, "gh")

  @doc "Read one endpoint, as decoded JSON."
  @spec get(String.t()) :: {:ok, term()} | {:error, String.t()}
  def get(endpoint), do: run(["api", endpoint])

  @doc "Read an endpoint that answers with text, not JSON: a job's log."
  @spec text(String.t()) :: {:ok, String.t()} | {:error, String.t()}
  def text(endpoint) do
    if Programs.allowed?(command()) do
      case System.cmd(command(), ["api", endpoint], stderr_to_stdout: true) do
        {output, 0} -> {:ok, output}
        {output, _} -> {:error, String.trim(output)}
      end
    else
      {:error, Programs.refusal(command())}
    end
  rescue
    error in ErlangError -> {:error, "gh could not be run: #{inspect(error.original)}"}
  end

  @doc "Read a file's own text from the contents API, not the JSON that wraps it."
  @spec raw(String.t()) :: {:ok, String.t()} | {:error, String.t()}
  def raw(endpoint) do
    if Programs.allowed?(command()) do
      case System.cmd(
             command(),
             ["api", "-H", "Accept: application/vnd.github.raw+json", endpoint],
             stderr_to_stdout: true
           ) do
        {output, 0} -> {:ok, output}
        {output, _} -> {:error, String.trim(output)}
      end
    else
      {:error, Programs.refusal(command())}
    end
  rescue
    error in ErlangError -> {:error, "gh could not be run: #{inspect(error.original)}"}
  end

  @doc "Read every page of a list endpoint, as one list."
  @spec get_all(String.t()) :: {:ok, [term()]} | {:error, String.t()}
  def get_all(endpoint) do
    with {:ok, pages} <- run(["api", endpoint, "--paginate", "--slurp"]) do
      {:ok, pages |> List.wrap() |> Enum.flat_map(&List.wrap/1)}
    end
  end

  @doc """
  Change something on GitHub: a label, a comment, an issue. `gh` makes the request with its
  own credentials, so what it may change is what its login may.
  """
  @spec write(String.t(), String.t(), [String.t() | {:typed, String.t()}], keyword()) ::
          {:ok, term()} | {:error, String.t()}
  def write(method, endpoint, fields, options \\ []) do
    run(["api", "-X", method, endpoint | Enum.flat_map(fields, &field/1)], options)
  end

  # A field is sent as text; one marked `:typed` is sent as what it looks like, a number
  # for a number, as the sub-issue and dependency endpoints ask for the id of an issue.
  defp field({:typed, value}), do: ["-F", value]
  defp field(value), do: ["-f", value]

  @doc "Run a GraphQL query with variables, as decoded JSON."
  @spec graphql(String.t(), keyword()) :: {:ok, term()} | {:error, String.t()}
  def graphql(query, variables) do
    flags =
      Enum.flat_map(variables, fn
        {name, value} when is_integer(value) -> ["-F", "#{name}=#{value}"]
        {name, value} -> ["-f", "#{name}=#{value}"]
      end)

    run(["api", "graphql", "-f", "query=#{query}" | flags])
  end

  # `token:` makes `gh` act as that token for this one request, through its environment
  # so that it is in no argument.
  defp run(arguments, options \\ []) do
    if Programs.allowed?(command()) do
      execute(arguments, options)
    else
      {:error, Programs.refusal(command())}
    end
  end

  defp execute(arguments, options) do
    environment =
      case options[:token] do
        token when is_binary(token) -> [{"GH_TOKEN", token}]
        _ -> []
      end

    case System.cmd(command(), arguments, stderr_to_stdout: true, env: environment) do
      {output, 0} ->
        case Jason.decode(output) do
          {:ok, decoded} -> {:ok, decoded}
          {:error, _} -> {:error, "gh did not answer with JSON: #{String.slice(output, 0, 200)}"}
        end

      {output, _status} ->
        {:error, String.trim(output)}
    end
  rescue
    error in ErlangError -> {:error, "gh could not be run: #{inspect(error.original)}"}
  end
end
