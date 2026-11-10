// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Settings › Filters › Sieve scripts: every script of the account (RFC
// 9661), ours (the visual filters) and hand-written ones. Create, rename,
// delete, edit as Sieve, and choose which one runs: the server runs only
// one at a time, so activating one switches the others off (and, in
// Stalwart, a vacation response that isn't part of our filters).

import {
  AlertDialog,
  Badge,
  Button,
  Dialog,
  Field,
  IconButton,
  Input,
  Menu,
  MenuItem,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  CirclePause,
  CirclePlay,
  Ellipsis,
  FilePlus,
  Pencil,
  TextCursorInput,
  Trash2,
} from "lucide-react";
import { useState } from "react";

import { m } from "../../paraglide/messages.js";
import { OUR_SCRIPT_NAME } from "../../settings/filters/generate";
import { useChangeScript } from "../../settings/queries";
import type { ScriptChange } from "../../settings/queries";
import type {
  ScriptInfo,
  ScriptNameProblem,
  ScriptsState,
} from "../../settings/scripts";
import {
  NEW_SCRIPT_TEMPLATE,
  ScriptError,
  cleanScriptName,
  scriptNameProblem,
} from "../../settings/scripts";
import { settingsMeasure } from "./measure.stylex";
import { InlineEmpty, SubsectionHeader } from "./section";

const styles = stylex.create({
  wrap: { marginTop: "32px", maxWidth: settingsMeasure.width },
  list: { listStyle: "none", margin: 0, padding: 0 },
  row: {
    alignItems: "center",
    borderColor: colors.line,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    columnGap: "8px",
    display: "flex",
    marginBottom: "8px",
    paddingBlock: "8px",
    paddingInline: "12px 8px",
  },
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
  sub: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  footer: { display: "flex", gap: "8px", justifyContent: "flex-end" },
});

const PROBLEMS: Record<ScriptNameProblem, () => string> = {
  empty: m.scripts_name_empty,
  reserved: m.scripts_name_reserved,
  taken: m.scripts_name_taken,
  tooLong: m.scripts_name_too_long,
};

/** Asks for a script name (new script or rename). */
const NameDialog = ({
  initial,
  onCancel,
  onSubmit,
  others,
  pending,
  title,
}: {
  readonly title: string;
  readonly initial: string;
  readonly others: readonly ScriptInfo[];
  readonly pending: boolean;
  readonly onSubmit: (name: string) => void;
  readonly onCancel: () => void;
}) => {
  const [name, setName] = useState(initial);
  const [touched, setTouched] = useState(false);
  const problem = scriptNameProblem(name, others);
  const submit = (): void => {
    setTouched(true);
    if (problem === null) {
      onSubmit(cleanScriptName(name));
    }
  };
  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open) {
          onCancel();
        }
      }}
    >
      <Dialog.Content closeLabel={m.close()} size="sm">
        <Dialog.Title>{title}</Dialog.Title>
        <Dialog.Body>
          <form
            noValidate
            onSubmit={(event) => {
              event.preventDefault();
              submit();
            }}
          >
            <Field
              label={m.scripts_name()}
              {...(touched && problem !== null
                ? { error: PROBLEMS[problem]() }
                : {})}
            >
              <Input
                autoFocus
                maxLength={512}
                onChange={(event) => {
                  setName(event.target.value);
                }}
                value={name}
              />
            </Field>
          </form>
        </Dialog.Body>
        <Dialog.Footer>
          <div {...stylex.props(styles.footer)}>
            <Button onClick={onCancel} variant="secondary">
              {m.cancel()}
            </Button>
            <Button loading={pending} onClick={submit} variant="primary">
              {m.scripts_name_save()}
            </Button>
          </div>
        </Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
  );
};

const ours = (script: ScriptInfo): boolean => script.name === OUR_SCRIPT_NAME;
const displayName = (script: ScriptInfo): string =>
  ours(script) ? m.scripts_ours() : script.name;

const deleteText = (script: ScriptInfo): string =>
  script.isActive
    ? m.scripts_delete_active_text({ name: displayName(script) })
    : m.scripts_delete_text({ name: displayName(script) });

type Asking =
  | { readonly kind: "create" }
  | { readonly kind: "rename"; readonly script: ScriptInfo }
  | { readonly kind: "delete"; readonly script: ScriptInfo }
  | { readonly kind: "activate"; readonly script: ScriptInfo };

export const ScriptsList = ({
  onOpen,
  state,
  vacationOn,
}: {
  readonly state: ScriptsState;
  /** The vacation response runs as its own script (activating stops it). */
  readonly vacationOn: boolean;
  /** Edit a script as Sieve (our visual one: `null` = the rules). */
  readonly onOpen: (script: ScriptInfo | null) => void;
}) => {
  const toast = useToast();
  const change = useChangeScript();
  const [asking, setAsking] = useState<Asking | null>(null);

  const run = (next: ScriptChange, success: string, then?: () => void) => {
    change.mutate(next, {
      onError: (error) => {
        toast.show({
          description: error instanceof ScriptError ? error.message : undefined,
          title: m.scripts_change_error(),
          tone: "danger",
        });
      },
      onSuccess: () => {
        setAsking(null);
        toast.show({ title: success, tone: "success" });
        then?.();
      },
    });
  };

  const activate = (script: ScriptInfo): void => {
    run(
      { id: script.id, kind: "activate" },
      m.scripts_activated({ name: displayName(script) })
    );
  };

  const others = (script: ScriptInfo | null): ScriptInfo[] =>
    state.scripts.filter((item) => item.id !== script?.id);

  return (
    <section
      aria-labelledby="sieve-scripts-title"
      {...stylex.props(styles.wrap)}
    >
      <SubsectionHeader
        action={
          <Button
            icon={FilePlus}
            onClick={() => {
              setAsking({ kind: "create" });
            }}
            variant="secondary"
          >
            {m.scripts_new()}
          </Button>
        }
        description={m.scripts_hint()}
        id="sieve-scripts-title"
        title={m.scripts_title()}
      />
      {state.scripts.length === 0 ? (
        <InlineEmpty title={m.scripts_none_title()}>
          {m.scripts_none()}
        </InlineEmpty>
      ) : (
        <ul aria-label={m.scripts_title()} {...stylex.props(styles.list)}>
          {state.scripts.map((script) => {
            const isOurs = ours(script);
            const visual = isOurs && state.oursFilters !== null;
            return (
              <li key={script.id} {...stylex.props(styles.row)}>
                <span {...stylex.props(styles.text)}>
                  <span {...stylex.props(styles.name)}>
                    <bdi>{displayName(script)}</bdi>
                  </span>
                  <span {...stylex.props(styles.sub)}>
                    {visual ? m.scripts_ours_hint() : m.scripts_sieve_hint()}
                  </span>
                </span>
                {script.isActive ? (
                  <Badge tone="ok">{m.scripts_active()}</Badge>
                ) : null}
                <IconButton
                  icon={Pencil}
                  label={m.scripts_edit({ name: displayName(script) })}
                  onClick={() => {
                    onOpen(visual ? null : script);
                  }}
                />
                <Menu
                  align="end"
                  trigger={
                    <IconButton
                      icon={Ellipsis}
                      label={m.scripts_more({ name: displayName(script) })}
                    />
                  }
                >
                  {script.isActive ? (
                    <MenuItem
                      icon={CirclePause}
                      onClick={() => {
                        run(
                          { id: null, kind: "activate" },
                          m.scripts_deactivated({ name: displayName(script) })
                        );
                      }}
                    >
                      {m.scripts_deactivate()}
                    </MenuItem>
                  ) : (
                    <MenuItem
                      icon={CirclePlay}
                      onClick={() => {
                        if (vacationOn) {
                          setAsking({ kind: "activate", script });
                        } else {
                          activate(script);
                        }
                      }}
                    >
                      {m.scripts_activate()}
                    </MenuItem>
                  )}
                  {isOurs ? null : (
                    <MenuItem
                      icon={TextCursorInput}
                      onClick={() => {
                        setAsking({ kind: "rename", script });
                      }}
                    >
                      {m.scripts_rename()}
                    </MenuItem>
                  )}
                  <MenuItem
                    danger
                    icon={Trash2}
                    onClick={() => {
                      setAsking({ kind: "delete", script });
                    }}
                  >
                    {m.scripts_delete()}
                  </MenuItem>
                </Menu>
              </li>
            );
          })}
        </ul>
      )}
      {asking?.kind === "create" ? (
        <NameDialog
          initial=""
          onCancel={() => {
            setAsking(null);
          }}
          onSubmit={(name) => {
            run(
              { content: NEW_SCRIPT_TEMPLATE, kind: "create", name },
              m.scripts_created({ name })
            );
          }}
          others={others(null)}
          pending={change.isPending}
          title={m.scripts_new()}
        />
      ) : null}
      {asking?.kind === "rename" ? (
        <NameDialog
          initial={asking.script.name}
          onCancel={() => {
            setAsking(null);
          }}
          onSubmit={(name) => {
            run(
              { id: asking.script.id, kind: "rename", name },
              m.scripts_renamed({ name })
            );
          }}
          others={others(asking.script)}
          pending={change.isPending}
          title={m.scripts_rename_title({ name: asking.script.name })}
        />
      ) : null}
      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.scripts_delete()}
        description={asking?.kind === "delete" ? deleteText(asking.script) : ""}
        onConfirm={() => {
          if (asking?.kind === "delete") {
            run(
              { kind: "delete", script: asking.script },
              m.scripts_deleted({ name: displayName(asking.script) })
            );
          }
        }}
        onOpenChange={(open) => {
          if (!open) {
            setAsking(null);
          }
        }}
        open={asking?.kind === "delete"}
        title={m.scripts_delete_title()}
        tone="danger"
      />
      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.scripts_activate()}
        description={m.scripts_activate_vacation_text()}
        onConfirm={() => {
          if (asking?.kind === "activate") {
            activate(asking.script);
          }
        }}
        onOpenChange={(open) => {
          if (!open) {
            setAsking(null);
          }
        }}
        open={asking?.kind === "activate"}
        title={m.scripts_activate_vacation_title()}
      />
    </section>
  );
};
