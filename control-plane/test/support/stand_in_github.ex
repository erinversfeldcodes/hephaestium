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

    File.write!(
      gh,
      String.replace(
        ~S"""
        #!/bin/sh
        d=@DIR@
        echo "$*" >> $d/gh-calls.log
        [ "$1" = "api" ] || exit 2
        shift
        sep=$(printf '\001')
        slurp=0; endpoint=""; number=""; method=GET; fields=""
        while [ $# -gt 0 ]; do
          case "$1" in
            --paginate) shift;;
            --slurp) slurp=1; shift;;
            -X) method="$2"; shift 2;;
            -H) echo "header: $2" >> $d/gh-headers.log; shift 2;;
            -f|-F) case "$2" in number=*) number="${2#*=}";; esac; fields="$fields$2$sep"; shift 2;;
            *) endpoint="$1"; shift;;
          esac
        done
        # Where a token came from is recorded, for the tests that say how it travels; it
        # is only ever a made-up one.
        case "$GH_TOKEN" in fake-installation-*|token-for-*) echo "$method $endpoint $GH_TOKEN" >> $d/gh-tokens.log;; esac
        name=$(printf '%s' "$endpoint" | tr '/?=&:%' '______')
        [ "$endpoint" = graphql ] && name="graphql_$number"
        if [ "$method" != GET ]; then
          # A write: record it whole (its method, its endpoint and its fields, which may
          # hold lines), and answer, or fail as told.
          printf '%s\000%s\000' "$method" "$endpoint" >> $d/writes.log
          printf '%s' "$fields" | tr '\001' '\000' >> $d/writes.log
          printf '\001\000' >> $d/writes.log
          if [ -f "$d/data/$name.$method.err" ]; then cat "$d/data/$name.$method.err" >&2; exit 1; fi
          # The nth write to this endpoint is answered from its own file if there is one,
          # then from the endpoint's, then with nothing.
          c="$d/data/$name.$method.count"
          n=$(cat "$c" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "$c"
          if [ -f "$d/data/$name.$method.$n.json" ]; then cat "$d/data/$name.$method.$n.json"; exit 0; fi
          if [ -f "$d/data/$name.$method.json" ]; then cat "$d/data/$name.$method.json"; exit 0; fi
          echo '{}'
          exit 0
        fi
        if [ -f "$d/data/$name.err" ]; then cat "$d/data/$name.err" >&2; exit 1; fi
        if [ -f "$d/data/$name.json" ]; then
          if [ $slurp = 1 ]; then printf '['; cat "$d/data/$name.json"; printf ']'; else cat "$d/data/$name.json"; fi
          exit 0
        fi
        echo 'gh: Not Found (HTTP 404)' >&2
        exit 1
        """,
        "@DIR@",
        dir
      )
    )

    heph = Path.join(dir, "heph")

    File.write!(heph, """
    #!/bin/sh
    d=#{dir}
    echo "$*" >> $d/heph-calls.log
    if [ "$1" = lenses ]; then
      shift
      while [ $# -gt 0 ]; do
        case "$1" in --manifest) cp "$2" $d/manifest-seen.toml; shift 2;; *) shift;; esac
      done
      if [ -f $d/heph-lenses.err ]; then cat $d/heph-lenses.err >&2; exit 3; fi
      cat $d/heph-lenses.json
      exit 0
    fi
    if [ -f $d/heph.err ]; then cat $d/heph.err >&2; exit 1; fi
    if [ -f $d/heph.warning ]; then cat $d/heph.warning >&2; fi
    cat $d/heph.json
    """)

    File.chmod!(gh, 0o755)
    File.chmod!(heph, 0o755)
    %{gh: gh, heph: heph, dir: dir}
  end

  defp name(endpoint), do: String.replace(endpoint, ~r{[/?=&:%]}, "_")

  @doc "Make the stand-in `gh` answer an endpoint with this, as JSON."
  def respond(%{dir: dir}, endpoint, term),
    do: File.write!(Path.join(dir, "data/#{name(endpoint)}.json"), Jason.encode!(term))

  @doc "Make the stand-in `gh` answer an endpoint with this text, as it is (a job's log)."
  def respond_text(%{dir: dir}, endpoint, text),
    do: File.write!(Path.join(dir, "data/#{name(endpoint)}.json"), text)

  @doc "Make the stand-in `gh` answer a GraphQL query about this issue number."
  def respond_graphql(%{dir: dir}, number, term),
    do: File.write!(Path.join(dir, "data/graphql_#{number}.json"), Jason.encode!(term))

  @doc "Make the stand-in `gh` fail on an endpoint, saying this."
  def fail(%{dir: dir}, endpoint, message),
    do: File.write!(Path.join(dir, "data/#{name(endpoint)}.err"), message)

  @doc "Let a read that was made to fail work again."
  def clear_failure(%{dir: dir}, endpoint),
    do: File.rm(Path.join(dir, "data/#{name(endpoint)}.err"))

  @doc "Make the stand-in `heph` print this workload."
  def heph_prints(%{dir: dir}, workload) do
    File.rm(Path.join(dir, "heph.err"))
    File.write!(Path.join(dir, "heph.json"), Jason.encode!(workload))
  end

  @doc "Make the stand-in `heph` print this on its error output, as it does while succeeding."
  def heph_warns(%{dir: dir}, message), do: File.write!(Path.join(dir, "heph.warning"), message)

  @doc "Make the stand-in `heph lenses` print this plan."
  def heph_lenses(%{dir: dir}, plan) do
    File.rm(Path.join(dir, "heph-lenses.err"))
    File.write!(Path.join(dir, "heph-lenses.json"), Jason.encode!(plan))
  end

  @doc "Make the stand-in `heph lenses` fail, saying this."
  def heph_lenses_fails(%{dir: dir}, message),
    do: File.write!(Path.join(dir, "heph-lenses.err"), message)

  @doc "The manifest the stand-in `heph lenses` was given, as text."
  def manifest_seen(%{dir: dir}), do: File.read(Path.join(dir, "manifest-seen.toml"))

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

  @doc """
  Make a write to an endpoint answer with this, as JSON: every write, or only the nth when
  `nth` is given.
  """
  def respond_write(%{dir: dir}, method, endpoint, term, nth \\ nil) do
    suffix = if nth, do: ".#{nth}", else: ""

    File.write!(
      Path.join(dir, "data/#{name(endpoint)}.#{method}#{suffix}.json"),
      Jason.encode!(term)
    )
  end

  @doc "Make a write to an endpoint fail, saying this."
  def fail_write(%{dir: dir}, method, endpoint, message),
    do: File.write!(Path.join(dir, "data/#{name(endpoint)}.#{method}.err"), message)

  @doc "Let a write that was made to fail work again."
  def clear_write_failure(%{dir: dir}, method, endpoint),
    do: File.rm!(Path.join(dir, "data/#{name(endpoint)}.#{method}.err"))

  @doc "The writes the stand-in `gh` was asked for, as `{method, endpoint, fields}`."
  def writes(%{dir: dir}) do
    case File.read(Path.join(dir, "writes.log")) do
      {:ok, bytes} -> parse_writes(String.split(bytes, <<0>>), [])
      _ -> []
    end
  end

  defp parse_writes([method, endpoint | rest], acc) do
    {fields, after_fields} = Enum.split_while(rest, &(&1 != <<1>>))
    parse_writes(Enum.drop(after_fields, 1), [{method, endpoint, fields} | acc])
  end

  defp parse_writes(_, acc), do: Enum.reverse(acc)
end
