/**
 * Settings copy that states a rule of the data, kept here so a test holds it to that rule.
 */

/**
 * What happens after "Delete all my synced data". The Mac app keeps syncing while its switch is
 * on; it remembers what it already sent, so what it sends again is what is new or changed.
 */
export const SYNC_AGAIN_NOTE =
  "Unless sync is turned off in the Mac app (Settings > Claude Code > Cloud), the app sends its data again on its next sync: new usage readings, and sessions that are new or have changed since. Signing in again later starts empty, apart from what a Mac sends after this.";
