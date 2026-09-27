/**
 * The period a breakdown shows comes from the address bar, not from the props of the page's
 * first render: Back restores a page with those props while the address keeps the period picked
 * since. Rendered on the server's terms (renderToString), which is also the first client render.
 */
import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { type UsagePeriod } from "~/lib/usage-period";

let search: URLSearchParams | null = null;
vi.mock("next/navigation", () => ({ useSearchParams: () => search }));

const { usePeriod } = await import("~/app/_components/usage-breakdown");

function Probe({ initial }: { initial: UsagePeriod }) {
  const { period, shown, stale } = usePeriod(initial);
  return createElement("p", null, `${period} ${shown} ${String(stale)}`);
}

const render = (initial: UsagePeriod) =>
  renderToString(createElement(Probe, { initial })).replace(/<\/?p>/g, "");

beforeEach(() => {
  search = null;
});

describe("usePeriod", () => {
  it("starts from the address bar, whatever the props say", () => {
    search = new URLSearchParams("period=30d&project=x");
    expect(render("7d")).toBe("30d 30d false");
    // No `?period=` is the default period, even when the first render had another.
    search = new URLSearchParams("project=x");
    expect(render("all")).toBe("7d 7d false");
    // Anything else is the default too.
    search = new URLSearchParams("period=90d");
    expect(render("30d")).toBe("7d 7d false");
  });

  it("falls back to the props outside the App Router", () => {
    expect(render("all")).toBe("all all false");
  });
});
