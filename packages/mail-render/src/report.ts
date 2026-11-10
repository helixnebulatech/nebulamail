// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Privacy report: "N trackers blocked", grouped by
// service, plus cleaned links and blocked images. DOM-free.

import type { BlockedImageReason } from "./urls";

export const SPY_PIXEL_SERVICE = "Spy pixel";

export interface TrackerReportEntry {
  readonly service: string;
  /** How many references (img, srcset candidate, CSS url) were removed. */
  readonly count: number;
  /** Distinct URLs, for the details view. */
  readonly urls: readonly string[];
}

export interface CleanedLink {
  readonly original: string;
  readonly cleaned: string;
}

export interface PrivacyReport {
  /** Total tracker references removed (list matches + spy pixels). */
  readonly trackersBlocked: number;
  readonly trackers: readonly TrackerReportEntry[];
  readonly linksCleaned: readonly CleanedLink[];
  /** Distinct remote image URLs that were not loaded. */
  readonly remoteImagesBlocked: number;
  /** Distinct remote image URLs loaded through the image source. */
  readonly remoteImagesLoaded: number;
  /** Image references that can never load (relative, SVG, bad scheme…). */
  readonly unsafeImagesRemoved: Readonly<
    Partial<Record<BlockedImageReason | "import" | "font", number>>
  >;
  /** Links whose href was removed (javascript:, data:, relative…). */
  readonly unsafeLinksRemoved: number;
  /** Elements removed by our rules after Proton's sanitizer (forms, base…). */
  readonly elementsRemoved: number;
}

export class ReportBuilder {
  readonly #trackers = new Map<string, { count: number; urls: Set<string> }>();
  readonly #linksCleaned: CleanedLink[] = [];
  readonly #remote = new Set<string>();
  readonly #loaded = new Set<string>();
  readonly #unsafe: Partial<
    Record<BlockedImageReason | "import" | "font", number>
  > = {};
  #unsafeLinks = 0;
  #elementsRemoved = 0;

  tracker(service: string, url: string): void {
    const entry = this.#trackers.get(service) ?? {
      count: 0,
      urls: new Set<string>(),
    };
    entry.count += 1;
    entry.urls.add(url);
    this.#trackers.set(service, entry);
  }

  linkCleaned(original: string, cleaned: string): void {
    this.#linksCleaned.push({ cleaned, original });
  }

  remoteImage(url: string, loaded: boolean): void {
    this.#remote.add(url);
    if (loaded) {
      this.#loaded.add(url);
    }
  }

  unsafeImage(reason: BlockedImageReason | "import" | "font"): void {
    this.#unsafe[reason] = (this.#unsafe[reason] ?? 0) + 1;
  }

  unsafeLink(): void {
    this.#unsafeLinks += 1;
  }

  elementRemoved(count = 1): void {
    this.#elementsRemoved += count;
  }

  /** Remote image URLs (not trackers), in document order. */
  remoteUrls(): string[] {
    return [...this.#remote];
  }

  build(): PrivacyReport {
    const trackers = [...this.#trackers.entries()]
      .map(([service, { count, urls }]) => ({
        count,
        service,
        urls: [...urls],
      }))
      .toSorted(
        (a, b) => b.count - a.count || a.service.localeCompare(b.service)
      );
    return {
      elementsRemoved: this.#elementsRemoved,
      linksCleaned: [...this.#linksCleaned],
      remoteImagesBlocked: [...this.#remote].filter(
        (url) => !this.#loaded.has(url)
      ).length,
      remoteImagesLoaded: this.#loaded.size,
      trackers,
      trackersBlocked: trackers.reduce((sum, entry) => sum + entry.count, 0),
      unsafeImagesRemoved: { ...this.#unsafe },
      unsafeLinksRemoved: this.#unsafeLinks,
    };
  }
}
