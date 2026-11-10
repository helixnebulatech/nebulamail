// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { EmailId } from "@nebulamail/jmap-client";
import { createFileRoute } from "@tanstack/react-router";

import { Reader, ReaderError } from "../components/mail/reader";
import { parseMailboxParam } from "../mail/account-scope";
import { emailQuery, threadQuery } from "../mail/queries";

const ReaderRoute = () => {
  const { emailId } = Route.useParams();
  const parsed = EmailId.safeParse(emailId);
  // A new Reader per email: image and view choices don't carry over.
  return parsed.success ? (
    <Reader emailId={parsed.data} key={parsed.data} />
  ) : null;
};

export const Route = createFileRoute("/mail/$mailbox/$emailId")({
  component: ReaderRoute,
  errorComponent: ReaderError,
  // Also runs on hover/focus of a link to the email (`defaultPreload:
  // "intent"`, router.tsx): the thread and the body start loading before
  // the click. Browser only (the token lives there) and not awaited, so
  // navigation stays instant; the reader shows its own loading state.
  loader: ({ context: { queryClient }, params }) => {
    const id = EmailId.safeParse(params.emailId);
    if (typeof document === "undefined" || !id.success) {
      return;
    }
    const account = parseMailboxParam(params.mailbox).accountId;
    // Fresh cached data (the default staleTime) isn't fetched again; a
    // failure is left to the reader, which asks again and shows it.
    queryClient.query(threadQuery(id.data, account)).catch(() => null);
    queryClient.query(emailQuery(id.data, account)).catch(() => null);
  },
});
