#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only
#
# Software bills of materials (CycloneDX JSON) of a release, made by syft
# (pinned image) from the lockfiles: `bun.lock` for the static zip (every
# package of the workspace, so a little more than what ends in the zip)
# and `bridge/Cargo.lock` for the bridge (test-only crates too). Writes
# into <out-dir>:
#   nebula-mail-static-<version>.cdx.json
#   nebula-bridge-<version>.cdx.json
#
#   scripts/release/sbom.sh 1.2.3 dist-release
set -eu

version=${1:?usage: sbom.sh <version> <out-dir>}
out=${2:?usage: sbom.sh <version> <out-dir>}
# anchore/syft v1.55.0 (2026-10-19), multi-arch index.
syft="anchore/syft:v1.55.0@sha256:4cc92b64ca74ffca4c94d4c8fcc01f217731375300be14d0b6eeb2e82eaa110a"

mkdir -p "$out"
docker run --rm -v "$PWD:/src:ro" "$syft" scan dir:/src \
  --exclude './bridge/**' --exclude './**/node_modules/**' --exclude './.github/**' \
  --source-name nebula-mail-static --source-version "$version" \
  -o cyclonedx-json >"$out/nebula-mail-static-$version.cdx.json"
docker run --rm -v "$PWD:/src:ro" "$syft" scan dir:/src/bridge \
  --exclude './target/**' \
  --source-name nebula-bridge --source-version "$version" \
  -o cyclonedx-json >"$out/nebula-bridge-$version.cdx.json"
