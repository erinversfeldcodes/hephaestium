#!/usr/bin/env bash
# Deploy the control plane to Fly, against Neon.
#
#   scripts/deploy-control-plane.sh preview --branch <branch>
#   scripts/deploy-control-plane.sh production
#
# preview     makes (or reuses) the pull request's own Fly app and its own Neon branch, a copy of
#             the staging project's `production` branch, and deploys onto them. It holds no
#             webhook secret, no GitHub token and no repositories, so it takes no ticket in and
#             two control planes never work the same tickets.
# production  deploys the production Fly app against the production Neon project's default
#             branch, with everything the hosted control plane needs.
#
# Nothing is typed in by hand. The pooled and direct connection strings are asked of the Neon API
# and given to Fly as secrets on every deploy, so rotating a value means changing the GitHub
# Actions secret and deploying again. Every secret that is needed is checked before anything is
# created or changed, and the message names the variable, never its value.
#
# Needed in both:  FLY_API_TOKEN HEPH_API_SECRET_KEY_BASE HEPH_API_GITHUB_CLIENT_ID
#                  HEPH_API_GITHUB_CLIENT_SECRET
# preview:         NEON_STAGING_API_KEY NEON_STAGING_PROJECT_ID
#                  (NEON_PARENT_BRANCH, default production; PREVIEW_SUFFIX, optional)
# production:      NEON_PRODUCTION_API_KEY NEON_PRODUCTION_PROJECT_ID HEPH_WEBHOOK_SECRET
#                  CONTROL_PLANE_GH_TOKEN HEPH_API_ALLOWED_LOGINS HEPH_REPOSITORIES
#                  (HEPH_REVIEW_APP_ID and HEPH_REVIEW_APP_KEY together, optional;
#                   FLY_APP, default hephaestium-control-plane; HEPH_API_BASE_URL, default
#                   https://<app>.fly.dev; NEON_DATABASE and NEON_ROLE, default neondb and
#                   neondb_owner)
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=lib/preview-names.sh
source "$root/scripts/lib/preview-names.sh"
# shellcheck source=lib/neon.sh
source "$root/scripts/lib/neon.sh"

die() {
  echo "FAIL deploy: $*" >&2
  exit 1
}

usage() {
  echo "usage: deploy-control-plane.sh preview --branch <branch> | production" >&2
  exit 2
}

# require <variable>...: every one set and not empty, or the deploy stops before it touches anything.
require() {
  local missing=() name
  for name in "$@"; do
    [[ -n ${!name:-} ]] || missing+=("$name")
  done
  if ((${#missing[@]})); then
    die "not set: ${missing[*]}. Nothing was created or changed."
  fi
}

mode=${1:-}
[[ -n $mode ]] || usage
shift
branch=""
while (($#)); do
  case $1 in
    --branch) branch=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done

command -v fly >/dev/null || die "'fly' is not on PATH (the flyctl action installs 'flyctl'; alias it)"
command -v curl >/dev/null || die "'curl' is not on PATH"
command -v python3 >/dev/null || die "'python3' is not on PATH"

require FLY_API_TOKEN HEPH_API_SECRET_KEY_BASE HEPH_API_GITHUB_CLIENT_ID HEPH_API_GITHUB_CLIENT_SECRET

database=${NEON_DATABASE:-neondb}
role=${NEON_ROLE:-neondb_owner}
secrets=()

case $mode in
  preview)
    [[ -n $branch ]] || die "preview needs --branch"
    require NEON_STAGING_API_KEY NEON_STAGING_PROJECT_ID
    derive_preview_names "$branch" || exit 1
    app=$PREVIEW_APP
    neon_key=$NEON_STAGING_API_KEY
    neon_project=$NEON_STAGING_PROJECT_ID
    base_url="https://${app}.fly.dev"
    ;;
  production)
    require NEON_PRODUCTION_API_KEY NEON_PRODUCTION_PROJECT_ID HEPH_WEBHOOK_SECRET \
      CONTROL_PLANE_GH_TOKEN HEPH_API_ALLOWED_LOGINS HEPH_REPOSITORIES
    if [[ -n ${HEPH_REVIEW_APP_ID:-} || -n ${HEPH_REVIEW_APP_KEY:-} ]]; then
      require HEPH_REVIEW_APP_ID HEPH_REVIEW_APP_KEY
    fi
    app=${FLY_APP:-hephaestium-control-plane}
    if [[ $app == heph-cp-pr-* ]]; then
      die "FLY_APP '${app}' is a preview's name; production never deploys onto one"
    fi
    neon_key=$NEON_PRODUCTION_API_KEY
    neon_project=$NEON_PRODUCTION_PROJECT_ID
    base_url=${HEPH_API_BASE_URL:-"https://${app}.fly.dev"}
    ;;
  *) usage ;;
esac

echo "==> ${mode}: Fly app ${app}"

# The Fly app. A preview's is made here; production's is made once by hand, so that a mistyped
# name cannot make one.
if ! fly status --app "$app" >/dev/null 2>&1; then
  if [[ $mode == preview ]]; then
    echo "    creating ${app}"
    fly apps create "$app" --org "${FLY_ORG:-personal}" >/dev/null || die "could not create the Fly app ${app}"
  else
    die "the production Fly app ${app} does not exist: create it once with 'fly apps create ${app}'"
  fi
fi

# The Neon branch the database lives on.
if [[ $mode == preview ]]; then
  parent_name=${NEON_PARENT_BRANCH:-production}
  parent=$(neon_branch_id_by_name "$neon_key" "$neon_project" "$parent_name") || die "could not list the Neon branches"
  [[ -n $parent ]] || die "the Neon parent branch '${parent_name}' does not exist in project ${neon_project}"
  stale=$(neon_branch_id_by_name "$neon_key" "$neon_project" "$PREVIEW_NEON_BRANCH") || die "could not list the Neon branches"
  if [[ -n $stale ]]; then
    echo "    replacing the earlier Neon branch ${PREVIEW_NEON_BRANCH}"
    neon_delete_branch "$neon_key" "$neon_project" "$stale" || die "could not delete the earlier Neon branch"
  fi
  neon_branch=$(neon_create_branch "$neon_key" "$neon_project" "$PREVIEW_NEON_BRANCH" "$parent") ||
    die "could not create the Neon branch ${PREVIEW_NEON_BRANCH}"
  echo "    Neon branch ${PREVIEW_NEON_BRANCH}"
else
  neon_branch=$(neon_default_branch_id "$neon_key" "$neon_project") || die "could not list the Neon branches"
  [[ -n $neon_branch ]] || die "project ${neon_project} has no default branch"
  echo "    Neon default branch of project ${neon_project}"
fi

pooled=$(neon_connection_uri "$neon_key" "$neon_project" "$neon_branch" pooled "$database" "$role") ||
  die "could not get the pooled connection string"
direct=$(neon_connection_uri "$neon_key" "$neon_project" "$neon_branch" direct "$database" "$role") ||
  die "could not get the direct connection string"
[[ $pooled != "$direct" ]] || die "the pooled and the direct connection strings are the same: the pooler's address was not given"

secrets+=("DATABASE_URL=${pooled}" "HEPH_MIGRATION_DATABASE_URL=${direct}"
  "HEPH_API_SECRET_KEY_BASE=${HEPH_API_SECRET_KEY_BASE}"
  "HEPH_API_GITHUB_CLIENT_ID=${HEPH_API_GITHUB_CLIENT_ID}"
  "HEPH_API_GITHUB_CLIENT_SECRET=${HEPH_API_GITHUB_CLIENT_SECRET}")
if [[ $mode == production ]]; then
  secrets+=("HEPH_WEBHOOK_SECRET=${HEPH_WEBHOOK_SECRET}" "GH_TOKEN=${CONTROL_PLANE_GH_TOKEN}"
    "HEPH_API_ALLOWED_LOGINS=${HEPH_API_ALLOWED_LOGINS}" "HEPH_REPOSITORIES=${HEPH_REPOSITORIES}")
  if [[ -n ${HEPH_REVIEW_APP_ID:-} ]]; then
    secrets+=("HEPH_REVIEW_APP_ID=${HEPH_REVIEW_APP_ID}" "HEPH_REVIEW_APP_KEY=${HEPH_REVIEW_APP_KEY}")
  fi
fi

# Staged, so the machine is restarted once, by the deploy that follows, with all of them.
fly secrets set --stage --app "$app" "${secrets[@]}" >/dev/null || die "could not set the secrets on ${app}"
echo "    secrets set: $(printf '%s\n' "${secrets[@]}" | cut -d= -f1 | tr '\n' ' ')"

# A new app has no public address until one is allocated, and a deploy allocates one only the first
# time it runs and only if the config then has a service. An address is asked for here, once each,
# so the result does not depend on what an earlier deploy found.
addresses=$(fly ips list --app "$app" 2>/dev/null || true)
awk '$1 == "v4" { found = 1 } END { exit !found }' <<<"$addresses" ||
  fly ips allocate-v4 --shared --app "$app" --yes >/dev/null || die "could not allocate an IPv4 address for ${app}"
awk '$1 == "v6" { found = 1 } END { exit !found }' <<<"$addresses" ||
  fly ips allocate-v6 --app "$app" >/dev/null || die "could not allocate an IPv6 address for ${app}"

echo "==> deploying ${app}"
(cd "$root" && fly deploy --config control-plane/fly.toml --app "$app" --ha=false --yes \
  --env "HEPH_API_BASE_URL=${base_url}") || die "fly deploy failed for ${app}"

# What a person would check first: the dashboard's page is served, and the API refuses a request
# with no session.
echo "==> checking ${base_url}"
# The check goes to the address Fly allocated, not through DNS: a new app's name can take longer to
# resolve than the check is willing to wait, and the name is Fly's to serve, not this script's.
host=${base_url#https://}
host=${host%%/*}
v4=$(fly ips list --app "$app" 2>/dev/null | awk '$1 == "v4" { print $2; exit }')
resolve=()
[[ -z $v4 ]] || resolve=(--resolve "${host}:443:${v4}")
attempt=0
until [[ $(curl -s ${resolve[@]+"${resolve[@]}"} -o /dev/null -w '%{http_code}' "${base_url}/") == 200 ]]; do
  attempt=$((attempt + 1))
  ((attempt < 20)) || die "${base_url}/ did not answer 200"
  sleep "${DEPLOY_CHECK_SLEEP:-5}"
done
code=$(curl -s ${resolve[@]+"${resolve[@]}"} -o /dev/null -w '%{http_code}' "${base_url}/api/session")
[[ $code == 401 ]] || die "${base_url}/api/session answered ${code}, not 401"
echo "    / is 200 and /api/session is 401"
echo "deployed ${app}"
