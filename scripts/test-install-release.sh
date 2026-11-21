#!/usr/bin/env bash
# Tests for install-release.sh against a stand-in release server: a directory read
# through file://, laid out as GitHub lays out releases. Nothing here reaches the network.
set -euo pipefail

script=$(cd "$(dirname "$0")" && pwd)/install-release.sh
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) platform=macos-arm64 ;;
  Linux-x86_64) platform=linux-x86_64 ;;
  *) echo "no release platform for this machine; skipping"; exit 0 ;;
esac
if command -v sha256sum >/dev/null 2>&1; then sum() { sha256sum "$1" | cut -d' ' -f1; }; else sum() { shasum -a 256 "$1" | cut -d' ' -f1; }; fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
failures=0

# release <tag> [skip-executor]: a release directory with a tarball and its SHA256SUMS.
release() {
  local tag=$1 dir=$scratch/base/download/$1 name=heph-$1-$platform
  mkdir -p "$dir" "$scratch/stage/$name"
  printf '#!/bin/sh\necho heph\n' >"$scratch/stage/$name/heph"
  printf '#!/bin/sh\necho executor\n' >"$scratch/stage/$name/executor"
  chmod +x "$scratch/stage/$name/"*
  [[ ${2:-} == skip-executor ]] && rm "$scratch/stage/$name/executor"
  tar -czf "$dir/$name.tar.gz" -C "$scratch/stage" "$name"
  rm -rf "$scratch/stage"
  printf '%s  %s\n' "$(sum "$dir/$name.tar.gz")" "$name.tar.gz" >"$dir/SHA256SUMS"
}

# run <root-name> <args...>: runs the installer, leaving output in $scratch/out and the status in $status.
run() {
  local root=$1
  shift
  set +e
  HEPH_RELEASE_BASE=file://$scratch/base HEPH_INSTALL_ROOT=$scratch/$root "$script" "$@" >"$scratch/out" 2>&1
  status=$?
  set -e
}

check() { # check <description> <condition status>
  if [[ $2 -eq 0 ]]; then echo "ok   $1"; else echo "FAIL $1"; sed 's/^/     /' "$scratch/out"; failures=$((failures + 1)); fi
}
holds() { grep -q -- "$1" "$scratch/out"; }
nothing_left() { [[ ! -e $scratch/$1 ]] || [[ -z $(ls -A "$scratch/$1") ]]; }

release v1.2.3
release v1.3.0
mkdir -p "$scratch/base" && echo v1.3.0 >"$scratch/base/latest-tag"

run good 1.2.3
check "a good download is unpacked and both binaries run" $([[ $status -eq 0 && -x $scratch/good/1.2.3/heph && $("$scratch/good/1.2.3/executor") == executor ]] && echo 0 || echo 1)
check "it names the two paths" $(holds "HEPH_BIN=$scratch/good/1.2.3/heph" && holds "HEPH_EXECUTOR=$scratch/good/1.2.3/executor" && echo 0 || echo 1)

run good --print-env v1.3.0
check "--print-env prints export lines and a v-prefixed version works" $(holds "^export HEPH_BIN=$scratch/good/1.3.0/heph$" && holds "^export HEPH_EXECUTOR=" && echo 0 || echo 1)
check "an earlier version is left in place" $([[ -x $scratch/good/1.2.3/heph ]] && echo 0 || echo 1)

run latest latest
check "latest resolves to the newest release" $([[ $status -eq 0 && -x $scratch/latest/1.3.0/heph ]] && echo 0 || echo 1)

run again 1.2.3
run again 1.2.3
check "installing again is a no-op that still names the paths" $([[ $status -eq 0 ]] && holds "HEPH_BIN=" && echo 0 || echo 1)

# A wrong checksum.
release v2.0.0
printf '%s  %s\n' "$(printf 0%.0s {1..64})" "heph-v2.0.0-$platform.tar.gz" >"$scratch/base/download/v2.0.0/SHA256SUMS"
run bad 2.0.0
check "a wrong checksum fails, says so, and leaves nothing" $([[ $status -ne 0 ]] && holds "does not match SHA256SUMS" && nothing_left bad && echo 0 || echo 1)

# A tarball swapped after SHA256SUMS was written.
release v2.1.0
echo tampered >>"$scratch/base/download/v2.1.0/heph-v2.1.0-$platform.tar.gz"
run swapped 2.1.0
check "a tarball changed after its checksum was written is refused" $([[ $status -ne 0 ]] && nothing_left swapped && echo 0 || echo 1)

# No checksum file.
release v3.0.0
rm "$scratch/base/download/v3.0.0/SHA256SUMS"
run nosums 3.0.0
check "a release with no SHA256SUMS fails and leaves nothing" $([[ $status -ne 0 ]] && holds "no SHA256SUMS" && nothing_left nosums && echo 0 || echo 1)

# No build for this platform: the sums list only another platform.
release v4.0.0
printf '%s  %s\n' "$(printf 1%.0s {1..64})" "heph-v4.0.0-plan9-mips.tar.gz" >"$scratch/base/download/v4.0.0/SHA256SUMS"
run noplatform 4.0.0
check "a release with no build for this platform fails and leaves nothing" $([[ $status -ne 0 ]] && holds "no $platform build" && nothing_left noplatform && echo 0 || echo 1)

# A version that does not exist.
run unknown 9.9.9
check "an unknown version fails and leaves nothing" $([[ $status -ne 0 ]] && holds "9.9.9" && nothing_left unknown && echo 0 || echo 1)

# A tarball without the executor, with a matching checksum.
release v5.0.0 skip-executor
run noexec 5.0.0
check "a tarball without the executor is refused and leaves nothing" $([[ $status -ne 0 ]] && holds "does not hold executor" && nothing_left noexec && echo 0 || echo 1)

run none
check "no version is a usage error" $([[ $status -ne 0 ]] && echo 0 || echo 1)

[[ $failures -eq 0 ]] || { echo "$failures failed"; exit 1; }
echo "all passed"
