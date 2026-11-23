#!/bin/sh
# Prove that the control plane image can build a workload: its `heph` runs, HEPH_BIN names it,
# and `heph workload` makes a workload for a fixture ticket from what the image carries
# (no checkout, no token, no model key). It also proves the image's own `gh` is new enough for
# what the control plane asks of it.
#
#   control-plane/image-test/check.sh IMAGE
set -eu
image=${1:?usage: check.sh IMAGE}
here=$(cd "$(dirname "$0")" && pwd)

bin=$(docker run --rm --entrypoint sh "$image" -c 'echo "$HEPH_BIN"')
[ -n "$bin" ] || { echo "the image sets no HEPH_BIN" >&2; exit 1; }
echo "HEPH_BIN=$bin"

docker run --rm --entrypoint "$bin" "$image" --version

# The real `gh` of the image, with the image's own PATH (the stand-in below is only for the
# workload). The control plane runs `gh api --paginate --slurp`, and `--slurp` came in gh 2.48.0
# (cli/cli#8620). Keep this minimum in step with what the code asks of gh.
min_gh=2.48.0
real=$(docker run --rm --entrypoint sh "$image" -c 'command -v gh') || real=
case "$real" in
  /stub/*|"") echo "the image has no real gh on its PATH (found: ${real:-none})" >&2; exit 1 ;;
esac
have=$(docker run --rm --entrypoint "$real" "$image" --version | sed -n '1s/^gh version \([0-9][0-9.]*\).*/\1/p')
[ -n "$have" ] || { echo "could not read the version of gh in the image" >&2; exit 1; }
echo "gh $have at $real"
[ "$(printf '%s\n%s\n' "$min_gh" "$have" | sort -V | head -n1)" = "$min_gh" ] \
  || { echo "gh $have is older than $min_gh, which the control plane needs" >&2; exit 1; }
docker run --rm --entrypoint "$real" "$image" api --help | grep -q -e '--slurp' \
  || { echo "gh api in the image does not list --slurp" >&2; exit 1; }
echo "gh $have lists --slurp"

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
