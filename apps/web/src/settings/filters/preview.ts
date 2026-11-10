// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "Test against existing mail": a rule's conditions as an Email/query
// filter. Only approximate: JMAP searches words (Stalwart: whole words),
// has no wildcards or regexes, and the server's Sieve sees the raw headers.

import type {
  EmailFilter,
  EmailFilterCondition,
} from "@nebulamail/jmap-client";

import type { Condition, Rule } from "./model";

export interface PreviewFilter {
  readonly filter: EmailFilter;
  /** Some condition could only be approximated or was left out. */
  readonly approximate: boolean;
}

/** The longest run of text between `*` / `?` wildcards. */
const literalPart = (pattern: string): string => {
  let best = "";
  for (const part of pattern.split(/[*?]/u)) {
    if (part.length > best.length) {
      best = part;
    }
  }
  return best.trim();
};

const conditionFilter = (
  condition: Condition
): { filter: EmailFilter; exact: boolean } | null => {
  switch (condition.kind) {
    case "size": {
      const bytes = condition.kilobytes * 1024;
      return {
        exact: true,
        filter:
          condition.op === "over" ? { minSize: bytes + 1 } : { maxSize: bytes },
      };
    }
    case "attachment": {
      return { exact: true, filter: { hasAttachment: !condition.negate } };
    }
    case "match": {
      if (condition.op === "regex") {
        return null;
      }
      const text =
        condition.op === "matches"
          ? literalPart(condition.value)
          : condition.value.trim();
      if (text === "") {
        return null;
      }
      const exact = condition.op === "contains";
      let filter: EmailFilter;
      switch (condition.field) {
        case "from":
        case "subject":
        case "body": {
          const key: keyof EmailFilterCondition = condition.field;
          filter = { [key]: text };
          break;
        }
        case "recipient": {
          filter = { conditions: [{ to: text }, { cc: text }], operator: "OR" };
          break;
        }
        case "listId": {
          filter = { header: ["List-Id", text] };
          break;
        }
        case "header": {
          filter = { header: [condition.header, text] };
          break;
        }
        default: {
          return condition.field satisfies never;
        }
      }
      return {
        exact,
        filter: condition.negate
          ? { conditions: [filter], operator: "NOT" }
          : filter,
      };
    }
    default: {
      return condition satisfies never;
    }
  }
};

/** `null`: nothing in the rule can be searched (only regexes, say). */
export const previewFilter = (rule: Rule): PreviewFilter | null => {
  const parts = rule.conditions.map(conditionFilter);
  const usable = parts.filter((part) => part !== null);
  if (rule.conditions.length > 0 && usable.length === 0) {
    return null;
  }
  const approximate =
    usable.length < parts.length || usable.some((part) => !part.exact);
  const filters = usable.map((part) => part.filter);
  if (filters.length === 0) {
    return { approximate, filter: {} };
  }
  if (filters.length === 1) {
    return { approximate, filter: filters[0] ?? {} };
  }
  return {
    approximate,
    filter: {
      conditions: filters,
      operator: rule.match === "all" ? "AND" : "OR",
    },
  };
};
