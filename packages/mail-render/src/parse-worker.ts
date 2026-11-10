// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Worker thread of `server-extract.ts`: parses email HTML
// with parse5 (the parser inside jsdom, same version, same options as
// jsdom's DOMParser: scripting disabled) and sends back a light tree of the
// elements. The main thread kills it when it takes too long: some inputs
// are quadratic in parse5 with a nearly empty stack of open elements
// (foster parenting in <table> plus the reconstruction of formatting
// elements), so no check on the string or on the stack bounds the time.
//
// Inside the worker, the depth is still capped: the stack of open
// elements while parsing (cuts deep inputs early) and the real depth of
// the tree, measured without recursion (it can be about twice the stack's,
// e.g. `<a><b></table>x<nobr><div><table>`).
//
// Only `node:worker_threads` and `parse5`, no relative imports: Node runs
// this file in a fresh thread, without the bundler or the dev loader.

import { parentPort, workerData } from "node:worker_threads";

import { defaultTreeAdapter, parse } from "parse5";
import type { DefaultTreeAdapterTypes } from "parse5";

/** One element: name, namespace, chosen attributes, text, element children. */
export interface LightElement {
  readonly n: string;
  readonly ns: string;
  readonly a: readonly (readonly [string, string])[];
  /** Text content, only for the names asked for (`<style>`). */
  readonly t?: string;
  readonly c: readonly LightElement[];
}

export interface ParseRequest {
  readonly id: number;
  readonly html: string;
  readonly maxDepth: number;
  /** Attributes copied to the light tree (lower case). */
  readonly attributes: readonly string[];
  /** Elements whose text content is copied. */
  readonly textOf: readonly string[];
}

export type ParseReply =
  | { readonly id: number; readonly ok: true; readonly root: LightElement }
  | {
      readonly id: number;
      readonly ok: false;
      readonly reason: "too-deep" | "error";
    };

type Element = DefaultTreeAdapterTypes.Element;
type ChildNode = DefaultTreeAdapterTypes.ChildNode;

class TooDeepError extends Error {
  override readonly name = "TooDeepError";
}

const isElement = (node: ChildNode): node is Element => "tagName" in node;

/** Text of every text node under `element` (its `textContent`). */
const textContent = (element: Element): string => {
  let text = "";
  const stack: ChildNode[] = [...element.childNodes].toReversed();
  let node = stack.pop();
  while (node !== undefined) {
    if (node.nodeName === "#text" && "value" in node) {
      text += node.value;
    } else if (isElement(node)) {
      for (let i = node.childNodes.length - 1; i >= 0; i -= 1) {
        const child = node.childNodes[i];
        if (child !== undefined) {
          stack.push(child);
        }
      }
    }
    node = stack.pop();
  }
  return text;
};

interface MutableElement extends LightElement {
  readonly c: LightElement[];
}

/**
 * The light tree of `root`, without recursion; `TooDeep` past `maxDepth`.
 * Children are the element children only (like `Element.children`):
 * `<template>` contents are not children, as in the DOM.
 */
const lighten = (root: Element, request: ParseRequest): LightElement => {
  const wanted = new Set(request.attributes);
  const textOf = new Set(request.textOf);
  const make = (source: Element): MutableElement => ({
    a: source.attrs
      .filter((attr) => attr.namespace === undefined && wanted.has(attr.name))
      .map((attr) => [attr.name, attr.value] as const),
    c: [],
    n: source.tagName,
    ns: source.namespaceURI,
    ...(textOf.has(source.tagName) ? { t: textContent(source) } : {}),
  });
  const top = make(root);
  // Each element adds its children, in order, when it is taken off the stack.
  const stack: { source: Element; light: MutableElement; depth: number }[] = [
    { depth: 1, light: top, source: root },
  ];
  let current = stack.pop();
  while (current !== undefined) {
    if (current.depth > request.maxDepth) {
      throw new TooDeepError();
    }
    for (const kid of current.source.childNodes) {
      if (isElement(kid)) {
        const light = make(kid);
        current.light.c.push(light);
        stack.push({ depth: current.depth + 1, light, source: kid });
      }
    }
    current = stack.pop();
  }
  return top;
};

export const parseLight = (request: ParseRequest): ParseReply => {
  const { id, maxDepth } = request;
  let depth = 0;
  try {
    const document = parse(request.html, {
      scriptingEnabled: false,
      treeAdapter: {
        ...defaultTreeAdapter,
        onItemPop: () => {
          depth -= 1;
        },
        onItemPush: () => {
          depth += 1;
          if (depth > maxDepth) {
            throw new TooDeepError();
          }
        },
      },
    });
    const root = document.childNodes.find(isElement);
    if (root === undefined) {
      return { id, ok: false, reason: "error" };
    }
    return { id, ok: true, root: lighten(root, request) };
  } catch (error) {
    return {
      id,
      ok: false,
      reason: error instanceof TooDeepError ? "too-deep" : "error",
    };
  }
};

/** Set in `workerData` by `server-extract.ts`: only then does it listen. */
export const PARSE_WORKER = "mailRenderParseWorker";
/** First message of a worker: its modules are loaded. */
export const WORKER_READY = "ready";

const port = parentPort;
if (
  port !== null &&
  typeof workerData === "object" &&
  workerData !== null &&
  Reflect.get(workerData, PARSE_WORKER) === true
) {
  port.on("message", (request: ParseRequest) => {
    port.postMessage(parseLight(request));
  });
  port.postMessage(WORKER_READY);
}
