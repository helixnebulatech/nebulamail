// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The rich editor's labels, apart from the composer so screens that only
// show an editor (vacation reply) don't load the whole composer.

import type { RichTextLabels } from "@nebulamail/ui";

import { m } from "../../paraglide/messages.js";

export const editorLabels = (): RichTextLabels => ({
  align: m.compose_align(),
  alignCenter: m.compose_align_center(),
  alignJustify: m.compose_align_justify(),
  alignLeft: m.compose_align_left(),
  alignRight: m.compose_align_right(),
  bold: m.compose_bold(),
  bulletList: m.compose_bullet_list(),
  clearFormatting: m.compose_clear_formatting(),
  codeBlock: m.compose_code_block(),
  colorCustom: m.compose_color_custom(),
  directionLtr: m.compose_direction_ltr(),
  directionRtl: m.compose_direction_rtl(),
  emoji: m.compose_emoji(),
  font: m.compose_font(),
  fontDefault: m.compose_font_default(),
  fontSize: m.compose_font_size(),
  fontSizeDefault: m.compose_font_size_default(),
  insertImage: m.compose_insert_image(),
  formattingOptions: m.compose_formatting_options(),
  more: m.compose_more_tools(),
  colorNone: m.compose_color_none(),
  colors: {
    blue: m.compose_color_blue(),
    gray: m.compose_color_gray(),
    green: m.compose_color_green(),
    orange: m.compose_color_orange(),
    pink: m.compose_color_pink(),
    purple: m.compose_color_purple(),
    red: m.compose_color_red(),
    yellow: m.compose_color_yellow(),
  },
  heading1: m.compose_heading1(),
  heading2: m.compose_heading2(),
  heading3: m.compose_heading3(),
  highlight: m.compose_highlight(),
  horizontalRule: m.compose_horizontal_rule(),
  italic: m.compose_italic(),
  link: m.compose_link(),
  linkApply: m.compose_link_apply(),
  linkCancel: m.compose_link_cancel(),
  linkInvalid: m.compose_link_invalid(),
  linkRemove: m.compose_link_remove(),
  linkUrl: m.compose_link_url(),
  orderedList: m.compose_ordered_list(),
  paragraph: m.compose_paragraph(),
  quote: m.compose_quote(),
  redo: m.compose_redo(),
  strike: m.compose_strike(),
  textColor: m.compose_text_color(),
  textStyle: m.compose_text_style(),
  hideQuote: m.compose_hide_quote(),
  showQuote: m.compose_show_quote(),
  toolbar: m.compose_toolbar(),
  underline: m.compose_underline(),
  undo: m.compose_undo(),
});
