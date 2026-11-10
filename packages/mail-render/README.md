<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# @nebulamail/mail-render

Safe rendering of email HTML. Pure functions over a DOM: they run in the browser and are tested with jsdom. No React. Licence AGPL-3.0-only (includes GPL-3.0-or-later code from Proton and BSD-3-Clause data from MailTrackerBlocker, see `vendor/`).

## Entry points

| Import | Runs on | What |
| --- | --- | --- |
| `@nebulamail/mail-render` | browser | `renderEmail` and everything below |
| `@nebulamail/mail-render/trackers` | anywhere (no DOM) | `isTrackerUrl`, `isSpyPixel`, list data, for the browser, BFF and proxy |
| `@nebulamail/mail-render/extract` | server, with an injected parser | `extractRemoteImageUrls` for the BFF |

## Using it in the app

```ts
const result = renderEmail({ html, text }, settings, {
  images: {
    kind: "proxy",
    origin: "https://img.example.com",
    rewrite: (url) => tickets.get(url) ?? null,
  },
  userApproved, // the user pressed "Show images"
  senderTrusted,
  cids: (cid) => inlineParts.get(cid), // data: URLs for result.neededCids
  styleNonce: pageCspNonce, // see "CSP nonce" below
});
// <iframe sandbox={IFRAME_SANDBOX} referrerPolicy={IFRAME_REFERRER_POLICY}
//         style={{ contain: FRAME_CONTAIN }} srcdoc={result.srcdoc} />
// attachFrame(iframe, { onLink, onHeight }) for click capture and auto-height.
// On click: resolveLinkForOpening(link.href, undefined, { appOrigin }) → analyzeLink(url, link.text)
//           → needsConfirmation(settings.linkConfirmation, analysis) → dialog → open.
```

- `result.remoteImages`: remote image URLs (trackers excluded). Use it to decide whether to show "Show images". The BFF must not trust a list from the browser: it calls `extractRemoteImageUrls` on the email it downloaded itself.
- `result.neededCids`: Content-IDs to download (with the token) and pass back as `cids`, as `data:` URLs. Anything else is ignored.
- Showing images means calling `renderEmail` again with `userApproved: true` and a rewriter that returns ticket URLs. The function is pure: always re-render from the original HTML.
- Other helpers: `cleanDisplayText` / `analyzeDisplayText` (sender names, subjects), `analyzeAttachment` / `isPreviewableImage`, `parseMailto` (only to/cc/bcc/subject/body), `linkifySegments`.

### Image sources

| `images.kind` | CSP `img-src` | Accepted rewriter output |
| --- | --- | --- |
| `none` (default) | `data:` | — |
| `proxy` | `data: <origin>` | URLs on that exact origin (https, or http://localhost in dev) |
| `batch` | `data: blob:` | `blob:` URLs |
| `direct` | `data: https:` | the original https URL (http blocked). Ignored unless `loadThroughProxy` is false |

The rewriter's output is validated, so a buggy or compromised rewriter cannot inject other URLs. Inline raster `data:` URLs are always accepted.

### CSP nonce

The app page sends `style-src 'self' 'nonce-<per-request>'` and `style-src-attr 'unsafe-inline'`. An `about:srcdoc` iframe **inherits the parent page's CSP** (it shares the parent's policy container), so inside the email iframe both policies apply: the page's and the iframe's own meta CSP. A `<style>` element without the page's nonce would be blocked. Pass **the current page's CSP nonce** as `styleNonce`: it is validated (16–64 base64/base64url characters, otherwise `InvalidStyleNonceError`) and set as a DOM attribute on every `<style>` of the output (our base style and the email's sanitized style blocks). The email cannot bring its own nonce: `nonce` attributes from the email are removed. Inline `style=""` attributes keep working through the page's `style-src-attr 'unsafe-inline'` plus the iframe's `style-src 'unsafe-inline'`.

## Pipeline (in this order)

0. **Limits** (`limits.ts`): HTML length, nesting depth estimated in a linear pass before parsing, element count after parsing. Over a limit → text view.
1. **Proton sanitizer** (`vendor/proton-sanitize`, adapter `proton.ts`): `protonizer` config renames every loading attribute (`src` → `proton-src`…) and `url(` → `proton-url(`, and neutralizes fixed/absolute positioning.
2. **Our removals** (`sanitize.ts`): `base`, `meta`, `link`, every form control (plus defence in depth: scripts, frames, plugins, SVG, MathML, `template`…). `form`, `button`, `label`, `output` are unwrapped (their text stays). Loading, submitting and focus-stealing attributes are removed.
3. **Links** (`links.ts`): `target` removed, `rel="noopener noreferrer"`. Only absolute http/https/mailto hrefs and `#fragment` survive.
4. **Inline images** (`images.ts`): raster `data:image/*` always; `cid:` via the caller's map.
5. **Trackers, always** (`trackers/`, `image-policy.ts`): MailTrackerBlocker list + spy-pixel rule, on `<img>` src/srcset, `background` and CSS `url()` (inline styles and `<style>`). A tracking `<img>` is removed with its element. Reported by service.
6. **Clean links** with `@protontech/tidy-url` (`allowAMP: true`). The result must still be http(s).
7. **Remote images**: off unless allowed (`shouldLoadRemoteImages`), and then only as the URL the image source returns. Relative, protocol-relative and non-http(s) URLs are always removed. `@import` and `@font-face` URLs never load.
8. **Document** (`document.ts`): fresh document; `<head>` starts with the CSP meta, then `referrer=no-referrer`, `x-dns-prefetch-control: off`, base style, then the email's styles.
9. **Iframe** (`frame.ts`): `IFRAME_SANDBOX = "allow-same-origin"`, `attachFrame`, `findLinkFromEvent`.
10. **Link confirmation**: `resolveLinkForOpening`, `analyzeLink`, `needsConfirmation`.
11. **Privacy report** (`report.ts`).
12. **Text view** (`text.ts`): Text nodes and `<a>` elements, never HTML strings.
13. **Trusted senders**: `senderTrusted` in `shouldLoadRemoteImages`.

Finally, `verify.ts` re-parses the serialized srcdoc and checks that it is inert (CSP first, no handlers, no foreign elements, only allowed URLs). On any finding the text view is shown (`fallbackReason: "integrity-check-failed"`), as a defence against mutation XSS.

## Decisions

- **`data:image/svg+xml` is never shown**, inline or in CSS. In `<img>` an SVG cannot run scripts or load resources, but it is the only image format that is a document, has had image-mode browser bugs, can be a CPU bomb, and Gmail and Outlook do not render it, so legitimate mail does not rely on it. Only raster `data:` types are accepted (png, jpeg, gif, webp, avif, bmp, ico).
- **Email `blob:` URLs are always blocked**: they would point at our own origin's blobs. `blob:` is allowed in the CSP only in batch mode, where our app creates them.
- **The BFF extracts with an injected parser instead of the full pipeline.** Proton's module uses DOMPurify's default instance, which binds to the global `window` at import time. On a server there is none, and faking one for the whole process is worse. Extraction is read-only. It parses inertly, walks the same references the browser restores, skips the subtrees whose content never reaches the output (`DROPPED_WITH_CONTENT`), normalizes CSS with Proton's pure `escape.ts`, and decides with the same `decideImage`. Tests check on the hostile corpus that the browser never wants an image the BFF would not ticket. The parser must be inert and spec-compliant: jsdom's `DOMParser` (parse5). Not linkedom: it doesn't follow the HTML spec for fragments (`<p>a</p><img>` leaves the `<img>` outside the document element), and most email bodies are fragments.
- **Tracker matching is on the URL**, not the whole `<img>` tag as upstream does, so all three places agree. Inputs over 4096 characters are truncated for matching. Image URLs over 2048 characters are blocked anyway.
- **"ask" and "trusted-senders" behave the same at render time.** Both load when the user approved or the sender is trusted; the difference is UI (whether to offer the button).
- **`linkConfirmation: "never"` still confirms deceptive links** (domain mismatch, punycode, userinfo, IP). Hiding a phishing warning is not a privacy preference. Plain `http` alone does not trigger it.
- **ZWJ/ZWNJ are kept** in display text (emoji sequences, Persian and Indic scripts). Every other zero-width or bidi control is removed.
- **Fonts**: `font-src data:` only. Remote web fonts never load, not even through the proxy.

## Tests

`bun run test` runs two vitest projects:

- `mail-render`: unit tests for every module, plus the hostile corpus (`security/hostile-emails/`). Every fixture is rendered with images off, images on and as text, then checked by an independent, stricter scanner (`test/network-sinks.ts`): no executable content and no URL-bearing attribute or CSS `url()` other than inline rasters (images off) or proxy URLs (images on).
- `proton-upstream`: Proton's own tests, unmodified, against our DOMPurify.

The trackers and extractor tests run in a `node` environment to prove they need no global DOM.

## TODO

- **Playwright** in Chromium and Firefox: render every corpus fixture in a real `<iframe sandbox srcdoc>` and assert **0 network requests** with images off and requests only to the proxy origin with images on. jsdom does not implement `srcdoc`, CSP or resource loading, so this layer is still missing. Not installed yet.
- **Trusted Types**: `iframe.srcdoc` is a TrustedHTML sink. If the app enables `require-trusted-types-for 'script'`, it needs a policy (e.g. `mail-render`) that wraps `result.srcdoc`.
- Spy-pixel extras (hidden images with `display:none`/`opacity:0`) were left out on purpose: the current rule was measured on ~3,000 emails and new rules need the same measurement.
