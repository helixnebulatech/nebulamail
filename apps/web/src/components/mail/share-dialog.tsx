// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Share one of our folders (RFC 8621 `shareWith`, mail:share): who has
// access and how (read / read and organize / manage), and add people from
// the server's directory (`Principal/query`, searched in the browser),
// plus what the server finds for the text typed (groups only found by
// name, an address the server shows no one for.7).
// The body is shared with the address books (contacts/sharing.ts) and the
// calendars (calendar/sharing.ts), which have their own levels and labels.
// Names and addresses come from the server: plain text, cleaned, in <bdi>.

import type { MailboxId, PrincipalId } from "@nebulamail/jmap-client";
import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  Avatar,
  Button,
  Dialog,
  IconButton,
  Input,
  Select,
  Spinner,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Search, UserMinus, UserPlus, Users } from "lucide-react";
import { useEffect, useState } from "react";

import { mailKeys } from "../../mail/queries";
import type { Person, Share, ShareLevel } from "../../mail/sharing";
import {
  SHARE_LEVELS,
  describePeople,
  getShares,
  isGroup,
  listPeople,
  matchPeople,
  mergeMatches,
  personDetail,
  searchPeople,
  serverSearchFilter,
  setShare,
} from "../../mail/sharing";
import { m } from "../../paraglide/messages.js";
import { shared } from "./styles";

const styles = stylex.create({
  list: {
    borderColor: colors.line,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
    maxHeight: "220px",
    overflowY: "auto",
  },
  muted: { color: colors.fg3, fontSize: font.sizeSm, marginBlock: "6px" },
  row: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: { ":last-child": 0, default: "1px" },
    columnGap: "10px",
    display: "flex",
    // On phones the access menu goes under the name instead of hiding it.
    flexWrap: "wrap",
    paddingBlock: "6px",
    paddingInline: "10px",
    rowGap: "6px",
  },
  rowText: {
    display: "flex",
    flexBasis: "120px",
    flexDirection: "column",
    flexGrow: 1,
    minWidth: 0,
  },
  section: { display: "flex", flexDirection: "column", rowGap: "8px" },
  sections: { display: "flex", flexDirection: "column", rowGap: "18px" },
  sub: { color: colors.fg3, fontSize: "12px" },
  title: { fontSize: "12.5px", fontWeight: font.weightButton, margin: 0 },
});

type LevelLabel<L extends string> = (level: L | "custom") => string;

const mailboxLevelLabel: LevelLabel<ShareLevel> = (level) =>
  ({
    custom: m.share_level_custom(),
    manage: m.share_level_manage(),
    read: m.share_level_read(),
    write: m.share_level_write(),
  })[level];

const LevelSelect = <L extends string>({
  label,
  levelLabel,
  levels,
  onChange,
  value,
}: {
  readonly label: string;
  readonly levels: readonly L[];
  readonly levelLabel: LevelLabel<L>;
  readonly value: L | "custom";
  readonly onChange: (level: L) => void;
}) => (
  <Select<L | "custom">
    label={label}
    onValueChange={(next) => {
      const level = levels.find((candidate) => candidate === next);
      if (level !== undefined) {
        onChange(level);
      }
    }}
    options={[
      // Set by another client, or a level this dialog doesn't offer.
      ...(value === "custom" || !levels.includes(value)
        ? [{ disabled: true, label: levelLabel(value), value }]
        : []),
      ...levels.map((level) => ({ label: levelLabel(level), value: level })),
    ]}
    value={value}
    width="fit"
  />
);

/** Its avatar: a group shows people instead of initials. */
const PersonAvatar = ({ person }: { readonly person: Person }) => (
  <Avatar
    {...(isGroup(person) ? { icon: Users } : {})}
    name={cleanDisplayText(person.name)}
    size={28}
  />
);

/** Name and, under it, the address and whether it is a group. */
const PersonText = ({ person }: { readonly person: Person }) => {
  const name = cleanDisplayText(person.name);
  const detail = personDetail(
    {
      email: cleanDisplayText(person.email ?? ""),
      name,
      type: person.type,
    },
    m.share_group()
  );
  return (
    <span {...stylex.props(styles.rowText)}>
      <bdi {...stylex.props(shared.ellipsis)}>{name}</bdi>
      {detail === null ? null : (
        <bdi {...stylex.props(styles.sub, shared.ellipsis)}>{detail}</bdi>
      )}
    </span>
  );
};

export interface ShareBodyProps<L extends string> {
  /** Query key of the current shares. */
  readonly sharesKey: readonly unknown[];
  /** Refreshed after every change. */
  readonly invalidateKey: readonly unknown[];
  readonly loadShares: (people: readonly Person[]) => Promise<Share<L>[]>;
  readonly saveShare: (person: PrincipalId, level: L | null) => Promise<void>;
  /** The levels offered, least to most. */
  readonly levels: readonly L[];
  /** Preselected for people being added. */
  readonly initialLevel: L;
  readonly levelLabel: LevelLabel<L>;
  /** The toast when a change fails (default: a generic one). */
  readonly errorText?: (error: unknown) => string;

  /** Under the list of people (what sharing gives). */
  readonly note?: string;
  /** When it is shared with nobody yet. */
  readonly nobody?: string;
  /** The toast after adding someone (default: "Shared with …"). */
  readonly addedText?: (name: string) => string;
}

/** Who the dialog offers for what was typed (none without a directory). */
const offered = (
  directory: readonly Person[] | null,
  query: string,
  fromServer: readonly Person[],
  taken: ReadonlySet<PrincipalId>
): Person[] =>
  query.trim() === "" || directory === null
    ? []
    : mergeMatches(matchPeople(directory, query, taken), fromServer, taken, 8);

/** Who has access and how, and adding people from the directory. */
export const ShareBody = <L extends string>({
  addedText,
  errorText,
  initialLevel,
  invalidateKey,
  levelLabel,
  levels,
  loadShares,
  nobody,
  note,
  saveShare,
  sharesKey,
}: ShareBodyProps<L>) => {
  const toast = useToast();
  const queryClient = useQueryClient();
  const [query, setQuery] = useState("");
  const [level, setLevel] = useState<L>(initialLevel);
  const people = useQuery({
    queryFn: listPeople,
    queryKey: [...mailKeys.all, "principals"],
    staleTime: 5 * 60_000,
  });
  const shares = useQuery({
    enabled: people.isSuccess,
    queryFn: async () => {
      const known = people.data ?? [];
      const list = await loadShares(known);
      // Someone found by a search, or an address: not in the directory.
      const unknown = list
        .map((share) => share.person.id)
        .filter((id) => !known.some((person) => person.id === id));
      const described = await describePeople(unknown).catch(() => []);
      return list.map((share) => ({
        ...share,
        person:
          described.find((person) => person.id === share.person.id) ??
          share.person,
      }));
    },
    queryKey: sharesKey,
  });
  // What the server finds for the text typed, a moment after typing.
  const [searched, setSearched] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => {
      setSearched(query.trim());
    }, 300);
    return () => {
      clearTimeout(timer);
    };
  }, [query]);
  const found = useQuery({
    enabled:
      people.isSuccess &&
      people.data !== null &&
      serverSearchFilter(searched) !== null,
    queryFn: async () => await searchPeople(searched),
    queryKey: [...mailKeys.all, "principals", "search", searched],
    staleTime: 60_000,
  });
  const change = useMutation({
    mutationFn: async ({
      next,
      person,
    }: {
      readonly person: PrincipalId;
      readonly next: L | null;
    }) => {
      await saveShare(person, next);
    },
    onError: (error) => {
      toast.show({
        priority: "high",
        title: errorText?.(error) ?? m.share_failed(),
        tone: "danger",
      });
    },
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: invalidateKey });
    },
  });

  if (people.status === "pending" || shares.status === "pending") {
    return <Spinner label={m.share_loading()} />;
  }
  if (people.status === "error" || shares.status === "error") {
    return <p role="alert">{m.share_load_failed()}</p>;
  }
  const current: readonly Share<L>[] = shares.data;
  // null: the server doesn't let us look people up (Stalwart 1.0 unless
  // its administrator allows directory queries); shares still show.
  const directory = people.data;
  const taken = new Set(current.map((share) => share.person.id));
  const fromServer =
    searched === query.trim() && found.isSuccess ? found.data : [];
  const matches = offered(directory, query, fromServer, taken);
  const searching = found.isFetching && matches.length === 0;

  return (
    <div {...stylex.props(styles.sections)}>
      <section {...stylex.props(styles.section)}>
        <h3 {...stylex.props(styles.title)}>{m.share_with_title()}</h3>
        {current.length === 0 ? (
          <p {...stylex.props(styles.muted)}>{nobody ?? m.share_nobody()}</p>
        ) : (
          <ul {...stylex.props(shared.resetList, styles.list)}>
            {current.map((share) => (
              <li key={share.person.id} {...stylex.props(styles.row)}>
                <PersonAvatar person={share.person} />
                <PersonText person={share.person} />
                <LevelSelect
                  label={m.share_level_for({
                    name: cleanDisplayText(share.person.name),
                  })}
                  levelLabel={levelLabel}
                  levels={levels}
                  onChange={(next) => {
                    change.mutate({ next, person: share.person.id });
                  }}
                  value={share.level}
                />
                <IconButton
                  icon={UserMinus}
                  label={m.share_remove({
                    name: cleanDisplayText(share.person.name),
                  })}
                  onClick={() => {
                    change.mutate({ next: null, person: share.person.id });
                  }}
                  size="sm"
                />
              </li>
            ))}
          </ul>
        )}
      </section>
      <section {...stylex.props(styles.section)}>
        <h3 {...stylex.props(styles.title)}>{m.share_add_title()}</h3>
        {directory === null ? (
          <p {...stylex.props(styles.muted)}>{m.share_directory_closed()}</p>
        ) : null}
        <Input
          disabled={directory === null}
          aria-label={m.share_search()}
          icon={Search}
          onChange={(event) => {
            setQuery(event.currentTarget.value);
          }}
          placeholder={m.share_search()}
          type="search"
          value={query}
        />
        <LevelSelect
          label={m.share_level_new()}
          levelLabel={levelLabel}
          levels={levels}
          onChange={setLevel}
          value={level}
        />
        {searching ? <Spinner label={m.share_searching()} /> : null}
        {query.trim() !== "" && matches.length === 0 && !searching ? (
          <p {...stylex.props(styles.muted)}>{m.share_no_match()}</p>
        ) : null}
        {matches.length > 0 ? (
          <ul {...stylex.props(shared.resetList, styles.list)}>
            {matches.map((person) => (
              <li key={person.id} {...stylex.props(styles.row)}>
                <PersonAvatar person={person} />
                <PersonText person={person} />
                <Button
                  icon={UserPlus}
                  loading={
                    change.isPending && change.variables.person === person.id
                  }
                  onClick={() => {
                    change.mutate(
                      { next: level, person: person.id },
                      {
                        onSuccess: () => {
                          setQuery("");
                          const name = cleanDisplayText(person.name);
                          toast.show({
                            title: addedText?.(name) ?? m.share_added({ name }),
                          });
                        },
                      }
                    );
                  }}
                  size="sm"
                  variant="secondary"
                >
                  {m.share_add()}
                </Button>
              </li>
            ))}
          </ul>
        ) : null}
        <p {...stylex.props(styles.muted)}>{note ?? m.share_note()}</p>
      </section>
    </div>
  );
};

export interface ShareDialogProps {
  readonly mailbox: { readonly id: MailboxId; readonly name: string } | null;
  readonly onOpenChange: (open: boolean) => void;
}

export const ShareDialog = ({ mailbox, onOpenChange }: ShareDialogProps) => (
  <Dialog.Root open={mailbox !== null} onOpenChange={onOpenChange}>
    <Dialog.Content closeLabel={m.close()}>
      <Dialog.Title>
        {mailbox === null
          ? m.share_title_generic()
          : m.share_title({ name: mailbox.name })}
      </Dialog.Title>
      <Dialog.Body>
        {mailbox === null ? null : (
          <ShareBody
            initialLevel="read"
            invalidateKey={mailKeys.mailboxes()}
            levelLabel={mailboxLevelLabel}
            levels={SHARE_LEVELS}
            loadShares={async (people) => await getShares(mailbox.id, people)}
            saveShare={async (person, level) => {
              await setShare(mailbox.id, person, level);
            }}
            sharesKey={[...mailKeys.mailboxes(), "shares", mailbox.id]}
          />
        )}
      </Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);
