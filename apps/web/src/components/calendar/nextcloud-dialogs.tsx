// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Nextcloud's calendar extras through the bridge
// subscribing to a calendar by its URL (the calendar server
// fetches it; the web never opens it) and the trash bin of deleted
// calendars and events. Offered only where the server has them.

import {
  Button,
  Dialog,
  Field,
  Input,
  Spinner,
  useToast,
} from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useState } from "react";

import { useCalendars } from "../../calendar/hooks";
import { subscriptionUrlProblem } from "../../calendar/nextcloud";
import type { TrashItem } from "../../calendar/nextcloud";
import {
  useChangeTrash,
  useSubscribe,
  useTrash,
} from "../../calendar/nextcloud-hooks";
import { dateTimeFormat } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import { shared } from "../mail/styles";
import { ColorChooser } from "./calendar-dialogs";
import { SafeText, titleText } from "./event-text";
import { useDateLocale } from "./use-calendar-prefs";

const styles = stylex.create({
  actions: { columnGap: "6px", display: "flex", flexShrink: 0 },
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  footer: { columnGap: "8px", display: "flex", justifyContent: "flex-end" },
  form: { display: "flex", flexDirection: "column", rowGap: "16px" },
  heading: {
    color: colors.fg3,
    fontSize: "11.5px",
    fontWeight: font.weightSemibold,
    letterSpacing: "0.02em",
    margin: "12px 0 4px",
    textTransform: "uppercase",
  },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  item: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "12px",
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "space-between",
    paddingBlock: "8px",
    rowGap: "6px",
  },
  itemText: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minWidth: "12rem",
  },
  meta: { color: colors.fg3, fontSize: font.sizeSm },
  name: { fontSize: font.sizeMd },
});

const urlError = (url: string): string | null => {
  const problem = subscriptionUrlProblem(url);
  if (problem === null) {
    return null;
  }
  return {
    credentials: m.cal_subscribe_url_credentials,
    empty: m.cal_subscribe_url_empty,
    scheme: m.cal_subscribe_url_scheme,
  }[problem]();
};

const SubscribeForm = ({ onDone }: { readonly onDone: () => void }) => {
  const toast = useToast();
  const subscribe = useSubscribe();
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [color, setColor] = useState("#0b8043");
  const [error, setError] = useState<string | null>(null);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        const problem = urlError(url);
        if (problem !== null) {
          setError(problem);
          return;
        }
        if (name.trim() === "") {
          setError(m.cal_calendar_name_missing());
          return;
        }
        subscribe.mutate(
          { color, name, url },
          {
            onError: () => {
              setError(m.cal_subscribe_error());
            },
            onSuccess: () => {
              toast.show({ title: m.cal_subscribed() });
              onDone();
            },
          }
        );
      }}
      {...stylex.props(styles.form)}
    >
      <Field description={m.cal_subscribe_hint()} label={m.cal_subscribe_url()}>
        <Input
          // The dialog opens to paste the address.
          // oxlint-disable-next-line jsx-a11y/no-autofocus
          autoFocus
          dir="ltr"
          inputMode="url"
          maxLength={2048}
          onChange={(event) => {
            setUrl(event.target.value);
            setError(null);
          }}
          placeholder="webcal://"
          value={url}
        />
      </Field>
      <Field label={m.cal_calendar_name()}>
        <Input
          dir="auto"
          maxLength={255}
          onChange={(event) => {
            setName(event.target.value);
            setError(null);
          }}
          value={name}
        />
      </Field>
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
        <Button loading={subscribe.isPending} type="submit" variant="primary">
          {m.cal_subscribe_button()}
        </Button>
      </div>
    </form>
  );
};

/** "Subscribe to a calendar": its address, a name and a colour. */
export const SubscribeDialog = ({
  onOpenChange,
  open,
}: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}) => (
  <Dialog.Root open={open} onOpenChange={onOpenChange}>
    <Dialog.Content closeLabel={m.close()} size="sm">
      <Dialog.Title>{m.cal_subscribe_title()}</Dialog.Title>
      <Dialog.Body>
        {open ? (
          <SubscribeForm
            onDone={() => {
              onOpenChange(false);
            }}
          />
        ) : null}
      </Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);

const TrashRow = ({
  busy,
  calendarName,
  item,
  onAction,
}: {
  readonly item: TrashItem;
  readonly calendarName: string | null;
  readonly busy: boolean;
  readonly onAction: (action: "restore" | "destroy") => void;
}) => {
  const locale = useDateLocale();
  const date = (ms: number, withTime: boolean): string =>
    dateTimeFormat(locale, {
      dateStyle: "medium",
      ...(withTime ? { timeStyle: "short" } : {}),
    }).format(ms);
  const name =
    item.kind === "event"
      ? titleText(item.name)
      : item.name || m.cal_trash_untitled();
  const meta = [
    item.start === null ? null : date(item.start, true),
    calendarName,
    item.deletedAt === null
      ? null
      : m.cal_trash_deleted_on({ date: date(item.deletedAt, false) }),
  ].filter((part): part is string => part !== null);
  return (
    <li {...stylex.props(styles.item)}>
      <span {...stylex.props(styles.itemText)}>
        <span {...stylex.props(styles.name, shared.ellipsis)}>
          <SafeText text={name} />
        </span>
        <span {...stylex.props(styles.meta)}>
          <SafeText text={meta.join(" · ")} />
        </span>
      </span>
      <span {...stylex.props(styles.actions)}>
        <Button
          aria-label={m.cal_trash_restore_label({ name })}
          disabled={busy}
          onClick={() => {
            onAction("restore");
          }}
          size="sm"
          variant="secondary"
        >
          {m.cal_trash_restore()}
        </Button>
        <Button
          aria-label={m.cal_trash_delete_label({ name })}
          disabled={busy}
          onClick={() => {
            onAction("destroy");
          }}
          size="sm"
          variant="danger"
        >
          {m.cal_trash_delete()}
        </Button>
      </span>
    </li>
  );
};

const TrashBody = () => {
  const toast = useToast();
  const trash = useTrash(true);
  const calendars = useCalendars();
  const change = useChangeTrash();
  if (trash.isPending) {
    return <Spinner label={m.cal_loading()} />;
  }
  if (trash.isError) {
    return <p role="alert">{m.cal_trash_load_error()}</p>;
  }
  const { items, retentionDays } = trash.data;
  const calendarName = (id: string | null): string | null =>
    id === null
      ? null
      : (calendars.data?.find((c) => c.owner === null && c.id === id)?.name ??
        null);
  const act = (item: TrashItem, action: "restore" | "destroy"): void => {
    change.mutate(
      { action, ids: [item.id] },
      {
        onError: () => {
          toast.show({ title: m.cal_trash_error(), tone: "danger" });
        },
        onSuccess: () => {
          toast.show({
            title:
              action === "restore"
                ? m.cal_trash_restored()
                : m.cal_trash_deleted(),
          });
        },
      }
    );
  };
  const groups = [
    ["calendar", m.cal_trash_calendars()],
    ["event", m.cal_trash_events()],
  ] as const;
  return (
    <div>
      {retentionDays === null ? null : (
        <p {...stylex.props(styles.hint)}>
          {m.cal_trash_retention({ days: String(retentionDays) })}
        </p>
      )}
      {items.length === 0 ? <p>{m.cal_trash_empty()}</p> : null}
      {groups.map(([kind, label]) => {
        const list = items.filter((item) => item.kind === kind);
        return list.length === 0 ? null : (
          <section aria-label={label} key={kind}>
            <h3 {...stylex.props(styles.heading)}>{label}</h3>
            <ul {...stylex.props(shared.resetList)}>
              {list.map((item) => (
                <TrashRow
                  busy={change.isPending}
                  calendarName={calendarName(item.calendarId)}
                  item={item}
                  key={item.id}
                  onAction={(action) => {
                    act(item, action);
                  }}
                />
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
};

/** Deleted calendars and events: restore them or delete them for good. */
export const TrashDialog = ({
  onOpenChange,
  open,
}: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}) => (
  <Dialog.Root open={open} onOpenChange={onOpenChange}>
    <Dialog.Content closeLabel={m.close()}>
      <Dialog.Title>{m.cal_trash_title()}</Dialog.Title>
      <Dialog.Body>{open ? <TrashBody /> : null}</Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);
