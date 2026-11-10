// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Whether an attachment is a vCard file (RFC 6350 §10.1: text/vcard; the
// older text/x-vcard and text/directory, RFC 2425, still seen in mail).
// Apart from the import dialog so the reader can find .vcf files without
// loading it.

/** .vcf attachments (by type or name). */
export const isVcard = (attachment: {
  readonly name: string;
  readonly type: string;
}): boolean =>
  /^text\/(?:x-)?vcard$|^text\/directory$/iu.test(attachment.type) ||
  /\.(?:vcf|vcard)$/iu.test(attachment.name.trim());
