"use client";

import Link from "next/link";

import {
  type AccountSelection,
  selectedAccounts,
} from "~/lib/account-selection";
import {
  accountTitle,
  formatCost,
  formatExact,
  formatTokens,
  isFixedWindow,
  plural,
} from "~/lib/format";
import { periodPhrase, withPeriod, type UsagePeriod } from "~/lib/usage-period";
import { api, type RouterOutputs } from "~/trpc/react";

import { ProjectUsageList } from "../_components/project-usage-list";
import { QueryBoundary } from "../_components/query-boundary";
import { RelativeTime } from "../_components/time";
import {
  cx,
  LoadingBlock,
  PageHeader,
  SectionHeading,
  Skeleton,
  Stat,
} from "../_components/ui";
import {
  PeriodPicker,
  usePeriod,
  UsageBreakdown,
  UsageBreakdownSkeleton,
  type BreakdownRow,
} from "../_components/usage-breakdown";
import { UsageMeter } from "../_components/usage-meter";
import { AccountPills, useAccountSelection } from "./account-filter";
import {
  asksForAccounts,
  combinedUsageInput,
  usageProjectsInput,
} from "./queries";
import { UsageOverTime } from "./usage-over-time";

type Account = RouterOutputs["accounts"]["list"][number];

/**
 * Usage and cost across accounts on one page: pick the period and the accounts to add up (pills,
 * kept in the address bar with the period), and every figure below follows. Limits are the one
 * thing that never adds up: each account's stays its own.
 */
export function UsageView({
  period,
  selection,
}: {
  /** The period the address bar asked for. */
  period: UsagePeriod;
  /** The accounts the address bar asked for. */
  selection: AccountSelection;
}) {
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-8 px-4 py-8 sm:py-10">
      <PageHeader
        title="Usage"
        description="What your Claude accounts used and cost, added up. Pick the period and which accounts to combine."
      />
      <QueryBoundary
        what="your accounts"
        fallback={
          <LoadingBlock label="Loading your accounts…">
            <div className="flex flex-wrap gap-2" aria-hidden="true">
              {[0, 1, 2].map((i) => (
                <Skeleton key={i} className="h-8 w-32 rounded-full" />
              ))}
            </div>
          </LoadingBlock>
        }
      >
        <Combined initialPeriod={period} initialSelection={selection} />
      </QueryBoundary>
    </div>
  );
}

function Combined({
  initialPeriod,
  initialSelection,
}: {
  initialPeriod: UsagePeriod;
  initialSelection: AccountSelection;
}) {
  const [accounts] = api.accounts.list.useSuspenseQuery();
  const period = usePeriod(initialPeriod);
  const picked = useAccountSelection(initialSelection);

  if (accounts.length === 0) {
    return (
      <p className="card p-5 text-sm text-ink-2">
        Nothing has synced yet.{" "}
        <Link href="/dashboard" className="link">
          Connect your Mac
        </Link>{" "}
        to see your usage here.
      </p>
    );
  }

  const chosen = selectedAccounts(accounts, picked.selection);
  const names = new Map(accounts.map((a) => [a.key, accountTitle(a)]));
  const accountName = (key: string) => names.get(key) ?? "Claude account";
  // The data follows the selection a render behind. Coming back from none, the one behind is
  // empty, which asks for nothing: go straight to the new one then.
  const selection = asksForAccounts(picked.shown)
    ? picked.shown
    : picked.selection;
  const shownPeriod = period.shown;
  const stale = period.stale || picked.stale;
  const resetKeys = [shownPeriod, picked.shownKey];
  const shownAccounts = selectedAccounts(accounts, selection);

  return (
    <>
      {/* The filters, date range first, scope everything below them. */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:gap-4">
        <div className="shrink-0">
          <PeriodPicker
            value={period.period}
            onChange={period.choose}
            label="Usage over"
          />
        </div>
        <AccountPills
          accounts={accounts}
          selection={picked.selection}
          onChange={picked.choose}
        />
      </div>

      {chosen.length === 0 ? (
        <div className="card flex flex-col items-start gap-3 p-5 text-sm text-ink-2">
          <p>No accounts selected. Pick at least one to see its usage.</p>
          <button
            type="button"
            className="btn btn-secondary btn-sm"
            onClick={() => picked.choose(null)}
          >
            Show all accounts
          </button>
        </div>
      ) : (
        <>
          <QueryBoundary
            what="the totals"
            fallback={
              <LoadingBlock label="Loading the totals…">
                <TotalsSkeleton />
              </LoadingBlock>
            }
            resetKeys={resetKeys}
          >
            <Totals
              input={combinedUsageInput(shownPeriod, selection)}
              stale={stale}
            />
          </QueryBoundary>

          <UsageOverTime
            period={shownPeriod}
            selection={selection}
            selectionKey={picked.shownKey}
            stale={stale}
            accountName={accountName}
          />

          <section
            aria-labelledby="by-account-title"
            className="flex flex-col gap-4"
          >
            <SectionHeading
              id="by-account-title"
              title="By account"
              description="Each selected account's share of the tokens, with what it cost, by the sessions started in the period. Open one for its own page."
            />
            <div className="card p-5">
              <QueryBoundary
                what="usage by account"
                fallback={
                  <LoadingBlock label="Loading usage by account…">
                    <UsageBreakdownSkeleton rows={Math.min(chosen.length, 4)} />
                  </LoadingBlock>
                }
                resetKeys={resetKeys}
              >
                <ByAccount
                  input={combinedUsageInput(shownPeriod, selection)}
                  accounts={accounts}
                  stale={stale}
                />
              </QueryBoundary>
            </div>
          </section>

          <Limits accounts={chosen} />

          <section
            aria-labelledby="usage-projects-title"
            className="flex flex-col gap-4"
          >
            <SectionHeading
              id="usage-projects-title"
              title="Usage by project"
              description="The tokens each project used on the selected accounts, by the sessions started in the period."
            />
            <div className="card p-5">
              <QueryBoundary
                what="usage by project"
                fallback={
                  <LoadingBlock label="Loading usage by project…">
                    <UsageBreakdownSkeleton />
                  </LoadingBlock>
                }
                resetKeys={resetKeys}
              >
                <ProjectUsageList
                  input={usageProjectsInput(shownPeriod, selection)}
                  stale={stale}
                  accountName={
                    shownAccounts.length > 1 ? accountName : undefined
                  }
                  showOwner={shownAccounts.some((a) => a.pooled)}
                  footer={
                    <Link
                      href={withPeriod("/projects", shownPeriod)}
                      className="link"
                    >
                      All projects
                    </Link>
                  }
                />
              </QueryBoundary>
            </div>
          </section>
        </>
      )}
    </>
  );
}

function TotalsSkeleton() {
  return (
    <div
      className="card grid grid-cols-2 gap-5 p-5 sm:grid-cols-4"
      aria-hidden="true"
    >
      {[0, 1, 2, 3].map((i) => (
        <div key={i} className="flex flex-col gap-2">
          <Skeleton className="h-3 w-16" />
          <Skeleton className="h-6 w-24" />
          <Skeleton className="h-3 w-28" />
        </div>
      ))}
    </div>
  );
}

/** The period on the selected accounts, added up. */
function Totals({
  input,
  stale,
}: {
  input: ReturnType<typeof combinedUsageInput>;
  stale: boolean;
}) {
  const [usage] = api.usage.combined.useSuspenseQuery(input);
  const { total } = usage;
  const cost = formatCost(total.costUsd);
  const perDay =
    total.costUsd !== null && usage.averageDays > 0
      ? formatCost(total.costUsd / usage.averageDays)
      : null;
  const lastUsedAt = usage.accounts.reduce<Date | null>(
    (latest, a) =>
      a.lastUsedAt !== null && (latest === null || a.lastUsedAt > latest)
        ? a.lastUsedAt
        : latest,
    null,
  );
  const accounts = plural(usage.accountKeys.length, "account", "accounts");

  return (
    <section
      aria-labelledby="totals-title"
      aria-busy={stale || undefined}
      className={cx(
        "card flex flex-col gap-4 p-5 transition-opacity",
        stale && "opacity-60",
      )}
    >
      <h2 id="totals-title" className="sr-only">
        Totals
      </h2>
      <dl className="grid grid-cols-2 gap-5 sm:grid-cols-4">
        <Stat
          label="Cost"
          value={cost ?? "–"}
          detail={
            cost
              ? `${accounts}, ${periodPhrase(usage.period)}`
              : "No cost estimate"
          }
        />
        <Stat
          label="Tokens"
          value={formatTokens(total.tokens.total)}
          title={`${formatExact(total.tokens.total)} tokens`}
          detail={`${formatTokens(total.tokens.cacheRead)} of them cache reads`}
        />
        <Stat
          label="Sessions"
          value={formatExact(total.sessions)}
          detail={
            lastUsedAt ? (
              <>
                Last activity <RelativeTime date={lastUsedAt} />
              </>
            ) : (
              "None yet"
            )
          }
        />
        <Stat
          label="Cost per day"
          value={perDay ?? "–"}
          detail={
            usage.averageDays > 0
              ? `Averaged over ${plural(usage.averageDays, "day", "days")}`
              : "No sessions in this period"
          }
        />
      </dl>
      <div className="flex flex-col gap-1 border-t border-line pt-4 text-xs text-ink-3">
        <p className="tabular-nums">
          <span className="sr-only">Tokens: </span>
          Input {formatTokens(total.tokens.input)} · Output{" "}
          {formatTokens(total.tokens.output)} · Cache write{" "}
          {formatTokens(total.tokens.cacheCreation)} · Cache read{" "}
          {formatTokens(total.tokens.cacheRead)}
        </p>
        <p>
          Costs are Claude Code&apos;s own figures, else worked out the same way
          at API list prices: what the tokens would cost through the API, not
          what a subscription bills.
          {total.sessions === 0 && usage.earlierSessions
            ? " Pick a longer period to see older usage."
            : ""}
        </p>
      </div>
    </section>
  );
}

/** Each selected account's part of the period, most tokens first. */
function ByAccount({
  input,
  accounts,
  stale,
}: {
  input: ReturnType<typeof combinedUsageInput>;
  accounts: readonly Account[];
  stale: boolean;
}) {
  const [usage] = api.usage.combined.useSuspenseQuery(input);
  const byKey = new Map(accounts.map((a) => [a.key, a]));

  if (usage.accounts.length === 0) {
    return (
      <p className="text-sm text-ink-2">
        None of these accounts is one you can see any more.
      </p>
    );
  }

  const rows: BreakdownRow[] = usage.accounts.map((part) => {
    const account = byKey.get(part.accountKey);
    const notes = [
      account?.plan ?? null,
      account?.pooled
        ? `Shared with ${plural(account.memberCount - 1, "person", "people")}`
        : account && !account.syncedByViewer
          ? "Shared with you"
          : null,
    ].filter((note): note is string => note !== null);
    return {
      key: part.accountKey,
      label: (
        <Link
          href={withPeriod(`/accounts/${part.accountKey}`, usage.period)}
          className="link"
        >
          {account ? accountTitle(account) : "Claude account"}
        </Link>
      ),
      detail: notes.length > 0 ? notes.join(" · ") : undefined,
      totals: part,
      lastUsedAt: part.lastUsedAt,
    };
  });

  return (
    <UsageBreakdown
      rows={rows}
      whole={usage.total.tokens.total}
      label={`Usage by account, ${periodPhrase(usage.period)}`}
      dimmed={stale}
    />
  );
}

/**
 * Each selected account's latest 5-hour, weekly and extra-usage readings, side by side. A limit
 * is a share of one account's own plan, so they are never added up, and the period doesn't
 * apply: these are where each account stands now.
 */
function Limits({ accounts }: { accounts: readonly Account[] }) {
  return (
    <section aria-labelledby="limits-title" className="flex flex-col gap-4">
      <SectionHeading
        id="limits-title"
        title="Usage limits"
        description="Where each selected account stands now. Each limit is a share of that account's own plan, so they don't add up across accounts, and the period doesn't apply."
      />
      <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {accounts.map((account) => {
          const meters = account.usage.filter((r) => isFixedWindow(r.windowId));
          return (
            // min-w-0: a grid item is otherwise as wide as its longest title.
            <li key={account.key} className="flex min-w-0">
              <article className="card flex w-full flex-col gap-3.5 p-5">
                <h3 className="truncate text-sm font-semibold">
                  <Link href={`/accounts/${account.key}`} className="link">
                    {accountTitle(account)}
                  </Link>
                </h3>
                {meters.length === 0 ? (
                  <p className="text-sm text-ink-2">
                    No usage readings in the last 35 days.
                  </p>
                ) : (
                  meters.map((reading) => (
                    <UsageMeter
                      key={reading.windowId}
                      reading={reading}
                      compact
                    />
                  ))
                )}
              </article>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
