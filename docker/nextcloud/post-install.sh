#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# Runs once, as www-data, right after the image installs Nextcloud
# (/docker-entrypoint-hooks.d/post-installation/): the test users alice
# and bob with Dovecot's test password and an address @nebulamail.test (their
# calendar user address), nothing that talks to the Internet, no mail, and
# the installing admin disabled (its password is never used again).
set -eu

occ() { php /var/www/html/occ --no-interaction "$@"; }

# The password policy would refuse Dovecot's "password"; the rest only
# adds web pages or calls home.
for app in password_policy firstrunwizard dashboard weather_status \
    user_status recommendations updatenotification survey_client \
    nextcloud_announcements support app_api photos activity \
    notifications privacy related_resources serverinfo sharebymail \
    federation; do
    occ app:disable "$app" >/dev/null 2>&1 || true
done
occ config:system:set appstoreenabled --value=false --type=boolean
occ config:system:set has_internet_connection --value=false --type=boolean
occ config:system:set updatechecker --value=false --type=boolean
# Never send mail: invitations and notifications go nowhere.
occ config:system:set mail_smtpmode --value=null
occ config:app:set dav sendInvitations --value=no
# Tests log in many times from the Docker gateway.
occ config:system:set auth.bruteforce.protection.enabled --value=false --type=boolean
occ config:system:set ratelimit.protection.enabled --value=false --type=boolean

: "${USER_PASSWORD:=password}"
export OC_PASS="$USER_PASSWORD"
for user in alice bob; do
    occ user:add --password-from-env --display-name="$user" "$user"
    occ user:setting "$user" settings email "$user@nebulamail.test"
done
# A group with bob in it, for sharing with a group (one sharee for all its
# members; the bridge's `alice_shares_with_a_group`).
occ group:add --display-name=Ventas ventas
occ group:adduser ventas bob
occ user:disable "$NEXTCLOUD_ADMIN_USER"
