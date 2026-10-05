/**
 * The usage page's accounts come from the address bar, not from the props of the page's first
 * render, as its period does (use-period.test.ts). Rendered on the server's terms
 * (renderToString), which is also the first client render.
 */
import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { type AccountSelection } from "~/lib/account-selection";

let search: URLSearchParams | null = null;
vi.mock("next/navigation", () => ({ useSearchParams: () => search }));

const { useAccountSelection } = await import("~/app/usage/account-filter");

const A = "a".repeat(64);
const B = "b".repeat(64);

function Probe({ initial }: { initial: AccountSelection }) {
  const { selection, shown, shownKey, stale } = useAccountSelection(initial);
  return createElement(
    "p",
    null,
    [
      JSON.stringify(selection),
      JSON.stringify(shown),
      JSON.stringify(shownKey),
      String(stale),
    ].join(" "),
  );
}

const render = (initial: AccountSelection) =>
  renderToString(createElement(Probe, { initial }))
    .replace(/<\/?p>/g, "")
    .replaceAll("&quot;", '"');

beforeEach(() => {
  search = null;
});

describe("useAccountSelection", () => {
  it("starts from the address bar, whatever the props say", () => {
    search = new URLSearchParams(`period=30d&accounts=${B},${A}`);
    expect(render(null)).toBe(
      `["${A}","${B}"] ["${A}","${B}"] "${A},${B}" false`,
    );
    // No `?accounts=` is every account, even when the first render had a selection.
    search = new URLSearchParams("period=30d");
    expect(render([A])).toBe("null null null false");
    // An empty one is none.
    search = new URLSearchParams("accounts=");
    expect(render(null)).toBe('[] [] "" false');
  });

  it("falls back to the props outside the App Router", () => {
    expect(render([A])).toBe(`["${A}"] ["${A}"] "${A}" false`);
  });
});
