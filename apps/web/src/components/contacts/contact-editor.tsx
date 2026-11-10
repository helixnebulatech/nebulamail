// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Create or edit a contact (or a group) in a dialog: names, emails, phones,
// addresses, organization, title, birthday, notes and photo. Used from the
// contacts page and from the reader ("Add to contacts"). Every field is
// plain text; the card keeps what we don't edit (contacts/card.ts).

import type { AccountId } from "@nebulamail/jmap-client";
import {
  Avatar,
  Button,
  Checkbox,
  Dialog,
  Field,
  IconButton,
  Input,
  SegmentedControl,
  Select,
  Textarea,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { ImagePlus, Plus, Trash2, X } from "lucide-react";
import { useDeferredValue, useId, useState } from "react";
import type { ReactNode } from "react";

import type { ContactSummary } from "../../contacts/api";
import type {
  AddressRow,
  BirthdayInput,
  CardFields,
  ContactForm,
  EmailRow,
  PhoneKind,
  PhoneRow,
  PlaceKind,
} from "../../contacts/card";
import {
  PHONE_KINDS,
  PLACE_KINDS,
  NO_BIRTHDAY,
  cardToForm,
  daysInMonth,
  emptyForm,
  formProblem,
  isGroup,
  parseBirthday,
} from "../../contacts/card";
import { describeDate, toGregorian } from "../../contacts/dates";
import {
  useAddressBooks,
  useCreateContact,
  usePhotoSrc,
  useUpdateContact,
} from "../../contacts/hooks";
import { PHOTO_ACCEPT, PhotoError, photoFromFile } from "../../contacts/photo";
import type { BookRef, ContactRef } from "../../contacts/ref";
import { searchContacts, sortContacts } from "../../contacts/search";
import { isValidEmail } from "../../mail/compose";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { shared } from "../mail/styles";
import { DateWords } from "./date-words";

const styles = stylex.create({
  add: { alignSelf: "flex-start" },
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  form: { display: "flex", flexDirection: "column", rowGap: "16px" },
  grid: {
    columnGap: "10px",
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(180px, 1fr))",
    rowGap: "12px",
  },
  legend: {
    color: colors.fg2,
    fontSize: "12.5px",
    fontWeight: font.weightButton,
    marginBottom: "6px",
    padding: 0,
  },
  members: {
    borderColor: colors.line,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
    display: "flex",
    flexDirection: "column",
    maxHeight: "240px",
    overflowY: "auto",
    padding: "6px 10px",
    rowGap: "4px",
  },
  muted: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  photo: { alignItems: "center", columnGap: "12px", display: "flex" },
  row: {
    alignItems: "flex-end",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "8px",
  },
  rowGrow: { flexBasis: "200px", flexGrow: 1, minWidth: 0 },
  year: { flexBasis: "140px", flexGrow: 0, minWidth: 0 },
  section: {
    borderStyle: "none",
    display: "flex",
    flexDirection: "column",
    margin: 0,
    minWidth: 0,
    padding: 0,
    rowGap: "8px",
  },
});

const Choice = <Value extends string>({
  label,
  onChange,
  options,
  value,
}: {
  readonly label: string;
  readonly value: Value;
  readonly options: readonly {
    readonly value: Value;
    readonly label: string;
  }[];
  readonly onChange: (value: Value) => void;
}) => (
  <Select
    label={label}
    onValueChange={onChange}
    options={options}
    value={value}
    width="fit"
  />
);

const placeLabel = (place: PlaceKind): string =>
  ({
    home: m.contacts_place_home(),
    other: m.contacts_place_other(),
    work: m.contacts_place_work(),
  })[place];

export const phoneLabel = (kind: PhoneKind): string =>
  ({
    home: m.contacts_phone_home(),
    mobile: m.contacts_phone_mobile(),
    voice: m.contacts_phone_voice(),
    work: m.contacts_phone_work(),
  })[kind];

const PLACE_OPTIONS = () =>
  PLACE_KINDS.map((value) => ({ label: placeLabel(value), value }));
const PHONE_OPTIONS = () =>
  PHONE_KINDS.map((value) => ({ label: phoneLabel(value), value }));

const Section = ({
  children,
  legend,
}: {
  readonly legend: string;
  readonly children: ReactNode;
}) => (
  <fieldset {...stylex.props(styles.section)}>
    <legend {...stylex.props(styles.legend)}>{legend}</legend>
    {children}
  </fieldset>
);

const replaceAt = <T,>(rows: readonly T[], index: number, row: T): T[] =>
  rows.map((current, at) => (at === index ? row : current));

const removeAt = <T,>(rows: readonly T[], index: number): T[] =>
  rows.filter((_, at) => at !== index);

const rowKey = (row: { readonly key: string | null }, index: number): string =>
  row.key ?? `new-${index}`;

const EmailRows = ({
  onChange,
  rows,
}: {
  readonly rows: readonly EmailRow[];
  readonly onChange: (rows: readonly EmailRow[]) => void;
}) => (
  <Section legend={m.contacts_emails()}>
    {rows.map((row, index) => (
      <div key={rowKey(row, index)} {...stylex.props(styles.row)}>
        <span {...stylex.props(styles.rowGrow)}>
          <Input
            aria-label={m.contacts_email()}
            autoComplete="off"
            invalid={row.address.trim() !== "" && !isValidEmail(row.address)}
            onChange={(event) => {
              onChange(
                replaceAt(rows, index, {
                  ...row,
                  address: event.currentTarget.value,
                })
              );
            }}
            type="email"
            value={row.address}
          />
        </span>
        <Choice
          label={m.contacts_kind()}
          onChange={(place) => {
            onChange(replaceAt(rows, index, { ...row, place }));
          }}
          options={PLACE_OPTIONS()}
          value={row.place}
        />
        <IconButton
          icon={Trash2}
          label={m.contacts_remove_row()}
          onClick={() => {
            onChange(removeAt(rows, index));
          }}
          size="sm"
        />
      </div>
    ))}
    <span {...stylex.props(styles.add)}>
      <Button
        icon={Plus}
        onClick={() => {
          onChange([...rows, { address: "", key: null, place: "other" }]);
        }}
        size="sm"
        variant="ghost"
      >
        {m.contacts_add_email()}
      </Button>
    </span>
  </Section>
);

const PhoneRows = ({
  onChange,
  rows,
}: {
  readonly rows: readonly PhoneRow[];
  readonly onChange: (rows: readonly PhoneRow[]) => void;
}) => (
  <Section legend={m.contacts_phones()}>
    {rows.map((row, index) => (
      <div key={rowKey(row, index)} {...stylex.props(styles.row)}>
        <span {...stylex.props(styles.rowGrow)}>
          <Input
            aria-label={m.contacts_phone()}
            autoComplete="off"
            onChange={(event) => {
              onChange(
                replaceAt(rows, index, {
                  ...row,
                  number: event.currentTarget.value,
                })
              );
            }}
            type="tel"
            value={row.number}
          />
        </span>
        <Choice
          label={m.contacts_kind()}
          onChange={(kind) => {
            onChange(replaceAt(rows, index, { ...row, kind }));
          }}
          options={PHONE_OPTIONS()}
          value={row.kind}
        />
        <IconButton
          icon={Trash2}
          label={m.contacts_remove_row()}
          onClick={() => {
            onChange(removeAt(rows, index));
          }}
          size="sm"
        />
      </div>
    ))}
    <span {...stylex.props(styles.add)}>
      <Button
        icon={Plus}
        onClick={() => {
          onChange([...rows, { key: null, kind: "mobile", number: "" }]);
        }}
        size="sm"
        variant="ghost"
      >
        {m.contacts_add_phone()}
      </Button>
    </span>
  </Section>
);

const ADDRESS_FIELDS = [
  ["street", () => m.contacts_street()],
  ["locality", () => m.contacts_locality()],
  ["region", () => m.contacts_region()],
  ["postcode", () => m.contacts_postcode()],
  ["country", () => m.contacts_country()],
] as const;

const AddressRows = ({
  onChange,
  rows,
}: {
  readonly rows: readonly AddressRow[];
  readonly onChange: (rows: readonly AddressRow[]) => void;
}) => (
  <Section legend={m.contacts_addresses()}>
    {rows.map((row, index) => (
      <div key={rowKey(row, index)} {...stylex.props(styles.section)}>
        <div {...stylex.props(styles.grid)}>
          {ADDRESS_FIELDS.map(([name, label]) => (
            <Field key={name} label={label()}>
              <Input
                autoComplete="off"
                onChange={(event) => {
                  onChange(
                    replaceAt(rows, index, {
                      ...row,
                      [name]: event.currentTarget.value,
                    })
                  );
                }}
                value={row[name]}
              />
            </Field>
          ))}
        </div>
        <div {...stylex.props(styles.row)}>
          <Choice
            label={m.contacts_kind()}
            onChange={(place) => {
              onChange(replaceAt(rows, index, { ...row, place }));
            }}
            options={PLACE_OPTIONS()}
            value={row.place}
          />
          <Button
            icon={Trash2}
            onClick={() => {
              onChange(removeAt(rows, index));
            }}
            size="sm"
            variant="ghost"
          >
            {m.contacts_remove_address()}
          </Button>
        </div>
      </div>
    ))}
    <span {...stylex.props(styles.add)}>
      <Button
        icon={Plus}
        onClick={() => {
          onChange([
            ...rows,
            {
              country: "",
              key: null,
              locality: "",
              place: "home",
              postcode: "",
              region: "",
              street: "",
            },
          ]);
        }}
        size="sm"
        variant="ghost"
      >
        {m.contacts_add_address()}
      </Button>
    </span>
  </Section>
);

const PhotoField = ({
  accountId,
  name,
  onChange,
  photo,
}: {
  readonly accountId: AccountId | null;
  readonly name: string;
  readonly photo: ContactForm["photo"];
  readonly onChange: (photo: ContactForm["photo"]) => void;
}) => {
  const toast = useToast();
  const inputId = useId();
  const src = usePhotoSrc(accountId, photo);
  const choose = async (file: File): Promise<void> => {
    try {
      onChange({ kind: "data", uri: await photoFromFile(file) });
    } catch (error) {
      toast.show({
        priority: "high",
        title:
          error instanceof PhotoError && error.problem === "type"
            ? m.contacts_photo_type()
            : m.contacts_photo_failed(),
        tone: "danger",
      });
    }
  };
  return (
    <div {...stylex.props(styles.photo)}>
      <Avatar
        name={name === "" ? "?" : name}
        size={56}
        {...(src === undefined ? {} : { src })}
      />
      <input
        accept={PHOTO_ACCEPT}
        id={inputId}
        onChange={(event) => {
          const file = event.currentTarget.files?.[0];
          event.currentTarget.value = "";
          if (file !== undefined) {
            void choose(file);
          }
        }}
        tabIndex={-1}
        type="file"
        aria-hidden
        {...stylex.props(shared.srOnly)}
      />
      <Button
        icon={ImagePlus}
        onClick={() => {
          document
            .querySelector<HTMLInputElement>(`#${CSS.escape(inputId)}`)
            ?.click();
        }}
        size="sm"
        variant="secondary"
      >
        {photo === null ? m.contacts_photo_add() : m.contacts_photo_change()}
      </Button>
      {photo === null ? null : (
        <Button
          icon={X}
          onClick={() => {
            onChange(null);
          }}
          size="sm"
          variant="ghost"
        >
          {m.contacts_photo_remove()}
        </Button>
      )}
      {photo?.kind === "remote" ? (
        <p {...stylex.props(styles.muted)}>{m.contacts_photo_remote()}</p>
      ) : null}
    </div>
  );
};

const MemberPicker = ({
  contacts,
  members,
  onChange,
}: {
  readonly contacts: readonly ContactSummary[];
  readonly members: readonly string[];
  readonly onChange: (members: readonly string[]) => void;
}) => {
  const [query, setQuery] = useState("");
  const people = sortContacts(
    contacts.filter((contact) => !contact.group),
    getLocale()
  );
  // The list follows the typing a moment later: the box never waits.
  const deferredQuery = useDeferredValue(query);
  const shown = searchContacts(people, deferredQuery).slice(0, 200);
  return (
    <Section legend={m.contacts_members()}>
      <Input
        aria-label={m.contacts_members_search()}
        onChange={(event) => {
          setQuery(event.currentTarget.value);
        }}
        placeholder={m.contacts_members_search()}
        type="search"
        value={query}
      />
      <div {...stylex.props(styles.members)}>
        {shown.length === 0 ? (
          <p {...stylex.props(styles.muted)}>{m.contacts_none_found()}</p>
        ) : (
          shown.map((contact) => {
            const { uid } = contact;
            return (
              <Checkbox
                checked={uid !== null && members.includes(uid)}
                disabled={uid === null}
                key={contact.id}
                label={
                  <bdi>
                    {contact.name === "" ? m.contacts_no_name() : contact.name}
                    {uid === null ? ` · ${m.contacts_no_uid()}` : ""}
                  </bdi>
                }
                onCheckedChange={(checked) => {
                  if (uid === null) {
                    return;
                  }
                  onChange(
                    checked
                      ? [...members, uid]
                      : members.filter((member) => member !== uid)
                  );
                }}
              />
            );
          })
        )}
      </div>
    </Section>
  );
};

export interface ContactEditorProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  /** Editing this card; creating a new one when absent. */
  readonly card?: CardFields;
  /** The edited card's account (null or absent: ours). */
  readonly accountId?: AccountId | null;
  /** Starting values for a new contact (e.g. a sender's name and address). */
  readonly initial?: ContactForm;
  /** For groups: everyone who can be a member. */
  readonly contacts?: readonly ContactSummary[];
  readonly onSaved?: (ref: ContactRef) => void;
}

const NO_CONTACTS: readonly ContactSummary[] = [];

type Patch = (changes: Partial<ContactForm>) => void;

const EditorFooter = ({
  formId,
  onCancel,
  saving,
}: {
  readonly formId: string;
  readonly saving: boolean;
  readonly onCancel: () => void;
}) => (
  <div {...stylex.props(styles.row)}>
    <span {...stylex.props(styles.rowGrow)} />
    <Button onClick={onCancel} variant="ghost">
      {m.cancel()}
    </Button>
    <Button form={formId} loading={saving} type="submit" variant="primary">
      {m.contacts_save()}
    </Button>
  </div>
);

/** Photo, first and last name, and the display name. */
const NameFields = ({
  accountId,
  form,
  patch,
}: {
  readonly accountId: AccountId | null;
  readonly form: ContactForm;
  readonly patch: Patch;
}) => {
  const displayed = [form.given, form.surname]
    .filter((part) => part.trim() !== "")
    .join(" ");
  return (
    <>
      <PhotoField
        accountId={accountId}
        name={form.full === "" ? displayed : form.full}
        onChange={(photo) => {
          patch({ photo });
        }}
        photo={form.photo}
      />
      <div {...stylex.props(styles.grid)}>
        <Field label={m.contacts_given()}>
          <Input
            autoComplete="off"
            autoFocus
            onChange={(event) => {
              patch({ given: event.currentTarget.value });
            }}
            value={form.given}
          />
        </Field>
        <Field label={m.contacts_surname()}>
          <Input
            autoComplete="off"
            onChange={(event) => {
              patch({ surname: event.currentTarget.value });
            }}
            value={form.surname}
          />
        </Field>
      </div>
      <Field description={m.contacts_full_hint()} label={m.contacts_full()}>
        <Input
          autoComplete="off"
          onChange={(event) => {
            patch({ full: event.currentTarget.value });
          }}
          placeholder={displayed}
          value={form.full}
        />
      </Field>
    </>
  );
};

const monthOptions = (locale: string) => {
  const format = new Intl.DateTimeFormat(locale, {
    month: "long",
    timeZone: "UTC",
  });
  return Array.from({ length: 12 }, (_, index) => ({
    label: format.format(new Date(Date.UTC(2000, index, 1))),
    value: String(index + 1),
  }));
};

/**
 * Day, month and an optional year (a birthday without a year is common:
 * vCard `--1224`). A date in another calendar is shown, not edited here;
 * "Change" turns it into a Gregorian one (the same day when we know it).
 */
const BirthdayField = ({
  onChange,
  value,
}: {
  readonly value: BirthdayInput;
  readonly onChange: (value: BirthdayInput) => void;
}) => {
  const locale = getLocale();
  const yearId = useId();
  const parsed = parseBirthday(value);
  if (value.calendarScale !== null && parsed !== null && parsed !== "invalid") {
    const text = describeDate(parsed, locale);
    return (
      <Section legend={m.contacts_birthday()}>
        <p {...stylex.props(styles.muted)}>
          <DateWords text={text} />
        </p>
        <span {...stylex.props(styles.add)}>
          <Button
            onClick={() => {
              const gregorian =
                parsed.year === null ? null : toGregorian(parsed);
              onChange(
                gregorian === null
                  ? NO_BIRTHDAY
                  : {
                      calendarScale: null,
                      day: String(gregorian.getUTCDate()),
                      month: String(gregorian.getUTCMonth() + 1),
                      year: String(gregorian.getUTCFullYear()),
                    }
              );
            }}
            size="sm"
            variant="secondary"
          >
            {m.contacts_birthday_to_gregorian()}
          </Button>
        </span>
      </Section>
    );
  }
  const month = Number(value.month);
  const year = /^\d{4}$/u.test(value.year) ? Number(value.year) : null;
  const days = value.month === "" ? 31 : daysInMonth(month, year);
  const empty = { label: "—", value: "" };
  return (
    <Section legend={m.contacts_birthday()}>
      <div {...stylex.props(styles.row)}>
        <Choice
          label={m.contacts_birthday_day()}
          onChange={(day) => {
            onChange({ ...value, day });
          }}
          options={[
            empty,
            ...Array.from({ length: days }, (_, index) => ({
              label: String(index + 1),
              value: String(index + 1),
            })),
            // A day the month doesn't have stays visible (and is refused).
            ...(Number(value.day) > days
              ? [{ label: value.day, value: value.day }]
              : []),
          ]}
          value={value.day}
        />
        <Choice
          label={m.contacts_birthday_month()}
          onChange={(next) => {
            onChange({ ...value, month: next });
          }}
          options={[empty, ...monthOptions(locale)]}
          value={value.month}
        />
        <span {...stylex.props(styles.year)}>
          <label htmlFor={yearId} {...stylex.props(shared.srOnly)}>
            {m.contacts_birthday_year()}
          </label>
          <Input
            autoComplete="off"
            id={yearId}
            inputMode="numeric"
            maxLength={4}
            onChange={(event) => {
              onChange({ ...value, year: event.currentTarget.value });
            }}
            placeholder={m.contacts_birthday_year_optional()}
            value={value.year}
          />
        </span>
        {value.day === "" && value.month === "" && value.year === "" ? null : (
          <IconButton
            icon={X}
            label={m.contacts_birthday_clear()}
            onClick={() => {
              onChange(NO_BIRTHDAY);
            }}
            size="sm"
          />
        )}
      </div>
    </Section>
  );
};

/** Phones, company, job title, addresses and birthday (people only). */
const PersonFields = ({
  form,
  patch,
}: {
  readonly form: ContactForm;
  readonly patch: Patch;
}) => (
  <>
    <PhoneRows
      onChange={(phones) => {
        patch({ phones });
      }}
      rows={form.phones}
    />
    <div {...stylex.props(styles.grid)}>
      <Field label={m.contacts_organization()}>
        <Input
          autoComplete="off"
          onChange={(event) => {
            patch({ organization: event.currentTarget.value });
          }}
          value={form.organization}
        />
      </Field>
      <Field label={m.contacts_title()}>
        <Input
          autoComplete="off"
          onChange={(event) => {
            patch({ title: event.currentTarget.value });
          }}
          value={form.title}
        />
      </Field>
    </div>
    <AddressRows
      onChange={(addresses) => {
        patch({ addresses });
      }}
      rows={form.addresses}
    />
    <BirthdayField
      onChange={(birthday) => {
        patch({ birthday });
      }}
      value={form.birthday}
    />
  </>
);

const useSave = ({
  accountId,
  card,
  onOpenChange,
  onSaved,
}: {
  readonly card: CardFields | undefined;
  readonly onOpenChange: (open: boolean) => void;
  readonly onSaved: ((ref: ContactRef) => void) | undefined;
  readonly accountId: AccountId | null;
}) => {
  const toast = useToast();
  const create = useCreateContact();
  const update = useUpdateContact();
  const failed = (): void => {
    toast.show({
      priority: "high",
      title: m.contacts_save_failed(),
      tone: "danger",
    });
  };
  const done = (ref: ContactRef): void => {
    toast.show({ title: m.contacts_saved() });
    onSaved?.(ref);
    onOpenChange(false);
  };
  const save = (form: ContactForm, book: BookRef | null): void => {
    if (card !== undefined) {
      update.mutate(
        { accountId, form, original: card },
        {
          onError: failed,
          onSuccess: () => {
            done({ accountId, id: card.id });
          },
        }
      );
    } else if (book === null) {
      failed();
    } else {
      create.mutate({ book, form }, { onError: failed, onSuccess: done });
    }
  };
  return { save, saving: create.isPending || update.isPending };
};

const EditorBody = ({
  accountId = null,
  card,
  contacts = NO_CONTACTS,
  initial,
  onOpenChange,
  onSaved,
}: Omit<ContactEditorProps, "open">) => {
  const books = useAddressBooks();
  const { save, saving } = useSave({
    accountId,
    card,
    onOpenChange,
    onSaved,
  });
  const [form, setForm] = useState<ContactForm>(
    card === undefined ? (initial ?? emptyForm()) : cardToForm(card)
  );
  const [book, setBook] = useState<string | null>(null);
  const [tried, setTried] = useState(false);
  const formId = useId();
  const writable = (books.data ?? []).filter((entry) => entry.mayWrite);
  // Ours first (listAddressBooks), then writable books shared with us.
  const targetKey = book ?? writable[0]?.key ?? null;
  const target = writable.find((entry) => entry.key === targetKey) ?? null;
  // A new group's members come from the account it is created in.
  const candidates =
    card === undefined
      ? contacts.filter(
          (contact) => contact.accountId === (target?.accountId ?? null)
        )
      : contacts;
  const problem = formProblem(form, isValidEmail);
  const group = form.kind === "group";
  const patch: Patch = (changes) => {
    setForm((current) => ({ ...current, ...changes }));
  };

  return (
    <form
      id={formId}
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        setTried(true);
        if (problem === null) {
          save(
            form,
            target === null
              ? null
              : { accountId: target.accountId, id: target.id }
          );
        }
      }}
      {...stylex.props(styles.form)}
    >
      {card === undefined && initial === undefined ? (
        <SegmentedControl
          label={m.contacts_kind_label()}
          onValueChange={(kind) => {
            patch({ kind });
          }}
          options={[
            { label: m.contacts_kind_person(), value: "individual" },
            { label: m.contacts_kind_group(), value: "group" },
          ]}
          value={form.kind}
        />
      ) : null}
      {group ? (
        <Field label={m.contacts_group_name()}>
          <Input
            autoFocus
            onChange={(event) => {
              patch({ full: event.currentTarget.value });
            }}
            value={form.full}
          />
        </Field>
      ) : (
        <NameFields accountId={accountId} form={form} patch={patch} />
      )}
      <EmailRows
        onChange={(emails) => {
          patch({ emails });
        }}
        rows={form.emails}
      />
      {group ? (
        <MemberPicker
          contacts={candidates}
          members={form.members}
          onChange={(members) => {
            patch({ members });
          }}
        />
      ) : (
        <PersonFields form={form} patch={patch} />
      )}
      <Field label={m.contacts_notes()}>
        <Textarea
          onChange={(event) => {
            patch({ note: event.currentTarget.value });
          }}
          rows={3}
          value={form.note}
        />
      </Field>
      {card === undefined && writable.length > 1 && target !== null ? (
        <Field label={m.contacts_book()}>
          <Choice
            label={m.contacts_book()}
            onChange={setBook}
            options={writable.map((entry) => ({
              label:
                entry.owner === null
                  ? entry.name
                  : m.contacts_book_of({
                      name: entry.name,
                      owner: entry.owner,
                    }),
              value: entry.key,
            }))}
            value={target.key}
          />
        </Field>
      ) : null}
      {tried && problem !== null ? (
        <p role="alert" {...stylex.props(styles.error)}>
          {{
            empty: m.contacts_problem_empty,
            "invalid-birthday": m.contacts_problem_birthday,
            "invalid-email": m.contacts_problem_email,
          }[problem]()}
        </p>
      ) : null}
      <EditorFooter
        formId={formId}
        onCancel={() => {
          onOpenChange(false);
        }}
        saving={saving}
      />
    </form>
  );
};

const editorTitle = ({
  card,
  initial,
}: Pick<ContactEditorProps, "card" | "initial">): string => {
  if (card === undefined) {
    return initial?.kind === "group"
      ? m.contacts_new_group()
      : m.contacts_new_title();
  }
  return isGroup(card) ? m.contacts_edit_group() : m.contacts_edit_title();
};

export const ContactEditor = ({
  open,
  onOpenChange,
  ...props
}: ContactEditorProps) => (
  <Dialog.Root open={open} onOpenChange={onOpenChange}>
    <Dialog.Content closeLabel={m.close()} size="lg">
      <Dialog.Title>{editorTitle(props)}</Dialog.Title>
      <Dialog.Body>
        {/* A fresh form each time it opens. */}
        {open ? <EditorBody onOpenChange={onOpenChange} {...props} /> : null}
      </Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);
