// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The user's `preload` state from the image proxy, through the BFF
// (`POST /api/images/preload`, server/images/preload.ts)
// whether it is on, and whether their quota is full. A full quota
// (accepted risk H15: a sender can fill it on purpose) means new mail
// works like `cache` until `until`: images are fetched the first time
// they are shown, and the sender can tell when. Settings › Privacy and
// the privacy bar of each email say so.
//
// One query for both, so the reader doesn't ask again for every email.
// The static build has no BFF: always "unavailable".

import { queryOptions, useQuery } from "@tanstack/react-query";
import { z } from "zod";

const Full = z.object({ until: z.number().int().nonnegative().nullable() });

const Answer = z.union([
  z.object({ available: z.literal(false) }),
  z.object({
    available: z.literal(true),
    full: Full.nullable().optional(),
    optOut: z.boolean(),
  }),
]);

const TooMany = z.object({ error: z.literal("too-many-addresses") });

/** Full quota: preloading starts again at `until` (ms; null: unknown). */
export type PreloadFull = z.infer<typeof Full>;

export type PreloadState =
  /** More addresses than one signed request may carry (BFF answers 422). */
  | { readonly kind: "unavailable" | "too-many" }
  | {
      readonly kind: "ready";
      readonly optOut: boolean;
      readonly full: PreloadFull | null;
    };

const postToBff = async (body: {
  readonly optOut?: boolean;
}): Promise<PreloadState> => {
  const response = await fetch("/api/images/preload", {
    body: JSON.stringify(body),
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  if (
    response.status === 422 &&
    TooMany.safeParse(await response.json().catch(() => null)).success
  ) {
    return { kind: "too-many" };
  }
  if (!response.ok) {
    throw new Error(`Preload preference failed: HTTP ${response.status}`);
  }
  const answer = Answer.parse(await response.json());
  return answer.available
    ? { full: answer.full ?? null, kind: "ready", optOut: answer.optOut }
    : { kind: "unavailable" };
};

/** Read (no `optOut`) or change the user's preload choice. */
export const postPreload: typeof postToBff = import.meta.env.NEBULA_STATIC
  ? async () => await Promise.resolve({ kind: "unavailable" })
  : postToBff;

export const preloadStatusKey = ["preload-status"] as const;

export const preloadStatusQuery = () =>
  queryOptions({
    queryFn: async (): Promise<PreloadState> => {
      try {
        return await postPreload({});
      } catch {
        // No answer: hide the option and the warning rather than show a
        // wrong state.
        return { kind: "unavailable" };
      }
    },
    queryKey: preloadStatusKey,
    // The quota fills over days; a few minutes old is fine.
    staleTime: 5 * 60_000,
  });

export const usePreloadStatus = () => useQuery(preloadStatusQuery());

/** The full quota, if preload is on and full; else null. */
export const usePreloadFull = (enabled: boolean): PreloadFull | null => {
  const { data } = useQuery({ ...preloadStatusQuery(), enabled });
  return enabled && data?.kind === "ready" && !data.optOut
    ? (data.full ?? null)
    : null;
};
