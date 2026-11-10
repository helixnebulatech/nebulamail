// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The mail UI at phone and tablet widths: no sideways
// scroll, 44 px touch targets, the reader instead of the list on phones,
// every action reachable by touch. Screenshots go to /tmp/nebula-mobile
// (outside the repo) for a human look.

import type { Locator, Page, TestInfo } from "@playwright/test";
import { expect, test } from "@playwright/test";

import type { FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

const PHONE_MAX = 640;

const shot = async (page: Page, info: TestInfo, name: string) => {
  await page.screenshot({
    path: `/tmp/nebula-mobile/${info.project.name}/${name}.png`,
  });
};

const isPhone = (page: Page): boolean =>
  (page.viewportSize()?.width ?? 0) <= PHONE_MAX;

const isTouch = (info: TestInfo): boolean => info.project.use.hasTouch === true;

const expectNoSidewaysScroll = async (page: Page) => {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth
  );
  expect(overflow).toBeLessThanOrEqual(0);
};

/** Every visible button in `area` is a 44 px target, inside the screen. */
const expectTouchTargets = async (page: Page, area: Locator) => {
  const width = page.viewportSize()?.width ?? 0;
  const boxes = await area.getByRole("button").evaluateAll((buttons) =>
    buttons
      .map((button) => {
        const box = button.getBoundingClientRect();
        // Small chip buttons grow their hit area with an invisible ::after.
        const after = getComputedStyle(button, "::after");
        const grow =
          after.content !== "none" && after.position === "absolute"
            ? -2 * Number(after.top.replace(/px$/u, ""))
            : 0;
        return {
          height: box.height + grow,
          label: button.getAttribute("aria-label") ?? button.textContent,
          left: box.left,
          right: box.right,
          width: box.width + grow,
        };
      })
      .filter((box) => box.width > 0 && box.height > 0)
  );
  expect(boxes.length).toBeGreaterThan(0);
  for (const box of boxes) {
    expect.soft(box.height, `${box.label} height`).toBeGreaterThanOrEqual(44);
    expect.soft(box.width, `${box.label} width`).toBeGreaterThanOrEqual(44);
    expect.soft(box.left, `${box.label} left`).toBeGreaterThanOrEqual(0);
    expect.soft(box.right, `${box.label} right`).toBeLessThanOrEqual(width);
  }
};

const expectInsideScreen = async (page: Page, element: Locator) => {
  const box = await element.boundingBox();
  const width = page.viewportSize()?.width ?? 0;
  expect(box).not.toBeNull();
  expect(box?.x ?? -1).toBeGreaterThanOrEqual(0);
  expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(width + 0.5);
};

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
  await page.goto("/mail/inbox");
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
});

test("the list fits the screen", async ({ page }, info) => {
  await shot(page, info, "list");
  await expectNoSidewaysScroll(page);
  if (isTouch(info)) {
    await expectTouchTargets(page, page.getByRole("banner"));
  }
});

test("the inbox list scrolls inside its panel", async ({ page }) => {
  // Short enough that the sample inbox overflows even with compact rows.
  const size = page.viewportSize();
  if (size !== null) {
    await page.setViewportSize({ height: 420, width: size.width });
  }
  // The split-inbox tabs wrap the list: they must pass the panel's height
  // down, or the list grows past the panel and nothing scrolls.
  const list = await page
    .getByRole("region", { exact: true, name: "Messages" })
    .evaluate((region) => {
      const scroller = [...region.querySelectorAll("*")].find(
        (element) => getComputedStyle(element).overflowY === "auto"
      );
      if (scroller === undefined) {
        return null;
      }
      return {
        insidePanel:
          scroller.getBoundingClientRect().bottom <=
          region.getBoundingClientRect().bottom + 1,
        scrolls: scroller.scrollHeight > scroller.clientHeight + 10,
      };
    });
  expect(list).toStrictEqual({ insidePanel: true, scrolls: true });
});

test("reader: replaces the list on phones, back returns", async ({
  page,
}, info) => {
  await page.getByText("Weekly digest").first().click();
  const reader = page.getByRole("region", { name: "Message" });
  await expect(
    reader.getByRole("heading", { name: "Weekly digest" })
  ).toBeVisible();
  await shot(page, info, "reader");
  await expectNoSidewaysScroll(page);
  const list = page.getByRole("list", { exact: true, name: "Messages" });
  if (isPhone(page)) {
    await expect(list).toBeHidden();
  }
  if (isTouch(info)) {
    await expectTouchTargets(
      page,
      reader.getByRole("toolbar", { name: "Message actions" })
    );
  }
  // A 640 px newsletter is zoomed out to the iframe, not cut off.
  await expect
    .poll(
      async () =>
        await page
          .locator("iframe")
          .first()
          .evaluate(
            (iframe: HTMLIFrameElement) =>
              (iframe.contentDocument?.documentElement.scrollWidth ?? 0) <=
              iframe.clientWidth + 1
          )
    )
    .toBe(true);

  await reader
    .getByRole("button", { name: "Back to list" })
    .or(reader.getByRole("button", { exact: true, name: "Close" }))
    .first()
    .click();
  await expect(list).toBeVisible();
  await expect(page).toHaveURL(/\/mail\/inbox$/u);
});

test("reader: move to a folder by touch", async ({ page }, info) => {
  await page.getByText("Your October invoice").first().click();
  const reader = page.getByRole("region", { name: "Message" });
  await expect(
    reader.getByRole("heading", { name: "Your October invoice" })
  ).toBeVisible();
  await reader.getByRole("button", { name: "Move to" }).click();
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem", { name: "Projects" })).toBeVisible();
  await expectInsideScreen(page, menu);
  await shot(page, info, "reader-move");
  await menu.getByRole("menuitem", { name: "Projects" }).click();
  await expect
    .poll(() => jmap.updates.at(-1)?.e3)
    .toMatchObject({ mailboxIds: { projects: true } });
});

test("reader: More has what the toolbar hides, and print", async ({
  page,
}, info) => {
  await page.getByText("Weekly digest").first().click();
  await page.getByRole("button", { exact: true, name: "More actions" }).click();
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem", { name: "Print" })).toBeVisible();
  // Touch screens: the toolbar hides these, so More has them. Desktop: the
  // toolbar shows them, and More doesn't repeat them.
  const hidden = ["Star", "Mark as unread", "Report spam"];
  await Promise.all(
    hidden.map(async (name) => {
      await (isTouch(info)
        ? expect(menu.getByRole("menuitem", { name })).toBeVisible()
        : expect(menu.getByRole("menuitem", { name })).toHaveCount(0));
    })
  );
  await expectInsideScreen(page, menu);
  await shot(page, info, "reader-more");
  if (isTouch(info)) {
    const item = await menu.getByRole("menuitem").first().boundingBox();
    expect(item?.height ?? 0).toBeGreaterThanOrEqual(44);
  }
});

test("compose is full screen on phones", async ({ page }, info) => {
  await page.getByText("Weekly digest").first().click();
  await page
    .getByRole("button", { name: /^Reply/u })
    .last()
    .click();
  const composer = page.locator("[data-composer]");
  await expect(composer).toBeVisible();
  await page.waitForTimeout(300);
  await shot(page, info, "compose");
  if (isPhone(page)) {
    const box = await composer.boundingBox();
    const size = page.viewportSize();
    expect(box?.width).toBe(size?.width);
    expect(box?.height).toBe(size?.height);
    // "Maximize" means nothing when it already fills the screen.
    await expect(
      composer.getByRole("button", { name: "Full screen" })
    ).toBeHidden();
  }
  await expectNoSidewaysScroll(page);
});

test("sidebar drawer on phones", async ({ page }, info) => {
  test.skip(!isPhone(page), "phones only");
  await page.getByRole("button", { name: "Open menu" }).click();
  const drawer = page.getByRole("dialog", { name: "Mail folders" });
  await expect(drawer).toBeVisible();
  await shot(page, info, "drawer");
  const projects = drawer.getByRole("link", { name: "Projects" });
  const link = await projects.boundingBox();
  expect(link?.height ?? 0).toBeGreaterThanOrEqual(44);
  await projects.click();
  await expect(drawer).toBeHidden();
  await expect(page).toHaveURL(/\/mail\/projects/u);
});

test("row checkbox: 44 px to touch, 16 px to see", async ({ page }, info) => {
  const checkbox = page.getByRole("checkbox", {
    name: /Your October invoice/u,
  });
  const box = await checkbox.boundingBox();
  expect(box?.width).toBe(16);
  const hits = await checkbox.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    const x = rect.left + rect.width / 2;
    const y = rect.top + rect.height / 2;
    // Just inside the corners of a 44 px square around the box.
    return [-21, 21].flatMap((dx) =>
      [-21, 21].map(
        (dy) =>
          document
            .elementFromPoint(x + dx, y + dy)
            ?.closest('[role="checkbox"]') === element
      )
    );
  });
  expect(hits).toEqual(
    isTouch(info) ? [true, true, true, true] : [false, false, false, false]
  );
});

test("command palette: a button on touch screens", async ({ page }, info) => {
  const button = page.getByRole("button", { exact: true, name: "Commands" });
  if (!isTouch(info)) {
    await expect(button).toBeHidden();
    return;
  }
  await button.click();
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  await expect(
    palette.getByRole("combobox").or(palette.getByRole("textbox")).first()
  ).toBeVisible();
  await expectInsideScreen(page, palette);
  await shot(page, info, "palette");
});

test("settings and select-many work on touch", async ({ page }, info) => {
  // Phones keep the bar for the title and search: settings are in the
  // account menu. Tablets keep the quick settings button.
  if (isPhone(page)) {
    await page.getByRole("button", { name: /^Account/u }).click();
    const menu = page.getByRole("menu");
    await expect(
      menu.getByRole("menuitem", { name: "All settings" })
    ).toBeVisible();
    await expectInsideScreen(page, menu);
  } else {
    await page.getByRole("button", { name: "Settings" }).click();
    const settings = page.getByRole("dialog");
    await expect(settings).toBeVisible();
    await expectInsideScreen(page, settings);
  }
  await shot(page, info, "settings");
  await page.keyboard.press("Escape");

  // Tapping the avatar selects the row (like Gmail); the bulk bar fits.
  await page.getByRole("checkbox", { name: /Your October invoice/u }).click();
  const bulk = page.getByRole("toolbar", {
    name: "Actions for the selected messages",
  });
  await expect(bulk).toBeVisible();
  await shot(page, info, "bulk");
  await expectNoSidewaysScroll(page);
  if (isTouch(info)) {
    await expectTouchTargets(page, bulk);
  }
});
