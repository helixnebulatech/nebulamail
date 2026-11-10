// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Misleading sender names.
// The display name of a From is whatever the sender wrote, so
// a name that contains an address other than the real one
// (`"[EMAIL]" <[EMAIL]>`) is flagged, whatever the address
// is; a name that only repeats the real address is fine. Nothing is
// blocked.
//
// No list of brands (removed 2026-11-04): a hand-kept list gave false
// alarms for a brand's domains it lacked and covered only a few brands.
// Look-alike and punycode domains (sender-domain.ts), «Primera vez que te
// escribe» (sender-new.ts) and «Verificado» (DMARC) cover the impersonation
// without one.
//
// Untrusted input: invisible characters are removed first (from at most
// MAX_RAW characters), then the name is cut to MAX_NAME; every step is
// linear in its length.

export interface SenderNameWarning {
  /** The name contains another address. */
  readonly kind: "address";
  /** That address, as written (cleaned, cut), for the tooltip. */
  readonly shown: string;
}

/** Longest name looked at, after removing invisible characters. */
const MAX_NAME = 256;
/** Longest raw name read before that. */
const MAX_RAW = 1_000_000;
/** Longest address shown in the tooltip. */
const MAX_SHOWN = 80;
const INVISIBLE = /[\p{Default_Ignorable_Code_Point}\p{Cf}\p{Cc}]/gu;

/** Characters that end an address-like word in a name. */
const ADDRESS_END = /[\s"'<>()[\],;:]/u;

/**
 * Address-like words of the name (`user@host.tld`), normalized like the
 * real one would be: NFKC (`＠`), invisible characters removed, lower case,
 * without `mailto:`.
 */
const addressesIn = (name: string): string[] => {
  const normal = name
    .slice(0, MAX_RAW)
    .replaceAll(INVISIBLE, "")
    .slice(0, MAX_NAME)
    .normalize("NFKC")
    .replaceAll(INVISIBLE, "")
    .toLowerCase();
  const found: string[] = [];
  let current = "";
  const take = () => {
    const word = current.replace(/^mailto:/u, "").replace(/[.]+$/u, "");
    const at = word.lastIndexOf("@");
    if (at > 0 && word.slice(at + 1).includes(".")) {
      found.push(word);
    }
    current = "";
  };
  for (const char of normal) {
    if (ADDRESS_END.test(char) && !(char === ":" && current === "mailto")) {
      take();
    } else {
      current += char;
    }
  }
  take();
  return found;
};

const sameAs = (shown: string, real: string): boolean =>
  shown === real.trim().normalize("NFKC").toLowerCase();

/**
 * Why the sender's display name deserves a warning, or null. `name` is
 * the From's display name (untrusted), `email` its real address.
 */
export const senderNameWarning = (
  name: string | null | undefined,
  email: string
): SenderNameWarning | null => {
  if (name === null || name === undefined || name.trim() === "") {
    return null;
  }
  const other = addressesIn(name).find((shown) => !sameAs(shown, email));
  return other === undefined
    ? null
    : { kind: "address", shown: other.slice(0, MAX_SHOWN) };
};
