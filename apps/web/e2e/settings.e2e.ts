// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The settings page: sections, the visual filter
// editor, raw scripts from other clients and the vacation response, at
// phone, tablet and desktop widths, against the fake JMAP server (which
// keeps Sieve scripts like Stalwart: one active at a time, and the
// vacation response as its own script).

import type { Page, TestInfo } from "@playwright/test";
import { expect, test } from "@playwright/test";

import {
  OUR_SCRIPT_NAME,
  generateScript,
} from "../src/settings/filters/generate";
import type { Rule } from "../src/settings/filters/model";
import type { FakeJmap } from "./fake-jmap";
import { addScript, installFakeJmap } from "./fake-jmap";
import { chooseOption, expectValue } from "./select";
import { signIn } from "./session";

const shot = async (page: Page, info: TestInfo, name: string) => {
  await page.screenshot({
    path: `/tmp/nebula-settings/${info.project.name}/${name}.png`,
  });
};

const expectNoSidewaysScroll = async (page: Page) => {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth
  );
  expect(overflow).toBeLessThanOrEqual(0);
};

/** The content of our filters script, and whether it is active. */
const ourScript = (jmap: FakeJmap) => {
  const script = jmap.scripts.find((item) => item.name === OUR_SCRIPT_NAME);
  return {
    active: script?.isActive ?? false,
    content: script === undefined ? "" : (jmap.blobs.get(script.blobId) ?? ""),
  };
};

const rule = (name: string, value: string): Rule => ({
  actions: [{ kind: "markRead" }],
  conditions: [
    {
      field: "from",
      header: "",
      kind: "match",
      negate: false,
      op: "contains",
      value,
    },
  ],
  enabled: true,
  id: name,
  match: "all",
  name,
});

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
});

test("settings: sections, the general controls, from quick settings", async ({
  page,
}, info) => {
  await page.goto("/mail/inbox");
  // Hydrated (the list is client-rendered) before using the popover.
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
  // Phones: from the account menu (their bar has no quick settings).
  if ((page.viewportSize()?.width ?? 1280) <= 640) {
    await page.getByRole("button", { name: /^Account/u }).click();
    await page.getByRole("menuitem", { name: "All settings" }).click();
  } else {
    await page.getByRole("button", { name: "Settings" }).click();
    await page.getByRole("link", { name: "All settings" }).click();
  }
  await expect(page).toHaveURL(/\/settings\/general$/u);
  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  await expect(page.getByText("Theme", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("switch", { name: /Verified sender/u })
  ).toBeVisible();
  await expectNoSidewaysScroll(page);
  await shot(page, info, "general");
  await page.getByRole("link", { name: "Filters" }).click();
  await expect(page.getByRole("button", { name: "New filter" })).toBeVisible();
  await expectNoSidewaysScroll(page);
});

test("privacy: the preload switch shows only with preload and saves the choice", async ({
  page,
}) => {
  // Without preload (the test server has no ticket keys) there is no switch.
  await page.goto("/settings/general");
  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  await expect(page.getByText("Sync between devices")).toBeVisible();
  await expect(
    page.getByRole("switch", { name: /Download images when/u })
  ).toHaveCount(0);

  // With preload: the BFF's answer, as the proxy would give it.
  const sent: unknown[] = [];
  let optOut = false;
  await page.route("**/api/images/preload", async (route) => {
    const body: unknown = route.request().postDataJSON();
    sent.push(body);
    if (typeof body === "object" && body !== null && "optOut" in body) {
      optOut = body.optOut === true;
    }
    await route.fulfill({ json: { available: true, optOut } });
  });
  await page.reload();
  const toggle = page.getByRole("switch", { name: /Download images when/u });
  await expect(toggle).toBeChecked();
  await toggle.click();
  await expect(toggle).not.toBeChecked();
  expect(sent).toStrictEqual([{}, { optOut: true }]);
});

test("filters: create a rule, turn it off, delete it", async ({
  page,
}, info) => {
  await page.goto("/settings/filters");
  await expect(page.getByText(/No filters yet/u)).toBeVisible();
  await page.getByRole("button", { name: "New filter" }).click();
  await page.getByLabel("Name").fill("Weekly news");
  await page.getByLabel("Condition 1: value").fill("weekly.example");
  await page.getByRole("button", { name: "Add action" }).click();
  await chooseOption(page.getByLabel("Action 1: folder"), {
    label: "Projects",
  });
  await expectNoSidewaysScroll(page);
  await shot(page, info, "rule-editor");
  await page.getByRole("button", { name: "Save filter" }).click();

  await expect(page.getByText("Weekly news")).toBeVisible();
  await expect.poll(() => ourScript(jmap).active).toBeTruthy();
  const saved = ourScript(jmap).content;
  expect(saved).toContain('# Rule: "Weekly news"');
  expect(saved).toContain('header :contains "from" "weekly.example"');
  expect(saved).toContain('fileinto :mailboxid "projects" "Projects";');
  await shot(page, info, "rules");

  await page.getByRole("switch", { name: "Filter Weekly news on" }).click();
  await expect.poll(() => ourScript(jmap).content).toContain("if false {");

  await page
    .getByRole("button", { name: "More actions for Weekly news" })
    .click();
  await page.getByRole("menuitem", { name: "Delete filter" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Delete filter" })
    .click();
  await expect(page.getByText(/No filters yet/u)).toBeVisible();
  await expect.poll(() => ourScript(jmap).content).not.toContain("Rule:");
});

test("filters: invalid rules don't save; forwarding warns", async ({
  page,
}) => {
  await page.goto("/settings/filters");
  await page.getByRole("button", { name: "New filter" }).click();
  await page.getByRole("button", { name: "Save filter" }).click();
  await expect(page.getByText("Add at least one action.")).toBeVisible();
  await expect(
    page.getByText("A condition has no text to look for.")
  ).toBeVisible();
  await page.getByRole("button", { name: "Add action" }).click();
  await chooseOption(page.getByLabel("Action 1", { exact: true }), {
    label: "Forward to",
  });
  await expect(
    page.getByText(/Forward only to addresses you control/u)
  ).toBeVisible();
  expect(jmap.scripts).toHaveLength(0);
});

test("filters: reorder with the keyboard", async ({ page }) => {
  addScript(
    jmap,
    OUR_SCRIPT_NAME,
    generateScript({
      rules: [rule("First", "a@x.example"), rule("Second", "b@x.example")],
      vacation: false,
    }),
    true
  );
  await page.goto("/settings/filters");
  const handle = page.getByRole("button", { name: /^Reorder Second/u });
  await handle.focus();
  await page.keyboard.press("ArrowUp");
  await expect
    .poll(() => {
      const { content } = ourScript(jmap);
      return content.indexOf('"Second"') < content.indexOf('"First"');
    })
    .toBeTruthy();
  await expect(handle).toBeFocused();
  await expect(
    page.getByText(/Moved Second to position 1 of 2/u)
  ).toBeAttached();
});

test("filters: another client's script stays raw until asked", async ({
  page,
}, info) => {
  addScript(
    jmap,
    "my-rules",
    'require "fileinto";\nif true {\n  keep;\n}\n',
    true
  );
  await page.goto("/settings/filters");
  await expect(page.getByText(/“my-rules” was written by hand/u)).toBeVisible();
  const editor = page.getByLabel("Sieve script my-rules");
  await expect(editor).toHaveValue(/keep;/u);
  await editor.fill('require "fileinto";\n!! broken\n');
  await page.getByRole("button", { name: "Check and save" }).click();
  await expect(page.getByText(/^Line 2:/u)).toBeVisible();
  await expectNoSidewaysScroll(page);
  await shot(page, info, "raw");
  // Nothing was overwritten.
  expect(jmap.blobs.get(jmap.scripts[0]?.blobId ?? "")).toContain("keep;");

  await page.getByRole("button", { name: "Use visual filters" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Use visual filters" })
    .click();
  await expect(page.getByRole("button", { name: "New filter" })).toBeVisible();
  await expect.poll(() => ourScript(jmap).active).toBeTruthy();
  expect(
    jmap.scripts.find((item) => item.name === "my-rules")?.isActive
  ).toBeFalsy();
});

test("create a filter from an email (mailing list)", async ({ page }) => {
  await page.goto("/mail/inbox/e2");
  await page
    .getByRole("button", { name: "More actions for this message" })
    .click();
  await page
    .getByRole("menuitem", { name: "Create filter from this email" })
    .click();
  await expect(page).toHaveURL(/\/settings\/filters\?from=e2/u);
  await expectValue(page.getByLabel("Condition 1: what to check"), "listId");
  await expect(page.getByLabel("Condition 1: value")).toHaveValue(
    "weekly.example"
  );
  // The bridge can't search headers: «no preview», not an error.
  await page.getByRole("button", { name: "Test on recent mail" }).click();
  await expect(
    page.getByText("No preview: the server's search can't run this filter", {
      exact: false,
    })
  ).toBeVisible();
  await expect(page.getByText("Couldn't search your mail.")).toHaveCount(0);
});

test("vacation response: a pushed change shows, but never over typing", async ({
  page,
}) => {
  // The app only listens for scripts on the bridge.
  jmap.calendar.bridge = true;
  await page.goto("/settings/vacation");
  const subject = page.getByLabel("Subject");
  await expect(subject).toHaveValue("");

  // Another client changes it: an untouched form follows.
  const reread = page.waitForResponse(
    (response) =>
      response.request().postData()?.includes("VacationResponse/get") === true,
    { timeout: 20_000 }
  );
  jmap.vacation.subject = "From elsewhere";
  jmap.pushChanges.push("SieveScript");
  await reread;
  await expect(subject).toHaveValue("From elsewhere");

  // The same with the user typing: what they wrote stays.
  await subject.fill("Mine");
  const asked = page.waitForResponse(
    (response) =>
      response.request().postData()?.includes("VacationResponse/get") === true,
    { timeout: 20_000 }
  );
  jmap.vacation.subject = "From elsewhere again";
  jmap.pushChanges.push("SieveScript");
  await asked;
  await expect(subject).toHaveValue("Mine");
});

test("vacation response: on, banner in the mail UI", async ({ page }, info) => {
  await page.goto("/settings/vacation");
  await page.getByRole("switch", { name: "Vacation response on" }).click();
  await page.getByLabel("Subject").fill("Out until Monday");
  await page.getByRole("textbox", { name: "Message" }).fill("I'm away.");
  await expectNoSidewaysScroll(page);
  await shot(page, info, "vacation");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect
    .poll(() => jmap.vacationUpdates.at(-1))
    .toMatchObject({
      isEnabled: true,
      subject: "Out until Monday",
      textBody: "I'm away.\n",
    });
  await page.goto("/mail/inbox");
  await expect(page.getByText("Your vacation response is on.")).toBeVisible();
  await expectNoSidewaysScroll(page);
  await shot(page, info, "banner");
});

test("vacation response with our filters active keeps both", async ({
  page,
}) => {
  addScript(
    jmap,
    OUR_SCRIPT_NAME,
    generateScript({ rules: [rule("First", "a@x.example")], vacation: false }),
    true
  );
  await page.goto("/settings/vacation");
  await page.getByRole("switch", { name: "Vacation response on" }).click();
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect
    .poll(() => ourScript(jmap).content)
    .toContain('include :personal :optional "vacation";');
  expect(ourScript(jmap).active).toBeTruthy();
  expect(jmap.vacationUpdates.at(-1)).toMatchObject({ isEnabled: false });
  await page.goto("/mail/inbox");
  await expect(page.getByText("Your vacation response is on.")).toBeVisible();
});

test("emails can open on the full page, like Proton", async ({
  page,
}, info) => {
  test.skip(info.project.name !== "desktop", "phones already show one panel");
  await page.goto("/mail/inbox");
  const list = page.getByRole("region", { exact: true, name: "Messages" });
  await expect(list.getByText("Weekly digest").first()).toBeVisible();
  // Side by side by default: the empty reading pane is there.
  await expect(page.getByText("Pick a message")).toBeVisible();

  await page.getByRole("button", { name: "Settings" }).click();
  await page
    .getByRole("radiogroup", { name: "Open emails" })
    .getByRole("radio", { name: "Full page" })
    .click();
  await page.keyboard.press("Escape");
  // The list alone, the whole width.
  await expect(page.getByText("Pick a message")).toHaveCount(0);

  await list.getByText("Weekly digest").first().click();
  const reader = page.getByRole("region", { exact: true, name: "Message" });
  await expect(
    reader.getByRole("button", { name: "Back to list" })
  ).toBeVisible();
  await expect(list).toBeHidden();

  await reader.getByRole("button", { name: "Back to list" }).click();
  await expect(list).toBeVisible();
  await expect(page).toHaveURL(/\/mail\/inbox$/u);
});
