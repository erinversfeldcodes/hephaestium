#!/usr/bin/env bash
# Install a named release of heph and the executor, checked against the release's SHA256SUMS.
#
#   install-release.sh [--print-env] <version|latest>
#
# The release is unpacked into ~/.local/share/hephaestium/<version>/, and any earlier
# version is left where it is. The two paths the control plane runs (HEPH_BIN and
# HEPH_EXECUTOR) are printed; with --print-env they are printed as `export` lines a shell
# can eval. Nothing is configured and nothing is restarted.
#
# This is a script and not a heph subcommand because it is how a machine gets heph in the
# first place, so it cannot need heph to run, and it needs nothing a base system lacks
# (curl, tar, and sha256sum or shasum).
#
# Settings, for tests and mirrors:
#   HEPH_RELEASE_BASE   where releases are served (default: this repository's GitHub releases).
#                       Release assets are read from <base>/download/<tag>/<file> and the
#                       newest tag from <base>/latest-tag, or from GitHub's redirect.
#   HEPH_INSTALL_ROOT   where versions are unpacked (default: ~/.local/share/hephaestium).
set -euo pipefail

repository=erinversfeldcodes/hephaestium
base=${HEPH_RELEASE_BASE:-https://github.com/$repository/releases}
root=${HEPH_INSTALL_ROOT:-$HOME/.local/share/hephaestium}

fail() {
  echo "install-release: $*" >&2
  exit 1
}

print_env=false
if [[ ${1:-} == --print-env ]]; then
  print_env=true
  shift
fi
requested=${1:?usage: install-release.sh [--print-env] <version|latest>}
[[ $# -eq 1 ]] || fail "usage: install-release.sh [--print-env] <version|latest>"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) platform=macos-arm64 ;;
  Linux-x86_64) platform=linux-x86_64 ;;
  *) fail "no release is built for $(uname -s) $(uname -m)" ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  fail "neither sha256sum nor shasum is installed, so a download cannot be checked"
fi

fetch() { curl --fail --silent --show-error --location "$1" --output "$2"; }

if [[ $requested == latest ]]; then
  if [[ -n ${HEPH_RELEASE_BASE:-} ]]; then
    tag=$(curl --fail --silent --show-error "$base/latest-tag") || fail "cannot read the newest release from $base"
  else
    final=$(curl --silent --location --output /dev/null --write-out '%{url_effective}' "$base/latest") \
      || fail "cannot reach GitHub to find the newest release"
    tag=${final##*/}
  fi
  [[ $tag == v* ]] || fail "the newest release is not named like a version: '$tag'"
else
  tag=v${requested#v}
fi
version=${tag#v}
name=heph-$tag-$platform
destination=$root/$version

announce() {
  if $print_env; then
    echo "export HEPH_BIN=$destination/heph"
    echo "export HEPH_EXECUTOR=$destination/executor"
  else
    echo "installed $tag in $destination"
    echo "set HEPH_BIN=$destination/heph"
    echo "set HEPH_EXECUTOR=$destination/executor"
  fi
}

if [[ -x $destination/heph && -x $destination/executor ]]; then
  # Installed by an earlier run, which only ever moves a checked release into place.
  announce
  exit 0
fi

[[ ! -e $destination ]] || fail "$destination exists but does not hold both binaries; remove it and run again"

mkdir -p "$root"
work=$(mktemp -d "$root/.install.XXXXXX")
trap 'rm -rf "$work"' EXIT

fetch "$base/download/$tag/SHA256SUMS" "$work/SHA256SUMS" \
  || fail "release $tag has no SHA256SUMS (or does not exist), so its download cannot be checked"
# A line is "<hash>  <file>": the file's name must match exactly, not as a suffix.
expected=$(awk -v f="$name.tar.gz" '$2 == f { print $1 }' "$work/SHA256SUMS")
[[ -n $expected ]] || fail "release $tag has no $platform build: SHA256SUMS does not list $name.tar.gz"

fetch "$base/download/$tag/$name.tar.gz" "$work/$name.tar.gz" \
  || fail "cannot download $name.tar.gz from release $tag"
actual=$(sha256 "$work/$name.tar.gz")
[[ $actual == "$expected" ]] || fail "$name.tar.gz does not match SHA256SUMS (expected $expected, got $actual); nothing was installed"

mkdir "$work/unpacked"
tar -xzf "$work/$name.tar.gz" -C "$work/unpacked"
for binary in heph executor; do
  [[ -f $work/unpacked/$name/$binary ]] || fail "$name.tar.gz does not hold $binary; nothing was installed"
done

mv "$work/unpacked/$name" "$destination"
announce
