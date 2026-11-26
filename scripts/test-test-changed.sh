#!/usr/bin/env bash
# Tests for test-changed.sh with stand-ins for cargo and mix that record their arguments. They
# prove which tests the script selects; they do not prove cargo or mix then run them.
set -uo pipefail

script=$(cd "$(dirname "$0")" && pwd)/test-changed.sh
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
failures=0

mkdir -p "$scratch/bin" "$scratch/cp/lib/app" "$scratch/cp/test/app"
for tool in cargo mix; do
  printf '#!/bin/sh\necho "%s $*"\n' "$tool" >"$scratch/bin/$tool"
  chmod +x "$scratch/bin/$tool"
done
touch "$scratch/cp/lib/app/thing.ex" "$scratch/cp/test/app/thing_test.exs" "$scratch/cp/lib/app/untested.ex"

# expect <description> <directory> <expected output> <stack> <path>...
expect() {
  local description=$1 dir=$2 want=$3
  shift 3
  local got
  got=$(cd "$dir" && PATH="$scratch/bin:$PATH" "$script" "$@" 2>&1)
  if [[ "$got" == "$want" ]]; then
    echo "ok: $description"
  else
    echo "FAIL: $description"
    echo "  wanted: $want"
    echo "  got:    $got"
    failures=$((failures + 1))
  fi
}

none="test-changed: no crate's files changed; the gates run the rest"
no_test="test-changed: no test file covers these paths; the gates run the rest"

expect "a crate's file selects that crate" "$scratch" \
  "cargo test --locked -p executor" rust crates/executor/src/run.rs
expect "two files of one crate and one of another select each crate once" "$scratch" \
  "cargo test --locked -p executor -p heph" rust crates/executor/src/a.rs crates/heph/src/b.rs crates/executor/tests/c.rs
expect "a path outside the crates runs nothing" "$scratch" "$none" rust README.md
expect "a lib file selects the test file beside it, from the component's root" "$scratch/cp" \
  "mix test test/app/thing_test.exs" elixir lib/app/thing.ex
expect "a changed test file selects itself" "$scratch/cp" \
  "mix test test/app/thing_test.exs" elixir test/app/thing_test.exs
expect "a lib file with no test file runs nothing" "$scratch/cp" "$no_test" elixir lib/app/untested.ex
expect "an unknown stack is refused" "$scratch" "test-changed: unknown stack 'go'" go x

if [[ $failures -ne 0 ]]; then
  echo "$failures failed"
  exit 1
fi
