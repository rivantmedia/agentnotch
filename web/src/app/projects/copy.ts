/**
 * Project page copy that states a rule of the data, kept here so a test holds it to that rule.
 */

/**
 * Project keys are made with a secret each Mac keeps to itself and with the account's key
 * (contract/README.md), so the website can't tell that two Macs or two accounts worked in the
 * same folder. The project pages group a person's folders by name across both
 * (services/project-usage.ts), and say so.
 */
export const PROJECT_GROUPING_NOTE =
  "A project is one person's folders of one name, from all their Macs and Claude accounts together: the website can't tell whether they are the same folder, so it adds them up. Limits are per account, so a project's usage is its share of the tokens, not of a limit.";

/**
 * The list holds the projects with sessions in the period picked (usage by project), not every
 * project ever synced, and says so.
 */
export const PROJECTS_PAGE_DESCRIPTION =
  "The projects your Claude accounts worked in, by the sessions started in the period, with the tokens each used and the accounts it used them on. Open one to see its usage by account and its sessions.";
