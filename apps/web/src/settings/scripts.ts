// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The account's Sieve scripts (RFC 9661), for the filters and the vacation
// response. Domain functions: the settings UI calls these, never JMAP.
//
// Stalwart runs one script at a time, and its vacation response is a
// script of its own ("vacation"). So when our filters are active, they
// `include` the vacation script instead of letting it replace them
// (filtros y respuesta automática).

import type {
  AccountId,
  BlobId,
  JmapClient,
  SieveScript,
  SieveScriptId,
} from "@nebulamail/jmap-client";
import {
  VACATION_SCRIPT_NAME,
  sieveCapability,
  sieveErrorPosition,
} from "@nebulamail/jmap-client";

import { getClient } from "../mail/client";
import type { GenerateOptions } from "./filters/generate";
import { OUR_SCRIPT_NAME } from "./filters/generate";
import type { FilterSet } from "./filters/model";
import { parseFilterScript } from "./filters/parse";
import { withCrlf } from "./filters/sieve-string";

const DEFAULT_MAX_SCRIPT = 100 * 1024;

export interface ScriptInfo {
  readonly id: SieveScriptId;
  readonly name: string;
  readonly isActive: boolean;
  readonly blobId: BlobId;
}

export interface ScriptsState {
  /** Every script except the server's vacation one. */
  readonly scripts: readonly ScriptInfo[];
  /** The active script, unless it is the vacation one. */
  readonly active: ScriptInfo | null;
  readonly ours: ScriptInfo | null;
  /** Our script as rules; `null` if it was edited by hand (or is missing). */
  readonly oursFilters: FilterSet | null;
  /** Text of the active script when it isn't readable as our rules. */
  readonly rawContent: string | null;
  /** The server's vacation script is the active one. */
  readonly vacationScriptActive: boolean;
  /** How many forwards Stalwart allows per message (null: no limit known). */
  readonly maxRedirects: number | null;
}

export class ScriptError extends Error {
  /** Line of the problem in the script, when the server says. */
  readonly line: number | null;
  constructor(message: string, line: number | null) {
    super(message);
    this.name = "ScriptError";
    this.line = line;
  }
}

interface Context {
  readonly client: JmapClient;
  readonly accountId: AccountId;
}

const context = async (): Promise<Context> => {
  const client = await getClient();
  return { accountId: client.primaryAccountId(), client };
};

const maxScriptBytes = ({ client, accountId }: Context): number =>
  sieveCapability(client.session, accountId)?.maxSizeScript ??
  DEFAULT_MAX_SCRIPT;

const readScript = async (
  ctx: Context,
  script: Pick<SieveScript, "blobId">
): Promise<string> => {
  const response = await ctx.client.download({
    accountId: ctx.accountId,
    blobId: script.blobId,
    maxBytes: maxScriptBytes(ctx) * 2,
    name: "script.siv",
    type: "application/sieve",
  });
  return await response.text();
};

const info = (script: SieveScript): ScriptInfo => ({
  blobId: script.blobId,
  id: script.id,
  isActive: script.isActive,
  name: script.name ?? "",
});

const isVacation = (script: SieveScript): boolean =>
  script.name?.toLowerCase() === VACATION_SCRIPT_NAME;

export const loadScripts = async (): Promise<ScriptsState> => {
  const ctx = await context();
  const [result] = await ctx.client.request((b) => [
    b.call("SieveScript/get", { accountId: ctx.accountId }),
  ]);
  const scripts = result.list.filter((script) => !isVacation(script));
  const active = scripts.find((script) => script.isActive) ?? null;
  const ours =
    scripts.find((script) => script.name === OUR_SCRIPT_NAME) ?? null;
  // Another active script is shown as raw text whatever ours holds: both
  // are downloaded at once.
  const other = active !== null && active.id !== ours?.id ? active : null;
  const [oursText, otherText] = await Promise.all([
    ours === null ? null : readScript(ctx, ours),
    other === null ? null : readScript(ctx, other),
  ]);
  const oursFilters = oursText === null ? null : parseFilterScript(oursText);
  let rawContent: string | null = null;
  if (other !== null) {
    rawContent = otherText;
  } else if (ours !== null && oursFilters === null) {
    rawContent = oursText;
  }
  return {
    active: active === null ? null : info(active),
    maxRedirects:
      sieveCapability(ctx.client.session, ctx.accountId)?.maxNumberRedirects ??
      null,
    ours: ours === null ? null : info(ours),
    oursFilters,
    rawContent,
    scripts: scripts.map(info),
    vacationScriptActive: result.list.some(
      (script) => isVacation(script) && script.isActive
    ),
  };
};

/**
 * How our rules name folders on this server: by id only where it has
 * `mailboxid` (Stalwart); the bridge doesn't announce it (its mailbox ids
 * aren't the server's), so there folders go by path.
 */
export const filterOptions = async (): Promise<GenerateOptions> => {
  const ctx = await context();
  const extensions =
    sieveCapability(ctx.client.session, ctx.accountId)?.sieveExtensions ?? [];
  return { mailboxIds: extensions.includes("mailboxid") };
};

/** Our rules are the active script and run the vacation response. */
export const vacationIncluded = (state: ScriptsState): boolean =>
  state.active !== null &&
  state.active.id === state.ours?.id &&
  state.oursFilters?.vacation === true;

/**
 * Uploads `content` with CRLF line breaks (RFC 5228 §8.1), has the server
 * validate it, then saves it as script `id` (or a new one named `name`)
 * and, if asked, activates it. Throws ScriptError with the server's
 * message (and line) if it's invalid.
 */
export const saveScript = async (options: {
  readonly id: SieveScriptId | null;
  readonly name: string;
  readonly content: string;
  readonly activate: boolean;
}): Promise<void> => {
  const ctx = await context();
  const bytes = new TextEncoder().encode(withCrlf(options.content));
  if (bytes.byteLength > maxScriptBytes(ctx)) {
    throw new ScriptError("tooLarge", null);
  }
  const upload = await ctx.client.upload(
    ctx.accountId,
    new Blob([bytes], { type: "application/sieve" }),
    { type: "application/sieve" }
  );
  const { blobId } = upload;
  const [validation] = await ctx.client.request((b) => [
    b.call("SieveScript/validate", { accountId: ctx.accountId, blobId }),
  ]);
  if (validation.error !== null) {
    const message = validation.error.description ?? validation.error.type;
    throw new ScriptError(message, sieveErrorPosition(message)?.line ?? null);
  }
  const activate = options.activate
    ? { onSuccessActivateScript: options.id ?? "#script" }
    : {};
  const [set] = await ctx.client.request((b) => [
    b.call("SieveScript/set", {
      accountId: ctx.accountId,
      ...(options.id === null
        ? { create: { script: { blobId, name: options.name } } }
        : { update: { [options.id]: { blobId } } }),
      ...activate,
    }),
  ]);
  const error =
    Object.values(set.notCreated)[0] ?? Object.values(set.notUpdated)[0];
  if (error !== undefined) {
    const message = error.description ?? error.type;
    throw new ScriptError(message, sieveErrorPosition(message)?.line ?? null);
  }
};

// --- Several scripts (RFC 9661) ---------------------------------------------

/** The text of any script (for the Sieve editor). */
export const readScriptText = async (
  script: Pick<ScriptInfo, "blobId">
): Promise<string> => await readScript(await context(), script);

/** Default for servers that don't say (RFC 9661 suggests at least 512). */
const DEFAULT_MAX_NAME = 512;

export type ScriptNameProblem = "empty" | "tooLong" | "reserved" | "taken";

/** Names keep to one line without control or invisible format characters. */
export const cleanScriptName = (name: string): string =>
  name.replaceAll(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, "").trim();

/** What is wrong with a name for a new or renamed script, if anything. */
export const scriptNameProblem = (
  name: string,
  others: readonly Pick<ScriptInfo, "name">[],
  maxBytes: number = DEFAULT_MAX_NAME
): ScriptNameProblem | null => {
  const clean = cleanScriptName(name);
  if (clean === "") {
    return "empty";
  }
  if (new TextEncoder().encode(clean).byteLength > maxBytes) {
    return "tooLong";
  }
  const lower = clean.toLowerCase();
  if (lower === VACATION_SCRIPT_NAME || clean === OUR_SCRIPT_NAME) {
    return "reserved";
  }
  return others.some((script) => script.name.toLowerCase() === lower)
    ? "taken"
    : null;
};

export const maxScriptNameBytes = async (): Promise<number> => {
  const ctx = await context();
  return (
    sieveCapability(ctx.client.session, ctx.accountId)?.maxSizeScriptName ??
    DEFAULT_MAX_NAME
  );
};

/** A starting point for a new hand-written script. */
export const NEW_SCRIPT_TEMPLATE = `require ["fileinto"];

# Example: file mail from a list into a folder.
# if header :contains "list-id" "news.example.com" {
#   fileinto "News";
# }
`;

interface SetErrorLike {
  readonly type: string;
  readonly description?: string | null | undefined;
}

type ErrorMap = Readonly<Partial<Record<string, SetErrorLike>>>;

const setError = (set: {
  readonly notCreated: ErrorMap;
  readonly notUpdated: ErrorMap;
  readonly notDestroyed: ErrorMap;
}): ScriptError | null => {
  const error =
    Object.values(set.notCreated)[0] ??
    Object.values(set.notUpdated)[0] ??
    Object.values(set.notDestroyed)[0];
  if (error === undefined) {
    return null;
  }
  const message = error.description ?? error.type;
  return new ScriptError(message, sieveErrorPosition(message)?.line ?? null);
};

export const renameScript = async (
  id: SieveScriptId,
  name: string
): Promise<void> => {
  const ctx = await context();
  const [set] = await ctx.client.request((b) => [
    b.call("SieveScript/set", {
      accountId: ctx.accountId,
      update: { [id]: { name: cleanScriptName(name) } },
    }),
  ]);
  const error = setError(set);
  if (error !== null) {
    throw error;
  }
};

/**
 * Deletes a script. The active one can't be deleted (RFC 9661 §2.4
 * `sieveIsActive`; Stalwart says `scriptIsActive`), so it is switched off
 * first, in the same request.
 */
export const deleteScript = async (
  script: Pick<ScriptInfo, "id" | "isActive">
): Promise<void> => {
  const ctx = await context();
  const responses = await ctx.client.request((b) => [
    ...(script.isActive
      ? [
          b.call("SieveScript/set", {
            accountId: ctx.accountId,
            onSuccessDeactivateScript: true,
          }),
        ]
      : []),
    b.call("SieveScript/set", {
      accountId: ctx.accountId,
      destroy: [script.id],
    }),
  ]);
  for (const set of responses) {
    const error = setError(set);
    if (error !== null) {
      throw error;
    }
  }
};

/**
 * Makes `id` the account's active script, or switches every script off
 * (`null`). In Stalwart this also turns a vacation response off (its
 * script stops being the active one).
 */
export const activateScript = async (
  id: SieveScriptId | null
): Promise<void> => {
  const ctx = await context();
  const [set] = await ctx.client.request((b) => [
    b.call(
      "SieveScript/set",
      id === null
        ? { accountId: ctx.accountId, onSuccessDeactivateScript: true }
        : { accountId: ctx.accountId, onSuccessActivateScript: id }
    ),
  ]);
  const error = setError(set);
  if (error !== null) {
    throw error;
  }
};
