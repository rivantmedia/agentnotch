/**
 * Which accounts the usage page adds up, kept in `?accounts=` as account keys joined by commas,
 * so a view can be reloaded or shared. No parameter means every account the viewer sees (and any
 * they come to see later); an empty one means none. Pure, so the page, its client components and
 * the tests read it the same way.
 */

export const ACCOUNTS_PARAM = "accounts";

/** Every account the viewer sees (null), or these keys, sorted and without repeats. */
export type AccountSelection = null | readonly string[];

/** The most accounts a selection names; the API takes no more (routers/inputs.ts). */
export const MAX_SELECTED_ACCOUNTS = 100;

const ACCOUNT_KEY = /^[0-9a-f]{64}$/;

/** The selection a page's `?accounts=` names. Anything that isn't an account key is dropped. */
export function selectionFromParam(
  value: string | string[] | null | undefined,
): AccountSelection {
  const first = Array.isArray(value) ? value[0] : value;
  if (first === undefined || first === null) return null;
  const keys = first
    .split(",")
    .map((key) => key.trim().toLowerCase())
    .filter((key) => ACCOUNT_KEY.test(key));
  return [...new Set(keys)].sort().slice(0, MAX_SELECTED_ACCOUNTS);
}

/** The `?accounts=` value for a selection; null leaves the parameter out (every account). */
export function selectionParam(selection: AccountSelection): string | null {
  return selection === null ? null : selection.join(",");
}

/**
 * The accounts a selection picks from the ones the viewer sees, in their order. Keys they don't
 * see (an old link, a pool they left) pick nothing.
 */
export function selectedAccounts<A extends { key: string }>(
  accounts: readonly A[],
  selection: AccountSelection,
): A[] {
  if (selection === null) return [...accounts];
  const chosen = new Set(selection);
  return accounts.filter((account) => chosen.has(account.key));
}

/**
 * A selection of `keys` among the visible ones: sorted, the keys the viewer doesn't see left
 * out, and every visible account written as null, so a selection of them all keeps up with
 * accounts added later.
 */
export function normalizeSelection(
  keys: readonly string[],
  visibleKeys: readonly string[],
): AccountSelection {
  const visible = new Set(visibleKeys);
  const kept = [...new Set(keys)].filter((key) => visible.has(key)).sort();
  return kept.length === visible.size ? null : kept;
}

/** The selection with one account switched on or off. */
export function toggleAccount(
  selection: AccountSelection,
  key: string,
  visibleKeys: readonly string[],
): AccountSelection {
  const current = selection ?? visibleKeys;
  const next = current.includes(key)
    ? current.filter((k) => k !== key)
    : [...current, key];
  return normalizeSelection(next, visibleKeys);
}
