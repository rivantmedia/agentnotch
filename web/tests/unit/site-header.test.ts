/**
 * The header offers the download page to everyone: signed out is when people need it most, and
 * the signed-in navigation isn't shown then.
 */
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

const current = vi.fn<() => Promise<{ email: string } | null>>();
vi.mock("~/trpc/server", () => ({ api: { viewer: { current } } }));
// A client component that needs the router; what it lists isn't under test here.
vi.mock("~/app/_components/nav-links", () => ({ NavLinks: () => null }));

const { SiteHeader } = await import("~/app/_components/site-header");

beforeEach(() => current.mockReset());

describe("the site header", () => {
  it("links the download page, signed in or out", async () => {
    for (const viewer of [null, { email: "ann@example.com" }]) {
      current.mockResolvedValue(viewer);
      const html = renderToString(await SiteHeader());
      expect(html, String(viewer?.email)).toMatch(
        /<a[^>]*href="\/download"[^>]*>Download<\/a>/,
      );
      expect(html).toContain(viewer ? "Sign out" : "Sign in");
    }
  });

  it("lets a long email shrink so Sign out stays on screen", async () => {
    // From md up the cluster holds Download, the email and Sign out. A flex item's minimum width
    // is its content's unless it says min-w-0, so without it the truncating email never
    // truncates and the row overflows a 768–853px screen. Below md the email is hidden, and a
    // cluster allowed to shrink there spills its buttons past a 640–655px screen instead.
    current.mockResolvedValue({
      email: "a.very.long.firstname.lastname@example-company.com",
    });
    const html = renderToString(await SiteHeader());
    const cluster = /<div class="([^"]*)"><a[^>]*href="\/download"/.exec(html);
    const classes = cluster?.[1]?.split(" ");
    expect(classes).toEqual(
      expect.arrayContaining(["ml-auto", "flex", "md:min-w-0"]),
    );
    expect(classes).not.toContain("min-w-0");
    const email = /<span class="([^"]*)"[^>]*>a\.very\.long/.exec(html);
    expect(email?.[1]?.split(" ")).toEqual(
      expect.arrayContaining(["truncate", "md:inline"]),
    );
  });
});
