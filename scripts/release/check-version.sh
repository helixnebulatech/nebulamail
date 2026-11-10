#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only
#
# A release tag `v<version>` must name the version the code says it is:
# `version` in apps/web/package.json (the static zip's stalwart-app.json)
# and `[workspace.package] version` in bridge/Cargo.toml (the bridge).
# Prints the version.
#
#   scripts/release/check-version.sh v1.2.3
set -eu

tag=${1:?usage: check-version.sh v<version>}
version=${tag#v}
if [ "$version" = "$tag" ] || ! echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
  echo "the tag must be v<major>.<minor>.<patch>[-pre], not $tag" >&2
  exit 1
fi
web=$(sed -n 's/^  "version": "\(.*\)",*$/\1/p' apps/web/package.json)
bridge=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' bridge/Cargo.toml)
status=0
if [ "$web" != "$version" ]; then
  echo "apps/web/package.json has version \"$web\", the tag says $version" >&2
  status=1
fi
if [ "$bridge" != "$version" ]; then
  echo "bridge/Cargo.toml has version \"$bridge\", the tag says $version" >&2
  status=1
fi
[ "$status" -eq 0 ] && echo "$version"
exit "$status"
