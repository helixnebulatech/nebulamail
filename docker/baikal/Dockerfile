# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# Tests only (sharing): Baïkal with our config and init script
# copied in, not bind-mounted, for the reason in ../radicale/Dockerfile (a
# container made from a deleted git worktree no longer starts on Docker
# Desktop). init-db.sh copies the config into place on every start.
FROM docker.io/ckulka/baikal:0.10.1-nginx@sha256:434bdd162247cc6aa6f878c9b4dce6216e39e79526b980453b13812d5f8ebf4b

COPY baikal.yaml /etc/nebulamail/baikal.yaml
COPY --chmod=755 init-db.sh /docker-entrypoint.d/35-nebula-init-db.sh
