// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { previewWithoutSubject } from "./preview-text";

describe(previewWithoutSubject, () => {
  it("drops the subject the preview starts with", () => {
    expect(
      previewWithoutSubject(
        "Invitation: Team sync @ Monday  This event has been updated",
        "Invitation: Team sync @ Monday"
      )
    ).toBe("This event has been updated");
  });

  it("ignores case and the separators after it", () => {
    expect(
      previewWithoutSubject("hello world — how are you", "Hello world")
    ).toBe("how are you");
  });

  it("keeps previews that don't start with the subject", () => {
    expect(previewWithoutSubject("Hola.", "push-after-verify")).toBe("Hola.");
  });

  it("doesn't cut a word in half", () => {
    expect(previewWithoutSubject("Hellos everyone", "Hello")).toBe(
      "Hellos everyone"
    );
  });

  it("keeps a preview that is only the subject", () => {
    expect(previewWithoutSubject("Lunch", "Lunch")).toBe("Lunch");
  });

  it("keeps the preview when there is no subject", () => {
    expect(previewWithoutSubject("Some text", "")).toBe("Some text");
  });
});
