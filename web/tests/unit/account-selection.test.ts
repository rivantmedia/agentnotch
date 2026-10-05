/**
 * The usage page's account selection (lib/account-selection.ts): read from and written to
 * `?accounts=`, and switched one account at a time.
 */
import { describe, expect, it } from "vitest";

import {
  MAX_SELECTED_ACCOUNTS,
  normalizeSelection,
  selectedAccounts,
  selectionFromParam,
  selectionParam,
  toggleAccount,
} from "~/lib/account-selection";

const A = "a".repeat(64);
const B = "b".repeat(64);
const C = "c".repeat(64);
const VISIBLE = [A, B, C];

describe("the selection in the address bar", () => {
  it("means every account without the parameter, and none with an empty one", () => {
    expect(selectionFromParam(undefined)).toBeNull();
    expect(selectionFromParam(null)).toBeNull();
    expect(selectionFromParam("")).toEqual([]);
  });

  it("keeps account keys only, sorted and once each", () => {
    expect(
      selectionFromParam(
        `${C},${A}, ${C.toUpperCase()},nope,${"a".repeat(63)}`,
      ),
    ).toEqual([A, C]);
    // The first of a repeated parameter, as for the period.
    expect(selectionFromParam([B, A])).toEqual([B]);
    const many = Array.from({ length: MAX_SELECTED_ACCOUNTS + 5 }, (_, i) =>
      i.toString(16).padStart(64, "0"),
    );
    expect(selectionFromParam(many.join(","))).toHaveLength(
      MAX_SELECTED_ACCOUNTS,
    );
  });

  it("writes back what it reads", () => {
    expect(selectionParam(null)).toBeNull();
    expect(selectionParam([])).toBe("");
    expect(selectionParam([A, C])).toBe(`${A},${C}`);
    for (const value of [null, "", `${A},${C}`]) {
      expect(selectionParam(selectionFromParam(value))).toBe(value);
    }
  });
});

describe("choosing accounts", () => {
  const accounts = VISIBLE.map((key) => ({ key }));

  it("picks the visible accounts it names, in their order", () => {
    expect(selectedAccounts(accounts, null)).toEqual(accounts);
    expect(selectedAccounts(accounts, [C, A]).map((a) => a.key)).toEqual([
      A,
      C,
    ]);
    // A key the viewer doesn't see (an old link) picks nothing.
    expect(selectedAccounts(accounts, ["d".repeat(64)])).toEqual([]);
  });

  it("writes every visible account as all, so later accounts join", () => {
    expect(normalizeSelection([C, B, A], VISIBLE)).toBeNull();
    expect(normalizeSelection([C, A, A, "d".repeat(64)], VISIBLE)).toEqual([
      A,
      C,
    ]);
    expect(normalizeSelection([], VISIBLE)).toEqual([]);
  });

  it("switches one account at a time", () => {
    // From all, switching one off leaves the others.
    expect(toggleAccount(null, B, VISIBLE)).toEqual([A, C]);
    expect(toggleAccount([A, C], A, VISIBLE)).toEqual([C]);
    expect(toggleAccount([C], C, VISIBLE)).toEqual([]);
    expect(toggleAccount([], B, VISIBLE)).toEqual([B]);
    // Switching the last one back on is all of them again.
    expect(toggleAccount([A, C], B, VISIBLE)).toBeNull();
    // Keys the viewer no longer sees drop out on the way.
    expect(toggleAccount([A, "d".repeat(64)], B, VISIBLE)).toEqual([A, B]);
  });
});
