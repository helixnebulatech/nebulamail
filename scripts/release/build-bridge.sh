#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only
#
# Builds the bridge for a release: a
# static musl binary for the machine it runs on, packed with the example
# configuration and the license. Run inside the pinned Rust Alpine image
# (musl is its native C library), with the repository mounted at /src:
#
#   docker run --rm -v "$PWD:/src" -w /src "$RUST_IMAGE" \
#     sh scripts/release/build-bridge.sh 1.2.3 dist-release
#
# The release workflow (.github/workflows/release.yml) runs exactly this on
# an x86_64 and an aarch64 runner. Writes into <out-dir>:
#   nebula-bridge-<version>-<arch>-linux-musl.tar.gz
# (<out-dir> is relative to the repository.)
set -eu

version=${1:?usage: build-bridge.sh <version> <out-dir>}
out=${2:?usage: build-bridge.sh <version> <out-dir>}

case $(uname -m) in
  x86_64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) echo "unsupported machine: $(uname -m)" >&2; exit 1 ;;
esac
target="$arch-unknown-linux-musl"
name="nebula-bridge-$version-$arch-linux-musl"

# C compilers for aws-lc-rs and the bundled SQLite; GNU tar for a tarball
# that is the same on every build.
apk add --no-cache musl-dev gcc make cmake perl binutils tar gzip >/dev/null

src=$(pwd)
export CARGO_HOME=/tmp/cargo-home
export CARGO_TARGET_DIR=/tmp/cargo-target
# The same commit gives the same bytes: no build paths in the binary, and
# the commit's time in the tarball.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$src" log -1 --format=%ct 2>/dev/null || echo 0)}"
export RUSTFLAGS="--remap-path-prefix=$src=/src --remap-path-prefix=$CARGO_HOME=/cargo -C target-feature=+crt-static"
jobs="${CARGO_BUILD_JOBS:-4}"

cd "$src/bridge"
cargo build --release --locked --jobs "$jobs" --target "$target" -p nebula-bridge

binary="$CARGO_TARGET_DIR/$target/release/nebula-bridge"
# Static: no dynamic loader, so it runs on any Linux of that architecture.
if readelf -l "$binary" | grep -q "INTERP"; then
  echo "$binary is not static" >&2
  exit 1
fi
"$binary" --help >/dev/null

stage=$(mktemp -d)
mkdir -p "$stage/$name/LICENSES"
cp "$binary" "$stage/$name/nebula-bridge"
cp "$src/bridge/bridge.example.toml" "$stage/$name/"
cp "$src/LICENSES/AGPL-3.0-only.txt" "$stage/$name/LICENSES/"
mkdir -p "$src/$out"
tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
  -C "$stage" -cf - "$name" | gzip -n -9 >"$src/$out/$name.tar.gz"
rm -rf "$stage"
echo "$out/$name.tar.gz"
