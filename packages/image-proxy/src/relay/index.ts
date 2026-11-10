// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `@nebulamail/image-proxy/relay`: the preload relay that runs next to
// Stalwart and sends the proxy only image URLs (README
// "Preload › Relay").

export {
  createRelay,
  DEFAULT_RELAY_TIMEOUT_MS,
  RELAY_HOOK_PATH,
  RelayConfigError,
  type RelayConfig,
  type RelayHandler,
  type RelayLogEvent,
} from "./relay";
export { URL_HOOK_PATH } from "../preload/url-hook";
