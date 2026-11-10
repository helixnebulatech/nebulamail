// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Typed JMAP client (RFC 8620/8621). Public entry point.
//
// This file is the package's only export (package.json "exports"), so it is
// a barrel by design. The module count the rule sees is mostly zod, which
// every consumer loads anyway.
/* oxlint-disable oxc/no-barrel-file */

export {
  CAPABILITIES,
  SEND_WHEN_SIGNED_OUT,
  SEND_WHEN_SIGNED_OUT_PATCH,
} from "./capabilities";
export type { Capability } from "./capabilities";
export { JmapClient } from "./client";
export type {
  ConnectAuth,
  ConnectBaseOptions,
  ConnectOptions,
  RequestOptions,
} from "./client";
export { JmapError, isJmapError, isMethodError } from "./errors";
export type {
  JmapErrorDetail,
  JmapErrorKind,
  MethodErrorType,
  RequestErrorType,
} from "./errors";
export {
  AccountId,
  AddressBookId,
  BlobId,
  CalendarEventId,
  CalendarId,
  ContactCardId,
  CreationId,
  EmailId,
  EmailSubmissionId,
  IdentityId,
  MailboxId,
  PrincipalId,
  QuotaId,
  State,
  ThreadId,
} from "./ids";
export type * from "./mail";
export type * from "./contacts";
export {
  SieveScriptId,
  VACATION_SCRIPT_NAME,
  sieveCapability,
  sieveErrorPosition,
} from "./sieve";
export type * from "./sieve";
export { FileNodeId, FileNodeSchema, fileNodeCreate } from "./filenode";
export type * from "./filenode";
export { ParsedCardSchema } from "./contacts";
export type * from "./mdn";
export type * from "./principals";
export type * from "./quota";
export type * from "./calendars";
export {
  AlertSchema,
  BRIDGE_CALENDAR,
  CALENDAR_CAPABILITIES,
  bridgePatchKey,
  ICalComponentSchema,
  ICalPropertySchema,
  RelationSchema,
} from "./calendars";
export type * from "./tasks";
export {
  JMAP_TASKS_CAPABILITY,
  StoredTaskSchema,
  TASK_CAPABILITIES,
  TASK_OCCURRENCE_PROPERTIES,
  TASK_PROGRESS,
  TASK_PROPERTIES,
  TASK_TYPE,
  TaskSchema,
  completedAt,
  completedProperty,
  formatICalUtc,
  icalProperties,
  icalValue,
  isTask,
  parentUids,
  parseICalUtc,
  taskUtc,
  withICalProperty,
} from "./tasks";
export {
  CalendarAlertSchema,
  EMAIL_PUSH,
  PushVerificationSchema,
  WEBPUSH_VAPID,
} from "./push-subscription";
export type * from "./push-subscription";
export { DEFAULT_EMAIL_PROPERTIES, IMAP_NAME_PROPERTY } from "./mail";
export type {
  BlobCopyArgs,
  BlobCopyResponse,
  EmailImportResponse,
  EmailSubmissionSetResponse,
  MailboxChangesResponse,
  MethodArgs,
  MethodMap,
  MethodName,
  ResponseFor,
  SearchSnippetGetResponse,
  Selected,
} from "./methods";
export { keywordPatch, mailboxPatch } from "./patch";
export { stateChanges } from "./push";
export type { PushOptions, StateChange } from "./push";
export { RequestBuilder } from "./request";
export type {
  CallArgs,
  CallHandle,
  PointerValue,
  Resolved,
  ResultReference,
  Settled,
  SettledResults,
} from "./request";
export {
  hasAccountCapability,
  hasCapability,
  mailCapability,
  parseSession,
  primaryAccountId,
  sendsWhenSignedOut,
  serverAlerts,
  submissionCapability,
} from "./session";
export type {
  Account,
  CoreCapability,
  MailAccountCapability,
  ServerAlert,
  Session,
  SubmissionAccountCapability,
} from "./session";
export { feedSse, initialSseState, parseSse } from "./sse";
export type { SseEvent, SseState } from "./sse";
export type * from "./standard";
export type { FetchLike, GetAccessToken, TokenRequest } from "./transport";
export type {
  DownloadOptions,
  UploadBody,
  UploadOptions,
  UploadResult,
} from "./blob";
export type { XhrFactory, XhrLike, XhrProgress, XhrUploads } from "./xhr";
export { encodeTemplateValue, expandTemplate } from "./url";
