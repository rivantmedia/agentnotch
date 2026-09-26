/**
 * Removing data on /settings: what has to be typed, and what the page says happens next.
 */
import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { SYNC_AGAIN_NOTE } from "~/app/settings/copy";
import {
  confirmsPhrase,
  DELETE_DATA_CONFIRMATION,
  REMOVE_SUMMARIES_CONFIRMATION,
} from "~/lib/data-controls";

describe("data controls", () => {
  it("need the phrase typed, in any case", () => {
    expect(confirmsPhrase("delete my data", DELETE_DATA_CONFIRMATION)).toBe(
      true,
    );
    expect(confirmsPhrase("  Delete My Data ", DELETE_DATA_CONFIRMATION)).toBe(
      true,
    );
    expect(confirmsPhrase("delete my dat", DELETE_DATA_CONFIRMATION)).toBe(
      false,
    );
    expect(confirmsPhrase("", REMOVE_SUMMARIES_CONFIRMATION)).toBe(false);
    expect(
      confirmsPhrase("remove summaries", REMOVE_SUMMARIES_CONFIRMATION),
    ).toBe(true);
  });

  it("say the app syncs again unless sync is off, and that a later sign-in starts empty", () => {
    expect(SYNC_AGAIN_NOTE).toMatch(/Unless sync is turned off in the Mac app/);
    expect(SYNC_AGAIN_NOTE).toMatch(/sends its data again on its next sync/);
    expect(SYNC_AGAIN_NOTE).toMatch(/Signing in again later starts empty/);
    // …on the page, next to the button.
    const source = readFileSync(
      path.resolve(
        import.meta.dirname,
        "../../src/app/settings/data-controls.tsx",
      ),
      "utf8",
    );
    expect(source).toContain("{SYNC_AGAIN_NOTE}");
    const page = readFileSync(
      path.resolve(import.meta.dirname, "../../src/app/settings/page.tsx"),
      "utf8",
    );
    expect(page).toContain("<DataControls />");
  });
});
