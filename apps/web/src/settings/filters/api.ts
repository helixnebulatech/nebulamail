// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Filters domain functions: save the visual rules or a raw script, preview
// a rule against existing mail, and pre-fill a rule from an email.

import type { EmailId, SieveScriptId } from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  IMAP_NAME_PROPERTY,
  hasCapability,
  isMethodError,
} from "@nebulamail/jmap-client";

import { getClient } from "../../mail/client";
import type { ScriptsState } from "../scripts";
import { filterOptions, saveScript, vacationIncluded } from "../scripts";
import { OUR_SCRIPT_NAME, generateScript } from "./generate";
import type { ImapNames } from "./mailboxes";
import type { FilterSet } from "./model";
import { previewFilter } from "./preview";

/**
 * What the IMAP server calls each folder, which is what its Sieve wants.
 * Only the bridge knows; `null` elsewhere (nothing is asked).
 */
export const imapNames = async (): Promise<ImapNames> => {
  const client = await getClient();
  if (!hasCapability(client.session, CAPABILITIES.bridge)) {
    return null;
  }
  const [result] = await client.request((b) => [
    b.call("Mailbox/get", {
      accountId: client.primaryAccountId(),
      properties: [IMAP_NAME_PROPERTY],
    }),
  ]);
  return new Map(
    result.list.flatMap((box) => {
      const name = box[IMAP_NAME_PROPERTY];
      return name === undefined ? [] : [[box.id, name]];
    })
  );
};

/** Saves the rules as our script and makes it the active one. */
export const saveFilters = async (
  filters: FilterSet,
  state: ScriptsState
): Promise<void> => {
  // Keep the vacation response running: it was on, as its own script or
  // inside ours (scripts.ts explains why).
  const vacation = state.vacationScriptActive || vacationIncluded(state);
  await saveScript({
    activate: true,
    content: generateScript({ ...filters, vacation }, await filterOptions()),
    id: state.ours?.id ?? null,
    name: OUR_SCRIPT_NAME,
  });
};

/** Saves a hand-written script in place (it stays active if it was). */
export const saveRawScript = async (
  id: SieveScriptId,
  content: string,
  activate: boolean
): Promise<void> => {
  await saveScript({ activate, content, id, name: "" });
};

export interface PreviewEmail {
  readonly id: EmailId;
  readonly from: string;
  readonly subject: string;
  readonly receivedAt: string;
}

export interface PreviewResult {
  readonly emails: readonly PreviewEmail[];
  readonly total: number;
  readonly approximate: boolean;
}

export type PreviewOutcome =
  /** The rule can't be searched at all (only regular expressions, etc.). */
  | { readonly kind: "unsearchable" }
  /**
   * The server's search refused the filter (`unsupportedFilter`: too many
   * conditions, too long a text, or one it can't search, like the bridge
   * with `header`). Its limits aren't copied here: the server decides.
   */
  | { readonly kind: "tooComplex" }
  | { readonly kind: "done"; readonly result: PreviewResult };

const PREVIEW_LIMIT = 20;

/** Recent emails a rule would (roughly) match. */
export const previewRule = async (
  rule: Parameters<typeof previewFilter>[0]
): Promise<PreviewOutcome> => {
  const preview = previewFilter(rule);
  if (preview === null) {
    return { kind: "unsearchable" };
  }
  const client = await getClient();
  const accountId = client.primaryAccountId();
  try {
    // `request` throws the first error in call order: Email/query's (the
    // Email/get after it then fails with invalidResultReference).
    const [query, get] = await client.request((b) => {
      const q = b.call("Email/query", {
        accountId,
        calculateTotal: true,
        filter: preview.filter,
        limit: PREVIEW_LIMIT,
        sort: [{ isAscending: false, property: "receivedAt" }],
      });
      const g = b.call("Email/get", {
        "#ids": q.ref("/ids"),
        accountId,
        properties: ["from", "subject", "receivedAt"],
      });
      return [q, g] as const;
    });
    return {
      kind: "done",
      result: {
        approximate: preview.approximate,
        emails: get.list.map((email) => ({
          from: email.from?.[0]?.name ?? email.from?.[0]?.email ?? "",
          id: email.id,
          receivedAt: email.receivedAt,
          subject: email.subject ?? "",
        })),
        total: query.total ?? get.list.length,
      },
    };
  } catch (error) {
    if (isMethodError(error, "unsupportedFilter")) {
      return { kind: "tooComplex" };
    }
    throw error;
  }
};

export interface EmailFacts {
  readonly from: string;
  readonly listId: string;
  readonly subject: string;
}

/** `<news.shop.example>` → `news.shop.example` (RFC 2919). */
export const listIdOf = (header: string | null): string => {
  const match = /<(?<id>[^<>]+)>/u.exec(header ?? "");
  return (match?.groups?.id ?? header ?? "").trim();
};

/** Sender, list id and subject of an email, for "Create filter". */
export const emailFacts = async (id: EmailId): Promise<EmailFacts | null> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const [result] = await client.request((b) => [
    b.call("Email/get", {
      accountId,
      ids: [id],
      properties: ["from", "subject", "header:List-Id:asText"],
    }),
  ]);
  const [email] = result.list;
  if (email === undefined) {
    return null;
  }
  const listId = email["header:List-Id:asText"];
  return {
    from: email.from?.[0]?.email ?? "",
    listId: listIdOf(typeof listId === "string" ? listId : null),
    subject: email.subject ?? "",
  };
};
