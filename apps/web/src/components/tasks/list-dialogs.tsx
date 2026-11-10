// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Create a task list, rename or recolour one, or delete it.
// A list is a calendar (tasks live in calendars), so these are the
// calendar's dialogs with the words of lists; a list shared by someone
// else only takes a colour, like their calendars. "Add a calendar…" shows
// an existing calendar as a list (its key kept in the synced preferences,
// `taskLists`, as when a list is created). "Remove from Tasks" does the
// opposite without touching the calendar (`removedTaskLists`).

import {
  AlertDialog,
  Button,
  Dialog,
  Field,
  Input,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { CALENDAR_PALETTE } from "../../calendar/colors";
import {
  useCalendars,
  useDeleteCalendar,
  useUpdateCalendar,
} from "../../calendar/hooks";
import { m } from "../../paraglide/messages.js";
import { rememberTaskList } from "../../tasks/api";
import {
  refreshTasks,
  useCreateTaskList,
  useRememberTaskList,
  useRemoveTaskList,
} from "../../tasks/hooks";
import { addableCalendars } from "../../tasks/lists";
import type { TaskList } from "../../tasks/model";
import { ColorChooser } from "../calendar/calendar-dialogs";
import { colorVars } from "../calendar/event-chip";
import { SafeText, titleText } from "../calendar/event-text";
import { shared } from "../mail/styles";
import { rowStyles } from "./task-row";

const styles = stylex.create({
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  footer: { columnGap: "8px", display: "flex", justifyContent: "flex-end" },
  form: { display: "flex", flexDirection: "column", rowGap: "16px" },
  hint: { color: colors.fg2, fontSize: font.sizeSm, margin: 0 },
  owner: { color: colors.fg3, flexShrink: 0, fontSize: font.sizeSm },
  pick: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.md,
    columnGap: "10px",
    display: "flex",
    fontSize: font.sizeMd,
    minHeight: { default: "36px", [touch.coarse]: touch.target },
    paddingInline: "10px",
    width: "100%",
  },
  picks: { display: "flex", flexDirection: "column", rowGap: "2px" },
});

const [FALLBACK] = CALENDAR_PALETTE;

const ListForm = ({
  list,
  onCreated,
  onDone,
}: {
  readonly list: TaskList | null;
  readonly onDone: () => void;
  readonly onCreated: (key: string) => void;
}) => {
  const toast = useToast();
  const client = useQueryClient();
  const create = useCreateTaskList();
  const update = useUpdateCalendar();
  const calendars = useCalendars();
  const [name, setName] = useState(list?.name ?? "");
  // A new list gets the next colour of the palette.
  const [color, setColor] = useState(
    list?.color ??
      CALENDAR_PALETTE[
        (calendars.data?.length ?? 0) % CALENDAR_PALETTE.length
      ] ??
      FALLBACK
  );
  const [error, setError] = useState<string | null>(null);
  const canRename = list === null || list.owner === null;

  const submit = (): void => {
    if (canRename && name.trim() === "") {
      setError(m.tasks_list_name_missing());
      return;
    }
    if (list === null) {
      create.mutate(
        { color, name },
        {
          onError: () => {
            setError(m.tasks_error_save());
          },
          onSuccess: (created) => {
            toast.show({ title: m.tasks_list_created() });
            onCreated(created.key);
            onDone();
          },
        }
      );
      return;
    }
    const calendar = calendars.data?.find((item) => item.key === list.key);
    if (calendar !== undefined) {
      update.mutate(
        {
          calendar,
          color,
          ...(canRename && name.trim() !== list.name ? { name } : {}),
        },
        {
          onError: () => {
            toast.show({ title: m.tasks_error_save(), tone: "danger" });
          },
          onSettled: () => {
            void refreshTasks(client);
          },
        }
      );
    }
    onDone();
  };

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
      {...stylex.props(styles.form)}
    >
      {canRename ? (
        <Field label={m.tasks_list_name()}>
          <Input
            // The dialog opens to type the name.
            // oxlint-disable-next-line jsx-a11y/no-autofocus
            autoFocus
            dir="auto"
            maxLength={255}
            onChange={(event) => {
              setName(event.target.value);
              setError(null);
            }}
            value={name}
          />
        </Field>
      ) : null}
      <ColorChooser onChange={setColor} value={color} />
      {error === null ? null : (
        <p role="alert" {...stylex.props(styles.error)}>
          {error}
        </p>
      )}
      <div {...stylex.props(styles.footer)}>
        <Button onClick={onDone} variant="secondary">
          {m.cancel()}
        </Button>
        <Button loading={create.isPending} type="submit" variant="primary">
          {list === null ? m.tasks_create_list() : m.tasks_save()}
        </Button>
      </div>
    </form>
  );
};

/** New list (`list` null) or its name and colour. */
export const ListDialog = ({
  list,
  onCreated,
  onOpenChange,
  open,
}: {
  readonly list: TaskList | null;
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onCreated: (key: string) => void;
}) => (
  <Dialog.Root open={open} onOpenChange={onOpenChange}>
    <Dialog.Content closeLabel={m.close()} size="sm">
      <Dialog.Title>
        {list === null ? m.tasks_new_list() : m.tasks_edit_list()}
      </Dialog.Title>
      <Dialog.Body>
        {open ? (
          <ListForm
            list={list}
            onCreated={onCreated}
            onDone={() => {
              onOpenChange(false);
            }}
          />
        ) : null}
      </Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);

/** "Delete «Groceries»?" — the list (a calendar) and all in it. */
/**
 * "Remove from Tasks": the list stops showing (its calendar and tasks stay),
 * with "Undo" in the notification. `onRemoved` leaves the list if open.
 */
export const useRemoveFromTasks = (onRemoved: (list: TaskList) => void) => {
  const toast = useToast();
  const remove = useRemoveTaskList();
  const remember = useRememberTaskList();
  return (list: TaskList): void => {
    remove(list.key);
    onRemoved(list);
    toast.show({
      action: {
        label: m.toast_undo(),
        onClick: () => {
          remember(list.key, true);
        },
      },
      title: m.tasks_list_removed({ name: titleText(list.name) }),
    });
  };
};

export const DeleteListDialog = ({
  list,
  onDeleted,
  onOpenChange,
}: {
  readonly list: TaskList | null;
  readonly onOpenChange: (open: boolean) => void;
  readonly onDeleted: () => void;
}) => {
  const toast = useToast();
  const client = useQueryClient();
  const calendars = useCalendars();
  const remove = useDeleteCalendar();
  const calendar = calendars.data?.find((item) => item.key === list?.key);
  if (list === null || calendar === undefined) {
    return null;
  }
  return (
    <AlertDialog
      cancelLabel={m.cancel()}
      confirmLabel={m.tasks_delete_list_confirm()}
      description={m.tasks_delete_list_body()}
      onConfirm={() => {
        onOpenChange(false);
        remove.mutate(calendar, {
          onError: () => {
            toast.show({ title: m.tasks_error_save(), tone: "danger" });
          },
          onSettled: () => {
            void refreshTasks(client);
          },
          onSuccess: () => {
            rememberTaskList(list.key, false);
            toast.show({ title: m.tasks_list_deleted() });
            onDeleted();
          },
        });
      }}
      onOpenChange={onOpenChange}
      open
      title={m.tasks_delete_list_title({ name: titleText(list.name) })}
      tone="danger"
    />
  );
};

/**
 * "Add a calendar…": the calendars that aren't lists yet (not those the
 * server says can't hold tasks); choosing one shows it as a list.
 */
export const AddCalendarDialog = ({
  lists,
  onAdded,
  onOpenChange,
  open,
}: {
  readonly lists: readonly TaskList[];
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly onAdded: (key: string) => void;
}) => {
  const toast = useToast();
  const calendars = useCalendars();
  const remember = useRememberTaskList();
  const choices = addableCalendars(calendars.data ?? [], lists);
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Content closeLabel={m.close()} size="sm">
        <Dialog.Title>{m.tasks_add_calendar_title()}</Dialog.Title>
        <Dialog.Body>
          <div {...stylex.props(styles.form)}>
            <p {...stylex.props(styles.hint)}>
              {choices.length === 0
                ? m.tasks_add_calendar_none()
                : m.tasks_add_calendar_body()}
            </p>
            {choices.length === 0 ? null : (
              <ul
                aria-label={m.tasks_add_calendar_title()}
                {...stylex.props(shared.resetList, styles.picks)}
              >
                {choices.map((calendar) => (
                  <li key={calendar.key}>
                    <button
                      onClick={() => {
                        remember(calendar.key, true);
                        toast.show({
                          title: m.tasks_list_added({
                            name: titleText(calendar.name),
                          }),
                        });
                        onOpenChange(false);
                        onAdded(calendar.key);
                      }}
                      type="button"
                      {...stylex.props(
                        shared.resetButton,
                        shared.focusRing,
                        styles.pick
                      )}
                    >
                      <span
                        aria-hidden
                        {...stylex.props(
                          rowStyles.dot,
                          colorVars.color(calendar.color)
                        )}
                      />
                      <span {...stylex.props(shared.ellipsis)}>
                        <SafeText text={calendar.name} />
                      </span>
                      {calendar.owner === null ? null : (
                        <span {...stylex.props(styles.owner)}>
                          <SafeText
                            text={m.cal_shared_by({ owner: calendar.owner })}
                          />
                        </span>
                      )}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </Dialog.Body>
      </Dialog.Content>
    </Dialog.Root>
  );
};
