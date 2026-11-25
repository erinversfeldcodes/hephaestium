#!/usr/bin/env bash
# Run only the tests that cover the files a change touched, so a change is not made to wait on a
# whole suite. The gates and CI run the whole suites; this is what an author runs on the way.
#
#   test-changed.sh rust PATH...      PATHs relative to the repository root
#   test-changed.sh elixir PATH...    PATHs relative to control-plane/
#
# Rust: the tests of each crate a path is under. A path outside the crates is not a crate's own,
# so it names nothing to run here, and the gates run what it affects.
# Elixir: the test files named, and the test file that sits beside each source file named under
# lib/. With none found, nothing runs and it says so.
set -euo pipefail

stack=${1:?usage: test-changed.sh rust|elixir PATH...}
shift

case "$stack" in
rust)
  crates=()
  for path in "$@"; do
    case "$path" in
    crates/*/*)
      name=${path#crates/}
      name=${name%%/*}
      [[ " ${crates[*]:-} " == *" $name "* ]] || crates+=("$name")
      ;;
    esac
  done
  if [[ ${#crates[@]} -eq 0 ]]; then
    echo "test-changed: no crate's files changed; the gates run the rest"
    exit 0
  fi
  args=()
  for name in "${crates[@]}"; do args+=(-p "$name"); done
  exec cargo test --locked "${args[@]}"
  ;;
elixir)
  tests=()
  for path in "$@"; do
    case "$path" in
    test/*_test.exs) candidate=$path ;;
    lib/*.ex) candidate="test/${path#lib/}"; candidate="${candidate%.ex}_test.exs" ;;
    *) continue ;;
    esac
    [[ -f "$candidate" ]] && tests+=("$candidate")
  done
  if [[ ${#tests[@]} -eq 0 ]]; then
    echo "test-changed: no test file covers these paths; the gates run the rest"
    exit 0
  fi
  exec mix test "${tests[@]}"
  ;;
*)
  echo "test-changed: unknown stack '$stack'" >&2
  exit 2
  ;;
esac
