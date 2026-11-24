defmodule ControlPlane.CodeHost do
  @moduledoc """
  Where a repository's code lives: its changes, their checks, merging and content. The
  control plane reaches a code host only through this boundary, and never sees a system's own
  payload: every operation answers in the control plane's own structures
  (`ControlPlane.Change`, `ControlPlane.Check` and their companions).

  Each operation names the repository the control plane knows the work by and is dispatched
  to the code host configured for it (`ControlPlane.Systems`); a module that implements the
  behaviour receives the repository's name in that system. Failures are `{:error, message}`.

  `pages:` on a files read says whether only the first page is read (`:first`, the default)
  or every page (`:all`).
  """

  alias ControlPlane.{Change, ChangeReview, Check, Systems}

  @type repository :: String.t()
  @type error :: {:error, String.t()}
  @type capability :: :approval | :check_rerun | :check_annotations
  @type permission :: :admin | :maintain | :write | :read | :none
  @type approval :: :approved | :submitted | :disabled | :unsupported

  @callback capabilities() :: MapSet.t(capability())
  @callback change(repository, number :: term()) :: {:ok, Change.t()} | error
  @callback change_files(repository, number :: term(), keyword()) :: {:ok, [String.t()]} | error
  @callback compare(repository, from :: String.t(), to :: String.t()) ::
              {:ok, %{commits: [%{sha: String.t(), message: String.t()}], files: [String.t()]}}
              | error
  @callback comment_on_change(repository, number :: term(), body :: String.t()) ::
              :ok | {:ok, url :: String.t()} | error
  @callback merge(repository, number :: term(), commit :: String.t()) ::
              :ok | {:refused, String.t()} | error
  @callback reviews(repository, number :: term()) :: {:ok, [ChangeReview.t()]} | error
  @callback approve(repository, number :: term(), commit :: String.t(), body :: String.t()) ::
              approval | error
  @callback required_checks(repository, branch :: String.t()) :: {:ok, [String.t()]} | error
  @callback checks(repository, commit :: String.t()) :: {:ok, [Check.t()]} | error
  @callback check_log(repository, check_id :: term()) :: {:ok, String.t()} | error
  @callback check_annotations(repository, check_id :: term()) :: {:ok, [String.t()]} | error
  @callback rerun_check(repository, check_id :: term()) :: :ok | error
  @callback file(repository, path :: String.t(), ref :: String.t()) :: {:ok, String.t()} | error
  @callback directory(repository, path :: String.t(), ref :: String.t()) ::
              {:ok, [%{name: String.t(), type: String.t()}]} | {:error, String.t() | :not_a_list}
  @callback permission(repository, login :: String.t()) :: {:ok, permission} | error

  @doc "What the repository's code host supports."
  def capabilities(repository) do
    {module, _name} = Systems.code_host(repository)
    module.capabilities()
  end

  def change(repository, number), do: call(repository, :change, [number])

  def change_files(repository, number, opts \\ []),
    do: call(repository, :change_files, [number, opts])

  def compare(repository, from, to), do: call(repository, :compare, [from, to])

  def comment_on_change(repository, number, body),
    do: call(repository, :comment_on_change, [number, body])

  def merge(repository, number, commit), do: call(repository, :merge, [number, commit])
  def reviews(repository, number), do: call(repository, :reviews, [number])

  def approve(repository, number, commit, body),
    do: call(repository, :approve, [number, commit, body])

  def required_checks(repository, branch), do: call(repository, :required_checks, [branch])
  def checks(repository, commit), do: call(repository, :checks, [commit])
  def check_log(repository, check_id), do: call(repository, :check_log, [check_id])

  def check_annotations(repository, check_id),
    do: call(repository, :check_annotations, [check_id])

  def rerun_check(repository, check_id), do: call(repository, :rerun_check, [check_id])
  def file(repository, path, ref), do: call(repository, :file, [path, ref])
  def directory(repository, path, ref), do: call(repository, :directory, [path, ref])
  def permission(repository, login), do: call(repository, :permission, [login])

  @doc "Whether a permission is one a maintainer has: the one an approver must hold."
  @spec writer?(permission) :: boolean()
  def writer?(permission), do: permission in [:admin, :maintain, :write]

  defp call(repository, operation, arguments) do
    {module, name} = Systems.code_host(repository)
    apply(module, operation, [name | arguments])
  end
end
