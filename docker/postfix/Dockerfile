# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# Tests only: a submission server that announces DSN
# (RFC 3461), which neither Stalwart nor Dovecot's submission do, so
# the bridge's own ENVID and the matching of real bounces can be tested.
# See entrypoint.sh for what it accepts and where mail goes. msmtp hands
# the mail to Dovecot's LMTP from Postfix's pipe(8) agent, under its own
# unprivileged user.
FROM docker.io/library/alpine:3.22.7

RUN apk add --no-cache 'postfix~3.10' 'cyrus-sasl~2.1' cyrus-sasl-login 'openssl~3.5' 'msmtp~1.8' \
  && adduser -S -D -H -h /nonexistent -s /sbin/nologin lmtp

COPY entrypoint.sh /usr/local/bin/entrypoint.sh

EXPOSE 465 587 4465
ENTRYPOINT ["/bin/sh", "/usr/local/bin/entrypoint.sh"]
