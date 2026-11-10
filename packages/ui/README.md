<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# @nebulamail/ui · Helix

Helix is the design system of Nebula Mail. This package is its source of truth.

Stack: StyleX (`defineVars`, `createTheme`, `create`) for every style, Base UI for behaviour and accessibility, lucide icons, Geist Sans. The app imports UI **only** from `@nebulamail/ui`, never from Base UI directly.

Catalog: run the dev server and open `/dev/ui` (every component and state, light and dark). Production builds answer 404 there.

## Principles

1. **Visible privacy.** Protection isn't hidden in settings: you can see it in every email (shield, report, counter), in one line.
2. **Glow with meaning.** Surfaces are neutral and calm. The nebula gradient only shows up where there is action or protection: compose, focus, shield, progress. The motif (two crossing strands + a four-point spark) appears at most once per area.
3. **Keyboard first.** Everything has a shortcut and shows it (keys in buttons, tooltips and menus; ⌘K / Ctrl K).
4. **One family.** Mail, Calendar, Drive and Contacts share stroke, radii and voice; only the centre of the mark changes.

Voice: short, warm, informal "tú" sentences. "3 rastreadores bloqueados · Ver informe", never "¡ALERTA!".

## Tokens (`@nebulamail/ui/tokens.stylex`)

| Group | Contents |
| --- | --- |
| `colors` | `bg canvas side raised hover line line2 fg fg2 fg3 acc accHi accFg accText accSoft sel selFg star starSoft starText ink inkFg ok okSoft warn warnSoft danger dangerFg dangerSoft info infoSoft paper paperFg n1 n2 n3 core activeFill subtle focus scrim glass` (+ `fgMuted`, alias of `fg3`) |
| `shadow` | `sh1` (active row, tab), `sh2` (floating panels), `sh3` (menus, composer, dialogs), `halo` (brand only), `primary`, `focusHalo`. Dark uses deep shadows plus a hairline ring |
| `gradient` | `nebula` (#D61F6D → #EE5A4B → #F5A524), `strand`, `strandVertical`, `compose`, `glow` |
| `tint` | lightness/saturation for avatar and label colours derived from a hue |
| `space` | `xxs 2 · xs 4 · s6 · sm 8 · s10 · md 12 · s14 · lg 16 · s20 · xl 24 · xxl 32 · xxxl 48` |
| `radius` | `xs 5 · sm 6 · md 8` (controls) `· control 9 · lg 12` (cards, menus, rows) `· xl 16` (panels, dialogs, composer) `· pill` |
| `font` | `sans` ("Geist Variable" + system fallbacks), `mono`, sizes `11 / 12 / 13.5 (UI) / 14.5 (reading) / 18 / 22 / 28 / 34`, weights `400 500 550 600 700 750`, line heights, tracking (`trackingBrand` −0.045em) |
| `motion` | `fast 120 · normal 180 · slow 300 · pulse 1200` ms, `easing`, `easingIn`, `easingSpring`. Durations become ~0 with `prefers-reduced-motion` |
| `zIndex` | `base raised sticky popover overlay dialog toast tooltip` |
| `density` | `navItem row input tab menuItem` (compact by default; `comfortable` is a theme) |

Colour values come from `helix-src/tokens.mjs`, where the WCAG contrast pairs are checked. Danger is orange-red (`#C2300A` / `#FF7A59`) so it never looks like the brand.

## Themes

`themes.ts` exports `lightTheme`, `darkTheme`, `systemTheme` (all as `themes[mode]`), `densities` and `themeProps(mode, density)`.

- `"system"` is one theme whose values switch with `@media (prefers-color-scheme: dark)`. Light, dark and system are all plain classes, so the server renders the right one from the preferences cookie: **no flash and no script**.
- `themeProps` also sets `color-scheme` and the document base (canvas background, text colour, Geist, 13.5 px).
- `<ThemeScope mode="dark">` themes a subtree (e.g. a preview). Menus, dialogs, tooltips and toasts opened inside portal into the scope, so they use its theme.
- High contrast: TODO (a fourth mode).

What the app does in `__root.tsx`:

```tsx
import { UIProvider, themeProps } from "@nebulamail/ui";

import fontsCss from "@nebulamail/ui/fonts.css?url";

// head().links: { href: fontsCss, rel: "stylesheet" }
<html
  lang={getLocale()}
  {...themeProps(preferences.theme, preferences.density)}
>
  <body>
    <UIProvider nonce={nonce}>
      <Outlet />
    </UIProvider>
  </body>
</html>;
```

`UIProvider` configures Base UI for the strict CSP (no inline `<style>` elements; the nonce goes to Base UI's opt-in inline scripts), shares tooltip delays and provides toasts.

## Fonts

`@nebulamail/ui/fonts.css` declares "Geist Variable" from `@fontsource-variable/geist` (SIL OFL 1.1), latin and latin-ext, normal style only. Served from `'self'` (CSP `font-src 'self'`); no network fonts.

## Components

| Component | API (short) |
| --- | --- |
| `Mark` | `size core? ("spark" ring dot layers person none) tone? ("brand" mono onDark calendar drive contacts) label?` |
| `AppIcon` | `size core? light? label?` |
| `Wordmark` | `product? ("mail") size? (sm md lg xl)`: "**nebula** mail" |
| `Spark`, `HelixLine`, `ShieldGlyph` | motif glyphs: four-point spark, double strand, shield with spark |
| `Button` | `variant (primary secondary ghost danger dangerSoft ink) size (sm md lg) icon? iconEnd? shortcut? loading? pressed? fullWidth?` + Base UI button props |
| `IconButton` | `icon label (required) size? (xs sm md) variant? shortcut? tooltip? pressed? accent?` |
| `ComposeButton` | `children (label) shortcut? collapsed?`: spark + halo |
| `Kbd`, `Shortcut` | `Kbd children tone?` · `Shortcut keys tone?` (⌘ ⇧ ⌥ ↵ ↑ ↓ become icons) |
| `Badge` | `tone (neutral brand ok warn danger info star) icon? mark?` |
| `Tag` | `hue children`: user label colours |
| `Avatar` | `name colorKey? src? size? official? unread? decorative?` (+ `initials`, `hueFor`) |
| `Input`, `SearchField`, `Field` | `Input icon? end? invalid?` · `SearchField label hint?` · `Field label description? error? disabled? children` |
| `Checkbox`, `Switch` | Base UI props + `label?` (checkbox supports `indeterminate`) |
| `SegmentedControl` | `options value? defaultValue? onValueChange? label` (radio group) |
| `Tabs`, `TabList`, `Tab`, `TabPanel` | `Tab value icon? count?`; the active tab gets the double strand |
| `Menu`, `MenuItem`, `MenuSeparator`, `MenuGroup`, `ContextMenu` | `Menu trigger side? align?` · `MenuItem icon? shortcut? danger? disabled? onClick? keepOpen?` · `ContextMenu menu children` |
| `Tooltip` | `content shortcut? side? children (trigger)` |
| `Dialog`, `AlertDialog` | `Dialog trigger? title description? footer? closeLabel size?` · `AlertDialog title description confirmLabel cancelLabel onConfirm tone?` |
| `Popover`, `PopoverSection` | `trigger title description? icon? accent? size?` (accent = nebula strip, privacy report) |
| `UIProvider`, `ToastProvider`, `useToast` | `useToast().show({ title description? tone? action? timeout? countdown? priority? })`; the countdown is a shrinking strand |
| `Separator` | `orientation?` |
| `ScrollArea` | `children xstyle?` |
| `Spinner` | `size? tone? label?`: helix of dots, static with reduced motion |
| `ProgressBar`, `ProgressRing` | `value max? label? end?` · `value max? size? label` |
| `NavItem` | `icon? hue? count? mutedCount? active? href? render? onClick?` |
| `TextLink` | `href? render?` |
| `Panel`, `Card`, `Box` | rounded floating panel / quiet card / plain div, with `xstyle` escape hatch |
| `EmptyState` + `IllustrationInboxZero`, `IllustrationNoResults`, `IllustrationPrivacy` | `title children? illustration? action? headingLevel?` |
| `PrivacyShield` | `size? pulse? label?`: one glow pulse each time `pulse` changes, never on first render |
| `ThemeScope` | `mode density? children` |

Rules: no `className`/`style` passthrough (only `xstyle` on layout components); logical properties (RTL-safe); every interactive part has a visible focus ring (brand outline + stardust ring on buttons).
