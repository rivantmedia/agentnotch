/**
 * The project page answers a real 404 for a project the viewer can't see, as the account page
 * does: the check runs in the route's layout, above its loading.tsx, so it throws notFound()
 * before anything streams.
 */
import { existsSync } from "node:fs";
import path from "node:path";

import { notFound } from "next/navigation";
import { beforeEach, describe, expect, it, vi } from "vitest";

const visible = vi.fn<(input: { id: string }) => Promise<boolean>>();
const requireViewer = vi.fn(async (_returnTo: string) => ({ id: "ann" }));

vi.mock("~/trpc/server", () => ({ api: { projects: { visible } } }));
vi.mock("~/server/pages", () => ({ requireViewer }));

const { default: ProjectLayout } = await import("~/app/projects/[id]/layout");

const ID = "cm1abcdef0000xyz";
const route = path.resolve(import.meta.dirname, "../../src/app/projects/[id]");
const app = path.resolve(import.meta.dirname, "../../src/app");

/** What notFound() throws, to compare with (Next marks it with a digest). */
function notFoundError(): unknown {
  try {
    notFound();
  } catch (error) {
    return error;
  }
}

function render(id: string) {
  return ProjectLayout({
    children: "the page",
    params: Promise.resolve({ id }),
  });
}

beforeEach(() => {
  visible.mockReset();
  requireViewer.mockClear();
});

describe("the project route's layout", () => {
  it("answers not found for a project the viewer can't see", async () => {
    visible.mockResolvedValue(false);
    const error: unknown = await render(ID).catch((e: unknown) => e);
    expect(error).toMatchObject({
      digest: (notFoundError() as { digest: string }).digest,
    });
    expect(visible).toHaveBeenCalledWith({ id: ID });
    expect(requireViewer).toHaveBeenCalledWith(`/projects/${ID}`);
  });

  it("answers not found for a malformed id without asking", async () => {
    const error: unknown = await render("not an id").catch((e: unknown) => e);
    expect(error).toMatchObject({
      digest: (notFoundError() as { digest: string }).digest,
    });
    expect(visible).not.toHaveBeenCalled();
    expect(requireViewer).not.toHaveBeenCalled();
  });

  it("renders the page for a project the viewer can see", async () => {
    visible.mockResolvedValue(true);
    expect(await render(ID)).toBe("the page");
  });

  it("sits above every loading boundary", () => {
    // Next nests loading.tsx inside the segment's layout: the check above runs before it.
    expect(existsSync(path.join(route, "layout.tsx"))).toBe(true);
    expect(existsSync(path.join(route, "loading.tsx"))).toBe(true);
    // A segment's loading.tsx also wraps every segment below it, layouts included: one in a
    // folder above would stream (with a 200) before this check ran.
    for (
      let dir = path.dirname(route);
      dir !== path.dirname(app);
      dir = path.dirname(dir)
    ) {
      expect(existsSync(path.join(dir, "loading.tsx")), dir).toBe(false);
    }
  });
});
