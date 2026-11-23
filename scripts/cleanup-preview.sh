#!/usr/bin/env bash
# Destroy what deploy-control-plane.sh made for a branch: its Fly app, then its Neon branch. The
# app goes first so that nothing is still connected when the database disappears. Safe to run
# again: what is already gone is said so and skipped.
#
#   scripts/cleanup-preview.sh --branch <branch>
#
# Needed: FLY_API_TOKEN NEON_STAGING_API_KEY NEON_STAGING_PROJECT_ID (PREVIEW_SUFFIX, if the deploy
# had one). A missing tool or secret is a failure, not a skip: a cleanup that deletes nothing and
# reports success is how previews leak.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=lib/preview-names.sh
source "$root/scripts/lib/preview-names.sh"
# shellcheck source=lib/neon.sh
source "$root/scripts/lib/neon.sh"

branch=""
while (($#)); do
  case $1 in
    --branch) branch=${2:-}; shift 2 ;;
    *) echo "usage: cleanup-preview.sh --branch <branch>" >&2; exit 2 ;;
  esac
done
[[ -n $branch ]] || { echo "usage: cleanup-preview.sh --branch <branch>" >&2; exit 2; }

command -v fly >/dev/null || {
  echo "FAIL cleanup: 'fly' is not on PATH; nothing was destroyed (a refusal, not a pass)" >&2
  exit 1
}
missing=()
for name in FLY_API_TOKEN NEON_STAGING_API_KEY NEON_STAGING_PROJECT_ID; do
  [[ -n ${!name:-} ]] || missing+=("$name")
done
if ((${#missing[@]})); then
  echo "FAIL cleanup: not set: ${missing[*]}; nothing was destroyed (a refusal, not a pass)" >&2
  exit 1
fi

derive_preview_names "$branch" || exit 1
echo "==> cleaning up the preview of ${branch}: ${PREVIEW_APP}, ${PREVIEW_NEON_BRANCH}"

if fly status --app "$PREVIEW_APP" >/dev/null 2>&1; then
  fly apps destroy "$PREVIEW_APP" --yes >/dev/null || { echo "FAIL cleanup: could not destroy ${PREVIEW_APP}" >&2; exit 1; }
  echo "    destroyed ${PREVIEW_APP}"
else
  echo "    ${PREVIEW_APP} not found (already gone)"
fi

id=$(neon_branch_id_by_name "$NEON_STAGING_API_KEY" "$NEON_STAGING_PROJECT_ID" "$PREVIEW_NEON_BRANCH") || {
  echo "FAIL cleanup: could not list the Neon branches" >&2
  exit 1
}
if [[ -n $id ]]; then
  neon_delete_branch "$NEON_STAGING_API_KEY" "$NEON_STAGING_PROJECT_ID" "$id" || {
    echo "FAIL cleanup: could not delete the Neon branch ${PREVIEW_NEON_BRANCH}" >&2
    exit 1
  }
  echo "    deleted the Neon branch ${PREVIEW_NEON_BRANCH}"
else
  echo "    Neon branch ${PREVIEW_NEON_BRANCH} not found (already gone)"
fi
echo "cleanup complete"
