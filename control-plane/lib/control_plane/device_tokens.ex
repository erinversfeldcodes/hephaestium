defmodule ControlPlane.DeviceTokens do
  @moduledoc """
  The tokens that let an executor on a person's machine claim that person's workloads.

  A token is made for a login that is signed in and shown to them once: only its SHA-256 is
  kept, so what is stored cannot be used, and a token is high-entropy random bytes, which is
  why a fast hash is enough. It is scoped to the login that asked for it and to nothing else,
  can be revoked by that login at any time, and stops working the moment its login leaves the
  allowlist. A token is never logged, and nothing here puts one in a message.
  """

  import Ecto.Query

  alias ControlPlane.{DeviceToken, ExecutorLink, Repo, WorkloadLease}
  alias ControlPlane.Api.Settings

  @prefix "hdt_"
  @name_limit 80

  @doc """
  Make a token for a login. The token is in the answer and is not recoverable afterwards.
  """
  @spec issue(String.t(), String.t()) ::
          {:ok, %{token: String.t(), record: %DeviceToken{}}} | {:error, :bad_name}
  def issue(login, name) when is_binary(login) and is_binary(name) do
    name = String.trim(name)

    if name == "" or String.length(name) > @name_limit do
      {:error, :bad_name}
    else
      token = @prefix <> Base.url_encode64(:crypto.strong_rand_bytes(32), padding: false)

      record =
        Repo.insert!(%DeviceToken{login: login, name: name, token_hash: hash(token)})

      {:ok, %{token: token, record: record}}
    end
  end

  def issue(_, _), do: {:error, :bad_name}

  @doc """
  The token a request presented, if it is one that works: it exists, it is not revoked, and its
  login is still allowed. Any other is `:error`, whatever the reason, so a caller cannot tell
  them apart.
  """
  @spec authenticate(term(), Settings.t()) :: {:ok, %DeviceToken{}} | :error
  def authenticate(token, settings) when is_binary(token) do
    with true <- String.starts_with?(token, @prefix),
         %DeviceToken{revoked_at: nil} = record <-
           Repo.get_by(DeviceToken, token_hash: hash(token)),
         true <- Settings.allowed?(settings, record.login) do
      {:ok, record}
    else
      _ -> :error
    end
  end

  def authenticate(_, _), do: :error

  @doc "Whether a token still works: it has not been revoked since it was read."
  @spec active?(Ecto.UUID.t()) :: boolean()
  def active?(id),
    do: Repo.exists?(from t in DeviceToken, where: t.id == ^id and is_nil(t.revoked_at))

  @doc "Say that the executor of this token is there."
  @spec seen(%DeviceToken{}) :: :ok
  def seen(%DeviceToken{id: id}) do
    Repo.update_all(from(t in DeviceToken, where: t.id == ^id), set: [last_seen_at: now()])
    :ok
  end

  @doc """
  Revoke a token of this login. A login cannot revoke another's. The leases the token holds end
  at once, so what it held is looked after as for any executor that was lost.
  """
  @spec revoke(String.t(), Ecto.UUID.t()) :: :ok | :not_found
  def revoke(login, id) do
    with {:ok, id} <- Ecto.UUID.cast(id),
         {1, _} <-
           Repo.update_all(
             from(t in DeviceToken,
               where: t.id == ^id and t.login == ^login and is_nil(t.revoked_at)
             ),
             set: [revoked_at: now()]
           ) do
      Repo.update_all(from(l in WorkloadLease, where: l.token_id == ^id and l.active),
        set: [expires_at: now()]
      )

      :ok
    else
      _ -> :not_found
    end
  end

  @doc "A login's tokens, newest first, revoked ones included."
  @spec list(String.t()) :: [%DeviceToken{}]
  def list(login),
    do: Repo.all(from t in DeviceToken, where: t.login == ^login, order_by: [desc: t.inserted_at])

  @doc "Whether this token has been there lately and has not been revoked."
  @spec recently_seen?(%DeviceToken{}) :: boolean()
  def recently_seen?(%DeviceToken{revoked_at: nil, last_seen_at: %DateTime{} = seen}) do
    DateTime.diff(now(), seen, :millisecond) <= ExecutorLink.connected_for_ms()
  end

  def recently_seen?(_token), do: false

  @doc """
  Whether an executor of this login has been there lately: one of its tokens, not revoked, has
  claimed within the time a claim waits and a little over. A workload whose approver has none
  waits.
  """
  @spec connected?(String.t()) :: boolean()
  def connected?(login) do
    since = DateTime.add(now(), -ExecutorLink.connected_for_ms(), :millisecond)

    Repo.exists?(
      from t in DeviceToken,
        where:
          fragment("lower(?)", t.login) == ^String.downcase(login) and is_nil(t.revoked_at) and
            t.last_seen_at >= ^since
    )
  end

  defp hash(token), do: :crypto.hash(:sha256, token) |> Base.encode16(case: :lower)

  defp now, do: DateTime.utc_now()
end
