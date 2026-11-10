#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# Runs from /docker-entrypoint.d/ before nginx starts: puts the config in
# place (Baïkal refuses a read-only one) and creates the SQLite database
# once, with the test users alice and bob (Dovecot's test password; Basic
# auth compares md5("user:realm:password")). 40-fix-baikal-file-permissions
# gives everything to nginx afterwards.
set -eu

cp /etc/nebulamail/baikal.yaml /var/www/baikal/config/baikal.yaml

: "${USER_PASSWORD:=password}"
db=/var/www/baikal/Specific/db/db.sqlite
mkdir -p "$(dirname "$db")"
if [ ! -s "$db" ]; then
    sqlite3 "$db" < /var/www/baikal/Core/Resources/Db/SQLite/db.sql
    for user in alice bob; do
        hash=$(printf '%s:BaikalDAV:%s' "$user" "$USER_PASSWORD" | md5sum | cut -d' ' -f1)
        sqlite3 "$db" "INSERT INTO users (username, digesta1) VALUES ('$user', '$hash');
            INSERT INTO principals (uri, email, displayname)
            VALUES ('principals/$user', '$user@nebulamail.test', '$user');"
    done
fi
