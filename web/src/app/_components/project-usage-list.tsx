"use client";

import Link from "next/link";
import { useState, type ReactNode } from "react";

import {
  formatCost,
  formatExact,
  formatShare,
  formatTokens,
  plural,
} from "~/lib/format";
import {
  OTHER_SLICE,
  PIE_SLOTS,
  pieSlices,
  sliceColor,
  sliceKeyFor,
} from "~/lib/pie-chart";
import { periodPhrase, withPeriod } from "~/lib/usage-period";
import { api, type RouterInputs, type RouterOutputs } from "~/trpc/react";

import { cx } from "./ui";
import { restRow, UsageBreakdown, type BreakdownRow } from "./usage-breakdown";
import { UsagePie, type PieDatum } from "./usage-pie";

export type ProjectUsage =
  RouterOutputs["projects"]["usage"]["projects"][number];

/**
 * Usage by project in a period, as a pie of the tokens with the projects listed beside it as its
 * legend: each project's share, what it cost and how many sessions it had, where it ran and whose
 * it is, linking to its page. The first PIE_SLOTS projects get slices of their own; the rest
 * share a gray "other" one, and their rows say so with a gray mark.
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
  const [active, setActive] = useState<string | null>(null);
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

  const whole = usage.total.tokens.total;
  const slices = pieSlices(
    usage.projects.map((p) => ({ key: p.id, value: p.tokens.total })),
    whole,
  );
  const slotOf = new Map(slices.map((slice) => [slice.key, slice.slot]));
  // The other slice holds the projects listed past the named slices and the ones folded away.
  const others = usage.projects.slice(PIE_SLOTS);
  const otherCount = others.length + (usage.rest?.projects ?? 0);
  const otherCosts = [
    ...others.map((p) => p.costUsd),
    usage.rest?.costUsd ?? null,
  ].filter((c): c is number => c !== null);
  const pie: PieDatum[] = slices.map((slice) => {
    const project = usage.projects.find((p) => p.id === slice.key);
    if (project) {
      return {
        ...slice,
        label: project.name,
        tokens: project.tokens.total,
        costUsd: project.costUsd,
        sessions: project.sessions,
      };
    }
    return {
      ...slice,
      label: plural(otherCount, "other project", "other projects"),
      tokens:
        whole -
        usage.projects
          .slice(0, PIE_SLOTS)
          .reduce((sum, p) => sum + p.tokens.total, 0n),
      costUsd:
        otherCosts.length > 0 ? otherCosts.reduce((a, b) => a + b, 0) : null,
      sessions:
        others.reduce((sum, p) => sum + p.sessions, 0) +
        (usage.rest?.sessions ?? 0),
    };
  });

  const rows: BreakdownRow[] = [
    ...usage.projects.map((project, index) => ({
      key: project.id,
      swatch: sliceColor(
        index < PIE_SLOTS ? (slotOf.get(project.id) ?? null) : null,
      ),
      // A project with no tokens has no slice to light.
      slice:
        index >= PIE_SLOTS || slotOf.has(project.id)
          ? sliceKeyFor(index, project.id)
          : undefined,
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
    ...restRow(usage.rest, { one: "project", many: "projects" }).map((row) => ({
      ...row,
      swatch: sliceColor(null),
      slice: OTHER_SLICE,
    })),
  ];
  const cost = formatCost(usage.total.costUsd);
  // The gray slice's own legend entry, where its projects are listed one by one below the named
  // ones (a folded tail has its row in the list instead).
  const otherSlice =
    others.length > 0 ? pie.find((slice) => slice.key === OTHER_SLICE) : null;

  return (
    <div className="flex flex-col gap-5">
      {/* grid-cols-1: a single column that a long unbroken name can't widen past the card. */}
      <div className="grid grid-cols-1 gap-6 md:grid-cols-[12rem_minmax(0,1fr)] md:gap-8">
        {/*
          Beside a long list the pie stays in view, next to the rows it keys. z-10 keeps its
          tooltip above the legend's swatches, which stack on their own.
        */}
        <div className="z-10 flex flex-col items-center gap-3 md:sticky md:top-20 md:self-start">
          <UsagePie
            slices={pie}
            active={active}
            onActive={setActive}
            dimmed={stale}
            className="w-48 max-w-full"
          />
          {otherSlice ? (
            <p
              className={cx(
                "flex max-w-56 items-baseline gap-2 rounded-md px-2 py-1 text-xs text-ink-2 tabular-nums transition-colors",
                active === OTHER_SLICE && "bg-surface-2",
                stale && "opacity-60",
              )}
              onPointerEnter={(e) => {
                if (e.pointerType === "mouse") setActive(OTHER_SLICE);
              }}
              onPointerLeave={(e) => {
                if (e.pointerType === "mouse") setActive(null);
              }}
            >
              <span
                aria-hidden="true"
                className="size-2.5 shrink-0 translate-y-px rounded-full"
                style={{ backgroundColor: sliceColor(null) }}
              />
              <span>
                Gray: {otherSlice.label},{" "}
                <span className="text-ink">
                  {formatTokens(otherSlice.tokens)} tokens
                </span>{" "}
                ·{" "}
                <span className="text-ink">
                  {formatShare(otherSlice.share)}
                </span>
              </span>
            </p>
          ) : null}
        </div>
        <UsageBreakdown
          rows={rows}
          whole={whole}
          label={`Usage by project, ${periodPhrase(period)}`}
          dimmed={stale}
          bars={false}
          activeSlice={active}
          onActiveSlice={setActive}
        />
      </div>
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
