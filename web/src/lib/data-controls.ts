/**
 * What someone types to confirm removing their data on /settings. The server takes nothing else
 * (src/server/api/routers/my-data.ts), so a stray request can't remove anything either.
 */
export const REMOVE_SUMMARIES_CONFIRMATION = "remove summaries";
export const DELETE_DATA_CONFIRMATION = "delete my data";

/** Whether `typed` confirms `phrase`: case and surrounding spaces don't matter. */
export function confirmsPhrase(typed: string, phrase: string): boolean {
  return typed.trim().toLowerCase() === phrase;
}
