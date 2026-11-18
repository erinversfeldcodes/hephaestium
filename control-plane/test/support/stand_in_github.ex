defmodule ControlPlane.StandInGitHub do
  @moduledoc """
  A stand-in for `gh` and for `heph`.

  `gh` answers `gh api ENDPOINT` from recorded responses in the shapes the real
  one gives, and logs what it was asked. `heph` prints a recorded workload, and
  logs how it was run. That proves the control plane's logic against those
  shapes; it does not prove that GitHub answers that way, and the shapes were
  read from the real GitHub.
  """

  @doc "Install both in `dir`, and return their paths."
  def install(dir) do
    File.mkdir_p!(Path.join(dir, "data"))

    gh = Path.join(dir, "gh")

    File.write!(gh, """
    #!/bin/sh
    d=#{dir}
    echo "$*" >> $d/gh-calls.log
    [ "$1" = "api" ] || exit 2
    shift
    slurp=0; endpoint=""; number=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --paginate) shift;;
        --slurp) slurp=1; shift;;
        -f|-F) case "$2" in number=*) number="${2#*=}";; esac; shift 2;;
        *) endpoint="$1"; shift;;
      esac
    done
    name=$(printf '%s' "$endpoint" | tr '/?=&:' '_____')
    [ "$endpoint" = graphql ] && name="graphql_$number"
    if [ -f "$d/data/$name.err" ]; then cat "$d/data/$name.err" >&2; exit 1; fi
    if [ -f "$d/data/$name.json" ]; then
      if [ $slurp = 1 ]; then printf '['; cat "$d/data/$name.json"; printf ']'; else cat "$d/data/$name.json"; fi
      exit 0
    fi
    echo 'gh: Not Found (HTTP 404)' >&2
    exit 1
    """)

    heph = Path.join(dir, "heph")

    File.write!(heph, """
    #!/bin/sh
    d=#{dir}
    echo "$*" >> $d/heph-calls.log
    if [ -f $d/heph.err ]; then cat $d/heph.err >&2; exit 1; fi
    cat $d/heph.json
    """)

    File.chmod!(gh, 0o755)
    File.chmod!(heph, 0o755)
    %{gh: gh, heph: heph, dir: dir}
  end

  defp name(endpoint), do: String.replace(endpoint, ~r{[/?=&:]}, "_")

  @doc "Make the stand-in `gh` answer an endpoint with this, as JSON."
  def respond(%{dir: dir}, endpoint, term),
    do: File.write!(Path.join(dir, "data/#{name(endpoint)}.json"), Jason.encode!(term))

  @doc "Make the stand-in `gh` answer a GraphQL query about this issue number."
  def respond_graphql(%{dir: dir}, number, term),
    do: File.write!(Path.join(dir, "data/graphql_#{number}.json"), Jason.encode!(term))

  @doc "Make the stand-in `gh` fail on an endpoint, saying this."
  def fail(%{dir: dir}, endpoint, message),
    do: File.write!(Path.join(dir, "data/#{name(endpoint)}.err"), message)

  @doc "Make the stand-in `heph` print this workload."
  def heph_prints(%{dir: dir}, workload) do
    File.rm(Path.join(dir, "heph.err"))
    File.write!(Path.join(dir, "heph.json"), Jason.encode!(workload))
  end

  @doc "Make the stand-in `heph` fail, saying this."
  def heph_fails(%{dir: dir}, message), do: File.write!(Path.join(dir, "heph.err"), message)

  @doc "What the stand-in `heph` was run with, one line a run."
  def heph_calls(%{dir: dir}) do
    case File.read(Path.join(dir, "heph-calls.log")) do
      {:ok, text} -> String.split(text, "\n", trim: true)
      _ -> []
    end
  end

  @doc "What the stand-in `gh` was asked, one line a call."
  def gh_calls(%{dir: dir}) do
    case File.read(Path.join(dir, "gh-calls.log")) do
      {:ok, text} -> String.split(text, "\n", trim: true)
      _ -> []
    end
  end
end
