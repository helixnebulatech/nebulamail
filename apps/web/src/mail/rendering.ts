// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Renders an open email with @nebulamail/mail-render. Browser only:
// the pipeline needs a DOM, and mail data is never rendered on the server.

import type {
  ImageSource,
  PrivacySettings,
  RenderResult,
} from "@nebulamail/mail-render";
import { DEFAULT_PRIVACY, renderEmail } from "@nebulamail/mail-render";
import { useEffect, useRef, useState } from "react";

import { readCspNonce } from "../lib/csp-nonce";
import type { EmailDetail } from "./api";
import type { RemoteImages } from "./images";
import { loadInlineImages } from "./images";

const imageSource = (remote: RemoteImages | null): ImageSource => {
  if (remote === null || remote.kind === "unavailable") {
    return { kind: "none" };
  }
  if (remote.kind === "direct") {
    return { kind: "direct" };
  }
  const rewrite = (url: string): string | null => remote.urls.get(url) ?? null;
  return remote.kind === "proxy"
    ? { kind: "proxy", origin: remote.origin, rewrite }
    : { kind: "batch", rewrite };
};

export interface RenderOptions {
  /** Remote images the user asked to see (null: blocked). */
  readonly remote: RemoteImages | null;
  readonly view: "html" | "text";
  readonly settings?: PrivacySettings;
  /** The app's theme, for plain text (mail-render `textScheme`). */
  readonly scheme?: "light" | "dark";
}

const NO_CIDS: ReadonlyMap<string, string> = new Map();

/**
 * The rendered email. Pure: computed during render (the React Compiler
 * memoizes it on its inputs). `cid:` images the HTML needs are fetched once
 * per email (one image budget for the whole email: a cid that didn't load,
 * over the budget or broken, isn't asked for again on the next pass) and
 * rendered in on the next pass.
 */
export const useRenderedEmail = (
  email: EmailDetail | null | undefined,
  {
    remote,
    scheme = "light",
    settings: chosen = DEFAULT_PRIVACY,
    view,
  }: RenderOptions
): RenderResult | null => {
  const [loaded, setLoaded] = useState<{
    readonly emailId: string;
    readonly cids: ReadonlyMap<string, string>;
  } | null>(null);
  const cids =
    email !== null && email !== undefined && loaded?.emailId === email.id
      ? loaded.cids
      : NO_CIDS;

  // mail-render only loads images straight from the sender with the proxy
  // turned off.
  const settings =
    remote?.kind === "direct" ? { ...chosen, loadThroughProxy: false } : chosen;
  const styleNonce =
    typeof document === "undefined" ? undefined : readCspNonce();
  const result =
    email === null || email === undefined || typeof document === "undefined"
      ? null
      : renderEmail({ html: email.html, text: email.text }, settings, {
          cids,
          images: imageSource(remote),
          textScheme: scheme,
          userApproved: remote !== null,
          view,
          ...(styleNonce === undefined ? {} : { styleNonce }),
        });

  const missing = (result?.neededCids ?? []).filter((cid) => !cids.has(cid));
  const missingKey = missing.join(" ");

  // The cids already asked for, and the email on screen (effects only).
  const requested = useRef<{
    readonly emailId: string;
    readonly cids: Set<string>;
  } | null>(null);
  const shown = useRef<string | null>(null);
  const emailId = email?.id ?? null;
  useEffect(() => {
    shown.current = emailId;
  }, [emailId]);

  useEffect(() => {
    if (email === null || email === undefined || missingKey === "") {
      return;
    }
    if (requested.current?.emailId !== email.id) {
      requested.current = { cids: new Set(), emailId: email.id };
    }
    const asked = requested.current.cids;
    const wanted = missingKey.split(" ").filter((cid) => !asked.has(cid));
    if (wanted.length === 0) {
      return;
    }
    // Marked before loading: a later pass never asks for them again.
    for (const cid of wanted) {
      asked.add(cid);
    }
    const load = async (): Promise<void> => {
      const fetched = await loadInlineImages(email.inlineParts, wanted);
      if (shown.current === email.id && fetched.size > 0) {
        setLoaded((current) => ({
          cids: new Map([
            ...(current?.emailId === email.id ? current.cids : NO_CIDS),
            ...fetched,
          ]),
          emailId: email.id,
        }));
      }
    };
    // Inline images are optional: the email still shows without them.
    load().catch(() => null);
  }, [email, missingKey]);

  return result;
};
