/**
 * The contract check's field coverage (tests/support/contract-rows.ts): every field the Mac app
 * sends is either compared with what the website stored or listed as not stored, so a field the
 * website doesn't know (and its schema strips without a word) fails the check.
 */
import { describe, expect, it } from "vitest";

import { type SyncRequest } from "~/server/app-api/schema";

import {
  expectedRows,
  leafPaths,
  NOT_STORED,
  unstoredFields,
} from "../support/contract-rows";
import { syncFixture, type SyncFixture } from "../support/fixtures";

/** The fixture as the app sends it, unparsed: nothing stripped. */
function sent(edit: (r: SyncFixture) => void = () => undefined): SyncRequest {
  const raw = syncFixture();
  edit(raw);
  return raw as unknown as SyncRequest;
}

describe("unstoredFields", () => {
  it("finds nothing unaccounted for in the contract's own request", () => {
    const request = sent();
    // Its optional summary is there, so the check covers it too.
    expect(request.sessions.some((s) => s.summary)).toBe(true);
    expect(unstoredFields(request, expectedRows)).toEqual([]);
  });

  it("finds a field the app sends that the website doesn't know", () => {
    const request = sent((r) => {
      r.sessions[0]!.gitBranch = "main";
      (r.device as Record<string, unknown>).osVersion = "26.1";
      (r.usage[0]!.windows[0] as Record<string, unknown>).label = "5-hour";
    });
    expect(unstoredFields(request, expectedRows)).toEqual([
      "device.osVersion",
      "sessions[].gitBranch",
      "usage[].windows[].label",
    ]);
  });

  it("finds a field the app renamed", () => {
    const request = sent((r) => {
      for (const session of r.sessions) {
        if (session.summary) {
          session.recap = session.summary;
          delete session.summary;
        }
      }
    });
    expect(unstoredFields(request, expectedRows)).toEqual([
      "sessions[].recap.generatedAt",
      "sessions[].recap.model",
      "sessions[].recap.text",
    ]);
  });

  it("finds a field the expected rows stopped comparing", () => {
    const request = sent();
    // Expected rows built without ever reading a session's title.
    const withoutTitles = (r: SyncRequest) =>
      expectedRows({
        ...r,
        sessions: r.sessions.map((s) => ({
          ...(Object.fromEntries(
            Object.keys(s)
              .filter((key) => key !== "title")
              .map((key) => [key, s[key as keyof typeof s]]),
          ) as typeof s),
          title: null,
        })),
      });
    expect(unstoredFields(request, withoutTitles)).toEqual([
      "sessions[].title",
    ]);
  });

  it("lets a field through only when it is listed as not stored, with a reason", () => {
    const request = sent();
    expect(Object.keys(NOT_STORED)).toEqual(["schemaVersion"]);
    expect(
      unstoredFields(request, expectedRows, {}),
      "schemaVersion needs its listing",
    ).toEqual(["schemaVersion"]);
    for (const reason of Object.values(NOT_STORED)) {
      expect(reason.length).toBeGreaterThan(20);
    }
  });
});

describe("leafPaths", () => {
  it("names every field once, arrays' elements as []", () => {
    expect(
      leafPaths({
        a: 1,
        b: { c: null, d: [1, 2] },
        e: [{ f: "x" }, { f: "y", g: true }],
        h: [],
        i: {},
      }),
    ).toEqual(["a", "b.c", "b.d[]", "e[].f", "e[].g", "h", "i"]);
  });
});
