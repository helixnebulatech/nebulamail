#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# The CI of .github/workflows/ci.yml, run on this machine (GitHub Actions is
# not available). KEEP IN SYNC: every job, step, command and env var here
# mirrors ci.yml; a change to one must change the other in the same commit.
#
#   scripts/ci-local.sh [web|runtimes|hostile-emails|web-browser|
#                        static-stalwart|rust|fuzz|bridge-servers|
#                        cargo-deny|reuse|all] ...
#
# With no argument, `all`. Jobs run one after another (never in parallel:
# bridge-servers shares accounts and Stalwart's send limit). Each job's
# output goes to the terminal and to .ci-local/<timestamp>/<job>.log
# (git-ignored); at the end, a PASS/FAIL/SKIP summary, and exit code 1 if
# any job failed.
#
# Differences from GitHub, on purpose:
# - Tools are not installed system-wide: `playwright install` runs without
#   `--with-deps` (needs root and apt; set CI_LOCAL_PLAYWRIGHT_DEPS=1 to add
#   it), cargo-fuzz and cargo-deny go to ~/.cargo/bin with `cargo install
#   --locked` and REUSE runs with `uvx reuse`, only when missing.
# - fuzz: without rustup's nightly, cargo-fuzz runs on the pinned stable
#   toolchain with RUSTC_BOOTSTRAP=1 and no sanitizer (`-s none`; ASan needs
#   a nightly std). With `cargo +nightly` available, it uses that as CI.
# - bridge-servers uses the running dev containers (nebula-dev-stalwart-1,
#   nebula-dev-dovecot-1) and their data and docker/.env: it never
#   recreates them, never writes docker/.env and doesn't run `dev:setup`
#   (CI does, on a fresh Stalwart; set CI_LOCAL_DEV_SETUP=1 to run it, it
#   only creates what is missing). Radicale, Postfix, Baïkal and Nextcloud
#   (with its PostgreSQL) are started only if not running, and only those
#   the script started are stopped at the end (CI stops Nextcloud right
#   after its tests).
# - runtimes: without deno installed, the Deno steps are skipped (with a
#   message) instead of installing it; CI installs Deno 2.9.8.
# - static-stalwart starts its own throwaway Stalwart (`stalwart-static`,
#   compose project nebula-static-test, port 8095) exactly as CI does and
#   removes it at the end; it never touches the dev containers or
#   docker/.env. It fails at once if 8095 or the test proxy's 8096 is busy.
# - test:live: if port 3001 is taken, the app is served on 3101 and Chromium
#   maps localhost:3001 to it (the OAuth redirect stays on 3001).
# - Browser tests run with DISPLAY and WAYLAND_DISPLAY unset (headless, as
#   on a runner).
#
# Stalwart limits alice → bob sends (about 25 an hour, docker/README.md):
# one bridge-servers run sends about 15-25 messages through it (calendar
# invitations, the bridge's SMTP tests against Stalwart and through
# Dovecot's relay, test:live, test:bridge). Don't run it in a loop. With
# CI_LOCAL_SKIP_MAIL=1 nothing is sent to Stalwart: the steps that send
# there are skipped or run without NEBULA_TEST_SMTP, and test:bridge sends
# through Postfix (to Dovecot) instead.
#
# Environment:
#   FUZZ_SECONDS=60          seconds per fuzz target (CI: 60)
#   FUZZ_TARGETS="a/b c/d"   only these crate/target pairs
#   CI_LOCAL_SKIP_MAIL=1     send nothing through Stalwart
#   CI_LOCAL_DEV_SETUP=1     run `bun run dev:setup` like CI
#   CI_LOCAL_PLAYWRIGHT_DEPS=1  `playwright install --with-deps`
#   CI_LOCAL_QUIET=1         logs only to the files, not to the terminal
#   CI_LOCAL_CI=0            don't export CI=true (GitHub sets it; it turns
#                            on Playwright's retries, forbidOnly and the
#                            "github" reporter)

set -uo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1

ALL_JOBS=(web runtimes hostile-emails web-browser static-stalwart rust cargo-deny reuse fuzz bridge-servers)

# ci.yml, job "fuzz": the matrix, crate/target.
FUZZ_MATRIX=(
  imap-proto/decode
  imap-proto/utf7
  jmap-proto/request
  jmap-proto/strict_json
  jmap-proto/calendar_contacts
  http/form
  http/basic_auth
  http/fields
  groupware/icalendar
  groupware/vcard
  groupware/json
  groupware/defaults
  groupware/counter
  dav-proto/multistatus
  dav-proto/href
  dav-proto/values
  dav-proto/schedule
  managesieve-proto/response
  managesieve-proto/command
)

COMPOSE=(docker compose -f docker/compose.yaml)
SKIP=77 # exit code of a job that skipped itself

usage() {
  sed -n '9,10p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
}

# GitHub Actions sets CI=true for every step.
if [[ ${CI_LOCAL_CI:-1} != 0 ]]; then
  export CI=true
fi

export PATH="$HOME/.cargo/bin:$HOME/.bun/bin:$HOME/.local/bin:$PATH"

step() {
  printf '\n==> %s\n' "$*"
  "$@"
}

# `env -u DISPLAY -u WAYLAND_DISPLAY`: headless browsers, as on a runner.
headless() {
  env -u DISPLAY -u WAYLAND_DISPLAY "$@"
}

playwright_install() {
  local deps=()
  [[ ${CI_LOCAL_PLAYWRIGHT_DEPS:-0} == 1 ]] && deps=(--with-deps)
  step bunx playwright install "${deps[@]}" "$@"
}

check_bun_version() {
  local want have
  want=$(sed -n 's/.*"packageManager": "bun@\([^"]*\)".*/\1/p' package.json)
  have=$(bun --version)
  if [[ $want != "$have" ]]; then
    echo "warning: bun $have here, CI uses $want (package.json)"
  fi
}

cargo_tool() { # cargo_tool <binary> <crate>: cargo install it if missing
  if ! command -v "$1" > /dev/null; then
    step cargo install --locked "$2"
  fi
}

# --- jobs (one function per job of ci.yml) --------------------------------

job_web() {
  # Placeholder config so the app builds; none of these are real secrets.
  export APP_URL=https://mail.example.com
  export JMAP_SERVER=https://jmap.example.com
  export OAUTH_CLIENT_ID=ci
  export OAUTH_CLIENT_SECRET=ci-placeholder-not-a-secret
  export SESSION_KEYS="1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
  check_bun_version
  step bun install --frozen-lockfile
  # GHSA-gcqx-w48m-w2h4: see ci.yml.
  step bun audit --ignore=GHSA-gcqx-w48m-w2h4
  # First: the build generates routeTree.gen.ts, which type-aware lint needs.
  step bun run build
  # The zip that Stalwart serves: built twice with the same
  # nonce, it must give the same SHA-256.
  (
    cd apps/web
    export NEBULA_STYLE_NONCE=ci-reproducible-build
    step bun run build:static
    first=$(cat dist-static/nebula-mail-static.zip.sha256)
    step bun run build:static
    [[ $first == "$(cat dist-static/nebula-mail-static.zip.sha256)" ]] || {
      echo "static build is not reproducible"
      exit 1
    }
  )
  step bun run check
  step bun run check-types
  step bun run test
  echo "==> Tracker list is up to date"
  (cd packages/mail-render && step bun run generate:trackers --check)
}

# net-guard and the image proxy on Bun and Deno.
job_runtimes() {
  check_bun_version
  step bun install --frozen-lockfile
  (cd packages/net-guard && step bun run test:bun)
  (cd packages/image-proxy && step bun run test:bun)
  if ! command -v deno > /dev/null; then
    echo "==> deno is not installed: Deno steps skipped (CI runs them)"
    return 0
  fi
  (cd packages/net-guard && step bun run test:deno)
  (cd packages/image-proxy && step bun run test:deno)
}

job_hostile-emails() {
  step bun install --frozen-lockfile
  cd packages/mail-render
  playwright_install chromium firefox
  # 0 requests with images off, only the proxy with images on.
  step headless bun run test:browser
}

job_web-browser() {
  step bun install --frozen-lockfile
  cd apps/web
  # varlock always loads .env.local, also for the test server: its values
  # fill what e2e/env.ts leaves unset (IMAGE_PROXY_KEYS turns the image
  # proxy on, with the fake origin img.test, and preload requests hang).
  # A runner has none; run from a worktree without it to match CI.
  if [[ -f .env.local ]]; then
    echo "warning: apps/web/.env.local exists; test:browser may fail where CI passes"
    echo "  (run from a git worktree, which has no .env.local)"
  fi
  playwright_install chromium
  # production build, fake session and fake JMAP.
  step headless bun run test:browser
}

# ci.yml, job "static-stalwart": its own compose project, never the dev one.
STATIC_COMPOSE=(docker compose -p nebula-static-test -f "$ROOT/docker/compose.yaml" --profile static)

remove_static_stalwart() {
  step "${STATIC_COMPOSE[@]}" rm -sfv stalwart-static
}

job_static-stalwart() {
  # The build needs the app's config to exist; none of it ends in the zip.
  export APP_URL=https://mail.example.com
  export JMAP_SERVER=https://jmap.example.com
  export OAUTH_CLIENT_ID=ci
  export OAUTH_CLIENT_SECRET=ci-placeholder-not-a-secret
  export SESSION_KEYS="1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
  local port
  for port in 8095 8096; do
    if ! port_free "$port"; then
      echo "Port $port is in use: the throwaway Stalwart (8095) and the test proxy (8096) need it."
      return 1
    fi
  done
  step bun install --frozen-lockfile
  (cd apps/web && step bun run build:static)
  echo "==> Start the throwaway Stalwart"
  STALWART_STATIC_RECOVERY_ADMIN="admin:$(openssl rand -hex 16)"
  export STALWART_STATIC_RECOVERY_ADMIN
  trap remove_static_stalwart EXIT
  step "${STATIC_COMPOSE[@]}" up -d --wait --no-deps stalwart-static
  cd apps/web
  playwright_install chromium
  if ! step headless bun run test:static; then
    echo "==> Server logs"
    "${STATIC_COMPOSE[@]}" logs --tail 200 stalwart-static
    return 1
  fi
}

job_rust() {
  cd bridge
  step cargo fmt --check
  step cargo clippy --all-targets --locked -- -D warnings
  step cargo test --locked
}

job_fuzz() {
  local seconds=${FUZZ_SECONDS:-60}
  local cargo=(cargo)
  local fuzz_env=()
  local fuzz_args=()
  if cargo +nightly --version > /dev/null 2>&1; then
    cargo=(cargo +nightly)
  else
    echo "No nightly toolchain: stable with RUSTC_BOOTSTRAP=1, no sanitizer (-s none)."
    fuzz_env=(RUSTC_BOOTSTRAP=1)
    fuzz_args=(-s none)
  fi
  cargo_tool cargo-fuzz cargo-fuzz
  local targets=("${FUZZ_MATRIX[@]}")
  if [[ -n ${FUZZ_TARGETS:-} ]]; then
    read -ra targets <<< "$FUZZ_TARGETS"
  fi
  local failed=() entry crate target
  # fail-fast: false, as the matrix.
  for entry in "${targets[@]}"; do
    crate=${entry%%/*}
    target=${entry#*/}
    if ! (
      cd "bridge/crates/$crate/fuzz" &&
        step env "${fuzz_env[@]}" "${cargo[@]}" fuzz run "${fuzz_args[@]}" "$target" \
          -- -max_total_time="$seconds" -rss_limit_mb=2048
    ); then
      echo "FAIL $entry: crashing input in bridge/crates/$crate/fuzz/artifacts/$target/"
      failed+=("$entry")
    fi
  done
  echo
  echo "fuzz: ${#targets[@]} targets, ${seconds} s each, ${#failed[@]} failed ${failed[*]}"
  [[ ${#failed[@]} -eq 0 ]]
}

# Waits up to 30 s (or WAIT_SECONDS) for a command to succeed (the
# "for _ in $(seq 30)" loops).
wait_for() {
  local _
  for _ in $(seq "${WAIT_SECONDS:-30}"); do
    "$@" && return 0
    sleep 1
  done
  echo "timed out waiting for: $*"
  return 1
}

imaps_greeting() {
  echo | timeout 2 openssl s_client -quiet -connect 127.0.0.1:2993 2> /dev/null | grep -q 'OK'
}
baikal_answers() { curl -s -o /dev/null http://127.0.0.1:5233/dav.php/; }
nextcloud_installed() {
  curl -sf http://127.0.0.1:5234/status.php | grep -q '"installed":true'
}
radicale_answers() {
  curl -s -o /dev/null http://127.0.0.1:5232/
}
postfix_greeting() {
  echo | timeout 2 openssl s_client -quiet -connect 127.0.0.1:2466 2> /dev/null | grep -q '^220'
}

running() { # running <container>
  [[ $(docker inspect -f '{{.State.Running}}' "$1" 2> /dev/null) == true ]]
}

port_free() { # port_free <port>
  ! (exec 3<> "/dev/tcp/127.0.0.1/$1") 2> /dev/null
}

STARTED=()
start_service() { # start_service <service> [compose up flags]: only if not running
  local service=$1
  shift
  if running "nebula-dev-$service-1"; then
    echo "$service already running: used as is."
  else
    step "${COMPOSE[@]}" --profile bridge up -d "$@" "$service"
    STARTED+=("$service")
  fi
}

stop_started() {
  if [[ ${#STARTED[@]} -gt 0 ]]; then
    step "${COMPOSE[@]}" --profile bridge stop "${STARTED[@]}"
  fi
}

load_docker_env() {
  # Read only: docker/.env is never written here.
  set -a
  # shellcheck source=/dev/null
  . "$ROOT/docker/.env"
  set +a
}

bridge_servers_steps() {
  local no_mail=${CI_LOCAL_SKIP_MAIL:-0}
  local smtp=(NEBULA_TEST_SMTP=1)
  if [[ $no_mail == 1 ]]; then
    echo "CI_LOCAL_SKIP_MAIL=1: nothing is sent through Stalwart."
    smtp=()
  fi

  step bun install --frozen-lockfile || return 1

  # CI: "Start Stalwart" (fresh, with a random recovery admin, then
  # dev:setup). Here: the dev container with its data, as it is.
  echo "==> Stalwart (dev container)"
  if ! running nebula-dev-stalwart-1; then
    echo "nebula-dev-stalwart-1 is not running. Start it (docker/README.md):"
    echo "  docker compose -f docker/compose.yaml up -d --wait stalwart"
    return 1
  fi
  if [[ ! -f docker/.env ]] || ! grep -q '^TEST_ALICE_PASSWORD=' docker/.env ||
    ! grep -q '^TEST_BOB_PASSWORD=' docker/.env; then
    echo "docker/.env has no TEST_ALICE_PASSWORD/TEST_BOB_PASSWORD: run bun run dev:setup once (docker/README.md)."
    return 1
  fi
  if [[ ${CI_LOCAL_DEV_SETUP:-0} == 1 ]]; then
    step bun run dev:setup || return 1
  fi

  # Calendar invitations to one occurrence, end to end.
  if [[ $no_mail == 1 ]]; then
    echo "==> SKIP Calendar integration tests (Stalwart): they send invitations"
  else
    echo "==> Calendar integration tests (Stalwart)"
    (
      cd apps/web &&
        load_docker_env &&
        CAL_LIVE_URL=http://localhost:8080 \
          CAL_LIVE_USER=alice@nebulamail.test CAL_LIVE_PASSWORD="$TEST_ALICE_PASSWORD" \
          CAL_LIVE_ORGANIZER=bob@nebulamail.test CAL_LIVE_ORGANIZER_PASSWORD="$TEST_BOB_PASSWORD" \
          CAL_LIVE_DELEGATE=carol@nebulamail.test CAL_LIVE_DELEGATE_PASSWORD="$TEST_CAROL_PASSWORD" \
          step bunx vitest run src/calendar/occurrence.live.test.ts src/calendar/sharing.live.test.ts src/calendar/defaults.live.test.ts
    ) || return 1
  fi

  echo "==> Dovecot (dev container)"
  if ! running nebula-dev-dovecot-1; then
    echo "nebula-dev-dovecot-1 is not running. Start it (docker/README.md):"
    echo "  docker compose -f docker/compose.yaml --profile bridge up -d dovecot"
    return 1
  fi
  wait_for imaps_greeting || return 1

  # One test at a time: the tests share alice's account, and Stalwart
  # indexes for search in a background queue that falls behind.
  echo "==> Integration tests (Stalwart)"
  (
    cd bridge &&
      step env NEBULA_TEST_IMAP=1 "${smtp[@]}" NEBULA_TEST_SERVER=stalwart \
        cargo test --locked --test server -- --test-threads=1
  ) || return 1
  # The live tests of `mail` are unit tests (they use the crate's insides):
  # `--test server` doesn't run them. Nothing is sent.
  echo "==> Mail live tests (Stalwart)"
  (
    cd bridge &&
      step env NEBULA_TEST_IMAP=1 NEBULA_TEST_SERVER=stalwart \
        cargo test --locked -p nebula-mail --lib live_tests -- --test-threads=1
  ) || return 1
  # Dovecot's submission relays to Stalwart: no SMTP with CI_LOCAL_SKIP_MAIL.
  echo "==> Integration tests (Dovecot)"
  (
    cd bridge &&
      step env NEBULA_TEST_IMAP=1 "${smtp[@]}" NEBULA_TEST_SERVER=dovecot \
        cargo test --locked --test server -- --test-threads=1
  ) || return 1
  echo "==> Mail live tests (Dovecot)"
  (
    cd bridge &&
      step env NEBULA_TEST_IMAP=1 NEBULA_TEST_SERVER=dovecot \
        cargo test --locked -p nebula-mail --lib live_tests -- --test-threads=1
  ) || return 1

  # Sieve over ManageSieve: Stalwart's 4190 and Dovecot's
  # Pigeonhole on 24190. Nothing is sent.
  echo "==> Sieve tests (Stalwart and Dovecot)"
  (
    cd bridge &&
      for server in stalwart dovecot; do
        step env NEBULA_TEST_SIEVE=1 NEBULA_TEST_SERVER=$server \
          cargo test --locked -p nebula-managesieve --test server -- --test-threads=1 || exit 1
        step env NEBULA_TEST_SIEVE=1 NEBULA_TEST_SERVER=$server \
          cargo test --locked -p nebula-jmap --test sieve -- --test-threads=1 || exit 1
      done
  ) || return 1

  # CalDAV/CardDAV: Stalwart's own DAV, then Radicale.
  # invitation_round_trip and task_assignment_round_trip make Stalwart
  # send two messages each.
  local skip_invites=()
  [[ $no_mail == 1 ]] && skip_invites=(--skip invitation_round_trip --skip task_assignment_round_trip)
  echo "==> DAV integration tests (Stalwart)"
  (
    cd bridge &&
      export NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=stalwart &&
      step cargo test --locked -p nebula-dav -p nebula-groupware -p nebula-jmap --test server -- --test-threads=1 "${skip_invites[@]}" &&
      step cargo test --locked -p nebula-jmap --test calendars -- --test-threads=1 &&
      step cargo test --locked -p nebula-jmap --test alerts
  ) || return 1

  echo "==> Start Radicale"
  start_service radicale --build || return 1
  wait_for radicale_answers || return 1
  # Started before the Radicale tests: there the bridge mails the
  # invitations itself through Postfix, which delivers to Dovecot.
  echo "==> Start Postfix"
  start_service postfix --build || return 1
  wait_for postfix_greeting || return 1

  echo "==> DAV integration tests (Radicale)"
  (
    cd bridge &&
      export NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=radicale &&
      step cargo test --locked -p nebula-dav -p nebula-groupware -p nebula-jmap --test server -- --test-threads=1 &&
      step cargo test --locked -p nebula-jmap --test calendars -- --test-threads=1 &&
      step cargo test --locked -p nebula-jmap --test alerts
  ) || return 1
  # Baïkal: Calendar Server sharing.
  echo "==> Start Baikal"
  start_service baikal --build || return 1
  wait_for baikal_answers || return 1
  echo "==> DAV sharing tests (Baikal)"
  (
    cd bridge &&
      step env NEBULA_TEST_BAIKAL=1 cargo test --locked -p nebula-jmap --test server sharing -- --test-threads=1
  ) || return 1
  # Nextcloud 34 on PostgreSQL. About a
  # minute to install the first time: wait up to 5 min. CI stops it right
  # after its tests; here, stop_started does at the end (if started here).
  echo "==> Start Nextcloud"
  start_service nextcloud-db --wait || return 1
  start_service nextcloud --build || return 1
  WAIT_SECONDS=300 wait_for nextcloud_installed || return 1
  echo "==> DAV integration tests (Nextcloud)"
  (
    cd bridge &&
      step env NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=nextcloud \
        cargo test --locked -p nebula-dav -p nebula-groupware -p nebula-jmap --test server -- --test-threads=1
  ) || return 1
  # Calendars on Radicale and contacts on Baïkal (`contacts_url`, as
  # Fastmail).
  echo "==> Split DAV test (Radicale + Baikal)"
  (
    cd bridge &&
      step env NEBULA_TEST_SPLIT_DAV=1 cargo test --locked -p nebula-jmap --test server split_dav -- --test-threads=1
  ) || return 1
  # Postfix: nothing goes to Stalwart.
  echo "==> Integration tests (Postfix)"
  (
    cd bridge &&
      step env NEBULA_TEST_IMAP=1 NEBULA_TEST_SMTP=1 NEBULA_TEST_SERVER=postfix \
        cargo test --locked -p nebula-smtp -p nebula-jmap --test server -- --test-threads=1
  ) || return 1
  echo "==> Sieve on delivered mail (Postfix + Dovecot)"
  (
    cd bridge &&
      step env NEBULA_TEST_SIEVE=1 NEBULA_TEST_SERVER=postfix \
        cargo test --locked -p nebula-jmap --test sieve -- --test-threads=1
  ) || return 1
  # Folders under INBOX., with its own Dovecot and Postfix.
  echo "==> Sieve with folders under INBOX. (Dovecot + Postfix of their own)"
  (
    docker compose -f docker/compose.yaml --profile bridge-prefix \
      up -d --build --no-deps dovecot-prefix postfix-prefix &&
      for _ in {1..30}; do
        echo | timeout 2 openssl s_client -quiet -connect 127.0.0.1:2467 2>/dev/null | grep -q '^220' && break
        sleep 1
      done &&
      cd bridge &&
      step env NEBULA_TEST_SIEVE=1 NEBULA_TEST_SERVER=dovecot-prefix \
        cargo test --locked -p nebula-jmap --test sieve -- --test-threads=1
  ) || return 1

  (cd bridge && step cargo build --locked -p nebula-bridge) || return 1
  (cd apps/web && playwright_install chromium) || return 1

  # The app against Stalwart itself (its OAuth page, port 3001 of dev:setup).
  if [[ $no_mail == 1 ]]; then
    echo "==> SKIP Delivery status and receipts in the browser (Stalwart): they send mail"
  else
    echo "==> Delivery status and receipts in the browser (Stalwart)"
    # With CI=true Playwright doesn't reuse a server on 3001, which may be
    # another project's: serve on 3101, and the browser maps 3001 to it.
    local live_port=3001
    port_free 3001 || live_port=3101
    (
      cd apps/web &&
        load_docker_env &&
        E2E_LIVE_PORT=$live_port MAIL_LIVE_URL=http://localhost:8080 \
          MAIL_LIVE_ALICE_PASSWORD="$TEST_ALICE_PASSWORD" \
          MAIL_LIVE_BOB_PASSWORD="$TEST_BOB_PASSWORD" \
          step headless bun run test:live
    ) || return 1
  fi

  # Mail, synced preferences, contacts and calendar through the bridge with
  # Dovecot and Radicale; the @postfix tests skip themselves here.
  if [[ $no_mail == 1 ]]; then
    # Dovecot's submission relays to Stalwart: send through Postfix
    # instead, which runs the @postfix tests as well in the same go.
    echo "==> The app through the bridge (Dovecot + Radicale), sending through Postfix"
    (cd apps/web && E2E_BRIDGE_SMTP=postfix step headless bun run test:bridge) || return 1
  else
    echo "==> The app through the bridge (Dovecot + Radicale)"
    (cd apps/web && step headless bun run test:bridge) || return 1
    echo "==> Receipts and invitations in the browser (bridge + Postfix)"
    (cd apps/web && E2E_BRIDGE_SMTP=postfix step headless bun run test:bridge -- --grep @postfix) || return 1
  fi
}

job_bridge-servers() {
  trap stop_started EXIT
  if ! bridge_servers_steps; then
    echo
    echo "==> Server logs"
    "${COMPOSE[@]}" --profile bridge logs --tail 200
    return 1
  fi
}

job_cargo-deny() {
  cargo_tool cargo-deny cargo-deny
  # What EmbarkStudios/cargo-deny-action runs with its defaults.
  step cargo deny --manifest-path bridge/Cargo.toml --log-level warn --all-features check
  # Cargo has no minimumReleaseAge: check that every crate is 2+ days old.
  echo "==> Crates published at least 2 days ago"
  (cd bridge && step python3 scripts/check-release-age.py)
}

job_reuse() {
  # fsfe/reuse-action runs `reuse lint`.
  if command -v reuse > /dev/null; then
    step reuse lint
  else
    step uvx reuse lint
  fi
}

# --- main -------------------------------------------------------------------

jobs=()
for arg in "${@:-all}"; do
  case $arg in
    all) jobs+=("${ALL_JOBS[@]}") ;;
    web | runtimes | hostile-emails | web-browser | static-stalwart | rust | fuzz | bridge-servers | cargo-deny | reuse) jobs+=("$arg") ;;
    -h | --help) usage ;;
    *)
      echo "unknown job: $arg" >&2
      usage
      ;;
  esac
done

LOG_DIR="$ROOT/.ci-local/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$LOG_DIR"
echo "Logs: $LOG_DIR"

declare -A RESULT DURATION
for job in "${jobs[@]}"; do
  log="$LOG_DIR/$job.log"
  printf '\n######## %s ########\n' "$job"
  start=$(date +%s)
  # A subshell per job (its own cd, env and traps) with GitHub's default
  # `bash -e`: errexit, so the first failing step fails the job, and no
  # pipefail or nounset (the `timeout openssl | grep -q` waits rely on
  # that). Not in an `if`/`||`, which would turn errexit off inside it.
  if [[ ${CI_LOCAL_QUIET:-0} == 1 ]]; then
    (set -eE +u +o pipefail; "job_$job") > "$log" 2>&1
    status=$?
  else
    (set -eE +u +o pipefail; "job_$job") 2>&1 | tee "$log"
    status=${PIPESTATUS[0]}
  fi
  DURATION[$job]=$(($(date +%s) - start))
  case $status in
    0) RESULT[$job]=PASS ;;
    "$SKIP") RESULT[$job]=SKIP ;;
    *) RESULT[$job]=FAIL ;;
  esac
  echo "######## $job: ${RESULT[$job]} (exit $status, ${DURATION[$job]} s) ########"
done

{
  printf '\nSummary (logs in %s)\n' "$LOG_DIR"
  for job in "${jobs[@]}"; do
    printf '  %-5s %-15s %5s s  %s\n' "${RESULT[$job]}" "$job" "${DURATION[$job]}" "$LOG_DIR/$job.log"
  done
} | tee "$LOG_DIR/summary.txt"
failed=0
for job in "${jobs[@]}"; do
  [[ ${RESULT[$job]} == FAIL ]] && failed=1
done
exit $failed
