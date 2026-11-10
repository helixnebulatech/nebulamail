// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The address books: ours (with "Share…" when the server can and the book
// allows it, `myRights.mayShare`) and the ones other people share with us,
// with what we may do in each. Sharing reuses the folders' share body.

import { Button, Dialog, Spinner } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, BookUser, Share2 } from "lucide-react";
import { useState } from "react";

import type { AddressBookSummary } from "../../contacts/api";
import { contactKeys, useAddressBooks } from "../../contacts/hooks";
import {
  bookSharingSupported,
  getBookShares,
  setBookShare,
} from "../../contacts/sharing";
import type { ShareLevel } from "../../mail/sharing";
import { SHARE_LEVELS } from "../../mail/sharing";
import { m } from "../../paraglide/messages.js";
import { ShareBody } from "../mail/share-dialog";
import { shared } from "../mail/styles";

const styles = stylex.create({
  list: {
    borderColor: colors.line,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
  },
  muted: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  row: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: { ":last-child": 0, default: "1px" },
    columnGap: "10px",
    display: "flex",
    minHeight: "44px",
    paddingBlock: "6px",
    paddingInline: "10px",
  },
  rowText: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minWidth: 0,
  },
  section: { display: "flex", flexDirection: "column", rowGap: "8px" },
  sections: { display: "flex", flexDirection: "column", rowGap: "18px" },
  sub: { color: colors.fg3, fontSize: "12px" },
  title: { fontSize: "12.5px", fontWeight: font.weightButton, margin: 0 },
});

const bookLevelLabel = (level: ShareLevel | "custom"): string =>
  ({
    custom: m.share_level_custom(),
    manage: m.contacts_share_level_manage(),
    read: m.contacts_share_level_read(),
    write: m.contacts_share_level_write(),
  })[level];

/** What we may do in a book shared with us. */
const accessText = (book: AddressBookSummary): string => {
  if (book.mayRemove) {
    return m.contacts_share_level_manage();
  }
  return book.mayWrite
    ? m.contacts_share_level_write()
    : m.contacts_share_level_read();
};

const BookList = ({
  canShare,
  onShare,
}: {
  readonly canShare: boolean;
  readonly onShare: (book: AddressBookSummary) => void;
}) => {
  const books = useAddressBooks();
  if (books.status === "pending") {
    return <Spinner label={m.contacts_loading()} />;
  }
  if (books.status === "error") {
    return <p role="alert">{m.contacts_error()}</p>;
  }
  const own = books.data.filter((book) => book.accountId === null);
  const others = books.data.filter((book) => book.accountId !== null);
  return (
    <div {...stylex.props(styles.sections)}>
      <section {...stylex.props(styles.section)}>
        <h3 {...stylex.props(styles.title)}>{m.contacts_books_own()}</h3>
        <ul
          aria-label={m.contacts_books_own()}
          {...stylex.props(shared.resetList, styles.list)}
        >
          {own.map((book) => (
            <li key={book.key} {...stylex.props(styles.row)}>
              <BookUser aria-hidden size={18} />
              <span {...stylex.props(styles.rowText)}>
                <bdi {...stylex.props(shared.ellipsis)}>{book.name}</bdi>
                {book.isDefault ? (
                  <span {...stylex.props(styles.sub)}>
                    {m.contacts_books_default()}
                  </span>
                ) : null}
              </span>
              {canShare && book.mayShare ? (
                <Button
                  icon={Share2}
                  onClick={() => {
                    onShare(book);
                  }}
                  size="sm"
                  variant="secondary"
                >
                  {m.contacts_books_share()}
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      </section>
      <section {...stylex.props(styles.section)}>
        <h3 {...stylex.props(styles.title)}>{m.contacts_books_shared()}</h3>
        {others.length === 0 ? (
          <p {...stylex.props(styles.muted)}>
            {m.contacts_books_none_shared()}
          </p>
        ) : (
          <ul
            aria-label={m.contacts_books_shared()}
            {...stylex.props(shared.resetList, styles.list)}
          >
            {others.map((book) => (
              <li key={book.key} {...stylex.props(styles.row)}>
                <BookUser aria-hidden size={18} />
                <span {...stylex.props(styles.rowText)}>
                  <bdi {...stylex.props(shared.ellipsis)}>{book.name}</bdi>
                  <bdi {...stylex.props(styles.sub, shared.ellipsis)}>
                    {book.owner}
                  </bdi>
                </span>
                <span {...stylex.props(styles.sub)}>{accessText(book)}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
};

export const BooksDialog = ({
  onOpenChange,
  open,
}: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}) => {
  const [sharing, setSharing] = useState<AddressBookSummary | null>(null);
  const support = useQuery({
    enabled: open,
    queryFn: bookSharingSupported,
    queryKey: [...contactKeys.all, "sharing"],
    staleTime: Infinity,
  });
  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          setSharing(null);
        }
        onOpenChange(next);
      }}
    >
      <Dialog.Content closeLabel={m.close()}>
        <Dialog.Title>
          {sharing === null
            ? m.contacts_books()
            : m.contacts_books_share_title({ name: sharing.name })}
        </Dialog.Title>
        <Dialog.Body>
          {sharing === null ? (
            <BookList canShare={support.data === true} onShare={setSharing} />
          ) : (
            <div {...stylex.props(styles.sections)}>
              <span>
                <Button
                  icon={ArrowLeft}
                  onClick={() => {
                    setSharing(null);
                  }}
                  size="sm"
                  variant="ghost"
                >
                  {m.contacts_books()}
                </Button>
              </span>
              <ShareBody
                initialLevel="read"
                invalidateKey={contactKeys.all}
                levelLabel={bookLevelLabel}
                levels={SHARE_LEVELS}
                loadShares={async (people) =>
                  await getBookShares(sharing.id, people)
                }
                nobody={m.contacts_books_share_nobody()}
                note={m.contacts_books_share_note()}
                saveShare={async (person, level) => {
                  await setBookShare(sharing.id, person, level);
                }}
                sharesKey={[...contactKeys.books(), "shares", sharing.id]}
              />
            </div>
          )}
        </Dialog.Body>
      </Dialog.Content>
    </Dialog.Root>
  );
};
