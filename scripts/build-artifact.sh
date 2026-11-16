#!/usr/bin/env bash
# Build heph for one target, prove it reports the version being released, and
# package it into dist/.
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
cargo build --release --locked --target "$target" -p heph

binary=target/$target/release/heph
scripts/release.sh check-version "$binary" "$version"

name=heph-v$version-$platform
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
mkdir "$staging/$name"
cp "$binary" LICENSE README.md "$staging/$name/"

mkdir -p dist
tar -czf "dist/$name.tar.gz" -C "$staging" "$name"
echo "dist/$name.tar.gz"
