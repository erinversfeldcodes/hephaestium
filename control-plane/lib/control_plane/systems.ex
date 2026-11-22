defmodule ControlPlane.Systems do
  @moduledoc """
  Where a repository's tickets live and where its code lives, as a pair. They are two
  systems that may differ, so each is named on its own: a module that implements the
  behaviour and the name the repository goes by in that system.

  A repository the configuration does not name, or names for one side only, has GitHub
  for the side that is missing, under the repository's own name, so a bare repository name
  keeps meaning what it always did. The configuration is
  `config :control_plane, :systems, %{"owner/name" => %{tracker: {module, project},
  code_host: {module, repository}}}`.
  """

  @github_tracker ControlPlane.GitHub.Tracker
  @github_code_host ControlPlane.GitHub.CodeHost

  @doc "The tracker for a repository, and the project the repository's tickets are in."
  @spec tracker(String.t()) :: {module(), String.t()}
  def tracker(repository), do: side(repository, :tracker, @github_tracker)

  @doc "The code host for a repository, and the repository's name in it."
  @spec code_host(String.t()) :: {module(), String.t()}
  def code_host(repository), do: side(repository, :code_host, @github_code_host)

  defp side(repository, key, default) do
    configured = Application.get_env(:control_plane, :systems, %{}) || %{}

    case configured |> Map.get(repository, %{}) |> Map.get(key) do
      {module, name} when is_atom(module) and is_binary(name) -> {module, name}
      _ -> {default, repository}
    end
  end
end
