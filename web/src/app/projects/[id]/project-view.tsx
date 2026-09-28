"use client";

import Link from "next/link";

import {
  accountSubtitle,
  accountTitle,
  formatCost,
  formatExact,
  formatTokens,
  plural,
} from "~/lib/format";
import {
  periodLabel,
  periodPhrase,
  type UsagePeriod,
} from "~/lib/usage-period";
import { api, type RouterOutputs } from "~/trpc/react";

import { QueryBoundary } from "../../_components/query-boundary";
import {
  SessionList,
  SessionListSkeleton,
} from "../../_components/session-list";
import { RelativeTime } from "../../_components/time";
import {
  Badge,
  cx,
  ErrorNotice,
  LoadingBlock,
  PeopleIcon,
  SectionHeading,
  Skeleton,
  Stat,
} from "../../_components/ui";
import {
  PeriodPicker,
  usePeriod,
  UsageBreakdown,
  UsageBreakdownSkeleton,
  type BreakdownRow,
} from "../../_components/usage-breakdown";
import { PROJECT_GROUPING_NOTE } from "../copy";
import {
  accountProjectHref,
  projectDetailInput,
  projectSessionsInput,
} from "../queries";

type Account = RouterOutputs["accounts"]["list"][number];
type Detail = RouterOutputs["projects"]["detail"];

/**
 * One project across every account the viewer sees it on: its usage in a period, split by
 * account, and its sessions. The header and the picker stay put while a period loads or fails;
 * only the numbers depend on the period.
 */
export function ProjectView({
  id,
  initialPeriod,
}: {
  id: string;
  initialPeriod: UsagePeriod;
}) {
  const { period, shown, stale, choose } = usePeriod(initialPeriod);
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-10 px-4 py-8 sm:py-10">
      <QueryBoundary what="the project" fallback={<HeaderSkeleton />}>
        <ProjectHeader id={id} period={initialPeriod} />
      </QueryBoundary>

      <section
        aria-labelledby="account-usage-title"
        className="flex flex-col gap-4"
      >
        <SectionHeading
          id="account-usage-title"
          title="Usage by account"
          description="Which Claude accounts this project's tokens came from, by the sessions started in the period. Open an account to see the project's sessions there."
          actions={
            <PeriodPicker
              value={period}
              onChange={choose}
              label="Usage by account over"
            />
          }
        />
        <QueryBoundary
          what="the project's usage"
          fallback={<UsageSkeleton />}
          resetKeys={[shown]}
        >
          <ProjectUsage id={id} period={shown} stale={stale} />
        </QueryBoundary>
      </section>

      <section aria-labelledby="sessions-title" className="flex flex-col gap-4">
        <SectionHeading
          id="sessions-title"
          title="Sessions"
          description="Newest first, from every account. Summaries appear when the person who ran the session turned them on in the app."
        />
        <QueryBoundary
          what="the sessions"
          fallback={
            <div className="card p-5">
              <LoadingBlock label="Loading sessions…">
                <SessionListSkeleton />
              </LoadingBlock>
            </div>
          }
        >
          <ProjectSessions id={id} />
        </QueryBoundary>
      </section>
    </div>
  );
}

/**
 * What the project is, whatever the period: read from the period the server prefetched, so
 * picking another one never reloads (or fails) the heading.
 */
function ProjectHeader({ id, period }: { id: string; period: UsagePeriod }) {
  const [project] = api.projects.detail.useSuspenseQuery(
    projectDetailInput(id, period),
  );
  return (
    <header className="flex flex-col gap-5">
      <nav aria-label="Breadcrumb" className="text-sm">
        <Link href="/projects" className="link">
          Projects
        </Link>
        <span aria-hidden="true" className="px-1.5 text-ink-3">
          /
        </span>
        <span aria-current="page" className="break-all text-ink-2">
          {project.name}
        </span>
      </nav>
      <div className="flex flex-col gap-1.5">
        <h1 className="text-2xl font-semibold tracking-tight break-words sm:text-3xl">
          {project.name}
        </h1>
        {project.owner.isViewer ? null : (
          <p className="text-ink-2">
            <span
              className="font-medium break-all text-ink"
              title={project.owner.name ?? undefined}
            >
              {project.owner.displayName}
            </span>
            &apos;s project, on the accounts they share with you
          </p>
        )}
        <p className="text-sm text-ink-3">
          {project.firstUsedAt && project.lastUsedAt ? (
            <>
              First used <RelativeTime date={project.firstUsedAt} />, last used{" "}
              <RelativeTime date={project.lastUsedAt} />
            </>
          ) : (
            "No sessions yet"
          )}
        </p>
      </div>
      <p className="max-w-3xl text-sm text-ink-2">{PROJECT_GROUPING_NOTE}</p>
    </header>
  );
}

function ProjectUsage({
  id,
  period,
  stale,
}: {
  id: string;
  period: UsagePeriod;
  stale: boolean;
}) {
  const [project] = api.projects.detail.useSuspenseQuery(
    projectDetailInput(id, period),
  );
  const [accounts] = api.accounts.list.useSuspenseQuery();
  const byKey = new Map(accounts.map((a) => [a.key, a]));
  const used = project.accounts.filter((a) => a.sessions > 0).length;
  const cost = formatCost(project.costUsd);
  const over = period === "all" ? "all time" : periodLabel(period);

  return (
    <div className="flex flex-col gap-4">
      <dl
        className={cx(
          "card grid grid-cols-2 gap-5 p-5 transition-opacity sm:grid-cols-4",
          stale && "opacity-60",
        )}
        aria-busy={stale || undefined}
      >
        <Stat
          label={`Sessions, ${over}`}
          value={formatExact(project.sessions)}
        />
        <Stat
          label={`Tokens, ${over}`}
          value={formatTokens(project.tokens.total)}
          title={`${formatExact(project.tokens.total)} tokens`}
        />
        <Stat
          label={`Cost, ${over}`}
          value={cost ?? "–"}
          detail={cost ? "Estimated" : "No cost estimate"}
        />
        <Stat
          label="Accounts"
          value={formatExact(used)}
          detail={
            project.accounts.length > used
              ? `of the ${formatExact(project.accounts.length)} it ran on`
              : `from ${plural(project.macCount, "Mac", "Macs")}`
          }
        />
      </dl>
      <div className="card flex flex-col gap-5 p-5">
        {project.sessions === 0 ? (
          <p className="text-sm text-ink-2">
            No sessions {periodPhrase(period)}.
            {period !== "all" && project.firstUsedAt ? (
              <>
                {" "}
                The accounts it ran on are listed; pick a longer period to see
                their usage.
              </>
            ) : null}
          </p>
        ) : null}
        <UsageBreakdown
          rows={accountRows(project, byKey, period)}
          whole={project.tokens.total}
          label={`Usage by account, ${periodPhrase(period)}`}
          dimmed={stale}
        />
        <p
          aria-live="polite"
          className="border-t border-line pt-4 text-sm text-ink-2"
        >
          {stale ? (
            "Updating…"
          ) : (
            <>
              {plural(project.sessions, "session", "sessions")}{" "}
              {periodPhrase(period)} on {plural(used, "account", "accounts")}:{" "}
              <span
                className="font-medium text-ink"
                title={`${formatExact(project.tokens.total)} tokens`}
              >
                {formatTokens(project.tokens.total)} tokens
              </span>
              {cost ? (
                <>
                  , <span className="font-medium text-ink">{cost}</span>
                </>
              ) : null}
              .
            </>
          )}
        </p>
      </div>
    </div>
  );
}

function accountRows(
  project: Detail,
  byKey: Map<string, Account>,
  period: UsagePeriod,
): BreakdownRow[] {
  return project.accounts.map((usage) => {
    const account = byKey.get(usage.accountKey);
    const title = account ? accountTitle(account) : "Claude account";
    const subtitle = account ? accountSubtitle(account) : null;
    return {
      key: usage.accountKey,
      label: (
        <Link
          href={accountProjectHref(usage.accountKey, usage.projectId, period)}
          className="link wrap-anywhere"
          aria-label={`${title}: ${project.name}'s sessions on this account`}
        >
          {title}
        </Link>
      ),
      action: account?.pooled ? (
        <Badge
          tone="accent"
          title={`Pooled: shared by ${plural(account.memberCount, "person", "people")}`}
        >
          <PeopleIcon />
          Shared
        </Badge>
      ) : null,
      detail: subtitle,
      totals: usage,
      lastUsedAt: usage.lastUsedAt,
    };
  });
}

function ProjectSessions({ id }: { id: string }) {
  const [accounts] = api.accounts.list.useSuspenseQuery();
  const [data, sessions] = api.sessions.list.useSuspenseInfiniteQuery(
    projectSessionsInput(id),
    { getNextPageParam: (page) => page.nextCursor },
  );
  const names = new Map(accounts.map((a) => [a.key, accountTitle(a)]));
  const items = data.pages.flatMap((page) => page.items);

  if (items.length === 0) {
    return (
      <p className="card p-5 text-sm text-ink-2">
        No sessions in this project yet.
      </p>
    );
  }

  return (
    <div className="card flex flex-col gap-5 p-5">
      <SessionList
        items={items}
        accountName={(key) => names.get(key) ?? "Claude account"}
        showOwner={false}
        showProject={false}
      />
      <div className="flex flex-wrap items-center justify-between gap-3 border-t border-line pt-4 text-sm text-ink-2">
        <p aria-live="polite">
          Showing {plural(items.length, "session", "sessions")}
          {sessions.hasNextPage ? "" : ", all of them"}.
        </p>
        {sessions.hasNextPage ? (
          <button
            type="button"
            className="btn btn-secondary btn-sm"
            onClick={() => void sessions.fetchNextPage()}
            disabled={sessions.isFetchingNextPage}
          >
            {sessions.isFetchingNextPage ? "Loading…" : "Load more"}
          </button>
        ) : null}
      </div>
      {sessions.isError ? (
        // The rows above stay; only the part that failed is offered again.
        <ErrorNotice
          error={sessions.error}
          what={
            sessions.isFetchNextPageError
              ? "more sessions"
              : "the latest sessions"
          }
          onRetry={() =>
            void (sessions.isFetchNextPageError
              ? sessions.fetchNextPage()
              : sessions.refetch())
          }
          retrying={sessions.isFetching}
        />
      ) : null}
    </div>
  );
}

function HeaderSkeleton() {
  return (
    <LoadingBlock label="Loading the project…" className="flex flex-col gap-5">
      <Skeleton className="h-4 w-40" />
      <div className="flex flex-col gap-2">
        <Skeleton className="h-8 w-72 max-w-full" />
        <Skeleton className="h-4 w-56 max-w-full" />
      </div>
    </LoadingBlock>
  );
}

function UsageSkeleton() {
  return (
    <LoadingBlock
      label="Loading the project's usage…"
      className="flex flex-col gap-4"
    >
      <div
        className="card grid grid-cols-2 gap-5 p-5 sm:grid-cols-4"
        aria-hidden="true"
      >
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="flex flex-col gap-2">
            <Skeleton className="h-3 w-24" />
            <Skeleton className="h-6 w-16" />
          </div>
        ))}
      </div>
      <div className="card p-5">
        <UsageBreakdownSkeleton rows={2} />
      </div>
    </LoadingBlock>
  );
}
