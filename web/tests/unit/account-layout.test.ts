/**
 * The account page answers a real 404 for an account the viewer can't see: the check runs in
 * the route's layout, above its loading.tsx, so it throws notFound() before anything streams
 * (after the loading fallback flushed, the status would already be 200).
 */
import { existsSync } from "node:fs";
import path from "node:path";

import { notFound } from "next/navigation";
import { beforeEach, describe, expect, it, vi } from "vitest";

const visible = vi.fn<(input: { accountKey: string }) => Promise<boolean>>();
const requireViewer = vi.fn(async (_returnTo: string) => ({ id: "ann" }));

vi.mock("~/trpc/server", () => ({ api: { accounts: { visible } } }));
vi.mock("~/server/pages", () => ({ requireViewer }));

const { default: AccountLayout } = await import("~/app/accounts/[key]/layout");

const KEY = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
const route = path.resolve(import.meta.dirname, "../../src/app/accounts/[key]");

/** What notFound() throws, to compare with (Next marks it with a digest). */
function notFoundError(): unknown {
  try {
    notFound();
  } catch (error) {
    return error;
  }
}

function render(key: string) {
  return AccountLayout({
    children: "the page",
    params: Promise.resolve({ key }),
  });
}

beforeEach(() => {
  visible.mockReset();
  requireViewer.mockClear();
});

describe("the account route's layout", () => {
  it("answers not found for an account the viewer can't see", async () => {
    visible.mockResolvedValue(false);
    const error: unknown = await render(KEY).catch((e: unknown) => e);
    expect(error).toMatchObject({
      digest: (notFoundError() as { digest: string }).digest,
    });
    expect(visible).toHaveBeenCalledWith({ accountKey: KEY });
    expect(requireViewer).toHaveBeenCalledWith(`/accounts/${KEY}`);
  });

  it("answers not found for a malformed key without asking", async () => {
    const error: unknown = await render("not-a-key").catch((e: unknown) => e);
    expect(error).toMatchObject({
      digest: (notFoundError() as { digest: string }).digest,
    });
    expect(visible).not.toHaveBeenCalled();
  });

  it("renders the page for an account the viewer can see", async () => {
    visible.mockResolvedValue(true);
    expect(await render(KEY)).toBe("the page");
  });

  it("sits above the route's loading boundary", () => {
    // Next nests loading.tsx inside the segment's layout: the check above runs before it.
    expect(existsSync(path.join(route, "layout.tsx"))).toBe(true);
    expect(existsSync(path.join(route, "loading.tsx"))).toBe(true);
  });
});
