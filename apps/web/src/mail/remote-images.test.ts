// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Tests run with the BFF build's `define` (vitest.config.ts, the same as
// vite.config.ts): the frame page is our server's route.

import { describe, expect, it } from "vitest";

import { FRAME_PAGE } from "../lib/frame-page";
import {
  AVAILABLE_REMOTE_IMAGE_MODES,
  DIRECT_FRAME_PAGE,
} from "./remote-images";

describe("remote images with our server", () => {
  it("shows direct images in the route's frame page", () => {
    expect(FRAME_PAGE).toBe("email-frame.html");
    expect(DIRECT_FRAME_PAGE).toBe("/email-frame.html");
  });

  it("offers the direct mode", () => {
    expect(AVAILABLE_REMOTE_IMAGE_MODES).toContain("direct");
  });
});
