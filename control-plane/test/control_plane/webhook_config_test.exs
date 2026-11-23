defmodule ControlPlane.WebhookConfigTest do
  @moduledoc """
  How webhooks are switched on, what the schedule is with them on and off, and how the
  delivery ids are kept. Off is the laptop's way and is the control plane as it was.
  """

  use ControlPlane.DataCase, async: false
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query

  alias ControlPlane.Jobs.{
    AnswerReplies,
    EvaluateMerged,
    Intake,
    KeepCurrent,
    PruneDeliveries,
    RollUpEpics,
    SettleTickets
  }

  alias ControlPlane.{Schedule, Webhooks}
  alias ControlPlane.Webhooks.Delivery

  @runtime Path.expand("../../config/runtime.exs", __DIR__)
  @names [
    "HEPH_WEBHOOK_SECRET_FILE",
    "HEPH_WEBHOOK_SECRET",
    "HEPH_API_PORT",
    "HEPH_API_SECRET_KEY_BASE"
  ]
  @api [{"HEPH_API_PORT", "4000"}, {"HEPH_API_SECRET_KEY_BASE", String.duplicate("k", 40)}]

  defp configured(environment) do
    previous = for name <- @names, do: {name, System.get_env(name)}

    try do
      for name <- @names, do: System.delete_env(name)
      for {name, value} <- environment, do: System.put_env(name, value)

      config = Config.Reader.read!(@runtime, env: :test, target: :host)
      get_in(config, [:control_plane, :webhooks])
    after
      for {name, value} <- previous do
        if value, do: System.put_env(name, value), else: System.delete_env(name)
      end
    end
  end

  describe "the setting" do
    test "is off unless a secret file is named" do
      assert configured([]) == nil
      assert configured(@api) == nil
      assert configured([{"HEPH_WEBHOOK_SECRET_FILE", ""} | @api]) == nil
    end

    test "is on with a secret file and the API, and holds the path and never the secret" do
      assert configured([{"HEPH_WEBHOOK_SECRET_FILE", "/run/secrets/hook"} | @api]) ==
               [secret_file: "/run/secrets/hook"]
    end

    test "is on with the secret in a variable, as Fly keeps secrets, and holds the name and never the secret" do
      assert configured([{"HEPH_WEBHOOK_SECRET", "super-secret"} | @api]) ==
               [secret_variable: "HEPH_WEBHOOK_SECRET"]
    end

    test "is refused when the secret is given both as a variable and as a file" do
      error =
        assert_raise RuntimeError, fn ->
          configured([
            {"HEPH_WEBHOOK_SECRET", "super-secret"},
            {"HEPH_WEBHOOK_SECRET_FILE", "/run/secrets/hook"} | @api
          ])
        end

      assert error.message =~ "HEPH_WEBHOOK_SECRET"
      refute error.message =~ "super-secret"
    end

    test "a variable needs the API too" do
      assert_raise RuntimeError, ~r/needs HEPH_API_PORT/, fn ->
        configured([{"HEPH_WEBHOOK_SECRET", "super-secret"}])
      end
    end

    test "is refused without the API, which is where the route is" do
      assert_raise RuntimeError, ~r/needs HEPH_API_PORT/, fn ->
        configured([{"HEPH_WEBHOOK_SECRET_FILE", "/run/secrets/hook"}])
      end
    end
  end

  describe "starting" do
    setup do
      saved = Application.fetch_env(:control_plane, :webhooks)

      on_exit(fn ->
        case saved do
          {:ok, value} -> Application.put_env(:control_plane, :webhooks, value)
          :error -> Application.delete_env(:control_plane, :webhooks)
        end
      end)
    end

    test "does not start with a secret file it cannot read or that is empty" do
      dir = Path.join(System.tmp_dir!(), "cp-hook-#{System.unique_integer([:positive])}")
      File.mkdir_p!(dir)
      on_exit(fn -> File.rm_rf(dir) end)

      Application.put_env(:control_plane, :webhooks, secret_file: Path.join(dir, "missing"))

      assert_raise ArgumentError, ~r/HEPH_WEBHOOK_SECRET_FILE/, fn -> Webhooks.check!() end

      File.write!(Path.join(dir, "empty"), "\n")
      Application.put_env(:control_plane, :webhooks, secret_file: Path.join(dir, "empty"))
      assert_raise ArgumentError, ~r/HEPH_WEBHOOK_SECRET_FILE/, fn -> Webhooks.check!() end
    end

    test "the error names the setting and never the secret" do
      dir = Path.join(System.tmp_dir!(), "cp-hook-#{System.unique_integer([:positive])}")
      File.mkdir_p!(dir)
      on_exit(fn -> File.rm_rf(dir) end)
      File.write!(Path.join(dir, "s"), "")

      Application.put_env(:control_plane, :webhooks, secret_file: Path.join(dir, "s"))

      error = assert_raise ArgumentError, fn -> Webhooks.check!() end
      refute Exception.message(error) =~ "super-secret"
    end

    test "does not start with a secret variable that is empty, and says which" do
      System.put_env("HEPH_WEBHOOK_SECRET", "  ")
      on_exit(fn -> System.delete_env("HEPH_WEBHOOK_SECRET") end)
      Application.put_env(:control_plane, :webhooks, secret_variable: "HEPH_WEBHOOK_SECRET")

      error = assert_raise ArgumentError, fn -> Webhooks.check!() end
      assert Exception.message(error) =~ "HEPH_WEBHOOK_SECRET"
    end

    test "starts with a good secret variable" do
      System.put_env("HEPH_WEBHOOK_SECRET", "super-secret")
      on_exit(fn -> System.delete_env("HEPH_WEBHOOK_SECRET") end)
      Application.put_env(:control_plane, :webhooks, secret_variable: "HEPH_WEBHOOK_SECRET")

      assert Webhooks.check!() == :ok
    end

    test "starts with a good secret file" do
      dir = Path.join(System.tmp_dir!(), "cp-hook-#{System.unique_integer([:positive])}")
      File.mkdir_p!(dir)
      on_exit(fn -> File.rm_rf(dir) end)
      File.write!(Path.join(dir, "s"), "super-secret\n")
      Application.put_env(:control_plane, :webhooks, secret_file: Path.join(dir, "s"))

      assert Webhooks.check!() == :ok
    end
  end

  describe "the schedule" do
    defp scheduled(crontab), do: Map.new(crontab, fn {expression, job} -> {job, expression} end)

    @polls [Intake, SettleTickets, KeepCurrent, EvaluateMerged, RollUpEpics, AnswerReplies]

    test "without webhooks is the poll as it always was" do
      schedule = scheduled(Schedule.crontab(false))

      assert schedule[Intake] == "* * * * *"
      assert schedule[SettleTickets] == "* * * * *"
      assert schedule[KeepCurrent] == "*/10 * * * *"
      assert schedule[EvaluateMerged] == "*/10 * * * *"
      assert schedule[RollUpEpics] == "*/10 * * * *"
      assert schedule[AnswerReplies] == "*/5 * * * *"
      assert schedule[ControlPlane.Jobs.RecheckOwed] == "7 * * * *"
      assert schedule[ControlPlane.Jobs.SweepWorkspaces] == "17 * * * *"
      refute Map.has_key?(schedule, PruneDeliveries)
    end

    test "with webhooks runs each job that reads the code host once an hour, offset from the others" do
      schedule = scheduled(Schedule.crontab(true))

      minutes =
        for job <- @polls do
          assert ["" <> minute, "*", "*", "*", "*"] = String.split(schedule[job], " ")
          assert {_, ""} = Integer.parse(minute), "#{inspect(job)} is not hourly"
          minute
        end

      others =
        for job <- [
              ControlPlane.Jobs.RecheckOwed,
              ControlPlane.Jobs.SweepWorkspaces,
              PruneDeliveries
            ],
            do: schedule[job] |> String.split(" ") |> hd()

      assert Enum.uniq(minutes ++ others) == minutes ++ others, "two jobs run in the same minute"
    end

    test "with webhooks leaves the jobs that read no code host as they were, and prunes ids" do
      on = scheduled(Schedule.crontab(true))
      off = scheduled(Schedule.crontab(false))

      assert on[ControlPlane.Jobs.RecheckOwed] == off[ControlPlane.Jobs.RecheckOwed]
      assert on[ControlPlane.Jobs.SweepWorkspaces] == off[ControlPlane.Jobs.SweepWorkspaces]
      assert Map.has_key?(on, PruneDeliveries)
    end

    test "is the Oban configuration's, and webhooks being off leaves the configured schedule alone" do
      oban = [plugins: [Oban.Plugins.Cron], queues: []]

      assert {Oban.Plugins.Cron, crontab: crontab} =
               oban |> Schedule.oban(false) |> Keyword.fetch!(:plugins) |> hd()

      assert crontab == Schedule.crontab(false)
      assert Schedule.oban([plugins: false], true) == [plugins: false]
    end
  end

  describe "a full pass when the control plane starts" do
    test "queues each job that reads the code host, once" do
      Application.put_env(:control_plane, :webhooks, secret_file: "/x")
      on_exit(fn -> Application.delete_env(:control_plane, :webhooks) end)

      Webhooks.start_pass()
      Webhooks.start_pass()

      for job <- [Intake, SettleTickets, KeepCurrent, EvaluateMerged, RollUpEpics, AnswerReplies] do
        assert [_] = all_enqueued(worker: job), "#{inspect(job)} was not queued once"
      end
    end

    test "queues nothing with webhooks off" do
      Application.delete_env(:control_plane, :webhooks)

      Webhooks.start_pass()

      assert all_enqueued() == []
    end
  end

  describe "the delivery ids" do
    defp delivery(id, received_at) do
      Repo.insert!(%Delivery{source: "github", delivery_id: id, received_at: received_at})
    end

    test "older than a week are deleted, and newer are kept" do
      now = ~U[2026-11-22 09:00:00.000000Z]
      delivery("old", DateTime.add(now, -8 * 86_400, :second))
      delivery("edge", DateTime.add(now, -7 * 86_400 + 60, :second))
      delivery("new", DateTime.add(now, -3600, :second))

      assert Webhooks.prune(now) == 1

      assert Repo.all(from d in Delivery, order_by: d.delivery_id, select: d.delivery_id) ==
               ["edge", "new"]
    end

    test "a delivery id is unique to its source" do
      delivery("same", DateTime.utc_now())

      assert_raise Ecto.ConstraintError, fn -> delivery("same", DateTime.utc_now()) end

      assert %Delivery{} =
               Repo.insert!(%Delivery{
                 source: "other",
                 delivery_id: "same",
                 received_at: DateTime.utc_now()
               })
    end

    test "the pruning job prunes" do
      delivery("old", DateTime.add(DateTime.utc_now(), -9 * 86_400, :second))

      assert :ok = perform_job(PruneDeliveries, %{})
      assert Repo.aggregate(Delivery, :count) == 0
    end
  end
end
