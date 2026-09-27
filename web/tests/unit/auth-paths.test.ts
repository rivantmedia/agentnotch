import { describe, expect, it } from "vitest";

import {
  DEFAULT_AFTER_LOGIN,
  isProtectedPath,
  safeNextPath,
} from "~/lib/auth-paths";

describe("isProtectedPath", () => {
  it("covers the signed-in pages and everything below them", () => {
    for (const path of [
      "/dashboard",
      "/dashboard/pools",
      "/accounts",
      "/accounts/abc",
      "/projects",
      "/projects/cm1abcdef0000xyz",
      "/pools",
      "/settings",
      "/settings/devices",
    ]) {
      expect(isProtectedPath(path), path).toBe(true);
    }
  });

  it("leaves public pages alone", () => {
    for (const path of [
      "/",
      "/login",
      "/auth/callback",
      "/auth/signout",
      "/download",
      "/download/mac",
      "/api/app/v1/config",
      "/api/trpc/viewer.current",
      "/dashboards",
      "/projectsx",
      "/poolside",
      "/settingsx",
    ]) {
      expect(isProtectedPath(path), path).toBe(false);
    }
  });
});

describe("safeNextPath", () => {
  it("keeps paths on this site, with their query and hash", () => {
    expect(safeNextPath("/pools")).toBe("/pools");
    expect(safeNextPath("/accounts/abc?tab=usage#top")).toBe(
      "/accounts/abc?tab=usage#top",
    );
    expect(safeNextPath("/a/../pools")).toBe("/pools");
  });

  it("falls back for anything that could leave the site", () => {
    for (const next of [
      null,
      undefined,
      "",
      "pools",
      "//evil.example",
      "//evil.example/path",
      "/\\evil.example",
      "/\\/evil.example",
      "/..//evil.example",
      "/.//evil.example",
      "https://evil.example",
      "javascript:alert(1)",
      "/\t/evil.example",
      "/\n/evil.example",
      "/%0a",
      `/${"a".repeat(3000)}`,
    ]) {
      const result = safeNextPath(next);
      expect(result.startsWith("//"), String(next)).toBe(false);
      expect(result.startsWith("/"), String(next)).toBe(true);
      if (next !== "/%0a")
        expect(result, String(next)).toBe(DEFAULT_AFTER_LOGIN);
    }
    expect(safeNextPath("//evil.example", "/login")).toBe("/login");
  });
});
