#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# Writes the test users (alice, bob) with the Dovecot test password, then
# starts Radicale. The file lives in /tmp, never in the repository.
set -eu

: "${USER_PASSWORD:=password}"
umask 077
printf 'alice:%s\nbob:%s\n' "$USER_PASSWORD" "$USER_PASSWORD" > /tmp/radicale-users
exec /app/bin/python /app/bin/radicale --config /etc/radicale/config
