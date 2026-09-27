/**
 * Project keys are per install (contract/README.md), so the website stores a project per Mac and
 * groups them by folder name for display. Where projects are listed, and in the README, the site
 * says so rather than letting two different folders of one name look like one by accident.
 */
import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import {
  PROJECT_USAGE_DESCRIPTION,
  PROJECTS_DESCRIPTION,
} from "~/app/accounts/[key]/copy";
import {
  PROJECT_GROUPING_NOTE,
  PROJECTS_PAGE_DESCRIPTION,
} from "~/app/projects/copy";

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

describe("usage by project", () => {
  it("is grouped across Macs and accounts, and the project pages say so", () => {
    expect(PROJECT_GROUPING_NOTE).toMatch(/one person's folders of one name/);
    expect(PROJECT_GROUPING_NOTE).toMatch(/all their Macs and Claude accounts/);
    expect(PROJECT_GROUPING_NOTE).toMatch(
      /can't tell whether they are the same folder/,
    );
    for (const file of [
      "src/app/projects/(list)/projects-view.tsx",
      "src/app/projects/[id]/project-view.tsx",
    ]) {
      expect(read(file)).toContain("{PROJECT_GROUPING_NOTE}");
    }
  });

  it("says which period each list covers", () => {
    // /projects lists the projects with sessions in the period, not every project ever synced.
    expect(PROJECTS_PAGE_DESCRIPTION).toMatch(/sessions started in the period/);
    expect(PROJECTS_PAGE_DESCRIPTION).not.toMatch(/^Every project/);
    expect(read("src/app/projects/(list)/projects-view.tsx")).toContain(
      "description={PROJECTS_PAGE_DESCRIPTION}",
    );
    expect(PROJECT_USAGE_DESCRIPTION).toMatch(/sessions started in the period/);
    // The account's projects table sits under the period's breakdown and is all time.
    expect(PROJECTS_DESCRIPTION).toMatch(/for all time/);
  });

  it("opens a project on the accounts the viewer can see it on", () => {
    // A pool member's project shows on the accounts they share, never on "your" other accounts.
    expect(PROJECT_USAGE_DESCRIPTION).toMatch(
      /every account you can see it on/,
    );
    expect(PROJECT_USAGE_DESCRIPTION).not.toMatch(/your other accounts/);
  });

  it("shares out tokens, never a limit, and says so", () => {
    // Limits are per account; nothing reports them per project.
    for (const copy of [PROJECT_USAGE_DESCRIPTION, PROJECT_GROUPING_NOTE]) {
      expect(copy).toMatch(/shares? of the tokens, not of a limit/);
    }
    expect(read("src/app/accounts/[key]/account-activity.tsx")).toMatch(
      /title="Usage by project"\s+description=\{PROJECT_USAGE_DESCRIPTION\}/,
    );
    expect(read("src/app/dashboard/dashboard-view.tsx")).toMatch(
      /shares of the tokens, not of a limit/,
    );
  });

  it("the README says so", () => {
    const readme = read("README.md");
    expect(readme).toContain("**Usage by project.**");
    expect(readme).toMatch(/across\s+accounts/);
    expect(readme).toMatch(/`\/projects\/<id>`/);
  });
});
