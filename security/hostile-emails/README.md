<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Hostile emails

Test corpus for the rendering pipeline. **These files are attack data: never format, lint-fix or "clean" them** (a formatter rewrites the vectors).

Each fixture is an HTML body (`<name>.html`) plus metadata (`<name>.json`): `title`, `attacks` (what it tries), optional `references`, and optional `expect` (visible text that must survive, exact remote image list, spy-pixel count, tracker services…). `bidi-spoofing.json` also carries a subject, sender name, attachments and links for the display-text, attachment and link-warning checks.

| Fixture | Attacks |
| --- | --- |
| `css-keylogger` | PortSwigger "CSS: the bomb inside your inbox": attribute-selector exfiltration, fake login form, `formaction`, `input type=image` |
| `css-import-fonts` | `@import` (url/string/escaped), external `@font-face`, `<link rel=stylesheet>` |
| `css-url-tricks` | `var()`, `image-set()`, `-webkit-image-set()`, `cross-fade()`, escapes, comments, entities, `image()`, `src()`, `behavior`, `-moz-binding`, `expression()` |
| `scripts-and-handlers` | `<script>`, split tags, speculation rules, `on*` handlers |
| `dangerous-links` | `javascript:` (case, tab, newline, entities, control chars), `data:text/html`, `vbscript:`, relative, `file:`, `blob:`, `ping`, `mailto:?attach=` |
| `nested-contexts` | `iframe` (src/srcdoc/data:), `object`, `embed`, `applet`, frames, `portal`, `fencedframe` |
| `meta-base-link` | `meta refresh`, meta CSP override, `base href`, `link` prefetch/preload/icon, `manifest` |
| `overlays` | full-screen `fixed`/`absolute`, escaped `position`, `popover`, `dialog` |
| `svg-tricks` | `feImage`, `animate`/`set` `href`, external `use`, `image`, `foreignObject`, MathML, `data:` SVG |
| `image-attributes` | `srcset`, `picture/source`, `background`, `poster`, `lowsrc`, `data-src`, credentials in URLs, `http:` |
| `spy-pixels` | every branch of the spy-pixel rule and the spacer exceptions |
| `tracker-urls` | MailTrackerBlocker matches in `img`, `srcset` and CSS |
| `relative-and-schemes` | relative, protocol-relative, UNC, `blob:`, `file:`, `ftp:`, `ws:`… image URLs |
| `data-uris` | raster `data:` (shown), SVG and HTML `data:` (blocked) |
| `mxss` | mutation XSS and namespace confusion classics |
| `compose-fonts` | `font-family`/`font-size` with `url()`, `var()`, `calc()`, `attr()`, escapes, `@font-face` spoofing a system font, `dir` injection (the compose allowlist, §4.4) |
| `bidi-spoofing` | RLO in subject/sender/filename, double extensions, MIME mismatch, deceptive link text, punycode, userinfo |

`packages/mail-render/test/hostile-corpus.test.ts` (vitest + jsdom) checks every fixture:

- the srcdoc has no executable content and nothing that loads with images off (an independent "network sinks" scanner inspects every URL-bearing attribute and every CSS `url()`);
- with images on, only proxy URLs, never for a tracker;
- the text view is inert too;
- the BFF extractor would ticket every image the browser wants;
- the fixture's own expectations.

TODO: run the same corpus with **Playwright in Chromium and Firefox** inside a real `<iframe sandbox="allow-same-origin" srcdoc>`, asserting 0 network requests with images off and only proxy requests with images on, no script execution, no form submission and no navigation of the parent page. jsdom implements neither `srcdoc`, CSP nor resource loading.

To add next: Shazzer vectors, more PortSwigger email PoCs, `.eml` files once a MIME parser exists.
