"use client";

import Link from "next/link";
import { type ReactNode } from "react";

import { formatCost, formatExact, formatTokens, plural } from "~/lib/format";
import { periodPhrase, withPeriod } from "~/lib/usage-period";
import { api, type RouterInputs, type RouterOutputs } from "~/trpc/react";

import { cx } from "./ui";
import { restRow, UsageBreakdown, type BreakdownRow } from "./usage-breakdown";

export type ProjectUsage =
  RouterOutputs["projects"]["usage"]["projects"][number];

/**
 * Usage by project in a period, as ranked bars: each project's share of the tokens, what it cost
 * and how many sessions it had, where it ran and whose it is. Each project links to its page.
 */
export function ProjectUsageList({
  input,
  stale,
  accountName,
  showOwner,
  action,
  footer,
}: {
  /** The query the page prefetched (projects.usage). */
  input: RouterInputs["projects"]["usage"];
  /** The period changed and this list still shows the previous one. */
  stale: boolean;
  /** Names each project's accounts under it; left out on a single account's page. */
  accountName?: (accountKey: string) => string;
  /** Say whose each project is (pooled accounts). */
  showOwner: boolean;
  /** A control beside each project's name. */
  action?: (project: ProjectUsage) => ReactNode;
  /** A line under the totals, e.g. a link to every project. */
  footer?: ReactNode;
}) {
  const [usage] = api.projects.usage.useSuspenseQuery(input);
  const period = usage.period;

  if (usage.projects.length === 0) {
    return (
      <div
        className={cx(
          "flex flex-col gap-1 text-sm text-ink-2 transition-opacity",
          stale && "opacity-60",
        )}
      >
        <p>No sessions {periodPhrase(period)}.</p>
        {usage.earlierSessions ? (
          <p>Pick a longer period to see older usage.</p>
        ) : null}
      </div>
    );
  }

  const rows: BreakdownRow[] = [
    ...usage.projects.map((project) => ({
      key: project.id,
      label: (
        <Link
          href={withPeriod(`/projects/${project.id}`, period)}
          className="link"
        >
          {project.name}
        </Link>
      ),
      action: action?.(project),
      detail: (
        <ProjectDetail
          project={project}
          accountName={accountName}
          showOwner={showOwner}
        />
      ),
      totals: project,
      lastUsedAt: project.lastUsedAt,
    })),
    ...restRow(usage.rest, { one: "project", many: "projects" }),
  ];
  const cost = formatCost(usage.total.costUsd);

  return (
    <div className="flex flex-col gap-5">
      <UsageBreakdown
        rows={rows}
        whole={usage.total.tokens.total}
        label={`Usage by project, ${periodPhrase(period)}`}
        dimmed={stale}
      />
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-2 border-t border-line pt-4 text-sm text-ink-2">
        <p aria-live="polite">
          {stale ? (
            "Updating…"
          ) : (
            <>
              {plural(usage.total.projects, "project", "projects")}{" "}
              {periodPhrase(period)}:{" "}
              <span
                className="font-medium text-ink"
                title={`${formatExact(usage.total.tokens.total)} tokens`}
              >
                {formatTokens(usage.total.tokens.total)} tokens
              </span>
              {cost ? (
                <>
                  , <span className="font-medium text-ink">{cost}</span>
                </>
              ) : null}{" "}
              in {plural(usage.total.sessions, "session", "sessions")}.
            </>
          )}
        </p>
        {footer}
      </div>
    </div>
  );
}

/** Under a project's bar: its accounts with their tokens, its Macs, and whose it is. */
function ProjectDetail({
  project,
  accountName,
  showOwner,
}: {
  project: ProjectUsage;
  accountName?: (accountKey: string) => string;
  showOwner: boolean;
}) {
  const parts: ReactNode[] = [];
  if (accountName) {
    parts.push(
      <span key="accounts">
        {project.accounts.length === 1 ? "On " : null}
        {project.accounts.map((account, i) => (
          <span key={account.accountKey}>
            {i > 0 ? ", " : null}
            <span className="text-ink">{accountName(account.accountKey)}</span>
            {project.accounts.length > 1 ? (
              <span
                className="tabular-nums"
                title={`${formatExact(account.tokens.total)} tokens`}
              >
                {" "}
                {formatTokens(account.tokens.total)}
              </span>
            ) : null}
          </span>
        ))}
      </span>,
    );
  }
  if (project.macCount > 1) {
    parts.push(
      <span key="macs">{plural(project.macCount, "Mac", "Macs")}</span>,
    );
  }
  if (showOwner) {
    parts.push(
      <span key="owner">
        by{" "}
        <span
          className="break-all text-ink"
          title={project.owner.name ?? undefined}
        >
          {project.owner.isViewer ? "you" : project.owner.displayName}
        </span>
      </span>,
    );
  }
  return (
    <>
      {parts.map((part, i) => (
        <span key={i}>
          {i > 0 ? " · " : null}
          {part}
        </span>
      ))}
    </>
  );
}
