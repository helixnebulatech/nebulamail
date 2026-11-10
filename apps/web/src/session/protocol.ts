// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Messages between the pages and the session worker (static build).
// Only pages of this origin can talk to the worker, but every
// message is still parsed, both ways.

import { z } from "zod";

export const RequestSchema = z.discriminatedUnion("op", [
  z.object({ id: z.number().int(), op: z.literal("status") }),
  z.object({
    forceRefresh: z.boolean(),
    id: z.number().int(),
    op: z.literal("grant"),
  }),
  z.object({
    clientId: z.string().min(1).max(256),
    code: z.string().min(1).max(4096),
    id: z.number().int(),
    op: z.literal("complete"),
    redirectUri: z.url(),
    verifier: z.string().min(43).max(128),
  }),
  z.object({ id: z.number().int(), op: z.literal("logout") }),
]);

export type WorkerRequest = z.infer<typeof RequestSchema>;

/** Distributive Omit, so each variant keeps its own fields. */
export type RequestBody = WorkerRequest extends infer Variant
  ? Variant extends WorkerRequest
    ? Omit<Variant, "id">
    : never
  : never;

export const WorkerErrorSchema = z.enum([
  /** No session (never logged in, logged out, refresh token rejected). */
  "expired",
  /** The server didn't answer properly: try again later. */
  "unavailable",
  /** A bad login callback (wrong code, no refresh token…). */
  "invalid",
]);

export type WorkerError = z.infer<typeof WorkerErrorSchema>;

export const TokenSchema = z.object({
  accessToken: z.string().min(1),
  expiresAt: z.number(),
});

export const ReplySchema = z.union([
  z.object({ id: z.number().int(), ok: z.literal(true), value: z.unknown() }),
  z.object({
    error: WorkerErrorSchema,
    id: z.number().int(),
    ok: z.literal(false),
  }),
]);

export type WorkerReply = z.infer<typeof ReplySchema>;

/** Sent to every page: the session ended (logout in a tab, refresh rejected). */
export const EventSchema = z.object({
  event: z.enum(["loggedIn", "loggedOut"]),
});

export type WorkerEvent = z.infer<typeof EventSchema>;
