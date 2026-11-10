// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// An email's invitation and assigned-task cards. The reader loads this
// module lazily, only for an email with a calendar part
// (calendar/invitation-part.ts): the cards bring iTIP, the calendar API
// and editing with them.

import { invitationSender } from "../../calendar/itip";
import type { EmailDetail } from "../../mail/api";
import { InvitationCard } from "../calendar/invitation-card";
import { TaskAssignmentCard } from "../tasks/assignment-card";

export const InvitationCards = ({
  email,
}: {
  readonly email: Pick<EmailDetail, "attachments" | "from" | "senderVerdict">;
}) => {
  const sender = invitationSender(email.from, email.senderVerdict);
  return (
    <>
      <InvitationCard attachments={email.attachments} sender={sender} />
      <TaskAssignmentCard attachments={email.attachments} sender={sender} />
    </>
  );
};
