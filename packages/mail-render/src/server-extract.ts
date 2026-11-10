// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Server-side image URL extraction with the parsing in a worker thread.
// Node, Bun and Deno only (`node:worker_threads`);
// never import it in the browser.
//
// Why a worker: parse5 (inside jsdom) has quadratic paths that no check on
// the string or on the parser's stack bounds; `"<table><a>x".repeat(160000)`
// (1.76 MB, under the 2 MB limit) takes about a minute with a stack of 4.
// On the main thread that would stop the whole server. Here each parse has
// a fixed deadline (2 s by default) and a memory limit; past either, the
// worker is terminated and the answer is `null`: no URLs (fail closed).
//
// The worker (`parse-worker.ts`) parses with the same parse5 and options as
// jsdom's DOMParser and sends back a light tree of the elements; the URLs
// are then picked by the same `extractFromElement` as with jsdom, so both
// give the same result (tests check it on the hostile corpus).

import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Worker } from "node:worker_threads";

import {
  EXTRACT_ATTRIBUTES,
  EXTRACT_TEXT_OF,
  extractFromElement,
  NO_IMAGES,
  skipsHtml,
} from "./extract";
import type { ExtractElement, ImageUrlExtractor } from "./extract";
import { DEFAULT_LIMITS } from "./limits";
import { PARSE_WORKER, WORKER_READY } from "./parse-worker";
import type { LightElement, ParseReply, ParseRequest } from "./parse-worker";

export type { ExtractedImages, ImageUrlExtractor } from "./extract";

export const DEFAULT_PARSE_TIMEOUT_MS = 2000;

export interface WorkerExtractorOptions {
  /** Deadline of one parse, from the moment a worker starts it. */
  readonly timeoutMs?: number;
  /** Worker threads (parses at the same time). Default 2. */
  readonly workers?: number;
  /** Parses waiting for a worker; more fail closed at once. Default 64. */
  readonly maxQueue?: number;
  /** Heap limit of each worker, in MB. Default 512. */
  readonly maxHeapMb?: number;
  /**
   * The worker's module. Default: `parse-worker.ts` next to this file, or
   * `@nebulamail/mail-render/parse-worker` from node_modules when this module
   * was bundled. A bundle without node_modules (Vercel) ships the worker
   * apart and passes it here.
   */
  readonly workerUrl?: URL | string;
}

export interface WorkerExtractor extends ImageUrlExtractor {
  /** Stop the workers; pending and later parses get `null`. */
  readonly close: () => Promise<void>;
}

/** A light element seen through the interface the extraction reads. */
class LightView implements ExtractElement {
  readonly #light: LightElement;
  #kids: LightView[] | undefined;

  constructor(light: LightElement) {
    this.#light = light;
  }

  get localName(): string {
    return this.#light.n;
  }

  get namespaceURI(): string {
    return this.#light.ns;
  }

  get textContent(): string {
    return this.#light.t ?? "";
  }

  get children(): readonly LightView[] {
    this.#kids ??= this.#light.c.map((child) => new LightView(child));
    return this.#kids;
  }

  getAttribute(name: string): string | null {
    return this.#light.a.find(([key]) => key === name)?.[1] ?? null;
  }
}

interface Task {
  readonly html: string;
  readonly maxDepth: number;
  readonly resolve: (reply: ParseReply | null) => void;
}

interface Slot {
  worker: Worker | undefined;
  /** The worker has loaded its modules (it said so). */
  ready: boolean;
  running:
    | { readonly task: Task; timer: NodeJS.Timeout | undefined }
    | undefined;
}

/** A cold worker that does not start within this fails the parse. */
const START_TIMEOUT_MS = 30_000;

/**
 * `parse-worker.ts` next to this file; when this module was bundled into a
 * server chunk (the BFF's build), the package's own copy in node_modules.
 * A path, not a URL object: under a DOM shim (tests) `URL` is not Node's.
 */
const defaultWorkerPath = (): string => {
  const beside = fileURLToPath(
    String(new URL("parse-worker.ts", import.meta.url))
  );
  return existsSync(beside)
    ? beside
    : fileURLToPath(
        import.meta.resolve("@nebulamail/mail-render/parse-worker")
      );
};

const positive = (value: number, name: string): number => {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new TypeError(`mail-render: ${name} must be a positive integer`);
  }
  return value;
};

/**
 * Extract image URLs with the parsing in worker threads. `extract` never
 * rejects: `null` means the email could not be parsed in time or within
 * the limits, so it gets no images.
 */
export const createWorkerExtractor = (
  options: WorkerExtractorOptions = {}
): WorkerExtractor => {
  const timeoutMs = positive(
    options.timeoutMs ?? DEFAULT_PARSE_TIMEOUT_MS,
    "timeoutMs"
  );
  const maxQueue = positive(options.maxQueue ?? 64, "maxQueue");
  const maxHeapMb = positive(options.maxHeapMb ?? 512, "maxHeapMb");
  const workerPath =
    options.workerUrl === undefined
      ? defaultWorkerPath()
      : fileURLToPath(String(options.workerUrl));
  const slots: Slot[] = Array.from(
    { length: positive(options.workers ?? 2, "workers") },
    () => ({ ready: false, running: undefined, worker: undefined })
  );
  const queue: Task[] = [];
  let nextId = 1;
  let closed = false;

  const settle = (slot: Slot, reply: ParseReply | null): void => {
    const { running } = slot;
    if (running === undefined) {
      return;
    }
    slot.running = undefined;
    clearTimeout(running.timer);
    running.task.resolve(reply);
    // oxlint-disable-next-line no-use-before-define -- mutual recursion.
    dispatch();
  };

  /**
   * Drop the worker (killed, crashed or out of memory): its parse fails
   * closed. After a timeout a new one starts at once, so the next parse
   * finds it warm; after a crash, only when the next parse needs it (no
   * restart loop).
   */
  const kill = (
    slot: Slot,
    worker: Worker | undefined,
    restart: boolean
  ): void => {
    if (worker === undefined || slot.worker !== worker) {
      return;
    }
    slot.worker = undefined;
    slot.ready = false;
    void worker.terminate().catch(Boolean);
    if (restart && !closed) {
      try {
        // oxlint-disable-next-line no-use-before-define -- mutual recursion.
        spawn(slot);
      } catch {
        // Retried by the next dispatch.
      }
    }
    settle(slot, null);
  };

  /** The parse's deadline runs once the worker is ready (not its start). */
  const startDeadline = (slot: Slot, ms: number): void => {
    const { running, worker } = slot;
    if (running !== undefined) {
      clearTimeout(running.timer);
      running.timer = setTimeout(() => {
        kill(slot, worker, true);
      }, ms);
    }
  };

  const spawn = (slot: Slot): Worker => {
    const worker = new Worker(workerPath, {
      resourceLimits: {
        maxOldGenerationSizeMb: maxHeapMb,
        maxYoungGenerationSizeMb: Math.min(64, maxHeapMb),
      },
      workerData: { [PARSE_WORKER]: true },
    });
    // Idle workers do not keep the process alive.
    worker.unref();
    worker.on("message", (reply: ParseReply | typeof WORKER_READY) => {
      if (slot.worker !== worker) {
        return;
      }
      if (reply === WORKER_READY) {
        slot.ready = true;
        startDeadline(slot, timeoutMs);
      } else {
        settle(slot, reply);
      }
    });
    worker.on("error", () => {
      kill(slot, worker, false);
    });
    worker.on("exit", () => {
      kill(slot, worker, false);
    });
    slot.worker = worker;
    slot.ready = false;
    return worker;
  };

  const dispatch = (): void => {
    for (const slot of slots) {
      if (slot.running === undefined) {
        const task = queue.shift();
        if (task === undefined) {
          return;
        }
        const request: ParseRequest = {
          attributes: EXTRACT_ATTRIBUTES,
          html: task.html,
          id: nextId,
          maxDepth: task.maxDepth,
          textOf: EXTRACT_TEXT_OF,
        };
        nextId += 1;
        try {
          const worker = slot.worker ?? spawn(slot);
          slot.running = { task, timer: undefined };
          startDeadline(slot, slot.ready ? timeoutMs : START_TIMEOUT_MS);
          // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a Node worker, not a window.
          worker.postMessage(request);
        } catch {
          // Could not start a worker: this parse fails closed.
          const { running } = slot;
          slot.running = undefined;
          clearTimeout(running?.timer);
          task.resolve(null);
        }
      }
    }
  };

  const parseInWorker = async (
    html: string,
    maxDepth: number
  ): Promise<ParseReply | null> => {
    if (closed || queue.length >= maxQueue) {
      return null;
    }
    const { promise, resolve } = Promise.withResolvers<ParseReply | null>();
    queue.push({ html, maxDepth, resolve });
    dispatch();
    return await promise;
  };

  return {
    close: async () => {
      closed = true;
      for (const task of queue.splice(0)) {
        task.resolve(null);
      }
      await Promise.all(
        slots.map(async (slot) => {
          const { worker } = slot;
          kill(slot, worker, false);
          await worker?.terminate();
        })
      );
    },
    extract: async (html, extractOptions = {}) => {
      if (skipsHtml(html, extractOptions.limits)) {
        return NO_IMAGES;
      }
      const maxDepth =
        extractOptions.limits?.maxNestingDepth ??
        DEFAULT_LIMITS.maxNestingDepth;
      const reply = await parseInWorker(html, maxDepth);
      if (reply?.ok !== true) {
        return null;
      }
      return extractFromElement(
        new LightView(reply.root),
        extractOptions.settings,
        extractOptions.limits
      );
    },
  };
};
