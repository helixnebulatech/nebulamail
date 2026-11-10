// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A birthday in words (contacts/dates.ts): in another calendar, also the
// Gregorian day it falls on.

import type { DateText } from "../../contacts/dates";
import { m } from "../../paraglide/messages.js";

export const dateWords = (text: DateText): string => {
  switch (text.kind) {
    case "gregorian": {
      return text.text;
    }
    case "converted": {
      return text.next
        ? m.contacts_date_next({ date: text.text, next: text.gregorian })
        : m.contacts_date_converted({
            date: text.text,
            gregorian: text.gregorian,
          });
    }
    case "raw": {
      return m.contacts_date_raw({
        calendar: text.calendar,
        date: text.text,
      });
    }
    default: {
      return text satisfies never;
    }
  }
};

export const DateWords = ({ text }: { readonly text: DateText }) => (
  <>{dateWords(text)}</>
);
