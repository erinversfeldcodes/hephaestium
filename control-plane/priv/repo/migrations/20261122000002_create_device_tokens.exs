defmodule ControlPlane.Repo.Migrations.CreateDeviceTokens do
  use Ecto.Migration

  def change do
    create table(:device_tokens, primary_key: false) do
      add :id, :uuid, primary_key: true
      # Whose executor it is: the GitHub login that asked for it.
      add :login, :text, null: false
      # What the person calls the device, so a list of tokens can be told apart.
      add :name, :text, null: false
      # The SHA-256 of the token. The token itself is shown once and kept nowhere.
      add :token_hash, :text, null: false
      add :last_seen_at, :utc_datetime_usec
      add :revoked_at, :utc_datetime_usec

      timestamps(updated_at: false, type: :utc_datetime_usec)
    end

    create unique_index(:device_tokens, [:token_hash])
    create index(:device_tokens, [:login])
  end
end
