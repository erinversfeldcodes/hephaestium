#!/usr/bin/env bash
# Tests for check-migrations.sh against throwaway repositories. A synthetic repository proves
# the script judges versions as it should; it does not prove what the real migrations hold.
set -uo pipefail

script=$(cd "$(dirname "$0")" && pwd)/check-migrations.sh
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
failures=0
dir=control-plane/priv/repo/migrations

# repo <name>: a repository whose main holds two migrations; prints its path.
repo() {
  local path=$scratch/$1
  mkdir -p "$path/$dir"
  (
    cd "$path" || exit 1
    git init -q -b main
    git config user.email t@example.com
    git config user.name t
    touch "$dir/20261120000001_create_a.exs" "$dir/20261124000002_create_b.exs"
    git add -A
    git commit -q -m base
    git checkout -q -b change
  )
  echo "$path"
}

# expect_fail <description> <repo> <text the message must hold>...
expect_fail() {
  local description=$1 path=$2 out
  shift 2
  if out=$(cd "$path" && "$script" main 2>&1); then
    echo "FAIL $description: passed"
    failures=$((failures + 1))
    return
  fi
  local want
  for want in "$@"; do
    if [[ $out != *"$want"* ]]; then
      echo "FAIL $description: the message lacks '$want': $out"
      failures=$((failures + 1))
      return
    fi
  done
  echo "ok   $description"
}
expect_pass() { # expect_pass <description> <repo>
  local out
  if out=$(cd "$2" && "$script" main 2>&1); then
    echo "ok   $1"
  else
    echo "FAIL $1: refused: $out"
    failures=$((failures + 1))
  fi
}

r=$(repo duplicate)
touch "$r/$dir/20261124000002_add_slots.exs"
expect_fail "two migrations sharing a version, one new" "$r" \
  "two migrations share version 20261124000002" "20261124000002_add_slots.exs" "20261124000002_create_b.exs"

r=$(repo duplicate-new)
touch "$r/$dir/20261125000000_one.exs" "$r/$dir/20261125000000_two.exs"
expect_fail "two new migrations sharing a version" "$r" \
  "20261125000000_one.exs" "20261125000000_two.exs"

r=$(repo older)
touch "$r/$dir/20261123000000_add_old.exs"
expect_fail "a new migration older than the base's newest" "$r" \
  "20261123000000_add_old.exs" "20261124000002_create_b.exs"

r=$(repo older-committed)
touch "$r/$dir/20261101000000_add_old.exs"
(cd "$r" && git add -A && git commit -q -m old)
expect_fail "an older migration already committed on the branch" "$r" "20261101000000_add_old.exs"

r=$(repo clean)
touch "$r/$dir/20261125000000_add_c.exs"
expect_pass "a clean addition" "$r"

r=$(repo untouched)
expect_pass "no migration changed" "$r"

r=$(repo no-base)
if (cd "$r" && "$script" nowhere >/dev/null 2>&1); then
  echo "FAIL an unknown base: passed"
  failures=$((failures + 1))
else
  echo "ok   an unknown base is an error, not a pass"
fi

[[ $failures -eq 0 ]] || { echo "$failures failed"; exit 1; }
echo "all passed"
