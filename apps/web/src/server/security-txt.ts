// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// /.well-known/security.txt (RFC 9116; policy in SECURITY.md).
//
// Served by every instance with the project's contacts. `Canonical` is the
// instance's own URL (from APP_URL), plus the project's when they differ,
// so a researcher can tell where the file came from.
//
// `Expires` is fixed on purpose (RFC 9116 §2.5.5: less than a year ahead, and
// refreshed by a person): `security-txt.test.ts` fails 30 days before it,
// which is the reminder to check the contacts and move the date.

export const SECURITY_TXT_EXPIRES = "2027-10-01T00:00:00.000Z";

const PROJECT_CANONICAL = "https://helixnebula.tech/.well-known/security.txt";

export const securityTxt = (appUrl: string): string => {
  const own = new URL("/.well-known/security.txt", appUrl).href;
  const canonical = [
    own,
    ...(own === PROJECT_CANONICAL ? [] : [PROJECT_CANONICAL]),
  ];
  return [
    "# Nebula Mail: report vulnerabilities privately, never in a public issue.",
    "# Informa en privado, nunca en un issue público.",
    "Contact: mailto:security@helixnebula.tech",
    "Contact: https://github.com/helixnebulatech/nebulamail/security/advisories/new",
    `Expires: ${SECURITY_TXT_EXPIRES}`,
    "Preferred-Languages: es, en",
    ...canonical.map((url) => `Canonical: ${url}`),
    "Policy: https://github.com/helixnebulatech/nebulamail/blob/main/SECURITY.md",
    "",
  ].join("\n");
};

export const securityTxtResponse = (appUrl: string): Response =>
  new Response(securityTxt(appUrl), {
    headers: {
      // Public and the same for everybody; no user data.
      "Cache-Control": "public, max-age=86400",
      "Content-Type": "text/plain; charset=utf-8",
    },
  });
