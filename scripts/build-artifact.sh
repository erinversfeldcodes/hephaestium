#!/usr/bin/env bash
# Build heph and the executor for one target, prove each reports the version being
# released, and package both into one tarball in dist/. The control plane runs both, so
# a release that held only one of them could not be run.
#
#   HEPHAESTIUM_VERSION=1.2.3 build-artifact.sh <rust-target> <platform>
#
# CI runs this on every pull request with a placeholder version, and the
# release workflow runs it with the real one. They share this script so that
# the release path is exercised before a release depends on it: a build that
# only ever runs at release time is first tested by failing to release.
set -euo pipefail

target=${1:?usage: build-artifact.sh <rust-target> <platform>}
platform=${2:?usage: build-artifact.sh <rust-target> <platform>}
version=${HEPHAESTIUM_VERSION:?HEPHAESTIUM_VERSION must name the version being built}

cd "$(git rev-parse --show-toplevel)"

rustup target add "$target"
binaries=(heph executor)

cargo build --release --locked --target "$target" -p heph -p executor

for binary in "${binaries[@]}"; do
  scripts/release.sh check-version "target/$target/release/$binary" "$version"
done

name=heph-v$version-$platform
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
mkdir "$staging/$name"
for binary in "${binaries[@]}"; do
  cp "target/$target/release/$binary" "$staging/$name/"
done
cp LICENSE README.md "$staging/$name/"

mkdir -p dist
tar -czf "dist/$name.tar.gz" -C "$staging" "$name"

# The tarball is what is shipped, so it is what is checked: both binaries are in it.
listing=$(tar -tzf "dist/$name.tar.gz")
for binary in "${binaries[@]}"; do
  if ! grep -qx "$name/$binary" <<<"$listing"; then
    echo "dist/$name.tar.gz does not hold $binary" >&2
    exit 1
  fi
done
echo "$listing"
echo "dist/$name.tar.gz"
