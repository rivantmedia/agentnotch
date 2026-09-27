/**
 * Account page copy that states a rule of the data, kept here so a test holds it to that rule.
 */

/**
 * Project keys are made with a secret each Mac keeps to itself (contract/README.md), so the
 * website can't tell that two Macs worked in the same folder. The table groups each person's
 * projects by folder name instead (services/projects.ts), and says so.
 */
export const PROJECTS_DESCRIPTION =
  "Each project folder this account worked in, with its totals for all time, by name only. A person's folders of the same name are one row, from all their Macs together: the website can't tell whether they are the same folder, so it adds them up and counts the Macs. Select one to see its sessions.";

/**
 * Usage limits are per account and nothing reports them per project, so the breakdown by
 * project shares out the tokens, and says so. A project opens on every account the viewer sees
 * it on, which for a pool member's project is only the accounts they share (project-usage.ts).
 */
export const PROJECT_USAGE_DESCRIPTION =
  "Which projects this account's tokens went to, by the sessions started in the period. Limits are per account, so these are shares of the tokens, not of a limit. Open a project to see its usage on every account you can see it on.";
