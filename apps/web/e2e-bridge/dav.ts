// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Radicale straight from the test, as "another client":
// to check what the app wrote really is on the DAV server, and to write
// there behind the bridge's back. Plain HTTP on loopback, Basic auth with
// Dovecot's password (docker/compose.yaml, profile "bridge").

import { DAV, DOVECOT } from "./env";

const authorization = (user: string): string =>
  `Basic ${btoa(`${user}:${DOVECOT.password}`)}`;

const PROPFIND = `<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:"><prop><resourcetype/></prop></propfind>`;

/** The user's collections of one kind (absolute URLs). */
export const davCollections = async (
  user: string,
  kind: "calendar" | "addressbook"
): Promise<string[]> => {
  const home = new URL(`${user}/`, DAV.url);
  const response = await fetch(home, {
    body: PROPFIND,
    headers: {
      authorization: authorization(user),
      "content-type": "application/xml",
      depth: "1",
    },
    method: "PROPFIND",
  });
  if (response.status !== 207) {
    throw new Error(`PROPFIND ${home.pathname}: HTTP ${response.status}`);
  }
  const body = await response.text();
  const tag = new RegExp(`<(?:\\w+:)?${kind}\\s*/>`, "u");
  return body
    .split(/<(?:\w+:)?response>/u)
    .filter((part) => tag.test(part))
    .flatMap((part) => {
      const href = /<(?:\w+:)?href>(?<href>[^<]+)</u.exec(part)?.groups?.href;
      return href === undefined ? [] : [new URL(href, DAV.url).href];
    });
};

/**
 * Every event or card of the user, as the DAV server has it (Radicale
 * answers GET on a collection with all its members in one text).
 */
export const davText = async (
  user: string,
  kind: "calendar" | "addressbook"
): Promise<string> => {
  const collections = await davCollections(user, kind);
  const texts = await Promise.all(
    collections.map(async (url) => {
      const response = await fetch(url, {
        headers: { authorization: authorization(user) },
      });
      return response.ok ? await response.text() : "";
    })
  );
  // Unfolded (RFC 5545 §3.1), so a long line can be searched whole.
  return texts.join("\n").replaceAll(/\r?\n[ \t]/gu, "");
};

/** Writes a new resource into a collection, as another client would. */
export const davPut = async (
  user: string,
  collection: string,
  name: string,
  body: string,
  type: "text/calendar" | "text/vcard"
): Promise<void> => {
  const response = await fetch(new URL(name, collection), {
    body,
    headers: {
      authorization: authorization(user),
      "content-type": `${type}; charset=utf-8`,
      "if-none-match": "*",
    },
    method: "PUT",
  });
  if (response.status !== 201 && response.status !== 204) {
    throw new Error(`PUT ${name}: HTTP ${response.status}`);
  }
};

const SETTINGS_PROPFIND = `<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><prop>
<C:calendar-timezone-id/><C:default-alarm-vevent-datetime/>
<C:default-alarm-vevent-date/></prop></propfind>`;

/**
 * The time zone ids and default alarms of the user's collections, as
 * Radicale returns them (the PROPFIND's XML, unfolded).
 */
export const davCalendarSettings = async (user: string): Promise<string> => {
  const home = new URL(`${user}/`, DAV.url);
  const response = await fetch(home, {
    body: SETTINGS_PROPFIND,
    headers: {
      authorization: authorization(user),
      "content-type": "application/xml",
      depth: "1",
    },
    method: "PROPFIND",
  });
  if (response.status !== 207) {
    throw new Error(`PROPFIND ${home.pathname}: HTTP ${response.status}`);
  }
  const body = await response.text();
  return body.replaceAll(/\r?\n[ \t]/gu, "");
};
