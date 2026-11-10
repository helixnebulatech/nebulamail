# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# Tests only: Nextcloud with our post-install
# hook copied in, not bind-mounted, for the reason in ../radicale/Dockerfile
# (a container made from a deleted git worktree no longer starts on Docker
# Desktop). The image runs only executable hooks.
FROM docker.io/library/nextcloud:34.0.5-apache@sha256:393d46e80fc241374a1b69064408cf0d54476580d7a00eb74cae3f4f2b2bb8a0

COPY --chmod=755 post-install.sh /docker-entrypoint-hooks.d/post-installation/10-nebulamail.sh
