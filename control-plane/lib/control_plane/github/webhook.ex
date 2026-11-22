defmodule ControlPlane.GitHub.Webhook do
  @moduledoc false

  def verified?(_secret, _headers, _body), do: raise("not built")
  def delivery_id(_headers), do: raise("not built")
  def normalise(_headers, _body, _now), do: raise("not built")
end
