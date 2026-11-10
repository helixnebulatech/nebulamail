// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

/**
 * Why the proxy gave up on a request (proxy.ts turns it into a fixed answer:
 * never the JMAP server's own text):
 *   - `too-large`: the browser's body passed the JMAP server's limit;
 *   - `redirect`: the JMAP server answered with a redirect (never followed);
 *   - `session`: the JMAP server's session isn't one the proxy accepts.
 */
export class ProxyError extends Error {
  override readonly name = "ProxyError";
  readonly kind: "too-large" | "redirect" | "session";

  constructor(kind: "too-large" | "redirect" | "session", message: string) {
    super(message);
    this.kind = kind;
  }
}
