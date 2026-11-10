// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What can go wrong when scheduling a message (schedule.ts): one error
// type whose `failure` says what, so the UI can't mix them up.

export type ScheduleProblem = "invalid" | "past" | "tooFar";

export type ScheduleFailure =
  /** The time can't be scheduled (checked before anything is sent). */
  | { readonly kind: "time"; readonly problem: ScheduleProblem }
  /** Too late to cancel: the server already released it. */
  | { readonly kind: "alreadySent" }
  /** Cancelled already (another tab, or the undo toast). */
  | { readonly kind: "alreadyCanceled" }
  /**
   * «Cambiar hora» cancelled the old time but the new submission failed
   * without a refusal (network, method error). Unless it turned out
   * pending after all, the email is back in Drafts (compose.ts).
   */
  | { readonly kind: "interrupted" }
  /**
   * The server refused the new time («Cambiar hora»); `maximum`: its
   * latest, if it said. The email is back in Drafts (compose.ts).
   */
  | { readonly kind: "refused"; readonly maximum: Date | null };

export class ScheduleError extends Error {
  readonly failure: ScheduleFailure;

  constructor(
    failure: ScheduleFailure,
    description?: string | null,
    options?: ErrorOptions
  ) {
    super(description ?? `Scheduling failed: ${failure.kind}`, options);
    this.name = "ScheduleError";
    this.failure = failure;
  }
}

export const scheduleFailure = (error: unknown): ScheduleFailure | null =>
  error instanceof ScheduleError ? error.failure : null;
