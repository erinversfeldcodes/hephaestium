#!/usr/bin/env bash
# Refuses a control-plane migration whose version clashes with another or is not later than the
# newest one on the base branch. Git sees two migrations with the same version as two files with
# different names and merges them without a conflict; Ecto then refuses to migrate, and a version
# chosen by hand is how two agents come to pick the same one. It reads files and git only: no
# database is needed.
#
#   check-migrations.sh [BASE_REF [MIGRATIONS_DIR]]
#
# Run from inside the repository, after the base branch has been brought in. BASE_REF defaults to
# origin/main. A migration is new when the base branch has no file of that name.
set -euo pipefail

base=${1:-origin/main}
dir=${2:-control-plane/priv/repo/migrations}
dir=${dir%/}

fail() {
  echo "check-migrations: $1" >&2
  exit 1
}

version_of() { # the digits before the first underscore of a file name
  local name=${1##*/}
  [[ $name =~ ^([0-9]+)_ ]] || fail "$1 has no version: a migration is named <version>_<name>.exs"
  echo "${BASH_REMATCH[1]}"
}

[[ -d $dir ]] || fail "$dir is not a directory"
git rev-parse --verify --quiet "$base^{commit}" >/dev/null ||
  fail "the base $base is not known here, so what is new cannot be told; fetch it first"

# Two migrations sharing a version, whether or not either is new.
seen=$(
  for file in "$dir"/*.exs; do
    [[ -e $file ]] || continue
    printf '%s %s\n' "$(version_of "$file")" "${file##*/}"
  done | sort
)
previous_version=
previous_file=
while read -r version file; do
  if [[ -n $version && $version == "$previous_version" ]]; then
    fail "two migrations share version $version: $previous_file, $file"
  fi
  previous_version=$version
  previous_file=$file
done <<<"$seen"

# A new migration must come after everything the base branch already has.
newest_version=
newest_file=
while read -r path; do
  [[ -n $path ]] || continue
  version=$(version_of "$path")
  if [[ -z $newest_version || $version > $newest_version ]]; then
    newest_version=$version
    newest_file=${path##*/}
  fi
done < <(git ls-tree --name-only "$base" -- "$dir/" | grep '\.exs$' || true)

if [[ -z $newest_version ]]; then
  echo "check-migrations: ok (the base has no migrations, and no version is shared)"
  exit 0
fi

checked=0
for file in "$dir"/*.exs; do
  [[ -e $file ]] || continue
  if git cat-file -e "$base:$file" 2>/dev/null; then
    continue
  fi
  checked=$((checked + 1))
  version=$(version_of "$file")
  # Versions are fixed-width timestamps, so a string compare is a time compare.
  if [[ ${#version} -ne ${#newest_version} ]]; then
    fail "${file##*/} has a version of a different length than $newest_file; generate one with mix ecto.gen.migration"
  fi
  if [[ ! $version > $newest_version ]]; then
    fail "migration ${file##*/} is not later than $newest_file, the newest on $base; generate a version with mix ecto.gen.migration"
  fi
done

echo "check-migrations: ok ($checked new, newest on $base is $newest_version)"
