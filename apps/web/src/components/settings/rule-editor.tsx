// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Edits one filter rule: name, "all / any", conditions and actions, with
// a test against recent mail. Nothing is saved until "Save"; destructive
// actions (reject, discard) ask first and forwarding shows a warning.

import {
  AlertDialog,
  Button,
  Field,
  IconButton,
  Input,
  SegmentedControl,
  Select,
  Spinner,
  Switch,
} from "@nebulamail/ui";
import type { SelectOption } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { FlaskConical, Plus, Trash2 } from "lucide-react";
import { useState } from "react";

import { formatListTime } from "../../lib/format";
import { useNow } from "../../lib/use-now";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import type { PreviewOutcome } from "../../settings/filters/api";
import { previewRule } from "../../settings/filters/api";
import type { FolderOption } from "../../settings/filters/mailboxes";
import type {
  Action,
  ActionKind,
  Condition,
  MatchField,
  Problem,
  Rule,
} from "../../settings/filters/model";
import { MAX_RULE_NAME, ruleProblems } from "../../settings/filters/model";
import { bp } from "../mail/breakpoints.stylex";
import { mailboxLabel } from "../mail/mailbox-label";
import { settingsMeasure } from "./measure.stylex";
import { Notice } from "./section";

const styles = stylex.create({
  editor: {
    display: "flex",
    flexDirection: "column",
    gap: "18px",
    maxWidth: settingsMeasure.width,
  },
  block: {
    borderColor: colors.line,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    display: "flex",
    flexDirection: "column",
    gap: "10px",
    padding: "14px",
  },
  blockTitle: {
    color: colors.fg,
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    margin: 0,
  },
  row: {
    alignItems: "center",
    display: "grid",
    gap: "8px",
    gridTemplateColumns: {
      default: "minmax(0, 11rem) minmax(0, 11rem) minmax(0, 1fr) auto",
      [bp.narrow]: "minmax(0, 1fr) auto",
    },
  },
  // On phones each control takes a full line; the remove button stays on
  // the first one.
  cell: { gridColumn: { default: null, [bp.narrow]: "1 / 2" } },
  remove: {
    gridColumn: { default: null, [bp.narrow]: "2 / 3" },
    gridRow: { default: null, [bp.narrow]: "1" },
  },
  stack: { display: "flex", flexDirection: "column", gap: "8px", minWidth: 0 },
  inline: {
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
    gap: "8px",
  },
  unit: { color: colors.fg3, fontSize: font.sizeSm },
  footer: {
    display: "flex",
    flexWrap: "wrap",
    gap: "8px",
    justifyContent: "flex-end",
  },
  problems: { margin: 0, paddingInlineStart: "18px" },
  preview: { display: "flex", flexDirection: "column", gap: "6px" },
  previewList: { listStyle: "none", margin: 0, padding: 0 },
  previewItem: {
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    display: "grid",
    fontSize: font.sizeMd,
    gap: "8px",
    gridTemplateColumns: "minmax(0, 10rem) minmax(0, 1fr) auto",
    paddingBlock: "6px",
  },
  ellipsis: {
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  muted: { color: colors.fg3 },
});

// --- Options ----------------------------------------------------------------

type FieldChoice = MatchField | "size" | "attachment";
type OpChoice =
  | "contains"
  | "notContains"
  | "is"
  | "notIs"
  | "matches"
  | "notMatches"
  | "regex"
  | "notRegex";

const FIELD_CHOICES: readonly FieldChoice[] = [
  "from",
  "recipient",
  "subject",
  "body",
  "listId",
  "header",
  "size",
  "attachment",
];

const OP_CHOICES: readonly OpChoice[] = [
  "contains",
  "notContains",
  "is",
  "notIs",
  "matches",
  "notMatches",
  "regex",
  "notRegex",
];

const ACTION_CHOICES: readonly ActionKind[] = [
  "move",
  "copy",
  "markRead",
  "star",
  "keyword",
  "forward",
  "reject",
  "discard",
  "stop",
];

const FIELD_LABELS: Record<FieldChoice, () => string> = {
  attachment: m.filter_field_attachment,
  body: m.filter_field_body,
  from: m.filter_field_from,
  header: m.filter_field_header,
  listId: m.filter_field_list,
  recipient: m.filter_field_recipient,
  size: m.filter_field_size,
  subject: m.filter_field_subject,
};

const OP_LABELS: Record<OpChoice, () => string> = {
  contains: m.filter_op_contains,
  is: m.filter_op_is,
  matches: m.filter_op_matches,
  notContains: m.filter_op_not_contains,
  notIs: m.filter_op_not_is,
  notMatches: m.filter_op_not_matches,
  notRegex: m.filter_op_not_regex,
  regex: m.filter_op_regex,
};

const ACTION_LABELS: Record<ActionKind, () => string> = {
  copy: m.filter_action_copy,
  discard: m.filter_action_discard,
  forward: m.filter_action_forward,
  keyword: m.filter_action_keyword,
  markRead: m.filter_action_read,
  move: m.filter_action_move,
  reject: m.filter_action_reject,
  star: m.filter_action_star,
  stop: m.filter_action_stop,
};

export const PROBLEM_LABELS: Record<Problem, () => string> = {
  badAddress: m.filter_problem_address,
  badHeader: m.filter_problem_header,
  badKeyword: m.filter_problem_keyword,
  badRegex: m.filter_problem_regex,
  badSize: m.filter_problem_size,
  conflict: m.filter_problem_conflict,
  emptyValue: m.filter_problem_empty,
  noActions: m.filter_problem_no_actions,
  noMailbox: m.filter_problem_mailbox,
  tooManyRedirects: m.filter_problem_redirects,
};

const opChoice = (
  condition: Extract<Condition, { kind: "match" }>
): OpChoice => {
  const base = condition.op;
  if (!condition.negate) {
    return base;
  }
  const negated = {
    contains: "notContains",
    is: "notIs",
    matches: "notMatches",
    regex: "notRegex",
  } as const;
  return negated[base];
};

const fromOpChoice = (
  choice: OpChoice
): Pick<Extract<Condition, { kind: "match" }>, "op" | "negate"> => {
  switch (choice) {
    case "contains":
    case "is":
    case "matches":
    case "regex": {
      return { negate: false, op: choice };
    }
    case "notContains": {
      return { negate: true, op: "contains" };
    }
    case "notIs": {
      return { negate: true, op: "is" };
    }
    case "notMatches": {
      return { negate: true, op: "matches" };
    }
    case "notRegex": {
      return { negate: true, op: "regex" };
    }
    default: {
      return choice satisfies never;
    }
  }
};

const conditionFor = (field: FieldChoice, previous: Condition): Condition => {
  if (field === "size") {
    return { kilobytes: 1024, kind: "size", op: "over" };
  }
  if (field === "attachment") {
    return { kind: "attachment", negate: false };
  }
  const base =
    previous.kind === "match"
      ? previous
      : { negate: false, op: "contains" as const, value: "" };
  return {
    field,
    header:
      field === "header" && previous.kind === "match" ? previous.header : "",
    kind: "match",
    negate: base.negate,
    op: base.op,
    value: base.value,
  };
};

const actionFor = (
  kind: ActionKind,
  folders: readonly FolderOption[]
): Action => {
  switch (kind) {
    case "move":
    case "copy": {
      const first =
        folders.find((folder) => folder.mailbox.role !== "inbox") ?? folders[0];
      return {
        kind,
        mailboxId: first?.id ?? "",
        mailboxName: first?.path ?? "",
      };
    }
    case "keyword": {
      return { keyword: "", kind };
    }
    case "forward": {
      return { address: "", keepCopy: true, kind };
    }
    case "reject": {
      return { kind, reason: "" };
    }
    case "markRead":
    case "star":
    case "discard":
    case "stop": {
      return { kind };
    }
    default: {
      return kind satisfies never;
    }
  }
};

const replaceAt = <T,>(list: readonly T[], index: number, item: T): T[] =>
  list.map((value, at) => (at === index ? item : value));

const removeAt = <T,>(list: readonly T[], index: number): T[] =>
  list.filter((_value, at) => at !== index);

// --- Rows -------------------------------------------------------------------

const ConditionRow = ({
  condition,
  index,
  onChange,
  onRemove,
  regex,
}: {
  readonly condition: Condition;
  readonly index: number;
  readonly onChange: (condition: Condition) => void;
  readonly onRemove: () => void;
  readonly regex: boolean;
}) => {
  const number = String(index + 1);
  const fieldValue: FieldChoice =
    condition.kind === "match" ? condition.field : condition.kind;
  const fieldOptions: SelectOption<FieldChoice>[] = FIELD_CHOICES.map(
    (value) => ({ label: FIELD_LABELS[value](), value })
  );
  const opOptions: SelectOption<OpChoice>[] = OP_CHOICES.filter(
    (value) => regex || !value.toLowerCase().includes("regex")
  ).map((value) => ({ label: OP_LABELS[value](), value }));

  let rest = null;
  if (condition.kind === "match") {
    rest = (
      <>
        <span {...stylex.props(styles.cell)}>
          <Select
            label={m.filter_condition_op({ number })}
            onValueChange={(choice) => {
              onChange({ ...condition, ...fromOpChoice(choice) });
            }}
            options={opOptions}
            value={opChoice(condition)}
          />
        </span>
        <span {...stylex.props(styles.cell, styles.stack)}>
          {condition.field === "header" ? (
            <Input
              aria-label={m.filter_condition_header({ number })}
              maxLength={76}
              onChange={(event) => {
                onChange({ ...condition, header: event.target.value });
              }}
              placeholder={m.filter_header_placeholder()}
              value={condition.header}
            />
          ) : null}
          <Input
            aria-label={m.filter_condition_value({ number })}
            maxLength={1000}
            onChange={(event) => {
              onChange({ ...condition, value: event.target.value });
            }}
            placeholder={
              condition.op === "matches"
                ? m.filter_matches_placeholder()
                : m.filter_value_placeholder()
            }
            value={condition.value}
          />
        </span>
      </>
    );
  } else if (condition.kind === "size") {
    rest = (
      <>
        <span {...stylex.props(styles.cell)}>
          <Select
            label={m.filter_condition_op({ number })}
            onValueChange={(op) => {
              onChange({ ...condition, op });
            }}
            options={[
              { label: m.filter_size_over(), value: "over" },
              { label: m.filter_size_under(), value: "under" },
            ]}
            value={condition.op}
          />
        </span>
        <span {...stylex.props(styles.cell, styles.inline)}>
          <Input
            aria-label={m.filter_condition_value({ number })}
            inputMode="numeric"
            min={1}
            onChange={(event) => {
              onChange({
                ...condition,
                kilobytes: Math.trunc(Number(event.target.value)),
              });
            }}
            type="number"
            value={String(condition.kilobytes)}
          />
          <span {...stylex.props(styles.unit)}>{m.filter_size_unit()}</span>
        </span>
      </>
    );
  } else {
    rest = (
      <span {...stylex.props(styles.cell)}>
        <Select
          label={m.filter_condition_op({ number })}
          onValueChange={(value) => {
            onChange({ ...condition, negate: value === "no" });
          }}
          options={[
            { label: m.filter_attachment_yes(), value: "yes" },
            { label: m.filter_attachment_no(), value: "no" },
          ]}
          value={condition.negate ? "no" : "yes"}
        />
      </span>
    );
  }

  return (
    <div {...stylex.props(styles.row)}>
      <span {...stylex.props(styles.cell)}>
        <Select
          label={m.filter_condition_field({ number })}
          onValueChange={(field) => {
            onChange(conditionFor(field, condition));
          }}
          options={fieldOptions}
          value={fieldValue}
        />
      </span>
      {rest}
      {condition.kind === "attachment" ? <span /> : null}
      <span {...stylex.props(styles.remove)}>
        <IconButton
          icon={Trash2}
          label={m.filter_condition_remove({ number })}
          onClick={onRemove}
        />
      </span>
    </div>
  );
};

const ActionRow = ({
  action,
  folders,
  index,
  onChange,
  onRemove,
}: {
  readonly action: Action;
  readonly folders: readonly FolderOption[];
  readonly index: number;
  readonly onChange: (action: Action) => void;
  readonly onRemove: () => void;
}) => {
  const number = String(index + 1);
  const kinds = ACTION_CHOICES;
  let detail = null;
  switch (action.kind) {
    case "move":
    case "copy": {
      const options = folders.map((folder) => ({
        label: `${"\u00A0\u00A0".repeat(folder.depth)}${mailboxLabel(folder.mailbox)}`,
        value: folder.id,
      }));
      const known = folders.some((folder) => folder.id === action.mailboxId);
      detail = (
        <Select
          label={m.filter_action_folder({ number })}
          onValueChange={(id) => {
            const folder = folders.find((item) => item.id === id);
            onChange({
              ...action,
              mailboxId: id,
              mailboxName: folder?.path ?? id,
            });
          }}
          options={
            known || action.mailboxId === ""
              ? options
              : [
                  {
                    label: m.filter_folder_missing({
                      name: action.mailboxName,
                    }),
                    value: action.mailboxId,
                  },
                  ...options,
                ]
          }
          value={action.mailboxId}
        />
      );
      break;
    }
    case "keyword": {
      detail = (
        <Input
          aria-label={m.filter_action_keyword_value({ number })}
          maxLength={64}
          onChange={(event) => {
            onChange({ ...action, keyword: event.target.value });
          }}
          placeholder={m.filter_keyword_placeholder()}
          value={action.keyword}
        />
      );
      break;
    }
    case "forward": {
      detail = (
        <span {...stylex.props(styles.stack)}>
          <Input
            aria-label={m.filter_action_address({ number })}
            autoComplete="email"
            inputMode="email"
            maxLength={254}
            onChange={(event) => {
              onChange({ ...action, address: event.target.value.trim() });
            }}
            placeholder={m.filter_address_placeholder()}
            type="email"
            value={action.address}
          />
          <Switch
            checked={action.keepCopy}
            label={m.filter_forward_keep()}
            onCheckedChange={(keepCopy) => {
              onChange({ ...action, keepCopy });
            }}
          />
        </span>
      );
      break;
    }
    case "reject": {
      detail = (
        <Input
          aria-label={m.filter_action_reason({ number })}
          maxLength={500}
          onChange={(event) => {
            onChange({ ...action, reason: event.target.value });
          }}
          placeholder={m.filter_reason_placeholder()}
          value={action.reason}
        />
      );
      break;
    }
    case "markRead":
    case "star":
    case "discard":
    case "stop": {
      break;
    }
    default: {
      action satisfies never;
    }
  }
  return (
    <div {...stylex.props(styles.row)}>
      <span {...stylex.props(styles.cell)}>
        <Select
          label={m.filter_action_kind({ number })}
          onValueChange={(kind) => {
            onChange(actionFor(kind, folders));
          }}
          options={kinds.map((kind) => ({
            label: ACTION_LABELS[kind](),
            value: kind,
          }))}
          value={action.kind}
        />
      </span>
      <span {...stylex.props(styles.cell)}>{detail}</span>
      <span />
      <span {...stylex.props(styles.remove)}>
        <IconButton
          icon={Trash2}
          label={m.filter_action_remove({ number })}
          onClick={onRemove}
        />
      </span>
    </div>
  );
};

// --- Preview ----------------------------------------------------------------

type PreviewState =
  | { readonly kind: "idle" }
  | { readonly kind: "loading" }
  | { readonly kind: "error" }
  | PreviewOutcome;

const Preview = ({ state }: { readonly state: PreviewState }) => {
  const now = new Date(useNow());
  switch (state.kind) {
    case "idle": {
      return null;
    }
    case "loading": {
      return <Spinner label={m.filter_preview_loading()} />;
    }
    case "unsearchable": {
      return <Notice>{m.filter_preview_unsupported()}</Notice>;
    }
    case "tooComplex": {
      return <Notice>{m.filter_preview_too_complex()}</Notice>;
    }
    case "error": {
      return <Notice tone="danger">{m.filter_preview_error()}</Notice>;
    }
    case "done": {
      const { result } = state;
      return (
        <div aria-live="polite" {...stylex.props(styles.preview)}>
          <p {...stylex.props(styles.muted)}>
            {m.filter_preview_count({ count: result.total })}
            {result.approximate ? ` ${m.filter_preview_approximate()}` : ""}
          </p>
          {result.emails.length === 0 ? null : (
            <ul {...stylex.props(styles.previewList)}>
              {result.emails.map((email) => (
                <li key={email.id} {...stylex.props(styles.previewItem)}>
                  <span {...stylex.props(styles.ellipsis)}>{email.from}</span>
                  <span {...stylex.props(styles.ellipsis)}>
                    {email.subject === "" ? m.row_no_subject() : email.subject}
                  </span>
                  <span {...stylex.props(styles.muted)}>
                    {formatListTime(email.receivedAt, now, getLocale())}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      );
    }
    default: {
      return state satisfies never;
    }
  }
};

// --- Editor -----------------------------------------------------------------

export interface RuleEditorProps {
  readonly initial: Rule;
  readonly isNew: boolean;
  readonly folders: readonly FolderOption[];
  /** Sieve extensions of the server (hides regex if missing). */
  readonly extensions: readonly string[];
  /** Forwards in the other enabled rules, and the server's limit. */
  readonly otherForwards: number;
  readonly maxRedirects: number | null;
  readonly saving: boolean;
  readonly onSave: (rule: Rule) => void;
  readonly onCancel: () => void;
}

export const RuleEditor = ({
  extensions,
  folders,
  initial,
  isNew,
  maxRedirects,
  onCancel,
  onSave,
  otherForwards,
  saving,
}: RuleEditorProps) => {
  const [rule, setRule] = useState(initial);
  const [tried, setTried] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [preview, setPreview] = useState<PreviewState>({ kind: "idle" });

  const forwards = rule.actions.filter(
    (action) => action.kind === "forward"
  ).length;
  const problems = ruleProblems(rule);
  if (
    rule.enabled &&
    maxRedirects !== null &&
    forwards > 0 &&
    otherForwards + forwards > maxRedirects
  ) {
    problems.push("tooManyRedirects");
  }
  const destructive = rule.actions.some(
    (action) => action.kind === "discard" || action.kind === "reject"
  );
  const regex = extensions.includes("regex");

  const update = (patch: Partial<Rule>): void => {
    setRule((current) => ({ ...current, ...patch }));
    setPreview({ kind: "idle" });
  };

  const save = (): void => {
    setTried(true);
    if (problems.length > 0) {
      return;
    }
    if (destructive) {
      setConfirming(true);
      return;
    }
    onSave(rule);
  };

  const test = async (): Promise<void> => {
    setPreview({ kind: "loading" });
    try {
      setPreview(await previewRule(rule));
    } catch {
      setPreview({ kind: "error" });
    }
  };

  return (
    <form
      aria-label={
        isNew ? m.filter_new() : m.filter_edit({ name: initial.name })
      }
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        save();
      }}
      {...stylex.props(styles.editor)}
    >
      <Field label={m.filter_name()}>
        <Input
          maxLength={MAX_RULE_NAME}
          onChange={(event) => {
            update({ name: event.target.value });
          }}
          placeholder={m.filter_name_placeholder()}
          value={rule.name}
        />
      </Field>

      <section
        aria-label={m.filter_conditions()}
        {...stylex.props(styles.block)}
      >
        <h3 {...stylex.props(styles.blockTitle)}>{m.filter_conditions()}</h3>
        {rule.conditions.length > 1 ? (
          <SegmentedControl
            label={m.filter_match()}
            onValueChange={(match) => {
              update({ match });
            }}
            options={[
              { label: m.filter_match_all(), value: "all" },
              { label: m.filter_match_any(), value: "any" },
            ]}
            value={rule.match}
          />
        ) : null}
        {rule.conditions.length === 0 ? (
          <p {...stylex.props(styles.muted)}>{m.filter_no_conditions()}</p>
        ) : null}
        {rule.conditions.map((condition, index) => (
          <ConditionRow
            condition={condition}
            index={index}
            // Conditions have no identity of their own; rows are positional.
            // oxlint-disable-next-line react/no-array-index-key -- see above.
            key={index}
            onChange={(next) => {
              update({ conditions: replaceAt(rule.conditions, index, next) });
            }}
            onRemove={() => {
              update({ conditions: removeAt(rule.conditions, index) });
            }}
            regex={regex}
          />
        ))}
        <span>
          <Button
            icon={Plus}
            onClick={() => {
              update({
                conditions: [
                  ...rule.conditions,
                  {
                    field: "subject",
                    header: "",
                    kind: "match",
                    negate: false,
                    op: "contains",
                    value: "",
                  },
                ],
              });
            }}
            size="sm"
            variant="ghost"
          >
            {m.filter_add_condition()}
          </Button>
        </span>
      </section>

      <section aria-label={m.filter_actions()} {...stylex.props(styles.block)}>
        <h3 {...stylex.props(styles.blockTitle)}>{m.filter_actions()}</h3>
        {rule.actions.map((action, index) => (
          <ActionRow
            action={action}
            folders={folders}
            index={index}
            // oxlint-disable-next-line react/no-array-index-key -- positional rows, like the conditions.
            key={index}
            onChange={(next) => {
              update({ actions: replaceAt(rule.actions, index, next) });
            }}
            onRemove={() => {
              update({ actions: removeAt(rule.actions, index) });
            }}
          />
        ))}
        <span>
          <Button
            icon={Plus}
            onClick={() => {
              update({
                actions: [...rule.actions, actionFor("move", folders)],
              });
            }}
            size="sm"
            variant="ghost"
          >
            {m.filter_add_action()}
          </Button>
        </span>
        {forwards > 0 ? (
          <Notice tone="warn">
            <span>{m.filter_forward_warning()}</span>
          </Notice>
        ) : null}
        {destructive ? (
          <Notice tone="warn">
            <span>{m.filter_destructive_warning()}</span>
          </Notice>
        ) : null}
      </section>

      <Switch
        checked={rule.enabled}
        label={m.filter_enabled()}
        onCheckedChange={(enabled) => {
          update({ enabled });
        }}
      />

      {tried && problems.length > 0 ? (
        <Notice tone="danger">
          <span>{m.filter_problems_title()}</span>
          <ul {...stylex.props(styles.problems)}>
            {problems.map((problem) => (
              <li key={problem}>{PROBLEM_LABELS[problem]()}</li>
            ))}
          </ul>
        </Notice>
      ) : null}

      <Preview state={preview} />

      <div {...stylex.props(styles.footer)}>
        <Button
          icon={FlaskConical}
          onClick={() => {
            void test();
          }}
          variant="secondary"
        >
          {m.filter_test()}
        </Button>
        <Button onClick={onCancel} variant="ghost">
          {m.cancel()}
        </Button>
        <Button loading={saving} type="submit" variant="primary">
          {m.filter_save()}
        </Button>
      </div>

      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.filter_confirm_destructive()}
        description={m.filter_confirm_destructive_text()}
        onConfirm={() => {
          setConfirming(false);
          onSave(rule);
        }}
        onOpenChange={setConfirming}
        open={confirming}
        title={m.filter_confirm_destructive_title()}
        tone="danger"
      />
    </form>
  );
};
