/**
 * Project keys are per install (contract/README.md), so the website keeps a project per Mac. Where
 * projects are listed, and in the README, the site says so rather than letting two rows of the
 * same folder look like a bug.
 */
import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { PROJECTS_DESCRIPTION } from "~/app/accounts/[key]/copy";

const web = path.resolve(import.meta.dirname, "../..");
const read = (file: string) => readFileSync(path.join(web, file), "utf8");

describe("projects are per Mac", () => {
  it("the projects table's description says so", () => {
    expect(PROJECTS_DESCRIPTION).toMatch(/per Mac/);
    expect(PROJECTS_DESCRIPTION).toMatch(/two Macs/);
    // …and it is the description the table is shown with.
    const source = read("src/app/accounts/[key]/account-activity.tsx");
    expect(source).toMatch(
      /title="Projects"\s+description=\{PROJECTS_DESCRIPTION\}/,
    );
  });

  it("the README says so", () => {
    const readme = read("README.md");
    expect(readme).toContain("**Projects are per Mac.**");
    expect(readme).toMatch(
      /same folder used on two Macs is listed as two\s+projects/,
    );
  });
});
