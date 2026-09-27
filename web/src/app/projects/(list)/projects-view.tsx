"use client";

import Link from "next/link";

import { accountTitle } from "~/lib/format";
import { type UsagePeriod } from "~/lib/usage-period";
import { api } from "~/trpc/react";

import { ProjectUsageList } from "../../_components/project-usage-list";
import { QueryBoundary } from "../../_components/query-boundary";
import { LoadingBlock, PageHeader } from "../../_components/ui";
import {
  PeriodPicker,
  usePeriod,
  UsageBreakdownSkeleton,
} from "../../_components/usage-breakdown";
import { PROJECT_GROUPING_NOTE, PROJECTS_PAGE_DESCRIPTION } from "../copy";
import { allProjectsInput } from "../queries";

export function ProjectsView({ period: initial }: { period: UsagePeriod }) {
  const { period, shown, stale, choose } = usePeriod(initial);
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-8 px-4 py-8 sm:py-10">
      <PageHeader
        title="Projects"
        description={PROJECTS_PAGE_DESCRIPTION}
        actions={
          <PeriodPicker value={period} onChange={choose} label="Usage over" />
        }
      />
      <QueryBoundary
        what="your projects"
        fallback={
          <div className="card p-5">
            <LoadingBlock label="Loading your projects…">
              <UsageBreakdownSkeleton rows={6} />
            </LoadingBlock>
          </div>
        }
        resetKeys={[shown]}
      >
        <AllProjects period={shown} stale={stale} />
      </QueryBoundary>
      <p className="max-w-3xl text-sm text-ink-3">{PROJECT_GROUPING_NOTE}</p>
    </div>
  );
}

function AllProjects({
  period,
  stale,
}: {
  period: UsagePeriod;
  stale: boolean;
}) {
  const [accounts] = api.accounts.list.useSuspenseQuery();
  if (accounts.length === 0) {
    return (
      <p className="card p-5 text-sm text-ink-2">
        Nothing has synced yet.{" "}
        <Link href="/dashboard" className="link">
          Connect your Mac
        </Link>{" "}
        to see your projects here.
      </p>
    );
  }
  const names = new Map(accounts.map((a) => [a.key, accountTitle(a)]));
  return (
    <div className="card p-5">
      <ProjectUsageList
        input={allProjectsInput(period)}
        stale={stale}
        accountName={
          accounts.length > 1
            ? (key) => names.get(key) ?? "Claude account"
            : undefined
        }
        showOwner={accounts.some((a) => a.pooled)}
      />
    </div>
  );
}
