# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# Tests only: Radicale with our config and entrypoint copied
# in, not bind-mounted. A bind mount of a single file ties the container to
# the checkout that created it: once that checkout (a git worktree) is gone,
# Docker Desktop on WSL2 makes the missing paths empty directories when the
# container starts again, and runc refuses to mount a directory onto a file
# ("not a directory"). Copied files don't depend on any checkout.
FROM ghcr.io/kozea/radicale:3.8.3@sha256:62e281f4b35f1a5e52209cd99ad7081add25532f4c10df225b3a1519e64f70ca

COPY config /etc/radicale/config
COPY --chmod=755 entrypoint.sh /etc/radicale/entrypoint.sh

ENTRYPOINT ["/bin/sh", "/etc/radicale/entrypoint.sh"]
