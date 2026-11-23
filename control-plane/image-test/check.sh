#!/bin/sh
# Prove that the control plane image can build a workload: its `heph` runs, HEPH_BIN names it,
# and `heph workload` makes a workload for a fixture ticket from what the image carries
# (no checkout, no token, no model key).
#
#   control-plane/image-test/check.sh IMAGE
set -eu
image=${1:?usage: check.sh IMAGE}
here=$(cd "$(dirname "$0")" && pwd)

bin=$(docker run --rm --entrypoint sh "$image" -c 'echo "$HEPH_BIN"')
[ -n "$bin" ] || { echo "the image sets no HEPH_BIN" >&2; exit 1; }
echo "HEPH_BIN=$bin"

docker run --rm --entrypoint "$bin" "$image" --version

out=$(docker run --rm --entrypoint sh --volume "$here:/stub:ro" --env PATH="/stub:/usr/local/bin:/usr/bin:/bin" "$image" \
  -c "\"\$HEPH_BIN\" workload 7 --repo example/repo --approver octocat")
echo "$out"
for want in '"number": "7"' '"title": "A fixture ticket"' '"name": "repo"' '"approvedBy": "octocat"' \
  '"baseCommit": "0123456789abcdef0123456789abcdef01234567"'; do
  case "$out" in
    *"$want"*) ;;
    *) echo "the workload lacks $want" >&2; exit 1 ;;
  esac
done
echo "the image builds a workload"
