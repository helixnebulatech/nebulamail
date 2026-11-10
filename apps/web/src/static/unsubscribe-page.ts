// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// One-click unsubscribe in the static build: there is no
// server of ours to send the RFC 8058 POST, and the app's CSP only lets
// it reach its own origin. So the app opens `unsubscribe.html#<url>`, a
// page of the zip with a CSP of its own: its script by hash, `connect-src
// https:`, no forms, nothing else. It shows the domain and sends
// `List-Unsubscribe=One-Click` only when the user presses its button (anyone
// can link to the page, so it never sends by itself). Off by default; the
// user turns it on in Settings (mail/one-click-unsubscribe.ts).
//
// RFC 8058 §3.1: "The POST request MUST NOT include cookies, HTTP
// authorization, or any other context information". A top-level form POST
// carries the sender's `SameSite=None` cookies (and, in some browsers,
// cookies without SameSite), so the page uses `fetch` instead:
// `credentials: "omit"` (no cookies, no HTTP auth, no client certificate;
// Fetch standard, "credentials mode"), `referrerPolicy: "no-referrer"` (no
// Referer, and `Origin: null`: Fetch, "append a request Origin header") and
// `mode: "no-cors"` (a CORS-safelisted POST: no preflight, nothing of ours
// in the request). The answer is opaque: the page can't know whether the
// sender accepted it, only that it was sent, and says so. Limitation: a
// no-cors fetch must follow redirects (Fetch, "main fetch"), so a sender
// that redirects in spite of RFC 8058 §3.1 gets a GET without credentials.

import { createHash } from "node:crypto";

import { isPublicHttpsUrl } from "../mail/public-url.ts";

export interface UnsubscribePageTexts {
  readonly title: string;
  /** With `{host}`. */
  readonly body: string;
  readonly confirm: string;
  readonly invalid: string;
  /** With `{host}`: sent, but the browser can't see the answer. */
  readonly sent: string;
  readonly failed: string;
}

const STYLE =
  ":root{color-scheme:light dark;font:16px/1.5 system-ui,sans-serif}" +
  "body{margin:0;display:grid;min-height:100vh;place-items:center;padding:24px;box-sizing:border-box}" +
  "main{max-width:420px}" +
  "h1{font-size:20px;margin:0 0 8px}" +
  "p{margin:0 0 20px}" +
  "button{font:inherit;padding:10px 18px;border-radius:10px;border:0;background:#4f46e5;color:#fff;cursor:pointer}" +
  "button:disabled{opacity:.6;cursor:default}";

/** `<` and friends escaped, so the JSON can't end the script element. */
const scriptJson = (value: unknown): string =>
  JSON.stringify(value)
    .replaceAll("<", "\\u003c")
    .replaceAll(">", "\\u003e")
    .replaceAll("&", "\\u0026");

/**
 * The request (RFC 8058 §3.1, §3.2): POST, form-urlencoded body
 * `List-Unsubscribe=One-Click`, no cookies, authorization, referrer or
 * cache. `content-type` is CORS-safelisted, so `no-cors` allows it.
 */
export const ONE_CLICK_INIT = Object.freeze({
  body: "List-Unsubscribe=One-Click",
  cache: "no-store",
  credentials: "omit",
  headers: Object.freeze({
    "content-type": "application/x-www-form-urlencoded",
  }),
  method: "POST",
  mode: "no-cors",
  redirect: "follow",
  referrerPolicy: "no-referrer",
} satisfies RequestInit);

const sha256 = (text: string): string =>
  `sha256-${createHash("sha256").update(text).digest("base64")}`;

const escapeAttribute = (value: string): string =>
  value.replaceAll("&", "&amp;").replaceAll('"', "&quot;");

/**
 * The script of the sandboxed frame that sends the POST. It has an opaque
 * origin (`sandbox="allow-scripts"`, no `allow-same-origin`), so its
 * requests say `Origin: null` instead of the app's origin, which
 * Chromium sends from the page itself even with `no-referrer`. It only acts
 * on a message from its parent and checks the URL again.
 */
const frameScript = (): string =>
  [
    "(()=>{",
    `const ok=${String(isPublicHttpsUrl)};`,
    'const say=(m)=>parent.postMessage(m,"*");',
    'addEventListener("message",(e)=>{',
    'if(e.source!==parent||typeof e.data!=="string"||!ok(e.data))return;',
    // RFC 8058 §3.1/§3.2: the exact body, nothing of the user's (see above).
    `fetch(e.data,${scriptJson(ONE_CLICK_INIT)})`,
    '.then(()=>say("sent"),()=>say("failed"))});',
    'say("ready")',
    "})()",
  ].join("");

/** The whole page, texts in English and Spanish. */
export const unsubscribePage = (
  texts: Readonly<Record<"en" | "es", UnsubscribePageTexts>>
): string => {
  const inner = frameScript();
  const script = [
    "(()=>{",
    `const ok=${String(isPublicHttpsUrl)};`,
    `const texts=${scriptJson(texts)};`,
    String.raw`const chosen=/(?:^|;\s*)wm-locale=(en|es)(?:;|$)/u.exec(document.cookie)?.[1];`,
    'const lang=chosen??(navigator.language.toLowerCase().startsWith("es")?"es":"en");',
    "const t=texts[lang];",
    "document.documentElement.lang=lang;document.title=t.title;",
    'const $=(id)=>document.getElementById(id);$("t").textContent=t.title;',
    'let target="";try{target=decodeURIComponent(location.hash.slice(1))}catch{}',
    // The URL stays out of the address bar and the history.
    'history.replaceState(null,"",location.pathname);',
    'if(!ok(target)){$("d").textContent=t.invalid;return}',
    // split/join: replaceAll would read "$&" and the like in the host.
    'const host=new URL(target).hostname;const fill=(s)=>s.split("{host}").join(host);',
    '$("d").textContent=fill(t.body);',
    'const b=$("b");b.textContent=t.confirm;const w=$("s").contentWindow;',
    'addEventListener("message",(e)=>{if(e.source!==w)return;',
    'if(e.data==="ready"){b.hidden=false}',
    'else if(e.data==="sent"){$("d").textContent=fill(t.sent);b.hidden=true}',
    'else if(e.data==="failed"){$("d").textContent=t.failed;b.disabled=false}});',
    // Only a click sends (anyone can link to this page).
    'b.addEventListener("click",()=>{b.disabled=true;w.postMessage(target,"*")});',
    "})()",
  ].join("");
  const csp = [
    "default-src 'none'",
    // The frame (srcdoc) inherits this policy: its script needs its hash.
    `script-src '${sha256(script)}' '${sha256(inner)}'`,
    `style-src '${sha256(STYLE)}'`,
    "connect-src https:",
    "form-action 'none'",
    "base-uri 'none'",
    "require-trusted-types-for 'script'",
    "trusted-types 'none'",
  ].join("; ");
  // In markup, not set from script: `srcdoc` is a Trusted Types sink.
  const frame = `<script>${inner}</script>`;
  return [
    "<!DOCTYPE html>",
    '<html><head><meta charset="utf-8">',
    `<meta http-equiv="Content-Security-Policy" content="${csp}">`,
    '<meta name="referrer" content="no-referrer">',
    '<meta name="viewport" content="width=device-width, initial-scale=1">',
    `<title></title><style>${STYLE}</style></head>`,
    '<body><main><h1 id="t"></h1><p id="d"></p>',
    '<button id="b" type="button" hidden></button></main>',
    `<iframe id="s" sandbox="allow-scripts" hidden srcdoc="${escapeAttribute(frame)}"></iframe>`,
    `<script>${script}</script></body></html>`,
    "",
  ].join("");
};
