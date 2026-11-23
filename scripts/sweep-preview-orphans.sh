#!/usr/bin/env bash
# Destroy the previews no open pull request owns: the backstop for a close that never ran the
# cleanup, a crashed run, or a deploy made by hand.
#
#   scripts/sweep-preview-orphans.sh [--dry-run]
#
# An orphan is a Fly app named like a preview, or a Neon branch under `preview/` of the staging
# project, that is not the name of any open pull request's branch and is older than the grace
# period (GRACE_HOURS, default 24), which protects a run that is still deploying. A listing that
# cannot be read, or that comes back empty of anything it should hold, is a failure: a sweep that
# sweeps nothing and says so cheerfully is how the last leak survived.
#
# Needed: FLY_API_TOKEN NEON_STAGING_API_KEY NEON_STAGING_PROJECT_ID, and `gh` with GH_TOKEN
# able to list the repository's pull requests (GH_REPO, optional).
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=lib/preview-names.sh
source "$root/scripts/lib/preview-names.sh"
# shellcheck source=lib/neon.sh
source "$root/scripts/lib/neon.sh"

dry=0
[[ ${1:-} == --dry-run ]] && dry=1
grace=$((${GRACE_HOURS:-24} * 3600))
now=$(date -u +%s)

die() { echo "FAIL sweep: $*" >&2; exit 1; }

for tool in fly gh curl python3; do
  command -v "$tool" >/dev/null || die "'${tool}' is not on PATH; a refusal, not a pass"
done
for name in FLY_API_TOKEN NEON_STAGING_API_KEY NEON_STAGING_PROJECT_ID; do
  [[ -n ${!name:-} ]] || die "${name} is not set; a refusal, not a pass"
done

branches=$(gh pr list ${GH_REPO:+--repo "$GH_REPO"} --state open --limit 200 --json headRefName --jq '.[].headRefName') ||
  die "could not list the open pull requests"
allow=$(mktemp)
trap 'rm -f "$allow"' EXIT
while IFS= read -r b; do
  [[ -n $b ]] || continue
  PREVIEW_SUFFIX="" derive_preview_names "$b" || continue
  printf '%s\n%s\n' "$PREVIEW_APP" "$PREVIEW_NEON_BRANCH" >>"$allow"
done <<<"$branches"
echo "==> sweeping preview orphans (grace ${GRACE_HOURS:-24}h, dry run: ${dry}); kept for open pull requests: $(grep -c . "$allow" || true) names"

swept=0 kept=0 failed=0

# Fly: the apps, and the age of the newest machine of each.
apps=$(fly apps list --json | python3 -c '
import json, sys
apps = json.load(sys.stdin)
if not isinstance(apps, list) or not apps:
    sys.exit(3)
for a in apps:
    print(a.get("Name") or a.get("name") or "")') || die "could not read the list of Fly apps (an empty or unreadable list is a failure)"

app_epoch() {
  fly machines list --app "$1" --json 2>/dev/null | python3 -c '
import json, sys, datetime
try:
    machines = json.load(sys.stdin)
except Exception:
    machines = []
if not machines:
    print(0)
else:
    t = max(m["created_at"] for m in machines).split(".")[0].rstrip("Z")
    print(int(datetime.datetime.fromisoformat(t).replace(tzinfo=datetime.timezone.utc).timestamp()))'
}

while IFS= read -r app; do
  [[ $app =~ $PREVIEW_APP_PATTERN ]] || continue
  if grep -qxF "$app" "$allow"; then
    echo "    KEEP ${app} (open pull request)"; kept=$((kept + 1)); continue
  fi
  born=$(app_epoch "$app") || born=0
  if [[ $born == 0 ]] || ((now - born < grace)); then
    echo "    KEEP ${app} (younger than the grace period, or undated)"; kept=$((kept + 1)); continue
  fi
  if ((dry)); then
    echo "    WOULD DESTROY ${app}"; swept=$((swept + 1))
  elif fly apps destroy "$app" --yes >/dev/null; then
    echo "    DESTROYED ${app}"; swept=$((swept + 1))
  else
    echo "    FAILED to destroy ${app}" >&2; failed=$((failed + 1))
  fi
done <<<"$apps"

# Neon: the branches under preview/.
listing=$(neon_get "$NEON_STAGING_API_KEY" "/projects/${NEON_STAGING_PROJECT_ID}/branches") || die "could not list the Neon branches"
rows=$(printf '%s' "$listing" | python3 -c '
import json, sys, datetime
branches = json.load(sys.stdin).get("branches")
if not isinstance(branches, list) or not branches:
    sys.exit(3)
for b in branches:
    created = (b.get("created_at") or "").split(".")[0].rstrip("Z")
    try:
        epoch = int(datetime.datetime.fromisoformat(created).replace(tzinfo=datetime.timezone.utc).timestamp())
    except ValueError:
        epoch = 0
    print("{}\t{}\t{}".format(b.get("id", ""), b.get("name", ""), epoch))') || die "the Neon listing was empty or unreadable"

while IFS=$'\t' read -r id name born; do
  [[ $name == preview/* ]] || continue
  if grep -qxF "$name" "$allow"; then
    echo "    KEEP neon:${name} (open pull request)"; kept=$((kept + 1)); continue
  fi
  if [[ $born == 0 ]] || ((now - born < grace)); then
    echo "    KEEP neon:${name} (younger than the grace period, or undated)"; kept=$((kept + 1)); continue
  fi
  if ((dry)); then
    echo "    WOULD DELETE neon:${name}"; swept=$((swept + 1))
  elif neon_delete_branch "$NEON_STAGING_API_KEY" "$NEON_STAGING_PROJECT_ID" "$id"; then
    echo "    DELETED neon:${name}"; swept=$((swept + 1))
  else
    echo "    FAILED to delete neon:${name}" >&2; failed=$((failed + 1))
  fi
done <<<"$rows"

echo "swept ${swept}, kept ${kept}, failed ${failed}"
((failed == 0)) || exit 1
