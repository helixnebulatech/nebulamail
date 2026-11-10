#!/bin/sh
# SPDX-FileCopyrightText: 2026 Nebula Mail contributors
# SPDX-License-Identifier: AGPL-3.0-only

# Runs the Node server (src/node/server.ts) on Deno 2 with the fewest
# permissions it needs. Deno loads the real `undici` from
# node_modules, so net-guard's pinned Node adapter works unchanged. Deno's
# own `fetch` / `Deno.createHttpClient` proxy transport is NEVER used for
# downloads: it sends https in clear.
#
#   APP_URL=… IMAGE_PROXY_KEYS=… sh scripts/start-deno.sh [deno run flags]
#
# Extra flags (e.g. --watch, --env-file=…) go before the entry point.
# `DENO` may point to the deno binary (default: `deno` from PATH).
#
# - --allow-net: listen, and download from any public address (net-guard
#   decides which; Deno cannot express "public only").
# - --allow-env: only the proxy's variables and the three undici reads at
#   load time. HTTP(S)_PROXY and the like are NOT allowed: reading them would
#   throw.
# - --allow-read: node_modules, the HTML parser's worker thread (a module
#   file: Deno asks read access for it; full-email preload only) and the cache
#   directory; --allow-write: the cache directory only.
# - No --allow-run, --allow-ffi, --allow-sys or --allow-all.

set -eu
cd "$(dirname "$0")/.."

PROXY_ENV="APP_URL,IMAGE_PROXY_CLIENT_IP,IMAGE_PROXY_HOST,IMAGE_PROXY_KEYS,IMAGE_PROXY_PORT,\
IMAGE_CACHE_DIR,IMAGE_CACHE_MAX_BYTES,IMAGE_CACHE_TTL_DAYS,\
IMAGE_CACHE_S3_ACCESS_KEY_ID,IMAGE_CACHE_S3_BUCKET,IMAGE_CACHE_S3_ENDPOINT,\
IMAGE_CACHE_S3_REGION,IMAGE_CACHE_S3_SECRET_ACCESS_KEY,\
IMAGE_CACHE_S3_SESSION_TOKEN,IMAGE_CACHE_S3_URL_STYLE,\
IMAGE_PRELOAD_SECRET,IMAGE_PRELOAD_FULL_EMAIL,IMAGE_PRELOAD_INDEX_KEY,\
IMAGE_PRELOAD_EMAIL_TOKENS,IMAGE_PRELOAD_MAX_MESSAGE_BYTES,IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT,\
IMAGE_PRELOAD_MAX_IMAGES_PER_RECIPIENT"
UNDICI_ENV="UNDICI_NO_WASM_SIMD,UNDICI_USE_MILO,JEST_WORKER_ID"

READ="../../node_modules,../mail-render/src/parse-worker.ts"
WRITE=""
if [ -n "${IMAGE_CACHE_DIR:-}" ]; then
  READ="$READ,$IMAGE_CACHE_DIR"
  WRITE="--allow-write=$IMAGE_CACHE_DIR"
fi

# shellcheck disable=SC2086 # WRITE is one flag or nothing.
exec "${DENO:-deno}" run --no-prompt --sloppy-imports --node-modules-dir=manual \
  --allow-net --allow-env="$PROXY_ENV,$UNDICI_ENV" \
  --allow-read="$READ" $WRITE "$@" src/node/server.ts
