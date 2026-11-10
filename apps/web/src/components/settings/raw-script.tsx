// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A script we can't show as rules (written by hand or by another app):
// Sieve with line numbers and syntax colours. The server validates it
// before saving and its error points at the line. Switching to rules
// always asks.

import { AlertDialog, Button, CodeEditor } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { ArrowLeft } from "lucide-react";
import { useId, useState } from "react";

import { m } from "../../paraglide/messages.js";
import { highlightSieve } from "../../settings/filters/highlight";
import { useSaveRawScript } from "../../settings/queries";
import { ScriptError } from "../../settings/scripts";
import type { ScriptInfo } from "../../settings/scripts";
import { Notice } from "./section";

const styles = stylex.create({
  wrap: {
    display: "flex",
    flexDirection: "column",
    gap: "12px",
    maxWidth: "900px",
  },
  footer: {
    display: "flex",
    flexWrap: "wrap",
    gap: "8px",
    justifyContent: "space-between",
  },
  error: { color: colors.danger, fontSize: font.sizeMd, margin: 0 },
  saved: { color: colors.ok, fontSize: font.sizeMd, margin: 0 },
});

export const RawScript = ({
  content,
  onBack,
  onUseRules,
  ours = false,
  script,
}: {
  readonly script: ScriptInfo;
  readonly content: string;
  /** Our own script, edited by hand (switching replaces the edits). */
  readonly ours?: boolean;
  /** Offers "Use visual filters" (the active script isn't our rules). */
  readonly onUseRules?: () => void;
  /** Opened from the script list: a way back to it. */
  readonly onBack?: () => void;
}) => {
  const errorId = useId();
  const [text, setText] = useState(content);
  const [error, setError] = useState<{
    message: string;
    line: number | null;
  } | null>(null);
  const [saved, setSaved] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const save = useSaveRawScript();

  const handleSave = (): void => {
    setSaved(false);
    save.mutate(
      { activate: script.isActive, content: text, id: script.id },
      {
        onError: (failure) => {
          setError(
            failure instanceof ScriptError
              ? {
                  line: failure.line,
                  message:
                    failure.message === "tooLarge"
                      ? m.filter_script_too_large()
                      : failure.message,
                }
              : { line: null, message: m.filter_save_error() }
          );
        },
        onSuccess: () => {
          setError(null);
          setSaved(true);
        },
      }
    );
  };

  return (
    <div {...stylex.props(styles.wrap)}>
      {onBack === undefined ? null : (
        <span>
          <Button icon={ArrowLeft} onClick={onBack} size="sm" variant="ghost">
            {m.scripts_back()}
          </Button>
        </span>
      )}
      {onUseRules === undefined ? null : (
        <Notice tone="warn">
          <span>
            {ours
              ? m.filter_raw_ours()
              : m.filter_raw_other({
                  name: script.name === "" ? "?" : script.name,
                })}
          </span>
        </Notice>
      )}
      <CodeEditor
        describedBy={errorId}
        errorLine={error?.line ?? null}
        highlight={highlightSieve}
        label={m.filter_raw_label({ name: script.name })}
        onValueChange={(value) => {
          setText(value);
          setSaved(false);
        }}
        value={text}
      />
      <div aria-live="polite" id={errorId}>
        {error === null ? null : (
          <p {...stylex.props(styles.error)}>
            {error.line === null
              ? m.filter_raw_error({ message: error.message })
              : m.filter_raw_error_line({
                  line: error.line,
                  message: error.message,
                })}
          </p>
        )}
        {saved ? (
          <p {...stylex.props(styles.saved)}>{m.filter_raw_saved()}</p>
        ) : null}
      </div>
      <div {...stylex.props(styles.footer)}>
        {onUseRules === undefined ? (
          <span />
        ) : (
          <Button
            onClick={() => {
              setConfirming(true);
            }}
            variant="secondary"
          >
            {m.filter_raw_use_rules()}
          </Button>
        )}
        <Button loading={save.isPending} onClick={handleSave} variant="primary">
          {m.filter_raw_save()}
        </Button>
      </div>
      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.filter_raw_use_rules()}
        description={
          ours
            ? m.filter_raw_replace_ours()
            : m.filter_raw_replace_other({ name: script.name })
        }
        onConfirm={() => {
          setConfirming(false);
          onUseRules?.();
        }}
        onOpenChange={setConfirming}
        open={confirming}
        title={m.filter_raw_use_rules_title()}
        tone={ours ? "danger" : "primary"}
      />
    </div>
  );
};
