// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// DOM-free tracker URL matching. The same code
// runs in the browser (rendering), the BFF (never sign a tracker ticket) and
// the image proxy (re-check before downloading).

import { trimEndChar } from "../urls";
import { MTB_RULES } from "./data.generated";

export interface TrackerMatch {
  /** Service name from the list, or a fixed label for our own rules. */
  readonly service: string;
  readonly source: "mailtrackerblocker" | "custom";
}

export interface TrackerMatchOptions {
  /** Use the MailTrackerBlocker list (default true). */
  readonly mailTrackerBlocker?: boolean;
  /** Hosts treated as trackers (matches the host and its subdomains). */
  readonly blockedHosts?: readonly string[];
  /** Hosts exempt from the MailTrackerBlocker list. */
  readonly allowedHosts?: readonly string[];
}

/**
 * Only this many characters are matched. Several upstream patterns
 * (`/CI0/.+/.+=…`) backtrack polynomially; image URLs longer than
 * MAX_IMAGE_URL_LENGTH are blocked before they get here anyway.
 */
export const MAX_TRACKER_INPUT = 4096;

export const CUSTOM_TRACKER_SERVICE = "Custom list";

interface CompiledRule {
  readonly service: string;
  readonly test: (input: string) => boolean;
}

/**
 * Linear equivalents for upstream patterns that backtrack badly (found by
 * the adversarial timing test). Keyed by the exact upstream pattern; tests
 * check each one agrees with its regex.
 */
export const LINEAR_EQUIVALENTS: Readonly<
  Record<string, (input: string) => boolean>
> = {
  // "/CI0/.+/.+=[0-9][0-9][0-9]" (Amazon SES) is cubic on "/CI0//CI0/…".
  // The earliest "/CI0/" leaves the most room, so it is enough to check it.
  // `.` does not match line terminators: no match may span one.
  "/CI0/.+/.+=[0-9][0-9][0-9]": (input) => {
    for (const line of input.split(/[\n\r\u2028\u2029]/u)) {
      const lower = line.toLowerCase();
      const start = lower.indexOf("/ci0/");
      if (start !== -1) {
        const slash = lower.indexOf("/", start + 6);
        if (slash !== -1 && /[=]\d{3}/u.test(lower.slice(slash + 2))) {
          return true;
        }
      }
    }
    return false;
  },
};

let compiled: readonly CompiledRule[] | undefined;

const rules = (): readonly CompiledRule[] => {
  compiled ??= MTB_RULES.flatMap(([service, patterns]) =>
    patterns.map((pattern) => {
      const regex = new RegExp(pattern, "iu");
      return {
        service,
        test:
          LINEAR_EQUIVALENTS[pattern] ?? ((input: string) => regex.test(input)),
      };
    })
  );
  return compiled;
};

const normalizeHostEntry = (entry: string): string =>
  trimEndChar(entry.trim().toLowerCase(), ".").replace(/^\.+/u, "");

/** `host` equals `entry` or is a subdomain of it. */
export const hostMatches = (host: string, entry: string): boolean => {
  const normalized = normalizeHostEntry(entry);
  if (normalized === "") {
    return false;
  }
  const h = trimEndChar(host.toLowerCase(), ".");
  return h === normalized || h.endsWith(`.${normalized}`);
};

const hostOf = (url: string): string | null => {
  try {
    return new URL(url).hostname;
  } catch {
    return null;
  }
};

/**
 * Returns the tracker service if `url` is a known tracker, else null.
 * Unlike MailTrackerBlocker (which matches the whole `<img>` tag text), we
 * match the URL only, so the result does not depend on HTML attributes and
 * the browser, the BFF and the proxy agree.
 */
export const isTrackerUrl = (
  url: string,
  options: TrackerMatchOptions = {}
): TrackerMatch | null => {
  const host = hostOf(url);
  if (
    host !== null &&
    options.blockedHosts?.some((entry) => hostMatches(host, entry)) === true
  ) {
    return { service: CUSTOM_TRACKER_SERVICE, source: "custom" };
  }
  if (options.mailTrackerBlocker === false) {
    return null;
  }
  if (
    host !== null &&
    options.allowedHosts?.some((entry) => hostMatches(host, entry)) === true
  ) {
    return null;
  }
  const input =
    url.length > MAX_TRACKER_INPUT ? url.slice(0, MAX_TRACKER_INPUT) : url;
  for (const rule of rules()) {
    if (rule.test(input)) {
      return { service: rule.service, source: "mailtrackerblocker" };
    }
  }
  return null;
};
