// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "This event or all events?" for changes to a repeating event. When only
// one occurrence can't be changed (it turns all-day or timed,
// `canChangeOneOccurrence` in calendar/editing.ts), it says so and offers
// only the whole series.

import { Button, Dialog } from "@nebulamail/ui";

import { m } from "../../paraglide/messages.js";
import type { Scope } from "./use-event-actions";

export interface ScopeDialogProps {
  readonly open: boolean;
  readonly title: string;
  readonly onChoose: (scope: Scope) => void;
  readonly onCancel: () => void;
  /** "This event" is possible (default: yes). */
  readonly oneAllowed?: boolean;
}

export const ScopeDialog = ({
  onCancel,
  onChoose,
  oneAllowed = true,
  open,
  title,
}: ScopeDialogProps) => (
  <Dialog.Root
    open={open}
    onOpenChange={(next) => {
      if (!next) {
        onCancel();
      }
    }}
  >
    <Dialog.Content closeLabel={m.close()} size="sm">
      <Dialog.Title>{title}</Dialog.Title>
      <Dialog.Body>
        {oneAllowed ? m.cal_scope_body() : m.cal_scope_kind_change()}
      </Dialog.Body>
      <Dialog.Footer>
        <Button onClick={onCancel} variant="ghost">
          {m.cancel()}
        </Button>
        <Button
          onClick={() => {
            onChoose("all");
          }}
          variant={oneAllowed ? "secondary" : "primary"}
        >
          {m.cal_scope_all()}
        </Button>
        {oneAllowed ? (
          <Button
            onClick={() => {
              onChoose("one");
            }}
            variant="primary"
          >
            {m.cal_scope_one()}
          </Button>
        ) : null}
      </Dialog.Footer>
    </Dialog.Content>
  </Dialog.Root>
);
