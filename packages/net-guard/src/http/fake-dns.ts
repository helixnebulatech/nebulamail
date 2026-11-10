// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// TEST-ONLY fake DNS server over UDP on loopback, for the c-ares resolver
// (`resolver.test.ts`, the adapters' default resolver, the URL vectors). Not
// listed in package.json `exports`. It records every question it gets, so
// tests can prove that an abort leaves no lookup (or retry) running. Also
// `withoutResolverCancel`, for the fail-closed check of the adapters.

import { createSocket } from "node:dgram";
import type { Socket } from "node:dgram";
import { Resolver as DnsResolver } from "node:dns/promises";

import ipaddr from "ipaddr.js";

export interface FakeDnsRecords {
  readonly A?: readonly string[];
  readonly AAAA?: readonly string[];
}

/** How the fake answers one family: its records, SERVFAIL, or nothing at all. */
export type FamilyAnswer = "records" | "servfail" | "silent";

export interface FakeDnsQuery {
  readonly name: string;
  readonly type: "A" | "AAAA";
}

export interface FakeDns {
  /** `ip:port`, for `setServers` or `CancelableResolverOptions.servers`. */
  readonly server: string;
  /** Every A/AAAA question received, in order (retries included). */
  readonly queries: readonly FakeDnsQuery[];
  readonly close: () => void;
}

export interface FakeDnsOptions {
  /**
   * Name (lower case, no final dot) → records. A name not listed gets
   * NXDOMAIN; an empty entry `{}` gets NOERROR with no answers. Without
   * `zones` every name has `DEFAULT_RECORDS`.
   */
  readonly zones?: ReadonlyMap<string, FakeDnsRecords>;
  readonly A?: FamilyAnswer;
  readonly AAAA?: FamilyAnswer;
}

/** The records of every name when there are no zones. */
export const DEFAULT_RECORDS = {
  A: ["93.184.216.34"],
  AAAA: ["2606:2800:220:1:248:1893:25c8:1946"],
} as const satisfies FakeDnsRecords;

const TYPE_A = 1;
const TYPE_AAAA = 28;
const HEADER_BYTES = 12;

/** RCODEs: NOERROR, SERVFAIL, NXDOMAIN. */
const NOERROR = 0;
const SERVFAIL = 2;
const NXDOMAIN = 3;

interface Question {
  readonly name: string;
  readonly type: number;
  /** Where the question (name, type and class) ends. */
  readonly end: number;
}

/** The first question of a query, its name decoded label by label. */
const parseQuestion = (query: Buffer): Question | undefined => {
  const labels: string[] = [];
  let at = HEADER_BYTES;
  while (at < query.length) {
    const length = query[at] ?? 0;
    if (length === 0) {
      break;
    }
    // Compression pointers never appear in a question we are sent.
    if (length > 63) {
      return undefined;
    }
    labels.push(query.toString("latin1", at + 1, at + 1 + length));
    at += length + 1;
  }
  const end = at + 5;
  if (end > query.length) {
    return undefined;
  }
  return {
    end,
    name: labels.join(".").toLowerCase(),
    type: query.readUInt16BE(at + 1),
  };
};

const rdataOf = (type: number, address: string): Buffer =>
  type === TYPE_A
    ? Buffer.from(ipaddr.IPv4.parse(address).toByteArray())
    : Buffer.from(ipaddr.IPv6.parse(address).toByteArray());

const reply = (
  query: Buffer,
  question: Question,
  rcode: number,
  addresses: readonly string[]
): Buffer => {
  const header = Buffer.alloc(HEADER_BYTES);
  query.copy(header, 0, 0, 2);
  // QR, RD, RA and the RCODE.
  header.writeUInt16BE(0x81_80 + rcode, 2);
  header.writeUInt16BE(1, 4);
  header.writeUInt16BE(addresses.length, 6);
  const records = addresses.map((address) => {
    const rdata = rdataOf(question.type, address);
    const record = Buffer.alloc(12);
    // Name: a pointer to the question's.
    record.writeUInt16BE(0xc0_0c, 0);
    record.writeUInt16BE(question.type, 2);
    record.writeUInt16BE(1, 4);
    record.writeUInt32BE(60, 6);
    record.writeUInt16BE(rdata.length, 10);
    return Buffer.concat([record, rdata]);
  });
  return Buffer.concat([
    header,
    query.subarray(HEADER_BYTES, question.end),
    ...records,
  ]);
};

export const fakeDns = async (
  options: FakeDnsOptions = {}
): Promise<FakeDns> => {
  const queries: FakeDnsQuery[] = [];
  const socket: Socket = createSocket("udp4");
  socket.on("message", (query, peer) => {
    const question = parseQuestion(query);
    if (
      question === undefined ||
      (question.type !== TYPE_A && question.type !== TYPE_AAAA)
    ) {
      return;
    }
    const family = question.type === TYPE_A ? "A" : "AAAA";
    queries.push({ name: question.name, type: family });
    const answer = options[family] ?? "records";
    if (answer === "silent") {
      return;
    }
    const send = (rcode: number, addresses: readonly string[]) => {
      socket.send(
        reply(query, question, rcode, addresses),
        peer.port,
        peer.address
      );
    };
    if (answer === "servfail") {
      send(SERVFAIL, []);
      return;
    }
    const records =
      options.zones === undefined
        ? DEFAULT_RECORDS
        : options.zones.get(question.name);
    if (records === undefined) {
      send(NXDOMAIN, []);
      return;
    }
    send(NOERROR, records[family] ?? []);
  });
  const ready = Promise.withResolvers<undefined>();
  socket.bind(0, "127.0.0.1", () => {
    ready.resolve(undefined);
  });
  await ready.promise;
  return {
    close: () => {
      socket.close();
    },
    queries,
    server: `127.0.0.1:${String(socket.address().port)}`,
  };
};

/**
 * Run `body` as on a runtime whose `dns.promises.Resolver` has no `cancel`.
 * The method may live on a parent prototype and need not be an own
 * property, so it is shadowed on the prototype that really has it (`delete`
 * alone may do nothing) and the original descriptor is restored afterwards.
 */
export const withoutResolverCancel = <T>(body: () => T): T => {
  let owner: object | null = DnsResolver.prototype;
  while (owner !== null && !Object.hasOwn(owner, "cancel")) {
    owner = Reflect.getPrototypeOf(owner);
  }
  const target: object = owner ?? DnsResolver.prototype;
  const saved = Object.getOwnPropertyDescriptor(target, "cancel");
  Object.defineProperty(target, "cancel", {
    configurable: true,
    value: undefined,
    writable: true,
  });
  try {
    return body();
  } finally {
    if (saved === undefined) {
      Reflect.deleteProperty(target, "cancel");
    } else {
      Object.defineProperty(target, "cancel", saved);
    }
  }
};
