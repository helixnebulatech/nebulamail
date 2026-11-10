#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# Tests only. Postfix with:
# - submission on 465 (implicit TLS) and 587 (STARTTLS required), self-signed
#   certificate, SASL PLAIN/LOGIN for `alice` and `bob` with the Dovecot test
#   password (USER_PASSWORD); log in as `alice` or `alice@nebulamail.test`;
# - DSN announced (Postfix's default);
# - mail to @nebulamail.test delivered to the test Dovecot, into the users
#   `alice` and `bob` (the same mailboxes as the Dovecot tests); only alice
#   and bob exist, other recipients there are refused at RCPT. Postfix does
#   the final delivery itself (pipe(8) + msmtp to Dovecot's LMTP, see
#   below), so a success report says `delivered`, not `relayed`;
# - submission without 8BITMIME on 4465 (implicit TLS, the same login):
#   Postfix still takes 8-bit mail there, it only doesn't announce it, so
#   the bridge's 7-bit downgrade is tested with a real server
#
# - port 10025, not published, without login: only for mail to @nebulamail.test,
#   so Dovecot's Sieve can send vacation responses (docker/dovecot/nebula.conf;
#   it can't log in to submit);
# - mail to @bounce.nebulamail.test accepted and then bounced by Postfix, so a
#   real bounce (DSN) goes back to the sender.
# Nothing else is relayed.
set -eu

password="${USER_PASSWORD:-password}"
# The Dovecot it delivers to, on the compose network.
dovecot_host="${DOVECOT_HOST:-dovecot}"

# Self-signed certificate for both submission ports. The tests skip the
# chain check on loopback but lettre (STARTTLS) still checks the name, so
# it is for `localhost`.
if [ ! -f /etc/postfix/tls.key ]; then
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 3650 \
    -subj "/CN=localhost" \
    -addext "subjectAltName=DNS:localhost,DNS:postfix.nebulamail.test,IP:127.0.0.1" \
    -keyout /etc/postfix/tls.key -out /etc/postfix/tls.crt 2>/dev/null
fi

# SASL users in a local sasldb, in the realm nebulamail.test: `alice` and
# `alice@nebulamail.test` both log in (smtpd_sasl_local_domain).
mkdir -p /etc/sasl2
printf '%s\n' \
  'pwcheck_method: auxprop' \
  'auxprop_plugin: sasldb' \
  'sasldb_path: /etc/sasl2/sasldb2' \
  'mech_list: PLAIN LOGIN' > /etc/sasl2/smtpd.conf
for user in alice bob; do
  printf '%s' "$password" | saslpasswd2 -p -c -u nebulamail.test -f /etc/sasl2/sasldb2 "$user"
done
chown postfix /etc/sasl2/sasldb2

# Final delivery: Postfix's lmtp(8) client would say `relayed` in success
# reports, because Dovecot's LMTP has no DSN (the report then comes from the
# last server that has it, Postfix). pipe(8) with flag X does the final
# delivery, so its reports say `delivered`, like a real mailbox server's
# (without X they say `relayed` too). It runs msmtp, which hands the message
# to Dovecot's LMTP (TLS, self-signed, on the compose network) for the bare
# user name (`alice`), one recipient at a time, keeping the null sender of
# reports. msmtp exits with sysexits codes: 75 (temporary) is retried,
# others bounce.
printf '%s\n' 'nebulamail.test dovecot:' > /etc/postfix/transport
# Only alice and bob exist (like in Stalwart): others are refused at RCPT.
printf '%s\n' \
  'alice@nebulamail.test ok' \
  'bob@nebulamail.test ok' > /etc/postfix/recipients

# bounce.nebulamail.test is Postfix's own local domain with no users and no
# check at RCPT: local(8) takes the message and then bounces it ("unknown
# user"), as a real server does.
postconf -e \
  'maillog_file = /dev/stdout' \
  'myhostname = postfix.nebulamail.test' \
  'mydomain = nebulamail.test' \
  'myorigin = nebulamail.test' \
  'mydestination = bounce.nebulamail.test' \
  'local_recipient_maps =' \
  'inet_interfaces = all' \
  'inet_protocols = ipv4' \
  'mynetworks = 127.0.0.0/8' \
  'smtpd_peername_lookup = no' \
  'relay_domains = nebulamail.test' \
  'relay_recipient_maps = texthash:/etc/postfix/recipients' \
  'transport_maps = texthash:/etc/postfix/transport' \
  'dovecot_destination_recipient_limit = 1' \
  'smtpd_tls_cert_file = /etc/postfix/tls.crt' \
  'smtpd_tls_key_file = /etc/postfix/tls.key' \
  'smtpd_tls_security_level = may' \
  'smtpd_tls_auth_only = yes' \
  'smtpd_sasl_type = cyrus' \
  'smtpd_sasl_path = smtpd' \
  'cyrus_sasl_config_path = /etc/sasl2' \
  'smtpd_sasl_local_domain = nebulamail.test' \
  'smtpd_relay_restrictions = permit_sasl_authenticated, reject' \
  'smtpd_recipient_restrictions = permit_sasl_authenticated, reject' \
  'compatibility_level = 3.10'

postconf -M \
  'submission/inet = submission inet n - n - - smtpd' \
  'submissions/inet = submissions inet n - n - - smtpd' \
  '10025/inet = 10025 inet n - n - - smtpd' \
  '4465/inet = 4465 inet n - n - - smtpd' \
  "dovecot/unix = dovecot unix - n n - - pipe flags=Xq user=lmtp null_sender= argv=/usr/bin/msmtp --host=$dovecot_host --port=31024 --protocol=lmtp --tls=on --tls-starttls=off --tls-certcheck=off --auth=off --set-date-header=off --set-msgid-header=off --remove-bcc-headers=off --from=\${sender} -- \${user}"
postconf -P \
  'submission/inet/smtpd_tls_security_level = encrypt' \
  'submission/inet/smtpd_sasl_auth_enable = yes' \
  'submissions/inet/smtpd_tls_wrappermode = yes' \
  'submissions/inet/smtpd_sasl_auth_enable = yes' \
  '4465/inet/smtpd_tls_wrappermode = yes' \
  '4465/inet/smtpd_sasl_auth_enable = yes' \
  '4465/inet/smtpd_discard_ehlo_keywords = 8bitmime' \
  '10025/inet/smtpd_relay_restrictions = permit_auth_destination,reject' \
  '10025/inet/smtpd_recipient_restrictions = permit_auth_destination,reject'

newaliases
exec postfix start-fg
