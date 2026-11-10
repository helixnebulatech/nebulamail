// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Settings › Filters: the visual rules (reorder by dragging or with the
// arrow keys on the handle, switch on/off, edit, delete), or the raw
// script when the active one isn't ours. Every change saves at once.
// Below, the account's Sieve scripts (scripts-list.tsx): any of them can
// be opened here as Sieve.

import { EmailId } from "@nebulamail/jmap-client";
import {
  AlertDialog,
  Button,
  IconButton,
  Menu,
  MenuItem,
  Spinner,
  Switch,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate, useSearch } from "@tanstack/react-router";
import {
  ArrowDown,
  ArrowUp,
  Ellipsis,
  GripVertical,
  Pencil,
  Plus,
  Trash2,
} from "lucide-react";
import { useRef, useState } from "react";
import type { ReactNode, DragEvent, KeyboardEvent } from "react";

import { useMailboxes } from "../../mail/hooks";
import { m } from "../../paraglide/messages.js";
import type { EmailFacts } from "../../settings/filters/api";
import {
  folderOptions,
  resolveFolders,
} from "../../settings/filters/mailboxes";
import type { FolderOption } from "../../settings/filters/mailboxes";
import type { Action, Condition, Rule } from "../../settings/filters/model";
import {
  emptyRule,
  forwardCount,
  newRuleId,
} from "../../settings/filters/model";
import {
  useEmailFacts,
  useImapNames,
  useSaveFilters,
  useScriptText,
  useScripts,
  useSupport,
} from "../../settings/queries";
import type { ScriptInfo, ScriptsState } from "../../settings/scripts";
import { ScriptError } from "../../settings/scripts";
import { mailboxLabel } from "../mail/mailbox-label";
import { settingsMeasure } from "./measure.stylex";
import { RawScript } from "./raw-script";
import { RuleEditor } from "./rule-editor";
import { ScriptsList } from "./scripts-list";
import {
  InlineEmpty,
  Notice,
  SectionHeader,
  SubsectionHeader,
} from "./section";

const styles = stylex.create({
  list: {
    listStyle: "none",
    margin: 0,
    maxWidth: settingsMeasure.width,
    padding: 0,
  },
  row: {
    alignItems: "center",
    backgroundColor: colors.bg,
    borderColor: colors.line,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    columnGap: "8px",
    display: "flex",
    marginBottom: "8px",
    paddingBlock: "8px",
    paddingInline: "6px 8px",
  },
  dropTarget: {
    borderColor: colors.acc,
    boxShadow: `0 0 0 2px ${colors.accSoft}`,
  },
  disabled: { opacity: 0.6 },
  handle: { cursor: "grab" },
  text: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minWidth: 0,
    rowGap: "2px",
  },
  name: {
    color: colors.fg,
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  summary: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  srOnly: {
    clipPath: "inset(50%)",
    height: "1px",
    overflow: "hidden",
    position: "absolute",
    whiteSpace: "nowrap",
    width: "1px",
  },
});

// --- Words for a rule -------------------------------------------------------

const quoteText = (text: string): string => `“${text}”`;

const describeCondition = (condition: Condition): string => {
  switch (condition.kind) {
    case "size": {
      return condition.op === "over"
        ? m.filter_sum_size_over({ size: String(condition.kilobytes) })
        : m.filter_sum_size_under({ size: String(condition.kilobytes) });
    }
    case "attachment": {
      return condition.negate
        ? m.filter_attachment_no()
        : m.filter_attachment_yes();
    }
    case "match": {
      const field =
        condition.field === "header"
          ? condition.header
          : {
              body: m.filter_field_body,
              from: m.filter_field_from,
              listId: m.filter_field_list,
              recipient: m.filter_field_recipient,
              subject: m.filter_field_subject,
            }[condition.field]();
      const op = {
        contains: condition.negate
          ? m.filter_op_not_contains
          : m.filter_op_contains,
        is: condition.negate ? m.filter_op_not_is : m.filter_op_is,
        matches: condition.negate
          ? m.filter_op_not_matches
          : m.filter_op_matches,
        regex: condition.negate ? m.filter_op_not_regex : m.filter_op_regex,
      }[condition.op]();
      return `${field} ${op.toLowerCase()} ${quoteText(condition.value)}`;
    }
    default: {
      return condition satisfies never;
    }
  }
};

const describeAction = (
  action: Action,
  folders: readonly FolderOption[]
): string => {
  const folder = (id: string, name: string): string => {
    const found = folders.find((item) => item.id === id);
    return found === undefined ? name : mailboxLabel(found.mailbox);
  };
  switch (action.kind) {
    case "move": {
      return m.filter_sum_move({
        folder: folder(action.mailboxId, action.mailboxName),
      });
    }
    case "copy": {
      return m.filter_sum_copy({
        folder: folder(action.mailboxId, action.mailboxName),
      });
    }
    case "markRead": {
      return m.filter_action_read();
    }
    case "star": {
      return m.filter_action_star();
    }
    case "keyword": {
      return m.filter_sum_keyword({ keyword: action.keyword });
    }
    case "forward": {
      return m.filter_sum_forward({ address: action.address });
    }
    case "reject": {
      return m.filter_action_reject();
    }
    case "discard": {
      return m.filter_action_discard();
    }
    case "stop": {
      return m.filter_action_stop();
    }
    default: {
      return action satisfies never;
    }
  }
};

const describeRule = (rule: Rule, folders: readonly FolderOption[]): string => {
  const joiner = rule.match === "all" ? m.filter_sum_and() : m.filter_sum_or();
  const conditions =
    rule.conditions.length === 0
      ? m.filter_sum_all_mail()
      : rule.conditions.map(describeCondition).join(` ${joiner} `);
  const actions = rule.actions
    .map((action) => describeAction(action, folders))
    .join(", ");
  return `${conditions} → ${actions}`;
};

/** A rule pre-filled from an email (the reader's "Create filter"). */
const ruleFromFacts = (facts: EmailFacts): Rule => {
  const condition: Condition =
    facts.listId === ""
      ? {
          field: "from",
          header: "",
          kind: "match",
          negate: false,
          op: "contains",
          value: facts.from,
        }
      : {
          field: "listId",
          header: "",
          kind: "match",
          negate: false,
          op: "contains",
          value: facts.listId,
        };
  return {
    ...emptyRule(facts.listId === "" ? facts.from : facts.listId),
    conditions: [condition],
  };
};

const move = <T,>(list: readonly T[], from: number, to: number): T[] => {
  const next = [...list];
  const [item] = next.splice(from, 1);
  if (item !== undefined) {
    next.splice(to, 0, item);
  }
  return next;
};

// --- Rule list --------------------------------------------------------------

interface Editing {
  readonly rule: Rule;
  /** Index in the list, or null for a new rule (added at the end). */
  readonly index: number | null;
}

const Rules = ({
  folders,
  onEdit,
  onChange,
  rules,
}: {
  readonly rules: readonly Rule[];
  readonly folders: readonly FolderOption[];
  readonly onEdit: (editing: Editing) => void;
  readonly onChange: (rules: Rule[]) => void;
}) => {
  const handles = useRef(new Map<string, HTMLButtonElement>());
  const [dragging, setDragging] = useState<number | null>(null);
  const [over, setOver] = useState<number | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const [deleting, setDeleting] = useState<Rule | null>(null);

  const moveTo = (from: number, to: number): void => {
    const rule = rules[from];
    if (rule === undefined || to < 0 || to >= rules.length || from === to) {
      return;
    }
    onChange(move(rules, from, to));
    setAnnouncement(
      m.filter_moved({
        name: rule.name,
        position: String(to + 1),
        total: String(rules.length),
      })
    );
    // React moves the row's node; give the focus back to its handle.
    requestAnimationFrame(() => {
      handles.current.get(rule.id)?.focus();
    });
  };

  const handleKey =
    (index: number) => (event: KeyboardEvent<HTMLButtonElement>) => {
      if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        event.preventDefault();
        moveTo(index, event.key === "ArrowUp" ? index - 1 : index + 1);
      }
    };

  if (rules.length === 0) {
    return (
      <InlineEmpty title={m.filter_empty_title()}>
        {m.filter_empty()}
      </InlineEmpty>
    );
  }

  return (
    <>
      <ul aria-label={m.filter_list()} {...stylex.props(styles.list)}>
        {rules.map((rule, index) => (
          // Dragging is a mouse shortcut: the handle's arrow keys and the
          // menu's "Move up/down" do the same for keyboard and touch.
          // oxlint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- see above.
          <li
            draggable
            key={rule.id}
            onDragEnd={() => {
              setDragging(null);
              setOver(null);
            }}
            onDragOver={(event: DragEvent) => {
              if (dragging !== null) {
                event.preventDefault();
                setOver(index);
              }
            }}
            onDragStart={(event: DragEvent) => {
              event.dataTransfer.effectAllowed = "move";
              event.dataTransfer.setData("text/plain", rule.name);
              setDragging(index);
            }}
            onDrop={(event: DragEvent) => {
              event.preventDefault();
              if (dragging !== null) {
                moveTo(dragging, index);
              }
              setDragging(null);
              setOver(null);
            }}
            {...stylex.props(
              styles.row,
              over === index && dragging !== index ? styles.dropTarget : null,
              rule.enabled ? null : styles.disabled
            )}
          >
            <span {...stylex.props(styles.handle)}>
              <IconButton
                icon={GripVertical}
                label={m.filter_reorder({ name: rule.name })}
                onKeyDown={handleKey(index)}
                ref={(element: HTMLButtonElement | null) => {
                  if (element === null) {
                    handles.current.delete(rule.id);
                  } else {
                    handles.current.set(rule.id, element);
                  }
                }}
                size="sm"
                tooltip={false}
              />
            </span>
            <span {...stylex.props(styles.text)}>
              <span {...stylex.props(styles.name)}>
                {rule.name === "" ? m.filter_untitled() : rule.name}
              </span>
              <span {...stylex.props(styles.summary)}>
                {describeRule(rule, folders)}
              </span>
            </span>
            <Switch
              aria-label={m.filter_toggle({ name: rule.name })}
              checked={rule.enabled}
              onCheckedChange={(enabled) => {
                onChange(
                  rules.map((item) =>
                    item.id === rule.id ? { ...item, enabled } : item
                  )
                );
              }}
            />
            <IconButton
              icon={Pencil}
              label={m.filter_edit({ name: rule.name })}
              onClick={() => {
                onEdit({ index, rule });
              }}
            />
            <Menu
              align="end"
              trigger={
                <IconButton
                  icon={Ellipsis}
                  label={m.filter_more({ name: rule.name })}
                />
              }
            >
              <MenuItem
                disabled={index === 0}
                icon={ArrowUp}
                onClick={() => {
                  moveTo(index, index - 1);
                }}
              >
                {m.filter_move_up()}
              </MenuItem>
              <MenuItem
                disabled={index === rules.length - 1}
                icon={ArrowDown}
                onClick={() => {
                  moveTo(index, index + 1);
                }}
              >
                {m.filter_move_down()}
              </MenuItem>
              <MenuItem
                danger
                icon={Trash2}
                onClick={() => {
                  setDeleting(rule);
                }}
              >
                {m.filter_delete()}
              </MenuItem>
            </Menu>
          </li>
        ))}
      </ul>
      <p aria-live="polite" {...stylex.props(styles.srOnly)}>
        {announcement}
      </p>
      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.filter_delete()}
        description={m.filter_delete_text({ name: deleting?.name ?? "" })}
        onConfirm={() => {
          const target = deleting;
          setDeleting(null);
          if (target !== null) {
            onChange(rules.filter((item) => item.id !== target.id));
          }
        }}
        onOpenChange={(open) => {
          if (!open) {
            setDeleting(null);
          }
        }}
        open={deleting !== null}
        title={m.filter_delete_title()}
        tone="danger"
      />
    </>
  );
};

// --- Section ----------------------------------------------------------------

const Visual = ({
  extensions,
  folders,
  fromEmail,
  state,
}: {
  readonly state: ScriptsState;
  readonly folders: readonly FolderOption[];
  readonly extensions: readonly string[];
  readonly fromEmail: EmailId | undefined;
}) => {
  const navigate = useNavigate();
  const toast = useToast();
  const save = useSaveFilters();
  const facts = useEmailFacts(fromEmail);
  const [editing, setEditing] = useState<Editing | null>(null);
  const filters = resolveFolders(
    state.oursFilters ?? { rules: [], vacation: false },
    folders
  );
  const { rules } = filters;

  // "Create filter from this email": an editor pre-filled from the URL.
  const fromFacts =
    editing === null &&
    fromEmail !== undefined &&
    facts.data !== undefined &&
    facts.data !== null
      ? { index: null, rule: ruleFromFacts(facts.data) }
      : null;
  const open = editing ?? fromFacts;

  const clearFrom = (): void => {
    if (fromEmail !== undefined) {
      void navigate({ replace: true, search: {}, to: "/settings/filters" });
    }
  };

  const persist = (next: Rule[], onDone?: () => void): void => {
    save.mutate(
      { filters: { ...filters, rules: next }, state },
      {
        onError: (error) => {
          toast.show({
            description:
              error instanceof ScriptError && error.message !== "tooLarge"
                ? error.message
                : undefined,
            title:
              error instanceof ScriptError && error.message === "tooLarge"
                ? m.filter_script_too_large()
                : m.filter_save_error(),
            tone: "danger",
          });
        },
        onSuccess: () => {
          onDone?.();
        },
      }
    );
  };

  const overLimit =
    state.maxRedirects !== null && forwardCount(filters) > state.maxRedirects;

  if (open !== null) {
    const others = rules.filter((_rule, index) => index !== open.index);
    return (
      <RuleEditor
        extensions={extensions}
        folders={folders}
        initial={open.rule}
        isNew={open.index === null}
        key={open.rule.id}
        maxRedirects={state.maxRedirects}
        onCancel={() => {
          setEditing(null);
          clearFrom();
        }}
        onSave={(rule) => {
          const next =
            open.index === null
              ? [...rules, rule]
              : rules.map((item, index) =>
                  index === open.index ? rule : item
                );
          persist(next, () => {
            setEditing(null);
            clearFrom();
            toast.show({ title: m.filter_saved(), tone: "success" });
          });
        }}
        otherForwards={forwardCount({ rules: others, vacation: false })}
        saving={save.isPending}
      />
    );
  }

  return (
    <>
      {overLimit ? (
        <Notice tone="warn">
          <span>
            {m.filter_redirect_limit({ max: String(state.maxRedirects ?? 0) })}
          </span>
        </Notice>
      ) : null}
      <SubsectionHeader
        action={
          <Button
            icon={Plus}
            onClick={() => {
              setEditing({
                index: null,
                rule: { ...emptyRule(""), id: newRuleId() },
              });
            }}
            variant="secondary"
          >
            {m.filter_new()}
          </Button>
        }
        description={
          // Dragging only means something with two rules or more.
          rules.length > 1 ? m.filter_rules_hint() : m.filter_rules_hint_few()
        }
        title={m.filter_rules_title()}
      />
      <Rules
        folders={folders}
        onChange={(next) => {
          persist(next);
        }}
        onEdit={setEditing}
        rules={rules}
      />
    </>
  );
};

/** A script opened from the list, as Sieve. */
const OpenedScript = ({
  onBack,
  script,
}: {
  readonly script: ScriptInfo;
  readonly onBack: () => void;
}) => {
  const text = useScriptText(script);
  if (text.isError) {
    return (
      <Notice tone="danger">
        <span>{m.filter_load_error()}</span>
      </Notice>
    );
  }
  if (text.data === undefined) {
    return <Spinner label={m.filter_loading()} />;
  }
  return (
    <RawScript
      content={text.data}
      key={script.id}
      onBack={onBack}
      script={script}
    />
  );
};

export const FiltersSection = () => {
  const support = useSupport();
  const scripts = useScripts();
  const mailboxes = useMailboxes();
  const names = useImapNames();
  const { from } = useSearch({ from: "/settings/filters" });
  const save = useSaveFilters();
  const toast = useToast();
  const [openedId, setOpenedId] = useState<string | null>(null);

  const header = (
    <SectionHeader
      description={m.filter_description()}
      title={m.settings_section_filters()}
    />
  );

  if (support.data?.sieve === false) {
    return (
      <>
        {header}
        <Notice>{m.filter_unsupported()}</Notice>
      </>
    );
  }
  if (scripts.isError) {
    return (
      <>
        {header}
        <Notice tone="danger">
          <span>{m.filter_load_error()}</span>
          <span>
            <Button
              onClick={() => {
                void scripts.refetch();
              }}
              size="sm"
              variant="secondary"
            >
              {m.retry()}
            </Button>
          </span>
        </Notice>
      </>
    );
  }
  const state = scripts.data;
  if (
    state === undefined ||
    mailboxes.data === undefined ||
    names.data === undefined ||
    support.data === undefined
  ) {
    return (
      <>
        {header}
        <Spinner label={m.filter_loading()} />
      </>
    );
  }
  const folders = folderOptions(mailboxes.data, names.data);
  const vacationOn =
    state.vacationScriptActive || state.oursFilters?.vacation === true;
  const list = (
    <ScriptsList
      onOpen={(script) => {
        setOpenedId(script?.id ?? null);
        globalThis.scrollTo?.({ top: 0 });
      }}
      state={state}
      vacationOn={state.vacationScriptActive}
    />
  );
  const page = (body: ReactNode) => (
    <>
      {header}
      {body}
      {list}
    </>
  );

  const opened = state.scripts.find((script) => script.id === openedId);
  if (opened !== undefined) {
    return page(
      <OpenedScript
        onBack={() => {
          setOpenedId(null);
        }}
        script={opened}
      />
    );
  }

  // The active script isn't readable as our rules: show it as Sieve.
  let rawScript: ScriptsState["active"] = null;
  if (state.rawContent !== null) {
    rawScript =
      state.active !== null &&
      (state.active.id !== state.ours?.id || state.oursFilters === null)
        ? state.active
        : state.ours;
  }

  if (rawScript !== null && state.rawContent !== null) {
    const ours = rawScript.id === state.ours?.id;
    return page(
      <RawScript
        content={state.rawContent}
        key={rawScript.id}
        onUseRules={() => {
          save.mutate(
            {
              // Our hand-edited script starts over; otherwise our saved
              // rules (if any) come back.
              filters: {
                rules: ours ? [] : (state.oursFilters?.rules ?? []),
                vacation: false,
              },
              state,
            },
            {
              onError: () => {
                toast.show({ title: m.filter_save_error(), tone: "danger" });
              },
            }
          );
        }}
        ours={ours}
        script={rawScript}
      />
    );
  }

  return page(
    <>
      {vacationOn ? <Notice>{m.filter_vacation_included()}</Notice> : null}
      <Visual
        extensions={support.data.extensions}
        folders={folders}
        fromEmail={EmailId.safeParse(from).data}
        state={state}
      />
    </>
  );
};
