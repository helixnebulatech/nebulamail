// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Settings › General › Writing: the format new messages
// start in and their default font and size. Synced like the rest.

import {
  COMPOSE_FONTS,
  COMPOSE_FONT_SIZES,
  isComposeFontId,
} from "@nebulamail/mail-render";
import { SegmentedControl, Select } from "@nebulamail/ui";
import { colors } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";

import { COMPOSE_FORMATS, useComposePrefs } from "../../mail/compose-prefs";
import type { ComposeFormat } from "../../mail/compose-prefs";
import { m } from "../../paraglide/messages.js";
import { SettingGroup, SettingRow } from "./section";

const styles = stylex.create({
  // The font menu shows each name in its own font.
  sample: (family: string) => ({ fontFamily: family }),
  sampleBox: {
    borderColor: colors.line,
    borderRadius: "8px",
    borderStyle: "solid",
    borderWidth: "1px",
    color: colors.fg,
    marginTop: "4px",
    paddingBlock: "8px",
    paddingInline: "12px",
  },
  sampleSize: (size: string) => ({ fontSize: size }),
});

const FORMAT_LABEL: Record<ComposeFormat, () => string> = {
  html: m.settings_compose_format_html,
  text: m.settings_compose_format_text,
};

export const WritingControls = () => {
  const prefs = useComposePrefs();
  const handleFormat = prefs.setFormat;
  const handleSize = prefs.setSize;
  const current = COMPOSE_FONTS.find((option) => option.id === prefs.font);
  return (
    <>
      <SettingGroup>
        <SettingRow
          description={m.settings_compose_format_hint()}
          label={m.settings_compose_format()}
        >
          <SegmentedControl
            label={m.settings_compose_format()}
            onValueChange={handleFormat}
            options={COMPOSE_FORMATS.map((value) => ({
              label: FORMAT_LABEL[value](),
              value,
            }))}
            value={prefs.format}
          />
        </SettingRow>
      </SettingGroup>
      <SettingGroup>
        <SettingRow label={m.settings_compose_font()}>
          <Select
            label={m.settings_compose_font()}
            onValueChange={(value) => {
              if (isComposeFontId(value)) {
                prefs.setFont(value);
              }
            }}
            options={COMPOSE_FONTS.map((option) => ({
              label: option.label,
              value: option.id,
            }))}
            value={prefs.font}
          />
          <Select
            label={m.settings_compose_font_size()}
            onValueChange={handleSize}
            options={COMPOSE_FONT_SIZES.map((size) => ({
              label: size.replace("px", " px"),
              value: size,
            }))}
            value={prefs.size}
          />
        </SettingRow>
        <p
          aria-hidden
          {...stylex.props(
            styles.sampleBox,
            styles.sample(current?.stack ?? "inherit"),
            styles.sampleSize(prefs.size)
          )}
        >
          {current?.label} · {prefs.size.replace("px", " px")}
        </p>
      </SettingGroup>
    </>
  );
};
