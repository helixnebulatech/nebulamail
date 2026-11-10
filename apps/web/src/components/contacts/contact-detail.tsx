// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// One contact: photo, names, ways to reach them, organization, addresses,
// birthday and notes, with actions (write, edit, export, delete). Every
// value is shown as text (cleaned, inside <bdi>); notes keep their line
// breaks but are never HTML. A remote photo is only fetched through the
// image proxy, and only when asked.

import type { AccountId, ContactCard } from "@nebulamail/jmap-client";
import {
  AlertDialog,
  Avatar,
  Button,
  EmptyState,
  IconButton,
  Panel,
  Spinner,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  ArrowLeft,
  Download,
  ImageDown,
  Mail,
  Pencil,
  Trash2,
  Users,
} from "lucide-react";
import { useState } from "react";
import type { ReactNode } from "react";

import { cardRights, exportVcf } from "../../contacts/api";
import type { ContactSummary, Rights } from "../../contacts/api";
import {
  birthdayOf,
  cardToForm,
  clean,
  cleanMultiline,
  displayName,
  isGroup,
  photoOf,
} from "../../contacts/card";
import { describeDate } from "../../contacts/dates";
import {
  useAddressBooks,
  useContact,
  useContactList,
  useDeleteContacts,
  usePhotoSrc,
} from "../../contacts/hooks";
import type { ContactRef } from "../../contacts/ref";
import { contactKey } from "../../contacts/ref";
import { useShortcut } from "../../lib/shortcuts";
import { saveBlob } from "../../mail/download";
import type { RemoteImages } from "../../mail/images";
import { loadContactPhotos } from "../../mail/images";
import { useFreeImages } from "../../mail/use-free-images";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { bp } from "../mail/breakpoints.stylex";
import type { Command } from "../mail/commands";
import { useCommands } from "../mail/commands";
import { useComposer } from "../mail/compose-context";
import { isInOverlay } from "../mail/mail-ui";
import { shared } from "../mail/styles";
import { ContactEditor, phoneLabel } from "./contact-editor";
import { dateWords } from "./date-words";

const styles = stylex.create({
  actions: {
    columnGap: "6px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "6px",
  },
  back: { display: { default: "none", [bp.narrow]: "inline-flex" } },
  body: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
    overflowY: "auto",
    paddingBlock: "20px 32px",
    paddingInline: { default: "28px", [bp.mobile]: "16px" },
    rowGap: "22px",
  },
  center: {
    alignItems: "center",
    display: "flex",
    flexGrow: 1,
    justifyContent: "center",
    padding: "24px",
  },
  dd: { margin: 0, minWidth: 0, overflowWrap: "anywhere" },
  dl: {
    columnGap: "16px",
    display: "grid",
    gridTemplateColumns: { default: "140px 1fr", [bp.mobile]: "1fr" },
    margin: 0,
    rowGap: { default: "12px", [bp.mobile]: "4px" },
  },
  dt: {
    color: colors.fg3,
    fontSize: "12.5px",
    paddingTop: { default: "1px", [bp.mobile]: "10px" },
  },
  hero: { alignItems: "center", columnGap: "16px", display: "flex" },
  heroText: {
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
    rowGap: "2px",
  },
  link: {
    color: colors.accText,
    textDecorationLine: { ":hover": "underline", default: "none" },
  },
  member: {
    alignItems: "center",
    borderRadius: radius.md,
    color: colors.fg,
    columnGap: "8px",
    display: "flex",
    paddingBlock: "4px",
    textDecoration: "none",
  },
  muted: { color: colors.fg3, fontSize: "12.5px" },
  name: {
    fontSize: "22px",
    fontWeight: font.weightBold,
    letterSpacing: "-0.02em",
    margin: 0,
  },
  note: { margin: 0, whiteSpace: "pre-wrap" },
  panel: { flexGrow: 1, minWidth: 0 },
  panelMobile: {
    borderRadius: { default: radius.xl, [bp.mobile]: 0 },
    borderWidth: { default: "1px", [bp.mobile]: 0 },
  },
  photoButton: { marginTop: "6px" },
  toolbar: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "4px",
    display: "flex",
    minHeight: "48px",
    paddingInline: "10px",
  },
  spacer: { flexGrow: 1 },
});

/** Runs `run` unless the key press happened in a dialog or menu. */
const guard =
  (run: () => void) =>
  (event: KeyboardEvent): void => {
    if (!isInOverlay(event.target)) {
      run();
    }
  };

const Row = ({
  children,
  term,
}: {
  readonly term: string;
  readonly children: ReactNode;
}) => (
  <>
    <dt {...stylex.props(styles.dt)}>{term}</dt>
    <dd {...stylex.props(styles.dd)}>{children}</dd>
  </>
);

/** Only digits and "+": a `tel:` link can't carry anything else. */
const telHref = (number: string): string | null => {
  const digits = number.replaceAll(/[^\d+]/gu, "");
  return /^\+?\d{3,}$/u.test(digits) ? `tel:${digits}` : null;
};

const formatBirthday = (card: ContactCard): string | null => {
  const date = birthdayOf(card);
  if (date === null) {
    return null;
  }
  return dateWords(describeDate(date, getLocale()));
};

/** A remote photo through the proxy, on request; freed when it changes. */
const useRemotePhoto = (contact: ContactRef) => {
  const toast = useToast();
  const [remote, setRemote] = useState<RemoteImages | null>(null);
  const [loading, setLoading] = useState(false);
  useFreeImages(remote);
  const load = async (): Promise<void> => {
    setLoading(true);
    try {
      setRemote(await loadContactPhotos(contact.id, contact.accountId));
    } catch {
      toast.show({
        priority: "high",
        title: m.contacts_photo_load_failed(),
        tone: "danger",
      });
    }
    setLoading(false);
  };
  return { load, loading, remote };
};

const Photo = ({
  accountId,
  card,
  name,
}: {
  readonly accountId: AccountId | null;
  readonly card: ContactCard;
  readonly name: string;
}) => {
  const photo = photoOf(card);
  const { load, loading, remote } = useRemotePhoto({ accountId, id: card.id });
  const remoteSrc =
    photo?.kind === "remote" &&
    (remote?.kind === "proxy" || remote?.kind === "batch")
      ? remote.urls.get(photo.url)
      : undefined;
  const src = usePhotoSrc(accountId, photo) ?? remoteSrc;
  return (
    <div>
      <Avatar
        colorKey={name}
        decorative={false}
        name={name}
        size={72}
        {...(src === undefined ? {} : { src })}
      />
      {photo?.kind === "remote" && src === undefined ? (
        <div {...stylex.props(styles.photoButton)}>
          {remote?.kind === "unavailable" ? (
            <span {...stylex.props(styles.muted)}>
              {m.contacts_photo_unavailable()}
            </span>
          ) : (
            <Button
              icon={ImageDown}
              loading={loading}
              onClick={() => {
                void load();
              }}
              size="sm"
              variant="ghost"
            >
              {m.contacts_photo_load()}
            </Button>
          )}
        </div>
      ) : null}
    </div>
  );
};

const Members = ({
  contacts,
  uids,
}: {
  readonly uids: readonly string[];
  readonly contacts: readonly ContactSummary[];
}) => {
  const known = uids.flatMap((uid) => {
    const contact = contacts.find((candidate) => candidate.uid === uid);
    return contact === undefined ? [] : [contact];
  });
  const unknown = uids.length - known.length;
  if (uids.length === 0) {
    return (
      <span {...stylex.props(styles.muted)}>{m.contacts_no_members()}</span>
    );
  }
  return (
    <ul {...stylex.props(shared.resetList)}>
      {known.map((contact) => {
        const name = contact.name === "" ? m.contacts_no_name() : contact.name;
        return (
          <li key={contact.key}>
            <Link
              params={{ contactId: contact.key }}
              to="/contacts/$contactId"
              {...stylex.props(styles.member, shared.focusRing)}
            >
              <Avatar name={name} size={24} />
              <bdi {...stylex.props(shared.ellipsis)}>{name}</bdi>
            </Link>
          </li>
        );
      })}
      {unknown > 0 ? (
        <li {...stylex.props(styles.muted)}>
          {m.contacts_members_unknown({ count: unknown })}
        </li>
      ) : null}
    </ul>
  );
};

const Details = ({
  accountId,
  card,
  contacts,
  owner,
  rights,
}: {
  readonly accountId: AccountId | null;
  readonly card: ContactCard;
  /** Cards of the same account (group members). */
  readonly contacts: readonly ContactSummary[];
  /** Whose address book it is, if shared with us. */
  readonly owner: string | null;
  /** Edit and delete only show when its address book allows them. */
  readonly rights: Rights;
}) => {
  const navigate = useNavigate();
  const toast = useToast();
  const composer = useComposer();
  const remove = useDeleteContacts();
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const form = cardToForm(card);
  const name = displayName(card) || m.contacts_no_name();
  const group = isGroup(card);
  const title = clean(form.title);
  const organization = clean(form.organization);
  const birthday = formatBirthday(card);
  const note = cleanMultiline(form.note);
  const emails = form.emails
    .map((row) => clean(row.address))
    .filter((address) => address !== "");

  const write = (to: readonly string[]): void => {
    composer.open({
      to: to.map((email) => (group ? { email } : { email, name })),
    });
  };

  const exportOne = async (): Promise<void> => {
    try {
      const { missingPhotos, text } = await exportVcf([
        { accountId, id: card.id },
      ]);
      const file = (
        name.replaceAll(/[^\p{L}\p{N} _-]/gu, "").trim() || "contact"
      ).slice(0, 60);
      saveBlob(new Blob([text], { type: "text/vcard" }), `${file}.vcf`);
      if (missingPhotos > 0) {
        toast.show({
          title: m.contacts_export_photos_missing({ count: missingPhotos }),
        });
      }
    } catch {
      toast.show({
        priority: "high",
        title: m.contacts_export_failed(),
        tone: "danger",
      });
    }
  };

  const destroy = async (): Promise<void> => {
    try {
      await remove.mutateAsync([{ accountId, id: card.id }]);
      toast.show({ title: m.contacts_deleted({ name }) });
      void navigate({ to: "/contacts" });
    } catch {
      toast.show({
        priority: "high",
        title: m.contacts_delete_failed(),
        tone: "danger",
      });
    }
  };

  useShortcut(
    "e",
    guard(() => {
      if (rights.mayWrite) {
        setEditing(true);
      }
    })
  );
  useShortcut(
    "#",
    guard(() => {
      if (rights.mayRemove) {
        setConfirming(true);
      }
    })
  );
  useShortcut(
    "r",
    guard(() => {
      if (emails.length > 0) {
        write(group ? emails : emails.slice(0, 1));
      }
    })
  );
  useShortcut(
    "escape",
    guard(() => {
      void navigate({ to: "/contacts" });
    })
  );

  const commands: Command[] = [
    ...(rights.mayWrite
      ? [
          {
            group: "actions" as const,
            icon: Pencil,
            id: "contact-edit",
            label: m.contacts_edit(),
            run: () => {
              setEditing(true);
            },
            shortcut: "e",
          },
        ]
      : []),
    ...(rights.mayRemove
      ? [
          {
            group: "actions" as const,
            icon: Trash2,
            id: "contact-delete",
            label: m.contacts_delete(),
            run: () => {
              setConfirming(true);
            },
            shortcut: "#",
          },
        ]
      : []),
    ...(emails.length > 0
      ? [
          {
            group: "actions" as const,
            icon: Mail,
            id: "contact-write",
            label: m.contacts_write(),
            run: () => {
              write(group ? emails : emails.slice(0, 1));
            },
            shortcut: "r",
          },
        ]
      : []),
  ];
  useCommands(commands);

  return (
    <>
      <div {...stylex.props(styles.toolbar)}>
        <span {...stylex.props(styles.back)}>
          <IconButton
            icon={ArrowLeft}
            label={m.contacts_back()}
            onClick={() => {
              void navigate({ to: "/contacts" });
            }}
          />
        </span>
        <span {...stylex.props(styles.spacer)} />
        {rights.mayWrite ? (
          <IconButton
            icon={Pencil}
            label={m.contacts_edit()}
            onClick={() => {
              setEditing(true);
            }}
            shortcut={["E"]}
          />
        ) : null}
        <IconButton
          icon={Download}
          label={m.contacts_export_one()}
          onClick={() => {
            void exportOne();
          }}
        />
        {rights.mayRemove ? (
          <IconButton
            icon={Trash2}
            label={m.contacts_delete()}
            onClick={() => {
              setConfirming(true);
            }}
            shortcut={["#"]}
          />
        ) : null}
      </div>
      <div {...stylex.props(styles.body)}>
        <div {...stylex.props(styles.hero)}>
          {group ? (
            <span aria-hidden>
              <Users size={40} />
            </span>
          ) : (
            <Photo accountId={accountId} card={card} name={name} />
          )}
          <div {...stylex.props(styles.heroText)}>
            <h2 {...stylex.props(styles.name)}>
              <bdi>{name}</bdi>
            </h2>
            {title !== "" || organization !== "" ? (
              <bdi {...stylex.props(styles.muted)}>
                {[title, organization]
                  .filter((text) => text !== "")
                  .join(" · ")}
              </bdi>
            ) : null}
            {owner === null ? null : (
              <bdi {...stylex.props(styles.muted)}>
                {rights.mayWrite
                  ? m.contacts_shared_by({ owner })
                  : m.contacts_shared_by_read_only({ owner })}
              </bdi>
            )}
          </div>
        </div>
        {emails.length > 0 ? (
          <div {...stylex.props(styles.actions)}>
            <Button
              icon={Mail}
              onClick={() => {
                write(group ? emails : emails.slice(0, 1));
              }}
              shortcut={["R"]}
              variant="primary"
            >
              {group ? m.contacts_write_group() : m.contacts_write()}
            </Button>
          </div>
        ) : null}
        <dl {...stylex.props(styles.dl)}>
          {form.emails.map((row) => (
            <Row key={row.key} term={m.contacts_email()}>
              <button
                onClick={() => {
                  write([row.address.trim()]);
                }}
                type="button"
                {...stylex.props(
                  shared.resetButton,
                  styles.link,
                  shared.focusRing
                )}
              >
                <bdi>{clean(row.address)}</bdi>
              </button>
            </Row>
          ))}
          {form.phones.map((row) => {
            const href = telHref(row.number);
            // A number kept as a URI (vCard `TEL;VALUE=uri`, RFC 6350
            // §6.4.1) is shown without its scheme.
            const text = clean(row.number.replace(/^tel:/iu, ""));
            return (
              <Row key={row.key} term={phoneLabel(row.kind)}>
                {href === null ? (
                  <bdi>{text}</bdi>
                ) : (
                  <a
                    href={href}
                    {...stylex.props(styles.link, shared.focusRing)}
                  >
                    <bdi>{text}</bdi>
                  </a>
                )}
              </Row>
            );
          })}
          {form.addresses.map((row) => (
            <Row key={row.key} term={m.contacts_address()}>
              <bdi {...stylex.props(styles.note)}>
                {[
                  row.street,
                  [row.postcode, row.locality]
                    .filter((part) => part.trim() !== "")
                    .join(" "),
                  row.region,
                  row.country,
                ]
                  .map((part) => clean(part))
                  .filter((part) => part !== "")
                  .join("\n")}
              </bdi>
            </Row>
          ))}
          {birthday === null ? null : (
            <Row term={m.contacts_birthday()}>{birthday}</Row>
          )}
          {group ? (
            <Row term={m.contacts_members()}>
              <Members contacts={contacts} uids={form.members} />
            </Row>
          ) : null}
          {note === "" ? null : (
            <Row term={m.contacts_notes()}>
              <p dir="auto" {...stylex.props(styles.note)}>
                {note}
              </p>
            </Row>
          )}
        </dl>
      </div>
      <ContactEditor
        accountId={accountId}
        card={card}
        contacts={contacts}
        onOpenChange={setEditing}
        open={editing && rights.mayWrite}
      />
      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.contacts_delete()}
        description={m.contacts_delete_confirm({ name })}
        onConfirm={() => {
          void destroy();
        }}
        onOpenChange={setConfirming}
        open={confirming}
        title={m.contacts_delete_title()}
        tone="danger"
      />
    </>
  );
};

const NO_RIGHTS: Rights = { mayRemove: false, mayWrite: false };
const ALL_RIGHTS: Rights = { mayRemove: true, mayWrite: true };

export const ContactDetail = ({
  reference,
}: {
  readonly reference: ContactRef;
}) => {
  const contact = useContact(reference);
  const list = useContactList();
  const books = useAddressBooks();
  const navigate = useNavigate();
  const content = (): ReactNode => {
    if (contact.status === "pending") {
      return (
        <div {...stylex.props(styles.center)}>
          <Spinner label={m.contacts_loading()} />
        </div>
      );
    }
    if (contact.status === "error" || contact.data === null) {
      return (
        <div {...stylex.props(styles.center)}>
          <EmptyState title={m.contacts_not_found()}>
            <Button
              onClick={() => {
                void navigate({ to: "/contacts" });
              }}
              variant="secondary"
            >
              {m.contacts_back()}
            </Button>
          </EmptyState>
        </div>
      );
    }
    const card = contact.data;
    const { accountId } = reference;
    const key = contactKey(reference);
    const summary = list.data?.contacts.find((entry) => entry.key === key);
    const sameAccount = (list.data?.contacts ?? []).filter(
      (entry) => entry.accountId === accountId
    );
    // Rights come from the card's books in its account (RFC 9610 myRights).
    // Until they load: everything on our own cards, nothing on shared ones.
    let rights: Rights = accountId === null ? ALL_RIGHTS : NO_RIGHTS;
    if (books.data !== undefined) {
      rights = cardRights(
        Object.entries(card.addressBookIds ?? {}).flatMap(([id, inBook]) =>
          inBook === true
            ? books.data
                .filter(
                  (book) => book.accountId === accountId && book.id === id
                )
                .map((book) => book.id)
            : []
        ),
        books.data.filter((book) => book.accountId === accountId)
      );
    }
    return (
      <Details
        accountId={accountId}
        card={card}
        contacts={sameAccount}
        owner={
          summary?.owner ??
          books.data?.find((book) => book.accountId === accountId)?.owner ??
          null
        }
        rights={rights}
      />
    );
  };
  return (
    <Panel
      label={m.contacts_detail_label()}
      xstyle={[styles.panel, styles.panelMobile]}
    >
      {content()}
    </Panel>
  );
};
