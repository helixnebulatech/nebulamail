// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The Select of @nebulamail/ui is Base UI's (a combobox button and a
// listbox), not a native `<select>`, so Playwright's `selectOption()` and
// `toHaveValue()` don't work on it. These open it and click the option by
// its role, as a person would. Also used by e2e-bridge.

import { expect } from "@playwright/test";
import type { Locator } from "@playwright/test";

/** An option by its value (`data-value`) or its visible name. */
export type OptionChoice =
  | string
  | { readonly value: string }
  | { readonly label: string | RegExp };

/** The listbox that the select (a combobox button) opened. */
const listboxOf = async (select: Locator): Promise<Locator> => {
  const page = select.page();
  await expect(select).toHaveAttribute("aria-expanded", "true");
  const id = await select.getAttribute("aria-controls");
  return id === null || id === ""
    ? page.getByRole("listbox").last()
    : page.locator(`[id="${id}"]`);
};

/** Opens the select and clicks the option; waits until it closes. */
export const chooseOption = async (
  select: Locator,
  choice: OptionChoice
): Promise<void> => {
  await select.click();
  const listbox = await listboxOf(select);
  const option =
    typeof choice === "string" || "value" in choice
      ? listbox
          .getByRole("option")
          .and(
            listbox.locator(
              `[data-value="${typeof choice === "string" ? choice : choice.value}"]`
            )
          )
      : listbox.getByRole("option", { exact: true, name: choice.label });
  await option.click();
  // The listbox, not the select: choosing may rename what holds the select
  // (a group named after the reminder), and then its locator finds nothing.
  await expect(listbox).toBeHidden();
};

/** The names of the options, in order (opens and closes the select). */
export const optionNames = async (select: Locator): Promise<string[]> => {
  await select.click();
  const listbox = await listboxOf(select);
  const names = await listbox.getByRole("option").allInnerTexts();
  await select.page().keyboard.press("Escape");
  await expect(select).toHaveAttribute("aria-expanded", "false");
  return names.map((name) => name.trim());
};

/** The value of the chosen option (like `toHaveValue` on a `<select>`). */
export const expectValue = async (
  select: Locator,
  value: string
): Promise<void> => {
  await expect(select).toHaveAttribute("data-value", value);
};
