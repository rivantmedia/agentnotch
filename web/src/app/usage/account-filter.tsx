"use client";

import { useSearchParams } from "next/navigation";
import {
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import {
  ACCOUNTS_PARAM,
  selectedAccounts,
  selectionFromParam,
  selectionParam,
  toggleAccount,
  type AccountSelection,
} from "~/lib/account-selection";
import { accountSubtitle, accountTitle } from "~/lib/format";

import { cx } from "../_components/ui";

/**
 * The accounts the usage page adds up, kept in `?accounts=` like the period (usePeriod): the
 * address bar is the source of truth and `initial` (the server's reading of it) only a fallback.
 * The page's data follows a render behind (`shown`), so while a new selection loads the old
 * figures stay, faded, instead of flashing back to skeletons.
 */
export function useAccountSelection(initial: AccountSelection) {
  const params = useSearchParams();
  // The selection as its parameter (null for every account), so equal selections compare equal.
  const fromUrl = params
    ? selectionParam(selectionFromParam(params.get(ACCOUNTS_PARAM)))
    : selectionParam(initial);
  const [value, setValue] = useState(fromUrl);
  const [seen, setSeen] = useState(fromUrl);
  if (fromUrl !== seen) {
    // As in usePeriod: never show a selection the address doesn't.
    setSeen(fromUrl);
    setValue(fromUrl);
  }
  const shownValue = useDeferredValue(value);
  const selection = useMemo(() => selectionFromParam(value), [value]);
  const shown = useMemo(() => selectionFromParam(shownValue), [shownValue]);
  const choose = useCallback((next: AccountSelection) => {
    const param = selectionParam(next);
    setValue(param);
    const url = new URL(window.location.href);
    if (param === null) url.searchParams.delete(ACCOUNTS_PARAM);
    else url.searchParams.set(ACCOUNTS_PARAM, param);
    window.history.replaceState(null, "", url);
  }, []);
  return {
    selection,
    shown,
    /** What the data below is keyed by (resets a failed section when it changes). */
    shownKey: shownValue,
    stale: shownValue !== value,
    choose,
  };
}

type Account = {
  key: string;
  label: string | null;
  email: string | null;
  organizationName: string | null;
};

/**
 * One pill per account the viewer sees, each a checkbox: the checked ones are added up. "All
 * accounts" first, checked when every one is, mixed when some are; it checks them all, or,
 * when they all are, clears them.
 */
export function AccountPills({
  accounts,
  selection,
  onChange,
}: {
  accounts: readonly Account[];
  selection: AccountSelection;
  onChange: (next: AccountSelection) => void;
}) {
  const visibleKeys = accounts.map((a) => a.key);
  const chosen = new Set(
    selectedAccounts(accounts, selection).map((a) => a.key),
  );
  const all = chosen.size === accounts.length;
  const some = chosen.size > 0 && !all;

  return (
    <fieldset className="min-w-0">
      <legend className="sr-only">Accounts to add up</legend>
      <div className="flex flex-wrap gap-2">
        <Pill
          checked={all}
          mixed={some}
          onChange={() => onChange(all ? [] : null)}
        >
          All accounts
        </Pill>
        {accounts.map((account) => (
          <Pill
            key={account.key}
            checked={chosen.has(account.key)}
            title={accountSubtitle(account) ?? undefined}
            onChange={() =>
              onChange(toggleAccount(selection, account.key, visibleKeys))
            }
          >
            {accountTitle(account)}
          </Pill>
        ))}
      </div>
    </fieldset>
  );
}

function Pill({
  checked,
  mixed = false,
  title,
  onChange,
  children,
}: {
  checked: boolean;
  /** Some of what it stands for is checked (the "all" pill). */
  mixed?: boolean;
  title?: string;
  onChange: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = mixed;
  }, [mixed]);
  return (
    <label
      title={title}
      className={cx(
        "inline-flex max-w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-full border px-3 py-1 text-sm font-medium transition-colors select-none",
        "has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-focus",
        // Filled like the period picker's choice, so a checked pill stands out by more than hue.
        checked
          ? "border-accent bg-accent text-on-accent"
          : "border-edge bg-surface text-ink-2 hover:bg-surface-2 hover:text-ink",
      )}
    >
      <input
        ref={ref}
        type="checkbox"
        checked={checked}
        onChange={onChange}
        className="sr-only"
      />
      <svg
        viewBox="0 0 12 12"
        aria-hidden="true"
        className="size-3 shrink-0"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.75"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        {checked ? (
          <path d="M2.5 6.25 5 8.5l4.5-5" />
        ) : mixed ? (
          <path d="M3 6h6" />
        ) : (
          <circle cx="6" cy="6" r="3.5" strokeWidth="1.25" />
        )}
      </svg>
      <span className="truncate">{children}</span>
    </label>
  );
}
