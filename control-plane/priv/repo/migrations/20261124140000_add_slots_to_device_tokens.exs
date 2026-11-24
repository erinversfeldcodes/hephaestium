defmodule ControlPlane.Repo.Migrations.AddSlotsToDeviceTokens do
  use Ecto.Migration

  def change do
    alter table(:device_tokens) do
      # How many workloads the executor said it may run at once, as of its last claim.
      add :slots, :integer, null: false, default: 1
    end
  end
end
