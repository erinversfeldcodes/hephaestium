#!/usr/bin/env bash
# The few Neon API calls the deploy, the teardown and the sweep make. Each takes the project and
# the key as arguments and prints what it found; none prints a key or a connection string except
# the one function whose job is to return one, and that one prints only to its caller.

NEON_API=${NEON_API:-https://console.neon.tech/api/v2}

# neon_get <key> <path>: the JSON answer, or a failure with the HTTP status on standard error.
neon_get() {
  local key=$1 path=$2 body status
  body=$(mktemp)
  status=$(curl -sSL --retry 3 --retry-all-errors -o "$body" -w '%{http_code}' \
    -H "Authorization: Bearer ${key}" "${NEON_API}${path}") || {
    rm -f "$body"
    echo "FAIL neon: could not reach the API for ${path%%\?*}" >&2
    return 1
  }
  if [[ $status != 200 ]]; then
    rm -f "$body"
    echo "FAIL neon: ${path%%\?*} answered HTTP ${status}" >&2
    return 1
  fi
  cat "$body"
  rm -f "$body"
}

# neon_branch_field <json> <name|id|default> <value-of-selector>: see the callers below.
_neon_branches_py='
import json, sys
mode, wanted = sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else ""
branches = json.load(sys.stdin).get("branches")
if not isinstance(branches, list):
    sys.exit(3)
for b in branches:
    if mode == "name" and b.get("name") == wanted:
        print(b["id"]); break
    if mode == "default" and b.get("default"):
        print(b["id"]); break
'

# neon_branch_id_by_name <key> <project> <name>: the id, or nothing when there is no such branch.
neon_branch_id_by_name() {
  local json
  json=$(neon_get "$1" "/projects/$2/branches") || return 1
  printf '%s' "$json" | python3 -c "$_neon_branches_py" name "$3"
}

# neon_default_branch_id <key> <project>
neon_default_branch_id() {
  local json
  json=$(neon_get "$1" "/projects/$2/branches") || return 1
  printf '%s' "$json" | python3 -c "$_neon_branches_py" default
}

# neon_create_branch <key> <project> <name> <parent-id>: the new branch's id. A read-write
# endpoint is made with it, which is what a connection string needs.
neon_create_branch() {
  local key=$1 project=$2 name=$3 parent=$4 response
  response=$(curl -sSL -X POST -H "Authorization: Bearer ${key}" -H 'Content-Type: application/json' \
    -d "{\"branch\":{\"name\":\"${name}\",\"parent_id\":\"${parent}\"},\"endpoints\":[{\"type\":\"read_write\"}]}" \
    "${NEON_API}/projects/${project}/branches") || return 1
  printf '%s' "$response" | python3 -c '
import json, sys
d = json.load(sys.stdin)
b = d.get("branch") or {}
if not b.get("id"):
    sys.exit(1)
print(b["id"])'
}

# neon_delete_branch <key> <project> <id>
neon_delete_branch() {
  curl -sSfL -X DELETE -H "Authorization: Bearer ${1}" "${NEON_API}/projects/${2}/branches/${3}" >/dev/null
}

# neon_connection_uri <key> <project> <branch-id> <pooled|direct> [database] [role]: a connection
# string with its password, without the query string. The control plane turns TLS on itself
# (HEPH_DB_SSL), so the options Neon adds are dropped.
neon_connection_uri() {
  local key=$1 project=$2 branch=$3 kind=$4 database=${5:-neondb} role=${6:-neondb_owner} pooled=false json
  [[ $kind == pooled ]] && pooled=true
  json=$(neon_get "$key" "/projects/${project}/connection_uri?branch_id=${branch}&database_name=${database}&role_name=${role}&pooled=${pooled}") || return 1
  printf '%s' "$json" | python3 -c '
import json, sys
uri = json.load(sys.stdin).get("uri", "")
if not uri.startswith(("postgres://", "postgresql://")):
    sys.exit(1)
print(uri.split("?", 1)[0])'
}
