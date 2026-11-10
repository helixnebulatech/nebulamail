#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

"""Fail if a crate in Cargo.lock was published less than N days ago.

Cargo has no `minimumReleaseAge` like Bun, so this checks it:
a just-published version may be a compromised release that nobody noticed yet.

    python3 scripts/check-release-age.py [--days 2] [Cargo.lock]

Fix a hit with `cargo update -p <crate> --precise <older version>`.
Asks crates.io once per crate (about 1 request per second, their policy) and
remembers answers in ~/.cache/nebula-release-age.json: a published version's
date never changes.
"""

import argparse
import datetime
import json
import pathlib
import sys
import time
import tomllib
import urllib.request

# Versions allowed before their 2 days, each with the reason. Only for fixes
# of a published security advisory: a known hole is worse than the small risk
# of a fresh release.
EXCEPTIONS = {
    "ctutils@0.4.4": "fixes RUSTSEC-2026-0385 (non-constant-time masks)",
}

CACHE = pathlib.Path.home() / ".cache" / "nebula-release-age.json"
USER_AGENT = "nebula-release-age-check (https://github.com/helixnebulatech/nebulamail)"


def published(name: str, version: str, cache: dict[str, str]) -> datetime.datetime:
    key = f"{name}@{version}"
    if key not in cache:
        url = f"https://crates.io/api/v1/crates/{name}/{version}"
        request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(request, timeout=30) as response:
            cache[key] = json.load(response)["version"]["created_at"]
        time.sleep(1)
    return datetime.datetime.fromisoformat(cache[key].replace("Z", "+00:00"))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--days", type=float, default=2)
    parser.add_argument("lockfile", nargs="?", default="Cargo.lock")
    args = parser.parse_args()

    lock = tomllib.loads(pathlib.Path(args.lockfile).read_text())
    cache = json.loads(CACHE.read_text()) if CACHE.exists() else {}
    limit = datetime.datetime.now(datetime.UTC) - datetime.timedelta(days=args.days)
    too_new = []
    try:
        for package in lock["package"]:
            if not package.get("source", "").startswith("registry+"):
                continue
            if f"{package['name']}@{package['version']}" in EXCEPTIONS:
                continue
            date = published(package["name"], package["version"], cache)
            if date > limit:
                too_new.append(f"{package['name']} {package['version']} ({date:%Y-%m-%d %H:%M} UTC)")
    finally:
        CACHE.parent.mkdir(parents=True, exist_ok=True)
        CACHE.write_text(json.dumps(cache, indent=0, sort_keys=True))

    if too_new:
        print(f"Published less than {args.days:g} days ago:", *too_new, sep="\n  ")
        return 1
    print(f"All crates were published at least {args.days:g} days ago.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
