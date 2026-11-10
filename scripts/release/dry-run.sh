#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only
#
# The build steps of .github/workflows/release.yml on this machine, without
# signing or publishing (those need the workflow's GitHub identity): the
# static zip built twice (must be equal), the bridge for this machine's
# architecture in the pinned Rust Alpine image, the SBOMs and SHA256SUMS.
# Needs Bun and Docker. Writes into dist-release/ (ignored by git).
#
#   scripts/release/dry-run.sh 1.2.3
set -eu

version=${1:?usage: dry-run.sh <version>}
out=dist-release
rust_image=$(sed -n 's/^ *RUST_IMAGE: *//p' .github/workflows/release.yml)
rm -rf "$out"
mkdir -p "$out"

(
  cd apps/web
  export APP_URL=https://mail.example.com JMAP_SERVER=https://jmap.example.com \
    OAUTH_CLIENT_ID=release OAUTH_CLIENT_SECRET=release-placeholder-not-a-secret \
    SESSION_KEYS="1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
  NEBULA_STYLE_NONCE="nebula-mail-$(printf %s "$version" | tr . -)"
  export NEBULA_STYLE_NONCE
  bun run build:static
  first=$(cat dist-static/nebula-mail-static.zip.sha256)
  bun run build:static
  [ "$first" = "$(cat dist-static/nebula-mail-static.zip.sha256)" ] || {
    echo "the static build is not reproducible" >&2
    exit 1
  }
  cp dist-static/nebula-mail-static.zip "../../$out/nebula-mail-static-$version.zip"
)
scripts/release/sbom.sh "$version" "$out"
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
export SOURCE_DATE_EPOCH
docker run --rm -e SOURCE_DATE_EPOCH -v "$PWD:/src" -w /src "$rust_image" \
  sh -c "sh scripts/release/build-bridge.sh '$version' '$out' && chown -R $(id -u):$(id -g) '$out'"
(cd "$out" && sha256sum -- * >SHA256SUMS && cat SHA256SUMS)
