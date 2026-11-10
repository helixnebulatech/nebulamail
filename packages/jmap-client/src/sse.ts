// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Pure `text/event-stream` parser (WHATWG HTML §9.2.6). We can't use
// `EventSource` because it doesn't allow an `Authorization` header, so push
// is read with `fetch` and parsed here. The parser is a reducer: feed it the
// previous state and a decoded text chunk; it returns the new state and the
// events completed by that chunk. Chunks may split lines (even CRLF) anywhere.

export interface SseEvent {
  /** `event:` field, or "message" if absent. */
  readonly type: string;
  /** `data:` lines joined with "\n". */
  readonly data: string;
  /** Last `id:` seen so far (persists across events, per spec). */
  readonly lastEventId: string;
}

export interface SseState {
  /** Incomplete line carried over from the previous chunk. */
  readonly pending: string;
  /** Previous chunk ended in CR: drop a leading LF (split CRLF). */
  readonly afterCr: boolean;
  /** No input seen yet (a leading BOM must be stripped). */
  readonly atStart: boolean;
  readonly eventType: string;
  readonly data: string;
  readonly lastEventId: string;
  /** Reconnection time requested by the server (`retry:`), in ms. */
  readonly retryMs: number | undefined;
}

export const initialSseState: SseState = {
  afterCr: false,
  atStart: true,
  data: "",
  eventType: "",
  lastEventId: "",
  pending: "",
  retryMs: undefined,
};

interface Fields {
  eventType: string;
  data: string;
  lastEventId: string;
  retryMs: number | undefined;
}

const DIGITS = /^\d+$/u;
// Built from its code point so no invisible character sits in the source.
const BOM = String.fromCodePoint(0xfe_ff);

/** Applies one complete line; returns an event if the line dispatches one. */
const processLine = (fields: Fields, line: string): SseEvent | undefined => {
  if (line === "") {
    const { data, eventType } = fields;
    fields.data = "";
    fields.eventType = "";
    if (data === "") {
      return undefined;
    }
    return {
      data: data.slice(0, -1),
      lastEventId: fields.lastEventId,
      type: eventType === "" ? "message" : eventType,
    };
  }
  if (line.startsWith(":")) {
    return undefined;
  }
  const colon = line.indexOf(":");
  const name = colon === -1 ? line : line.slice(0, colon);
  const rawValue = colon === -1 ? "" : line.slice(colon + 1);
  const value = rawValue.startsWith(" ") ? rawValue.slice(1) : rawValue;
  switch (name) {
    case "event": {
      fields.eventType = value;
      break;
    }
    case "data": {
      fields.data += `${value}\n`;
      break;
    }
    case "id": {
      if (!value.includes("\0")) {
        fields.lastEventId = value;
      }
      break;
    }
    case "retry": {
      if (DIGITS.test(value)) {
        fields.retryMs = Number(value);
      }
      break;
    }
    default: {
      break;
    }
  }
  return undefined;
};

/** Index of the next CR or LF at or after `from`, or -1. */
const nextBreak = (text: string, from: number): number => {
  const cr = text.indexOf("\r", from);
  const lf = text.indexOf("\n", from);
  if (cr === -1) {
    return lf;
  }
  if (lf === -1) {
    return cr;
  }
  return Math.min(cr, lf);
};

export const feedSse = (
  state: SseState,
  chunk: string
): { readonly state: SseState; readonly events: SseEvent[] } => {
  if (chunk === "") {
    return { events: [], state };
  }
  let input = chunk;
  if (state.atStart && input.startsWith(BOM)) {
    input = input.slice(1);
  }
  if (state.afterCr && input.startsWith("\n")) {
    input = input.slice(1);
  }
  const text = state.pending + input;
  const fields: Fields = {
    data: state.data,
    eventType: state.eventType,
    lastEventId: state.lastEventId,
    retryMs: state.retryMs,
  };
  const events: SseEvent[] = [];
  let start = 0;
  let afterCr = false;
  let end = nextBreak(text, start);
  while (end !== -1) {
    const event = processLine(fields, text.slice(start, end));
    if (event !== undefined) {
      events.push(event);
    }
    if (text[end] === "\r") {
      if (end + 1 === text.length) {
        afterCr = true;
      } else if (text[end + 1] === "\n") {
        end += 1;
      }
    }
    start = end + 1;
    end = nextBreak(text, start);
  }
  return {
    events,
    state: {
      ...fields,
      afterCr,
      atStart: false,
      pending: text.slice(start),
    },
  };
};

/** Convenience: parse a complete stream given as one string. */
export const parseSse = (text: string): SseEvent[] =>
  feedSse(initialSseState, text).events;
