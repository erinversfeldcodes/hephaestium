defmodule ControlPlane.Webhooks do
  @moduledoc false

  def enabled?, do: raise("not built")
  def check!, do: raise("not built")
  def receive(_source, _headers, _body), do: raise("not built")
  def wake_settling(_workload), do: raise("not built")
  def start_pass, do: raise("not built")
  def prune(_now), do: raise("not built")
end
