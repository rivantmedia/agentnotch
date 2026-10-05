"use client";

import Link from "next/link";

import {
  accountSubtitle,
  accountTitle,
  formatCost,
  formatExact,
  formatTokens,
  plural,
  usageSourceLabel,
} from "~/lib/format";
import { withPeriod, type UsagePeriod } from "~/lib/usage-period";
import { api, type RouterOutputs } from "~/trpc/react";

import { ConnectMacSteps } from "../_components/connect-mac";
import { ProjectUsageList } from "../_components/project-usage-list";
import { QueryBoundary } from "../_components/query-boundary";
import { SessionList, SessionListSkeleton } from "../_components/session-list";
import { RelativeTime } from "../_components/time";
import {
  Badge,
  LoadingBlock,
  PageHeader,
  PeopleIcon,
  SectionHeading,
  Skeleton,
} from "../_components/ui";
import {
  PeriodPicker,
  usePeriod,
  UsageBreakdownSkeleton,
} from "../_components/usage-breakdown";
import { UsageMeter } from "../_components/usage-meter";
import { dashboardProjectsInput, RECENT_SESSIONS_INPUT } from "./queries";

type AccountSummary = RouterOutputs["accounts"]["list"][number];

export function DashboardView({
  siteAddress,
  period,
}: {
  siteAddress: string | null;
  /** The usage-by-project period the address bar asked for. */
  period: UsagePeriod;
}) {
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-10 px-4 py-8 sm:py-10">
      <PageHeader
        title="Dashboard"
        description="Your Claude accounts, from every Mac you sync and every pool you're in."
        actions={
          <Link
            href={withPeriod("/usage", period)}
            className="btn btn-secondary btn-sm"
          >
            Usage across accounts
          </Link>
        }
      />
      <QueryBoundary
        what="your accounts"
        fallback={
          <LoadingBlock label="Loading your accounts…">
            <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
              {[0, 1, 2].map((i) => (
                <AccountCardSkeleton key={i} />
              ))}
            </div>
          </LoadingBlock>
        }
      >
        <Accounts siteAddress={siteAddress} period={period} />
      </QueryBoundary>
    </div>
  );
}

function Accounts({
  siteAddress,
  period,
}: {
  siteAddress: string | null;
  period: UsagePeriod;
}) {
  const [accounts] = api.accounts.list.useSuspenseQuery();

  if (accounts.length === 0) {
    return (
      <section
        aria-labelledby="connect-title"
        className="card flex flex-col gap-5 p-6 sm:p-8"
      >
        <div className="flex flex-col gap-2">
          <h2 id="connect-title" className="text-lg font-semibold">
            Connect your Mac to see your accounts here
          </h2>
          <p className="max-w-2xl text-sm text-ink-2">
            Nothing has synced yet. Agent Notch sends each Claude account&apos;s
            usage and sessions from your Mac once you turn on sync. Names and
            numbers only: no file paths, no prompts.
          </p>
        </div>
        <ConnectMacSteps address={siteAddress} />
        <p className="text-sm text-ink-2">
          Someone already shares an account with you?{" "}
          <Link href="/pools" className="link">
            Join their pool with a code
          </Link>
          .
        </p>
      </section>
    );
  }

  return (
    <>
      <section aria-labelledby="accounts-title" className="flex flex-col gap-4">
        <h2 id="accounts-title" className="sr-only">
          Accounts
        </h2>
        <ul className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          {accounts.map((account) => (
            // min-w-0: a grid item is otherwise as wide as its longest title, which truncates
            // only once the card may be narrower than it (a long email on a phone).
            <li key={account.key} className="flex min-w-0">
              <AccountCard account={account} />
            </li>
          ))}
        </ul>
      </section>
      <ProjectUsageSection accounts={accounts} initialPeriod={period} />
      <section aria-labelledby="recent-title" className="flex flex-col gap-4">
        <SectionHeading
          id="recent-title"
          title="Recent sessions"
          description="The newest sessions across all your accounts. Open an account for its full history."
        />
        <div className="card p-5">
          <QueryBoundary
            what="recent sessions"
            fallback={
              <LoadingBlock label="Loading recent sessions…">
                <SessionListSkeleton rows={3} />
              </LoadingBlock>
            }
          >
            <RecentSessions accounts={accounts} />
          </QueryBoundary>
        </div>
      </section>
    </>
  );
}

function AccountCard({ account }: { account: AccountSummary }) {
  const title = accountTitle(account);
  const subtitle = accountSubtitle(account);
  const latest = account.usage.reduce<AccountSummary["usage"][number] | null>(
    (newest, reading) =>
      !newest || reading.observedAt > newest.observedAt ? reading : newest,
    null,
  );
  const week = account.last7Days;
  const cost = formatCost(week.costUsd);

  return (
    <article className="card relative flex w-full flex-col gap-5 p-5 transition-colors hover:border-edge">
      <header className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="truncate font-semibold">
            <Link
              href={`/accounts/${account.key}`}
              className="rounded-sm after:absolute after:inset-0 after:rounded-xl"
            >
              {title}
            </Link>
          </h3>
          {subtitle ? (
            <p className="truncate text-sm text-ink-2" title={subtitle}>
              {subtitle}
            </p>
          ) : null}
          {!account.syncedByViewer ? (
            <p className="text-sm text-ink-2">Shared with you</p>
          ) : null}
        </div>
        <div className="flex shrink-0 flex-wrap justify-end gap-1.5">
          {account.plan ? <Badge>{account.plan}</Badge> : null}
          {account.pooled ? (
            <Badge
              tone="accent"
              title={`Pooled: this card includes the sessions and usage of ${plural(account.memberCount, "person", "people")}.`}
            >
              <PeopleIcon />
              Shared · {account.memberCount}
            </Badge>
          ) : null}
        </div>
      </header>

      <div className="flex flex-col gap-3.5">
        {account.usage.length === 0 ? (
          <p className="text-sm text-ink-2">
            No usage readings in the last 35 days.
          </p>
        ) : (
          account.usage.map((reading) => (
            <UsageMeter key={reading.windowId} reading={reading} compact />
          ))
        )}
        {latest ? (
          <p className="text-xs text-ink-3">
            Latest reading from {usageSourceLabel(latest.source)},{" "}
            <RelativeTime date={latest.observedAt} />
          </p>
        ) : null}
      </div>

      <dl className="mt-auto grid grid-cols-3 gap-3 border-t border-line pt-4">
        <div className="min-w-0">
          <dt className="text-xs text-ink-2">Sessions, 7 days</dt>
          <dd className="text-lg font-semibold">
            {formatExact(week.sessions)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="text-xs text-ink-2">Tokens, 7 days</dt>
          <dd
            className="text-lg font-semibold"
            title={`${formatExact(week.tokens.total)} tokens`}
          >
            {formatTokens(week.tokens.total)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="text-xs text-ink-2">Cost, 7 days</dt>
          <dd className="text-lg font-semibold">
            {cost ?? <span className="text-ink-3">–</span>}
          </dd>
        </div>
      </dl>
      <p className="-mt-2 text-xs text-ink-3">
        {account.lastActivityAt ? (
          <>
            Last session activity <RelativeTime date={account.lastActivityAt} />
          </>
        ) : (
          "No sessions yet"
        )}
      </p>
    </article>
  );
}

function AccountCardSkeleton() {
  return (
    <div className="card flex flex-col gap-5 p-5" aria-hidden="true">
      <div className="flex justify-between gap-3">
        <div className="flex flex-col gap-2">
          <Skeleton className="h-4 w-36" />
          <Skeleton className="h-3 w-48" />
        </div>
        <Skeleton className="h-5 w-16 rounded-full" />
      </div>
      {[0, 1].map((i) => (
        <div key={i} className="flex flex-col gap-2">
          <Skeleton className="h-3 w-full" />
          <Skeleton className="h-2 w-full rounded-full" />
        </div>
      ))}
      <div className="grid grid-cols-3 gap-3 border-t border-line pt-4">
        {[0, 1, 2].map((i) => (
          <Skeleton key={i} className="h-9" />
        ))}
      </div>
    </div>
  );
}

/** Which projects the accounts' tokens went to, across every account. */
function ProjectUsageSection({
  accounts,
  initialPeriod,
}: {
  accounts: AccountSummary[];
  initialPeriod: UsagePeriod;
}) {
  const { period, shown, stale, choose } = usePeriod(initialPeriod);
  const names = new Map(accounts.map((a) => [a.key, accountTitle(a)]));

  return (
    <section
      aria-labelledby="project-usage-title"
      className="flex flex-col gap-4"
    >
      <SectionHeading
        id="project-usage-title"
        title="Usage by project"
        description="The tokens each project used, across all your accounts, by the sessions started in the period. Limits are per account, so these are shares of the tokens, not of a limit."
        actions={
          <PeriodPicker
            value={period}
            onChange={choose}
            label="Usage by project over"
          />
        }
      />
      <div className="card p-5">
        <QueryBoundary
          what="usage by project"
          fallback={
            <LoadingBlock label="Loading usage by project…">
              <UsageBreakdownSkeleton />
            </LoadingBlock>
          }
          resetKeys={[shown]}
        >
          <ProjectUsageList
            input={dashboardProjectsInput(shown)}
            stale={stale}
            accountName={
              accounts.length > 1
                ? (key) => names.get(key) ?? "Claude account"
                : undefined
            }
            showOwner={accounts.some((a) => a.pooled)}
            footer={
              <Link href={withPeriod("/projects", shown)} className="link">
                All projects
              </Link>
            }
          />
        </QueryBoundary>
      </div>
    </section>
  );
}

function RecentSessions({ accounts }: { accounts: AccountSummary[] }) {
  const [sessions] = api.sessions.list.useSuspenseQuery(RECENT_SESSIONS_INPUT);
  const names = new Map(accounts.map((a) => [a.key, accountTitle(a)]));

  if (sessions.items.length === 0) {
    return (
      <p className="text-sm text-ink-2">
        No sessions yet. They appear here after Claude Code runs on a Mac that
        syncs.
      </p>
    );
  }
  return (
    <SessionList
      items={sessions.items}
      accountName={(key) => names.get(key) ?? "Claude account"}
      showOwner={accounts.some((a) => a.pooled)}
    />
  );
}
