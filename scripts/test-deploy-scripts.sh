#!/usr/bin/env bash
# Tests for the deploy, cleanup and sweep scripts, against stand-ins for `fly`, `curl` and `gh`
# that record what they were asked and answer from fixtures. Nothing here reaches Fly, Neon or
# GitHub. What this proves is what the scripts ask for and refuse; it does not prove that the
# real services answer as the fixtures do.
# The assertions read `$?` of the condition above them, and a failed one is reported, not fatal.
# shellcheck disable=SC2319,SC2251
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
failures=0

pass() { echo "ok   $1"; }
fail() {
  echo "FAIL $1" >&2
  failures=$((failures + 1))
}
# expect <description> <status>: the status is 0 when the thing held.
expect() {
  if [[ $2 -eq 0 ]]; then pass "$1"; else fail "$1"; fi
}

# ---- the stand-ins -------------------------------------------------------------------------
bin=$scratch/bin
mkdir -p "$bin"
export CALLS=$scratch/calls
export FIXTURES=$scratch/fixtures
mkdir -p "$FIXTURES"

cat >"$bin/fly" <<'STUB'
#!/usr/bin/env bash
echo "fly $*" >>"$CALLS"
case "$1 $2" in
  "status --app") grep -qxF "$3" "$FIXTURES/apps-existing" 2>/dev/null ;;
  "apps create") echo "$3" >>"$FIXTURES/apps-existing" ;;
  "apps list") cat "$FIXTURES/apps-list.json" ;;
  "machines list") cat "$FIXTURES/machines-$4.json" 2>/dev/null || echo '[]' ;;
  *) : ;;
esac
STUB
cat >"$bin/curl" <<'STUB'
#!/usr/bin/env bash
out="" method=GET write=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  case ${args[$i]} in
    -o) out=${args[$((i + 1))]} ;;
    -X) method=${args[$((i + 1))]} ;;
    -w) write=${args[$((i + 1))]} ;;
  esac
done
url=${args[$((${#args[@]} - 1))]}
echo "curl $method $url" >>"$CALLS"
status=200 body='{}'
case "$method $url" in
  "GET https://console.neon.tech/api/v2/projects/"*/branches) body=$(cat "$FIXTURES/branches.json") ;;
  "POST https://console.neon.tech/api/v2/projects/"*/branches) body='{"branch":{"id":"br-new","name":"x"}}' ;;
  "DELETE "*) body='{}' ;;
  *"connection_uri"*"pooled=true"*) body='{"uri":"postgresql://u:SECRETPW@ep-a-pooler.eu.neon.tech/neondb?sslmode=require"}' ;;
  *"connection_uri"*"pooled=false"*) body='{"uri":"postgresql://u:SECRETPW@ep-a.eu.neon.tech/neondb?sslmode=require"}' ;;
  "GET https://"*"/api/session") status=${SESSION_STATUS:-401} ;;
esac
if [[ -n $out && $out != /dev/null ]]; then printf '%s' "$body" >"$out"; elif [[ -z $out ]]; then printf '%s' "$body"; fi
[[ $write == *http_code* ]] && printf '%s' "$status"
exit 0
STUB
cat >"$bin/gh" <<'STUB'
#!/usr/bin/env bash
echo "gh $*" >>"$CALLS"
cat "$FIXTURES/open-prs.txt"
STUB
printf '#!/usr/bin/env bash\nexit 0\n' >"$bin/sleep"
chmod +x "$bin"/*

reset() {
  : >"$CALLS"
  find "${FIXTURES:?}" -type f -delete
  : >"$FIXTURES/apps-existing"
  echo '{"branches":[{"id":"br-parent","name":"production","default":true,"created_at":"2026-01-01T00:00:00Z"}]}' >"$FIXTURES/branches.json"
}

# run <script> <args...>: its output is in $scratch/out and its status in $status.
run() {
  local script=$1
  shift
  PATH="$bin:$PATH" "$here/$script" "$@" >"$scratch/out" 2>&1
  status=$?
}

export FLY_API_TOKEN=flytok HEPH_API_SECRET_KEY_BASE=sessionkey-value HEPH_API_GITHUB_CLIENT_ID=cid
export HEPH_API_GITHUB_CLIENT_SECRET=csecret-value
export NEON_STAGING_API_KEY=nstag NEON_STAGING_PROJECT_ID=proj-stag
export NEON_PRODUCTION_API_KEY=nprod NEON_PRODUCTION_PROJECT_ID=proj-prod
export HEPH_WEBHOOK_SECRET=hooksecret-value CONTROL_PLANE_GH_TOKEN=ghtoken-value
export HEPH_API_ALLOWED_LOGINS=octocat HEPH_REPOSITORIES=o/r DEPLOY_CHECK_SLEEP=0
unset PREVIEW_SUFFIX HEPH_REVIEW_APP_ID HEPH_REVIEW_APP_KEY FLY_APP HEPH_API_BASE_URL SESSION_STATUS

# ---- names ---------------------------------------------------------------------------------
# shellcheck source=lib/preview-names.sh
source "$here/lib/preview-names.sh"
derive_preview_names "Feature/Some_Long_Branch-Name-Exceeding-Thirty"
expect "a preview app name fits Fly's 30 characters" $((${#PREVIEW_APP} <= 30 ? 0 : 1))
[[ $PREVIEW_APP == heph-cp-pr-* && $PREVIEW_NEON_BRANCH == preview/* ]]
expect "the names carry the preview prefixes" $?
derive_preview_names "chore/x"
[[ $PREVIEW_APP == heph-cp-pr-chore-x ]]
expect "a branch is lower-cased and its slashes become dashes" $?
PREVIEW_SUFFIX=ci123456 derive_preview_names "feature/a-very-long-branch-name"
expect "a run suffix still fits" $((${#PREVIEW_APP} <= 30 ? 0 : 1))
[[ $PREVIEW_APP == *-ci123456 ]]
expect "a run suffix ends the name" $?
unset PREVIEW_SUFFIX
(derive_preview_names "///" 2>/dev/null) && r=0 || r=1
expect "a branch with nothing to name a preview by is refused" $((r == 1 ? 0 : 1))

# ---- deploy: refusals happen before anything is touched -------------------------------------
reset
(
  unset HEPH_API_SECRET_KEY_BASE
  run deploy-control-plane.sh preview --branch feat/x
  [[ $status -ne 0 ]] && grep -q "HEPH_API_SECRET_KEY_BASE" "$scratch/out" && ! grep -q . "$CALLS"
)
expect "a missing secret stops the deploy, naming it, before fly or neon is called" $?

reset
(
  unset HEPH_WEBHOOK_SECRET
  run deploy-control-plane.sh production
  [[ $status -ne 0 ]] && grep -q "HEPH_WEBHOOK_SECRET" "$scratch/out" && ! grep -q . "$CALLS"
)
expect "production refuses without the webhook secret, before anything is called" $?

reset
(
  export FLY_APP=heph-cp-pr-feat
  run deploy-control-plane.sh production
  [[ $status -ne 0 ]] && grep -q "preview's name" "$scratch/out"
)
expect "production refuses to deploy onto a preview's name" $?

reset
run deploy-control-plane.sh production
[[ $status -ne 0 ]] && grep -q "does not exist" "$scratch/out" && ! grep -q "apps create" "$CALLS"
expect "production does not create its own app" $?

# ---- deploy: a preview ---------------------------------------------------------------------
reset
run deploy-control-plane.sh preview --branch feat/x
[[ $status -eq 0 ]]
expect "a preview deploys" $?
grep -q "^fly apps create heph-cp-pr-feat-x --org personal" "$CALLS"
expect "a preview's Fly app is made" $?
grep -q "^curl POST .*/projects/proj-stag/branches" "$CALLS"
expect "its Neon branch is made in the staging project" $?
grep -q "^fly secrets set --stage --app heph-cp-pr-feat-x DATABASE_URL=postgresql://u:SECRETPW@ep-a-pooler.eu.neon.tech/neondb HEPH_MIGRATION_DATABASE_URL=postgresql://u:SECRETPW@ep-a.eu.neon.tech/neondb " "$CALLS"
expect "the pooled string is DATABASE_URL and the direct one is the migration's, query strings dropped" $?
! grep "secrets set" "$CALLS" | grep -q "HEPH_WEBHOOK_SECRET\|GH_TOKEN\|HEPH_REPOSITORIES"
expect "a preview is given no webhook secret, token or repositories" $?
grep -q "^fly deploy --config control-plane/fly.toml --app heph-cp-pr-feat-x --ha=false --yes --env HEPH_API_BASE_URL=https://heph-cp-pr-feat-x.fly.dev" "$CALLS"
expect "it deploys the one file with its own address" $?
! grep -q "SECRETPW\|sessionkey-value\|csecret-value" "$scratch/out"
expect "no secret is printed" $?

reset
echo '{"branches":[{"id":"br-parent","name":"production","default":true,"created_at":"2026-01-01T00:00:00Z"},{"id":"br-old","name":"preview/feat-x","created_at":"2026-01-02T00:00:00Z"}]}' >"$FIXTURES/branches.json"
echo heph-cp-pr-feat-x >>"$FIXTURES/apps-existing"
run deploy-control-plane.sh preview --branch feat/x
grep -q "^curl DELETE .*/branches/br-old" "$CALLS" && ! grep -q "apps create" "$CALLS"
expect "a second deploy replaces the earlier Neon branch and reuses the Fly app" $?

reset
echo '{"branches":[{"id":"br-x","name":"other"}]}' >"$FIXTURES/branches.json"
run deploy-control-plane.sh preview --branch feat/x
[[ $status -ne 0 ]] && grep -q "parent branch 'production' does not exist" "$scratch/out" && ! grep -q "secrets set" "$CALLS"
expect "a missing parent branch stops the deploy before any secret is set" $?

reset
SESSION_STATUS=200 run deploy-control-plane.sh preview --branch feat/x
[[ $status -ne 0 ]] && grep -q "not 401" "$scratch/out"
expect "a deployed API that answers a session-less request with 200 fails the deploy" $?

# ---- deploy: production ---------------------------------------------------------------------
reset
echo hephaestium-control-plane >>"$FIXTURES/apps-existing"
run deploy-control-plane.sh production
[[ $status -eq 0 ]]
expect "production deploys" $?
secrets_line=$(grep "secrets set" "$CALLS")
for name in DATABASE_URL HEPH_MIGRATION_DATABASE_URL HEPH_API_SECRET_KEY_BASE HEPH_API_GITHUB_CLIENT_ID \
  HEPH_API_GITHUB_CLIENT_SECRET HEPH_WEBHOOK_SECRET GH_TOKEN HEPH_API_ALLOWED_LOGINS HEPH_REPOSITORIES; do
  [[ $secrets_line == *" ${name}="* ]]
  expect "production sets ${name}" $?
done
grep -q "^curl GET .*/projects/proj-prod/branches" "$CALLS" && ! grep -q "^curl POST" "$CALLS"
expect "production reads the default branch of the production project and makes none" $?
grep -q "^fly deploy --config control-plane/fly.toml --app hephaestium-control-plane --ha=false --yes --env HEPH_API_BASE_URL=https://hephaestium-control-plane.fly.dev" "$CALLS"
expect "production deploys the one file" $?

# ---- cleanup --------------------------------------------------------------------------------
reset
echo '{"branches":[{"id":"br-parent","name":"production"},{"id":"br-p","name":"preview/feat-x"}]}' >"$FIXTURES/branches.json"
echo heph-cp-pr-feat-x >>"$FIXTURES/apps-existing"
run cleanup-preview.sh --branch feat/x
[[ $status -eq 0 ]]
expect "cleanup succeeds" $?
destroy_line=$(grep -n "fly apps destroy heph-cp-pr-feat-x --yes" "$CALLS" | cut -d: -f1)
delete_line=$(grep -n "curl DELETE .*/branches/br-p$" "$CALLS" | cut -d: -f1)
[[ -n $destroy_line && -n $delete_line && $destroy_line -lt $delete_line ]]
expect "the Fly app is destroyed before the Neon branch is deleted" $?
! grep DELETE "$CALLS" | grep -q "br-parent"
expect "the parent branch is not touched" $?

reset
run cleanup-preview.sh --branch feat/x
[[ $status -eq 0 ]] && grep -q "already gone" "$scratch/out"
expect "cleanup of what is already gone succeeds and says so" $?

reset
(
  unset NEON_STAGING_API_KEY
  run cleanup-preview.sh --branch feat/x
  [[ $status -ne 0 ]] && ! grep -q "destroy" "$CALLS"
)
expect "cleanup with a missing secret fails and destroys nothing" $?

(PATH="/usr/bin:/bin" "$here/cleanup-preview.sh" --branch feat/x >"$scratch/out" 2>&1) && r=0 || r=1
expect "cleanup with no fly on the path fails rather than skips" $((r == 1 ? 0 : 1))

# ---- sweep ----------------------------------------------------------------------------------
old=2020-01-01T00:00:00Z
young=$(date -u -v-1H +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%SZ)
reset
echo feat-open >"$FIXTURES/open-prs.txt"
echo '[{"Name":"hephaestium-control-plane"},{"Name":"heph-cp-pr-feat-open"},{"Name":"heph-cp-pr-gone"},{"Name":"heph-cp-pr-fresh"}]' >"$FIXTURES/apps-list.json"
echo "[{\"created_at\":\"$old\"}]" >"$FIXTURES/machines-heph-cp-pr-feat-open.json"
echo "[{\"created_at\":\"$old\"}]" >"$FIXTURES/machines-heph-cp-pr-gone.json"
echo "[{\"created_at\":\"$young\"}]" >"$FIXTURES/machines-heph-cp-pr-fresh.json"
echo "{\"branches\":[{\"id\":\"br-parent\",\"name\":\"production\",\"created_at\":\"$old\"},{\"id\":\"br-o\",\"name\":\"preview/feat-open\",\"created_at\":\"$old\"},{\"id\":\"br-g\",\"name\":\"preview/gone\",\"created_at\":\"$old\"},{\"id\":\"br-f\",\"name\":\"preview/fresh\",\"created_at\":\"$young\"}]}" >"$FIXTURES/branches.json"
run sweep-preview-orphans.sh --dry-run
[[ $status -eq 0 ]] && grep -q "WOULD DESTROY heph-cp-pr-gone" "$scratch/out" && ! grep -q "apps destroy" "$CALLS" && ! grep -q "DELETE" "$CALLS"
expect "a dry run says what would go and destroys nothing" $?
run sweep-preview-orphans.sh
grep -q "fly apps destroy heph-cp-pr-gone --yes" "$CALLS" && grep -q "DELETE .*/branches/br-g" "$CALLS"
expect "an app and a branch no open pull request owns are destroyed" $?
! grep -q "destroy heph-cp-pr-feat-open\|destroy heph-cp-pr-fresh\|destroy hephaestium-control-plane" "$CALLS" &&
  ! grep -q "DELETE .*/branches/br-o\|DELETE .*/branches/br-f\|DELETE .*/branches/br-parent" "$CALLS"
expect "an open pull request's, a young one's, production's and the parent are kept" $?

reset
echo "[]" >"$FIXTURES/apps-list.json"
: >"$FIXTURES/open-prs.txt"
run sweep-preview-orphans.sh
[[ $status -ne 0 ]] && grep -q "empty or unreadable" "$scratch/out"
expect "a sweep over an empty listing fails instead of reporting a clean pass" $?

if ((failures)); then
  echo "${failures} failed" >&2
  exit 1
fi
echo "all passed"
