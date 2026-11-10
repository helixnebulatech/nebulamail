// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Shown while a screen is still loading (router `defaultPendingComponent`):
// the turning helixnebula swirl on the app background. It fades in after a
// short delay, so quick loads show nothing at all.

import { Spinner } from "@nebulamail/ui";
import { colors, motion } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";

import { m } from "../paraglide/messages.js";

const REDUCED = "@media (prefers-reduced-motion: reduce)";

const appear = stylex.keyframes({
  from: { opacity: 0 },
  to: { opacity: 1 },
});

const styles = stylex.create({
  wrap: {
    alignItems: "center",
    animationDelay: { default: "250ms", [REDUCED]: "0s" },
    animationDuration: motion.normal,
    animationFillMode: "both",
    animationName: appear,
    backgroundColor: colors.canvas,
    display: "flex",
    justifyContent: "center",
    minHeight: "100%",
    paddingBlock: "48px",
  },
});

const Loader = () => (
  <div {...stylex.props(styles.wrap)}>
    <Spinner label={m.app_loading()} />
  </div>
);

export default Loader;
