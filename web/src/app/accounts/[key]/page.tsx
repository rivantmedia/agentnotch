import { type Metadata } from "next";
import Link from "next/link";
import { notFound } from "next/navigation";

import {
  accountSubtitle,
  accountTitle,
  formatCost,
  formatExact,
  formatTokens,
  plural,
} from "~/lib/format";
import { periodFromParam } from "~/lib/usage-period";
import { isTRPCCode, requireViewer } from "~/server/pages";
import { api, HydrateClient } from "~/trpc/server";

import { RelativeTime } from "../../_components/time";
import { Badge, PeopleIcon, Stat } from "../../_components/ui";
import { AccountActivity } from "./account-activity";
import {
  filterFromParams,
  isAccountKey,
  projectUsageInput,
  sessionsInput,
  usageFrom,
  usageInput,
} from "./queries";
import { UsageSection } from "./usage-section";

export const metadata: Metadata = { title: "Account" };

export default async function AccountPage({
  params,
  searchParams,
}: {
  params: Promise<{ key: string }>;
  searchParams: Promise<{
    project?: string | string[];
    member?: string | string[];
    period?: string | string[];
  }>;
}) {
  // layout.tsx already answered 404 for a key the viewer can't see, before anything streamed.
  // This render sits below loading.tsx, so a notFound() here comes after a 200; it only covers
  // access lost in between.
  const { key } = await params;
  if (!isAccountKey(key)) notFound();
  await requireViewer(`/accounts/${key}`);

  let account: Awaited<ReturnType<typeof api.accounts.get>>;
  try {
    account = await api.accounts.get({ accountKey: key });
  } catch (error) {
    // Not visible reads the same as missing, so an account's existence doesn't leak.
    if (isTRPCCode(error, "NOT_FOUND")) notFound();
    throw error;
  }

  const query = await searchParams;
  const filter = filterFromParams(query);
  const period = periodFromParam(query.period);
  const fromMs = usageFrom(Date.now());
  void api.usage.history.prefetch(usageInput(key, fromMs));
  void api.projects.usage.prefetch(projectUsageInput(key, period));
  void api.projects.list.prefetch({ accountKey: key });
  void api.sessions.list.prefetchInfinite(sessionsInput(key, filter));

  const title = accountTitle(account);
  const subtitle = accountSubtitle(account);
  const others = account.members.filter((m) => !m.isViewer);

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-10 px-4 py-8 sm:py-10">
      <header className="flex flex-col gap-5">
        <nav aria-label="Breadcrumb" className="text-sm">
          <Link href="/dashboard" className="link">
            Dashboard
          </Link>
          <span aria-hidden="true" className="px-1.5 text-ink-3">
            /
          </span>
          <span aria-current="page" className="text-ink-2">
            {title}
          </span>
        </nav>
        <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div className="flex min-w-0 flex-col gap-1.5">
            <h1 className="text-2xl font-semibold tracking-tight break-words sm:text-3xl">
              {title}
            </h1>
            {subtitle ? <p className="text-ink-2">{subtitle}</p> : null}
            <p className="text-sm text-ink-3">
              {account.lastSyncedAt ? (
                <>
                  Last synced from your Mac{" "}
                  <RelativeTime date={account.lastSyncedAt} />
                </>
              ) : (
                "Shared with you through a pool"
              )}
            </p>
          </div>
          <div className="flex flex-wrap gap-1.5">
            {account.plan ? <Badge>{account.plan}</Badge> : null}
            {account.pooled ? (
              <Badge tone="accent">
                <PeopleIcon />
                Shared · {plural(account.memberCount, "person", "people")}
              </Badge>
            ) : null}
          </div>
        </div>

        {account.pooled ? (
          <p className="text-sm text-ink-2">
            Pooled with{" "}
            {others.map((member, i) => (
              <span key={member.id}>
                {i > 0 ? (i === others.length - 1 ? " and " : ", ") : null}
                <span
                  className="font-medium break-all text-ink"
                  title={member.name ?? undefined}
                >
                  {member.displayName}
                </span>
              </span>
            ))}
            : everything below includes their sessions and usage on this
            account.{" "}
            <Link href="/pools" className="link">
              Manage pools
            </Link>
          </p>
        ) : null}

        <dl className="card grid grid-cols-2 gap-5 p-5 sm:grid-cols-4">
          <Stat
            label="Sessions, 7 days"
            value={formatExact(account.last7Days.sessions)}
            detail={`${formatExact(account.last30Days.sessions)} in 30 days`}
          />
          <Stat
            label="Tokens, 7 days"
            value={formatTokens(account.last7Days.tokens.total)}
            title={`${formatExact(account.last7Days.tokens.total)} tokens`}
            detail={`${formatTokens(account.last30Days.tokens.total)} in 30 days`}
          />
          <Stat
            label="Cost, 7 days"
            value={formatCost(account.last7Days.costUsd) ?? "–"}
            detail={
              formatCost(account.last30Days.costUsd)
                ? `${formatCost(account.last30Days.costUsd)} in 30 days`
                : "No cost estimate"
            }
          />
          <Stat
            label="Last session activity"
            value={
              account.lastActivityAt ? (
                <RelativeTime date={account.lastActivityAt} />
              ) : (
                "None yet"
              )
            }
          />
        </dl>
      </header>

      <HydrateClient>
        <UsageSection
          accountKey={key}
          current={account.usage}
          fromMs={fromMs}
        />
        <AccountActivity
          accountKey={key}
          members={account.members}
          pooled={account.pooled}
          initialFilter={filter}
          initialPeriod={period}
        />
      </HydrateClient>
    </div>
  );
}
