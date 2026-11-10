// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Commands for the ⌘K palette. Parts of the UI register what they can do
// right now (the open email's actions, the selection's, global ones); the
// palette reads the registry when it renders. A ref, not state: commands
// change on every render and the palette only needs them while open.

import type { Icon } from "@nebulamail/ui";
import { createContext, use, useEffect, useId, useRef } from "react";
import type { ReactNode, RefObject } from "react";

export type CommandGroup = "actions" | "goto" | "settings";

export interface Command {
  readonly id: string;
  readonly group: CommandGroup;
  readonly label: string;
  readonly icon?: Icon;
  /** Shortcut spec for the hint, e.g. "e" or "g i". */
  readonly shortcut?: string;
  /** Extra words that should match (e.g. "dark" for the theme). */
  readonly keywords?: string;
  /**
   * Order inside its group, lowest first (default 0; ties keep the order
   * they were registered in). Rare actions (sharing a folder) use a high
   * rank so the everyday ones are at hand when the palette opens.
   */
  readonly rank?: number;
  readonly run: () => void;
}

/** Commands of one group in palette order (by rank, then as registered). */
export const byRank = (commands: readonly Command[]): Command[] =>
  commands
    .map((command, index) => ({ command, index }))
    .toSorted(
      (a, b) =>
        (a.command.rank ?? 0) - (b.command.rank ?? 0) || a.index - b.index
    )
    .map(({ command }) => command);

type Registry = Map<string, readonly Command[]>;

const CommandsContext = createContext<RefObject<Registry> | null>(null);

export const CommandsProvider = ({
  children,
}: {
  readonly children: ReactNode;
}) => {
  const registry = useRef<Registry>(new Map());
  return <CommandsContext value={registry}>{children}</CommandsContext>;
};

const useRegistry = (): RefObject<Registry> => {
  const registry = use(CommandsContext);
  if (registry === null) {
    throw new Error("Commands need a CommandsProvider");
  }
  return registry;
};

/** Registers this component's commands while it is mounted. */
export const useCommands = (commands: readonly Command[]): void => {
  const registry = useRegistry();
  const id = useId();
  // Every render: the closures must see the latest state.
  useEffect(() => {
    registry.current.set(id, commands);
  });
  useEffect(
    () => () => {
      registry.current.delete(id);
    },
    [id, registry]
  );
};

/** All registered commands, de-duplicated by id (the latest wins). */
export const useCommandList = (): (() => readonly Command[]) => {
  const registry = useRegistry();
  return () => {
    const byId = new Map<string, Command>();
    for (const commands of registry.current.values()) {
      for (const command of commands) {
        byId.set(command.id, command);
      }
    }
    return [...byId.values()];
  };
};
