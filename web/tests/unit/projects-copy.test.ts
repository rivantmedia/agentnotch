/**
 * Project keys are per install (contract/README.md), so the website stores a project per Mac and
 * groups them by folder name for display. Where projects are listed, and in the README, the site
 * says so rather than letting two different folders of one name look like one by accident.
 */
import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { PROJECTS_DESCRIPTION } from "~/app/accounts/[key]/copy";

const web = path.resolve(import.meta.dirname, "../..");
const read = (file: string) => readFileSync(path.join(web, file), "utf8");

describe("projects are grouped by name across Macs", () => {
  it("the projects table's description says so", () => {
    expect(PROJECTS_DESCRIPTION).toMatch(/same name are one row/);
    expect(PROJECTS_DESCRIPTION).toMatch(/all their Macs/);
    expect(PROJECTS_DESCRIPTION).toMatch(/counts the Macs/);
    // The old rule is gone.
    expect(PROJECTS_DESCRIPTION).not.toMatch(/listed once for each/);
    // …and it is the description the table is shown with, which has a Macs column.
    const source = read("src/app/accounts/[key]/account-activity.tsx");
    expect(source).toMatch(
      /title="Projects"\s+description=\{PROJECTS_DESCRIPTION\}/,
    );
    expect(source).toMatch(/>\s*Macs\s*</);
  });

  it("the README says so", () => {
    const readme = read("README.md");
    expect(readme).toContain("**Projects are grouped by name.**");
    expect(readme).toMatch(/the project filter picks the whole\s+group/);
    expect(readme).not.toMatch(/listed as two\s+projects/);
  });
});
