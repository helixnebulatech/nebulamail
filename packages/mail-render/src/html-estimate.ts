// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Estimate of the tree the HTML parser will build, in one linear pass over
// the tokens and without building it. A prefilter: the real
// check is `measureTree` on the parsed document. It exists so that hostile
// HTML is refused BEFORE the parser does the work, because a few bytes can
// make the parser build a huge tree: every `<p>` pops the open formatting
// elements (`<b>`, `<font>`…) and the next text rebuilds all of them, so
// K formatting tags and M paragraphs give K·M elements ("reconstruct the
// active formatting elements", WHATWG HTML §13.2.4.3).
//
// So it models, roughly and erring towards more elements and more depth:
// - the stack of open elements, with the scope boundaries an end tag cannot
//   cross (`<div><table></div>` does not close the div), the elements that
//   close an open `<p>`, `<li>`, `<td>`… and the special elements that stop
//   `</span>`;
// - the list of active formatting elements, with its markers (cells,
//   captions…), its limit of three identical entries (Noah's Ark) and its
//   reconstruction on the next text or inline start tag;
// - a bounded extra cost for each misnested formatting end tag (the
//   adoption agency clones at most a few elements);
// - foreign content (svg, math): honours `/>` and parses `<style>` as markup;
// - the table insertion modes, read off the nearest table element in the
//   stack: table parts close the open cell or caption and "clear the stack
//   back to a table context" (the formatting elements foster-parented into
//   the table leave the stack but stay in the list, so the next text rebuilds
//   them), `<table>` inside a table closes it, and table parts outside any
//   table are ignored.
//
// Implied `<tbody>` and `<tr>` around a bare `<td>` are counted; other
// implied elements are a constant factor at most, which `measureTree`
// (on the parsed tree, in the browser and on the server) catches.

import { scanHtml } from "./html-scan";
import type { TagAction, TagToken } from "./html-scan";

// Void elements and elements whose end tag is optional or that the parser
// closes implicitly do not add depth: counting them would overestimate the
// depth of sloppy but harmless HTML (thousands of unclosed <p>, <td>, <li>…).
const VOID = new Set([
  "area",
  "base",
  "br",
  "col",
  "embed",
  "hr",
  "img",
  "image",
  "input",
  "link",
  "meta",
  "source",
  "track",
  "wbr",
  "keygen",
  "param",
  "basefont",
  "bgsound",
  "frame",
  "isindex",
]);
const NOT_COUNTED = new Set([
  "p",
  "li",
  "dt",
  "dd",
  "td",
  "th",
  "tr",
  "tbody",
  "thead",
  "tfoot",
  "colgroup",
  "caption",
  "option",
  "optgroup",
  "rb",
  "rt",
  "rtc",
  "rp",
]);
/** Merged into the existing ones, never pushed. */
const ROOTS = new Set(["html", "head", "body"]);

export const FORMATTING = new Set([
  "a",
  "b",
  "big",
  "code",
  "em",
  "font",
  "i",
  "nobr",
  "s",
  "small",
  "strike",
  "strong",
  "tt",
  "u",
]);

/** Scope boundaries: an end tag does not close anything below them. */
const BOUNDARY = new Set([
  "applet",
  "caption",
  "html",
  "table",
  "td",
  "th",
  "marquee",
  "object",
  "template",
]);
const FOREIGN_BOUNDARY = new Set([
  "mi",
  "mo",
  "mn",
  "ms",
  "mtext",
  "annotation-xml",
  "foreignobject",
  "desc",
  "title",
]);
/** End tags of these close in table scope (only tables stop them). */
const TABLE_PARTS = new Set([
  "table",
  "caption",
  "colgroup",
  "tbody",
  "thead",
  "tfoot",
  "tr",
  "td",
  "th",
]);
const TABLE_SCOPE = new Set(["html", "table", "template"]);

/** Start tags handled by the table insertion modes (ignored outside them). */
const TABLE_STRUCTURE = new Set([
  "caption",
  "col",
  "colgroup",
  "tbody",
  "thead",
  "tfoot",
  "tr",
  "td",
  "th",
]);
/** The elements that decide the table insertion mode (nearest one wins). */
const TABLE_CONTEXT = new Set([
  "table",
  "tbody",
  "thead",
  "tfoot",
  "tr",
  "td",
  "th",
  "caption",
  "template",
]);
/**
 * Table modes ("in table", "in table body", "in row") by the element that
 * sets them, ranked by how deep in the table they are.
 */
const TABLE_MODE_RANK: ReadonlyMap<string, number> = new Map([
  ["table", 0],
  ["tbody", 1],
  ["thead", 1],
  ["tfoot", 1],
  ["tr", 2],
]);
/** Cells and captions: closed by table parts, with their formatting. */
const CELL_LIKE = new Set(["td", "th", "caption"]);
/** HTML whitespace: in a table it is inserted without rebuilding anything. */
const NOT_WHITESPACE = /[^\t\n\f\r ]/u;

/** Elements that push a marker on the list of active formatting elements. */
const MARKER = new Set([
  "applet",
  "caption",
  "td",
  "th",
  "marquee",
  "object",
  "template",
]);

/** The HTML "special" category: `</span>` stops at them. */
const SPECIAL = new Set([
  ...BOUNDARY,
  "address",
  "article",
  "aside",
  "blockquote",
  "button",
  "center",
  "colgroup",
  "dd",
  "details",
  "dialog",
  "dir",
  "div",
  "dl",
  "dt",
  "fieldset",
  "figcaption",
  "figure",
  "footer",
  "form",
  "frameset",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "header",
  "hgroup",
  "iframe",
  "li",
  "listing",
  "main",
  "menu",
  "nav",
  "noembed",
  "noframes",
  "noscript",
  "ol",
  "p",
  "plaintext",
  "pre",
  "script",
  "search",
  "section",
  "select",
  "style",
  "summary",
  "tbody",
  "textarea",
  "tfoot",
  "thead",
  "title",
  "tr",
  "ul",
  "xmp",
]);

/**
 * Start tags that close an open `<p>` first (and with it the formatting
 * elements inside it). `table` only does so in no-quirks mode; it is here
 * anyway, which can only add elements to the estimate.
 */
const CLOSES_P = new Set([
  "address",
  "article",
  "aside",
  "blockquote",
  "center",
  "details",
  "dialog",
  "dir",
  "div",
  "dl",
  "fieldset",
  "figcaption",
  "figure",
  "footer",
  "form",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "header",
  "hgroup",
  "hr",
  "listing",
  "main",
  "menu",
  "nav",
  "ol",
  "p",
  "plaintext",
  "pre",
  "search",
  "section",
  "summary",
  "table",
  "ul",
  "xmp",
  "dd",
  "dt",
  "li",
]);

/** Start tags that close an open element of their group first. */
const CLOSES_GROUP: ReadonlyMap<string, readonly string[]> = new Map([
  ["li", ["li"]],
  ["dd", ["dd", "dt"]],
  ["dt", ["dd", "dt"]],
  ["option", ["option"]],
  ["optgroup", ["option", "optgroup"]],
  ["rb", ["rb", "rt", "rtc", "rp"]],
  ["rt", ["rb", "rt", "rp"]],
  ["rtc", ["rb", "rt", "rtc", "rp"]],
  ["rp", ["rb", "rt", "rp"]],
]);

/** HTML start tags that end foreign content (svg, math) when inside it. */
const BREAKS_FOREIGN = new Set([
  "b",
  "big",
  "blockquote",
  "body",
  "br",
  "center",
  "code",
  "dd",
  "div",
  "dl",
  "dt",
  "em",
  "embed",
  "font",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "head",
  "hr",
  "i",
  "img",
  "li",
  "listing",
  "menu",
  "meta",
  "nobr",
  "ol",
  "p",
  "pre",
  "ruby",
  "s",
  "small",
  "span",
  "strong",
  "strike",
  "sub",
  "sup",
  "table",
  "tt",
  "u",
  "ul",
  "var",
]);

/**
 * Start tags that insert without rebuilding the formatting elements (block
 * elements). Any other start tag, and any text, rebuilds them.
 */
const NO_RECONSTRUCT = new Set([
  ...CLOSES_P,
  "caption",
  "col",
  "colgroup",
  "frameset",
  "head",
  "html",
  "body",
  "tbody",
  "td",
  "tfoot",
  "th",
  "thead",
  "tr",
  "template",
  "script",
  "style",
  "title",
  "base",
  "link",
  "meta",
  "noframes",
]);

/** Clones an adoption agency run can add (8 outer × 3 inner, plus one). */
const MAX_ADOPTION_CLONES = 25;

interface Open {
  readonly id: number;
  readonly name: string;
  readonly counted: boolean;
  readonly foreign: boolean;
  readonly boundary: boolean;
  readonly tableBoundary: boolean;
  readonly special: boolean;
  formatting: Formatting | null;
}

interface Formatting {
  readonly name: string;
  /** Name plus raw attributes: Noah's Ark keeps three equal ones at most. */
  readonly key: string;
  /** Its element in the stack, or null if popped (it gets rebuilt). */
  open: Open | null;
  removed: boolean;
}

type ActiveItem = Formatting | "marker";

/** The formatting entries after one marker, by name and by key. */
interface Level {
  readonly byName: Map<string, Formatting[]>;
  readonly byKey: Map<string, Formatting[]>;
}

export interface HtmlEstimate {
  /** Deepest estimated nesting of counted elements. */
  readonly depth: number;
  /** Estimated elements created by the parser. */
  readonly elements: number;
  /** Which limit stopped the scan, if any. */
  readonly exceeded: "depth" | "elements" | null;
}

const last = <T>(items: readonly T[] | undefined): T | undefined =>
  items?.at(-1);

const pushTo = <K, V>(map: Map<K, V[]>, key: K, value: V): void => {
  const list = map.get(key);
  if (list === undefined) {
    map.set(key, [value]);
  } else {
    list.push(value);
  }
};

/** The last live (not removed) entry of a list, dropping removed ones. */
const lastLive = (list: Formatting[] | undefined): Formatting | undefined => {
  while (list?.at(-1)?.removed === true) {
    list.pop();
  }
  return list?.at(-1);
};

/** The live (not removed) formatting entries of a list, dropping the rest. */
const live = (list: Formatting[] | undefined): Formatting[] => {
  if (list === undefined) {
    return [];
  }
  let kept = 0;
  for (const entry of list) {
    if (!entry.removed) {
      list[kept] = entry;
      kept += 1;
    }
  }
  list.length = kept;
  return list;
};

/** One estimate run: the parser's stacks, simplified. */
class TreeModel {
  readonly #maxDepth: number;
  readonly #maxElements: number;
  #nextId = 0;
  readonly #stack: Open[] = [];
  /** Ids of the open elements, by name (ids grow with stack position). */
  readonly #ids = new Map<string, number[]>();
  readonly #boundaries: number[] = [];
  readonly #tableBoundaries: number[] = [];
  readonly #specials: number[] = [];
  /** Open HTML elements of `TABLE_CONTEXT`, innermost last. */
  readonly #tableContexts: Open[] = [];
  #foreign = 0;
  readonly #active: ActiveItem[] = [];
  readonly #levels: Level[] = [{ byKey: new Map(), byName: new Map() }];
  depth = 0;
  maxDepth = 0;
  /** `<html>`, `<head>` and `<body>` always exist. */
  elements = 3;
  exceeded: "depth" | "elements" | null = null;

  constructor(maxDepth: number, maxElements: number) {
    this.#maxDepth = maxDepth;
    this.#maxElements = maxElements;
  }

  #lastId(name: string): number {
    return last(this.#ids.get(name)) ?? -1;
  }

  #inScope(name: string, extra: readonly string[] = []): boolean {
    const id = this.#lastId(name);
    if (id === -1) {
      return false;
    }
    let boundary = last(this.#boundaries) ?? -1;
    for (const other of extra) {
      boundary = Math.max(boundary, this.#lastId(other));
    }
    return id >= boundary;
  }

  #level(): Level {
    return this.#levels.at(-1) ?? { byKey: new Map(), byName: new Map() };
  }

  #create(count: number): void {
    this.elements += count;
    if (this.elements > this.#maxElements) {
      this.exceeded = "elements";
    }
  }

  #push(name: string, foreign: boolean, formatting: Formatting | null): void {
    const counted = !NOT_COUNTED.has(name);
    const boundary = foreign ? FOREIGN_BOUNDARY.has(name) : BOUNDARY.has(name);
    const special = boundary || (!foreign && SPECIAL.has(name));
    const tableBoundary = !foreign && TABLE_SCOPE.has(name);
    const open: Open = {
      boundary,
      counted,
      foreign,
      formatting,
      id: this.#nextId,
      name,
      special,
      tableBoundary,
    };
    this.#nextId += 1;
    this.#stack.push(open);
    pushTo(this.#ids, name, open.id);
    if (boundary) {
      this.#boundaries.push(open.id);
    }
    if (special) {
      this.#specials.push(open.id);
    }
    if (tableBoundary) {
      this.#tableBoundaries.push(open.id);
    }
    if (!foreign && TABLE_CONTEXT.has(name)) {
      this.#tableContexts.push(open);
    }
    if (name === "svg" || name === "math") {
      this.#foreign += 1;
    }
    if (formatting !== null) {
      formatting.open = open;
    }
    if (!foreign && MARKER.has(name)) {
      this.#active.push("marker");
      this.#levels.push({ byKey: new Map(), byName: new Map() });
    }
    if (counted) {
      this.depth += 1;
      this.maxDepth = Math.max(this.maxDepth, this.depth);
      if (this.depth > this.#maxDepth) {
        this.exceeded = "depth";
      }
    }
  }

  /**
   * Bookkeeping for an element leaving the stack (`open` already out). The
   * list of active formatting elements keeps its markers: the parser only
   * clears up to the last one when it closes a cell, a caption, an object…
   * explicitly (`#clearToMarker`), never just by popping them.
   */
  #forget(open: Open): void {
    this.#ids.get(open.name)?.pop();
    if (open.boundary) {
      this.#boundaries.pop();
    }
    if (open.special) {
      this.#specials.pop();
    }
    if (open.tableBoundary) {
      this.#tableBoundaries.pop();
    }
    if (!open.foreign && TABLE_CONTEXT.has(open.name)) {
      this.#tableContexts.pop();
    }
    if (open.name === "svg" || open.name === "math") {
      this.#foreign -= 1;
    }
    if (open.counted) {
      this.depth -= 1;
    }
    if (open.formatting !== null) {
      // Still in the list: rebuilt on the next text or inline tag.
      open.formatting.open = null;
    }
  }

  /** Pops elements up to and including the one with this id. */
  #popTo(id: number): void {
    let open = this.#stack.pop();
    while (open !== undefined) {
      this.#forget(open);
      if (open.id <= id) {
        return;
      }
      open = this.#stack.pop();
    }
  }

  /** Pops the elements above the one with this id ("clear the stack back"). */
  #popAbove(id: number): void {
    let top = this.#stack.at(-1);
    while (top !== undefined && top.id > id) {
      this.#stack.pop();
      this.#forget(top);
      top = this.#stack.at(-1);
    }
  }

  /** Whether the open element with this id is foreign (looks from the top). */
  #isForeign(id: number): boolean {
    for (let index = this.#stack.length - 1; index >= 0; index -= 1) {
      const open = this.#stack[index];
      if (open === undefined || open.id < id) {
        return false;
      }
      if (open.id === id) {
        return open.foreign;
      }
    }
    return false;
  }

  /** The innermost table part, cell, caption or template that is open. */
  #tableContext(): Open | undefined {
    return this.#tableContexts.at(-1);
  }

  /** In "in table", "in table body" or "in row" (not in a cell). */
  #inTableMode(): boolean {
    const context = this.#tableContext();
    return context !== undefined && TABLE_MODE_RANK.has(context.name);
  }

  /**
   * A table part start tag: closes the open cell or caption (with their
   * formatting entries), then the rows and sections it cannot go in, and
   * clears the stack back to the table context, which leaves the
   * foster-parented elements in the list but not in the stack. False if
   * the parser ignores it (no table open).
   */
  #openTablePart(name: string): boolean {
    let allowed = 0;
    if (name === "td" || name === "th") {
      allowed = 2;
    } else if (name === "tr") {
      allowed = 1;
    }
    for (;;) {
      const context = this.#tableContext();
      if (context === undefined) {
        return false;
      }
      if (context.name === "template") {
        return true;
      }
      if (CELL_LIKE.has(context.name)) {
        // "Close the cell" / close the caption, then reprocess.
        this.#popTo(context.id);
        this.#clearToMarker();
        continue;
      }
      const rank = TABLE_MODE_RANK.get(context.name) ?? 0;
      this.#popAbove(context.id);
      if (rank <= allowed) {
        return true;
      }
      this.#popTo(context.id);
    }
  }

  /** What the table modes do with a start tag; false if it is ignored. */
  #tableModes(name: string): boolean {
    if (TABLE_STRUCTURE.has(name)) {
      if (!this.#openTablePart(name)) {
        return false;
      }
      if (name === "col" && this.#tableContext()?.name === "table") {
        // A bare `<col>` gets a `<colgroup>` around it.
        this.#create(1);
      }
    } else if (name === "table" && this.#inTableMode()) {
      // `<table>` in a table closes it first (and reopens one).
      this.#popTo(this.#lastId("table"));
    }
    return true;
  }

  #clearToMarker(): void {
    let item = this.#active.pop();
    while (item !== undefined && item !== "marker") {
      item.removed = true;
      item = this.#active.pop();
    }
    if (item === "marker" && this.#levels.length > 1) {
      this.#levels.pop();
    }
  }

  #removeFormatting(entry: Formatting): void {
    entry.removed = true;
    if (entry.open !== null) {
      entry.open.formatting = null;
    }
    // Usually the last one; otherwise it is skipped (and dropped) later.
    let tail = this.#active.at(-1);
    while (tail !== undefined && tail !== "marker" && tail.removed) {
      this.#active.pop();
      tail = this.#active.at(-1);
    }
  }

  /** Rebuilds the formatting elements that were popped but not closed. */
  #reconstruct(): void {
    let start = this.#active.length;
    while (start > 0) {
      const item = this.#active[start - 1];
      if (item === undefined || item === "marker") {
        break;
      }
      if (!item.removed && item.open !== null) {
        break;
      }
      start -= 1;
    }
    if (start === this.#active.length) {
      return;
    }
    const tail = this.#active.splice(start);
    for (const item of tail) {
      if (item !== "marker" && !item.removed) {
        this.#active.push(item);
        this.#create(1);
        this.#push(item.name, false, item);
      }
    }
  }

  #inForeign(): boolean {
    const top = this.#stack.at(-1);
    return top !== undefined && top.foreign && !FOREIGN_BOUNDARY.has(top.name);
  }

  #closeAny(names: readonly string[], extra: readonly string[] = []): void {
    for (;;) {
      let target = -1;
      for (const name of names) {
        if (this.#inScope(name, extra)) {
          target = Math.max(target, this.#lastId(name));
        }
      }
      if (target === -1) {
        return;
      }
      this.#popTo(target);
    }
  }

  /** What a start tag closes first (`<div>` an open `<p>`, `<li>` a `<li>`…). */
  #closeImplied(name: string): void {
    if (CLOSES_P.has(name) && this.#inScope("p", ["button"])) {
      this.#popTo(this.#lastId("p"));
    }
    const group = CLOSES_GROUP.get(name);
    if (group !== undefined) {
      this.#closeAny(group, name === "li" ? ["ol", "ul"] : []);
    }
    if (name === "td" || name === "th" || name === "tr") {
      this.#implyTableParts(name);
    }
  }

  /** A bare `<td>` in a table gets a `<tbody>` and a `<tr>` around it. */
  #implyTableParts(name: string): void {
    const table = last(this.#tableBoundaries) ?? -1;
    if (table === -1) {
      // Outside tables the parser ignores these tags.
      return;
    }
    const section = Math.max(
      this.#lastId("tbody"),
      this.#lastId("thead"),
      this.#lastId("tfoot")
    );
    if (section < table) {
      this.#create(1);
      this.#push("tbody", false, null);
    }
    if (name !== "tr" && this.#lastId("tr") < table) {
      this.#create(1);
      this.#push("tr", false, null);
    }
  }

  #addFormatting(tag: TagToken): void {
    const level = this.#level();
    const key = `${tag.name} ${tag.attributes}`;
    const same = live(level.byKey.get(key));
    const [oldest] = same;
    if (same.length >= 3 && oldest !== undefined) {
      this.#removeFormatting(oldest);
      same.shift();
    }
    const entry: Formatting = {
      key,
      name: tag.name,
      open: null,
      removed: false,
    };
    this.#active.push(entry);
    pushTo(level.byKey, key, entry);
    pushTo(level.byName, tag.name, entry);
    this.#push(tag.name, false, entry);
  }

  start(tag: TagToken): TagAction {
    const { name } = tag;
    if (ROOTS.has(name)) {
      // Merged into the elements that always exist.
      return "continue";
    }
    this.#create(1);
    if (this.#inForeign()) {
      if (!BREAKS_FOREIGN.has(name)) {
        if (!tag.selfClosing) {
          this.#push(name, true, null);
        }
        return "markup";
      }
      let top = this.#stack.at(-1);
      while (top?.foreign === true) {
        this.#popTo(top.id);
        top = this.#stack.at(-1);
      }
    }
    if (!this.#tableModes(name)) {
      // Outside tables the parser ignores table parts.
      return this.#foreign > 0 ? "markup" : "continue";
    }
    this.#closeImplied(name);
    if (!NO_RECONSTRUCT.has(name)) {
      this.#reconstruct();
    }
    if (name === "a" && lastLive(this.#level().byName.get("a")) !== undefined) {
      this.end({ ...tag, closing: true });
    }
    if (name === "svg" || name === "math") {
      if (!tag.selfClosing) {
        this.#push(name, true, null);
      }
    } else if (FORMATTING.has(name)) {
      this.#addFormatting(tag);
    } else if (!VOID.has(name)) {
      // `/>` on an HTML element that is not void is ignored by the parser.
      this.#push(name, false, null);
    }
    // Inside svg or math, `<style>` and `<script>` are not raw text.
    return this.#foreign > 0 ? "markup" : "continue";
  }

  #endFormatting(entry: Formatting): void {
    const { open } = entry;
    if (open === null) {
      // Popped already: only drop it from the list.
      this.#removeFormatting(entry);
      return;
    }
    if (open.id < (last(this.#boundaries) ?? -1)) {
      // Not in scope: ignored.
      return;
    }
    this.#removeFormatting(entry);
    const top = this.#stack.at(-1);
    if (top === open) {
      this.#popTo(open.id);
      return;
    }
    if ((last(this.#specials) ?? -1) < open.id) {
      // No block inside it: everything inside is popped (and the
      // formatting elements among it rebuilt later).
      this.#popTo(open.id);
      return;
    }
    // Misnested around a block (`<b><div></b>`): the block stays open,
    // only the formatting element leaves, and the parser clones a few.
    const index = this.#stack.lastIndexOf(open);
    this.#create(Math.min(this.#stack.length - index, MAX_ADOPTION_CLONES));
    this.#stack.splice(index, 1);
    // It was the last open element with its name (`entry` is the last one
    // in the list, and newer ones of that name would be after it).
    this.#forget(open);
  }

  /** `</td>`, `</tr>`, `</table>`…: closes its cells even without `</td>`. */
  #endTablePart(id: number): void {
    if (id < (last(this.#tableBoundaries) ?? -1)) {
      // Not in table scope: ignored.
      return;
    }
    const context = this.#tableContext();
    this.#popTo(id);
    if (
      context !== undefined &&
      CELL_LIKE.has(context.name) &&
      context.id >= id
    ) {
      // The open cell or caption is closed, once.
      this.#clearToMarker();
    }
  }

  #endSpecial(name: string, id: number): void {
    if (!this.#inScope(name)) {
      return;
    }
    // `</object>`, `</marquee>`, `</template>`… also clear the list of
    // active formatting elements up to their marker.
    const marker = MARKER.has(name) && !this.#isForeign(id);
    this.#popTo(id);
    if (marker) {
      this.#clearToMarker();
    }
  }

  end(tag: TagToken): void {
    const { name } = tag;
    if (name === "br") {
      // `</br>` is a `<br>` start tag without attributes (HTML spec, "in
      // body"): it also reconstructs the active formatting elements and
      // breaks out of foreign content, which a bare count would miss.
      this.start({
        ...tag,
        attributes: "",
        closing: false,
        selfClosing: false,
      });
      return;
    }
    if (FORMATTING.has(name)) {
      const entry = lastLive(this.#level().byName.get(name));
      if (entry !== undefined) {
        this.#endFormatting(entry);
        return;
      }
    }
    if (name === "p") {
      if (this.#inScope("p", ["button"])) {
        this.#popTo(this.#lastId("p"));
      } else {
        // `</p>` with no open `<p>` creates an empty one.
        this.#create(1);
      }
      return;
    }
    if (name === "li") {
      if (this.#inScope("li", ["ol", "ul"])) {
        this.#popTo(this.#lastId("li"));
      }
      return;
    }
    const id = this.#lastId(name);
    if (id === -1) {
      return;
    }
    if (TABLE_PARTS.has(name)) {
      this.#endTablePart(id);
      return;
    }
    const top = this.#stack.at(-1);
    const isSpecial =
      top !== undefined && top.id === id ? top.special : SPECIAL.has(name);
    if (isSpecial) {
      this.#endSpecial(name, id);
    } else if (id > (last(this.#specials) ?? -1)) {
      // `</span>`: closes it unless a special element is in between.
      this.#popTo(id);
    }
  }

  text(value: string): void {
    if (this.#inForeign()) {
      return;
    }
    const top = this.#stack.at(-1);
    if (
      top !== undefined &&
      !top.foreign &&
      TABLE_MODE_RANK.has(top.name) &&
      !NOT_WHITESPACE.test(value)
    ) {
      // "In table text": whitespace goes in the table, nothing rebuilt.
      return;
    }
    // Anything else, foster-parented in a table too, rebuilds them.
    this.#reconstruct();
  }
}

/**
 * Estimated depth and element count of the tree the parser will build
 * from `html`, stopping as soon as one passes its limit. Linear time.
 */
export const estimateHtml = (
  html: string,
  maxDepth = Number.POSITIVE_INFINITY,
  maxElements = Number.POSITIVE_INFINITY
): HtmlEstimate => {
  const model = new TreeModel(maxDepth, maxElements);
  scanHtml(
    html,
    (tag) => {
      let action: TagAction = "continue";
      if (tag.closing) {
        model.end(tag);
      } else {
        action = model.start(tag);
      }
      return model.exceeded === null ? action : "stop";
    },
    (text) => {
      if (model.exceeded === null && text !== "") {
        model.text(text);
      }
    }
  );
  return {
    depth: model.maxDepth,
    elements: model.elements,
    exceeded: model.exceeded,
  };
};
