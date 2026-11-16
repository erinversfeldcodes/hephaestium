#!/usr/bin/env bash
# Conventional-commit release helper. The repository has one version, taken
# from the commits on the default branch since the last v<version> tag.
#
#   release.sh version   print the next version, or nothing if no commit
#                        since the last release warrants one
#   release.sh notes     print release notes for those commits
#   release.sh lint      check the commit subjects read on stdin
#
# A breaking change (`type!:` or a BREAKING CHANGE footer) bumps the major
# version, feat the minor, fix and perf the patch. Other types (docs, test,
# ci, ...) are recorded in the notes of the next release but do not cause one.
#
# The version is derived, never written down: there is no version file to
# keep in step with the tags, so the two cannot disagree.
set -euo pipefail

TYPES="feat|fix|perf|refactor|docs|test|build|ci|chore|style|revert"
SUBJECT_RE="^($TYPES)(\([a-z0-9-]+\))?(!)?: .+"
BREAKING_RE="^[a-z]+(\([a-z0-9-]+\))?!: "
FEAT_RE="^feat(\([a-z0-9-]+\))?: "
FIX_RE="^(fix|perf)(\([a-z0-9-]+\))?: "

last_tag() {
  git tag --list 'v[0-9]*' --sort=-v:refname | head -n1
}

# Commit messages since the last tag, NUL-separated.
messages() {
  local tag
  tag=$(last_tag)
  git log -z --no-merges --format=%B "${tag:+$tag..}HEAD"
}

# 3 major, 2 minor, 1 patch, 0 no release.
bump_of() {
  local subject=${1%%$'\n'*}
  if [[ $subject =~ $BREAKING_RE ]] || grep -qE '^BREAKING[ -]CHANGE: ' <<<"$1"; then
    echo 3
  elif [[ $subject =~ $FEAT_RE ]]; then
    echo 2
  elif [[ $subject =~ $FIX_RE ]]; then
    echo 1
  else
    echo 0
  fi
}

version() {
  local bump=0 b msg tag current major minor patch
  while IFS= read -r -d '' msg; do
    b=$(bump_of "$msg")
    if ((b > bump)); then bump=$b; fi
  done < <(messages)
  ((bump > 0)) || return 0

  tag=$(last_tag)
  current=${tag#v}
  IFS=. read -r major minor patch <<<"${current:-0.0.0}"
  case $bump in
    3) echo "$((major + 1)).0.0" ;;
    2) echo "$major.$((minor + 1)).0" ;;
    1) echo "$major.$minor.$((patch + 1))" ;;
  esac
}

section() {
  local title=$1
  shift
  (($# > 0)) || return 0
  printf '## %s\n\n' "$title"
  printf -- '- %s\n' "$@"
  printf '\n'
}

notes() {
  local msg subject
  local -a breaking=() features=() fixes=() other=()
  while IFS= read -r -d '' msg; do
    subject=${msg%%$'\n'*}
    case $(bump_of "$msg") in
      3) breaking+=("$subject") ;;
      2) features+=("$subject") ;;
      1) fixes+=("$subject") ;;
      *) other+=("$subject") ;;
    esac
  done < <(messages)

  section "Breaking changes" "${breaking[@]+"${breaking[@]}"}"
  section "Features" "${features[@]+"${features[@]}"}"
  section "Fixes" "${fixes[@]+"${fixes[@]}"}"
  section "Other changes" "${other[@]+"${other[@]}"}"
}

# Every subject on stdin must be a conventional commit, because a subject
# that is not one is silently counted as "no release".
lint() {
  local subject status=0 seen=0
  while IFS= read -r subject; do
    [[ -n $subject ]] || continue
    seen=$((seen + 1))
    if ! [[ $subject =~ $SUBJECT_RE ]]; then
      echo "not a conventional commit: $subject" >&2
      status=1
    fi
  done
  if ((seen == 0)); then
    echo "no commit subjects were given to check" >&2
    return 1
  fi
  return $status
}

case ${1:-} in
  version) version ;;
  notes) notes ;;
  lint) lint ;;
  *)
    echo "usage: release.sh version|notes|lint" >&2
    exit 2
    ;;
esac
