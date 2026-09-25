"use client";

import { useCallback, useDeferredValue, useId, useRef, useState } from "react";

import { formatCost, formatExact, formatTokens, plural } from "~/lib/format";
import { api, type RouterOutputs } from "~/trpc/react";

import { QueryBoundary } from "../../_components/query-boundary";
import {
  SessionList,
  SessionListSkeleton,
} from "../../_components/session-list";
import { RelativeTime } from "../../_components/time";
import {
  cx,
  ErrorNotice,
  LoadingBlock,
  SectionHeading,
  Skeleton,
} from "../../_components/ui";
import { PROJECTS_DESCRIPTION } from "./copy";
import { sessionsInput, type SessionFilter } from "./queries";

type Person = RouterOutputs["accounts"]["get"]["members"][number];
type Project = RouterOutputs["projects"]["list"][number];

/**
 * The account's projects and sessions. They share one filter (project, member), kept in the
 * address bar so a filtered view can be reloaded or shared with a pool member.
 */
export function AccountActivity({
  accountKey,
  members,
  pooled,
  initialFilter,
}: {
  accountKey: string;
  members: Person[];
  pooled: boolean;
  initialFilter: SessionFilter;
}) {
  const [filter, setFilter] = useState<SessionFilter>(initialFilter);
  // The list follows the filter a render behind: while the new page loads, the old rows stay
  // (faded) instead of flashing back to a skeleton.
  const shownFilter = useDeferredValue(filter);
  const sessionsRef = useRef<HTMLElement>(null);

  const applyFilter = useCallback((next: SessionFilter) => {
    setFilter(next);
    // Keep the URL in step without a navigation (Next.js syncs history.replaceState).
    const url = new URL(window.location.href);
    if (next.projectId) url.searchParams.set("project", next.projectId);
    else url.searchParams.delete("project");
    if (next.ownerId) url.searchParams.set("member", next.ownerId);
    else url.searchParams.delete("member");
    window.history.replaceState(null, "", url);
  }, []);

  const showProject = (projectId: string) => {
    applyFilter({ ...filter, projectId });
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    sessionsRef.current?.scrollIntoView({
      behavior: still ? "auto" : "smooth",
      block: "start",
    });
    sessionsRef.current?.focus({ preventScroll: true });
  };

  return (
    <>
      <section aria-labelledby="projects-title" className="flex flex-col gap-4">
        <SectionHeading
          id="projects-title"
          title="Projects"
          description={PROJECTS_DESCRIPTION}
        />
        <QueryBoundary
          what="the projects"
          fallback={
            <LoadingBlock label="Loading projects…">
              <div className="card flex flex-col gap-3 p-5" aria-hidden="true">
                {[0, 1, 2].map((i) => (
                  <Skeleton key={i} className="h-8 w-full" />
                ))}
              </div>
            </LoadingBlock>
          }
        >
          <Projects
            accountKey={accountKey}
            showOwner={pooled}
            selectedId={filter.projectId}
            onSelect={showProject}
          />
        </QueryBoundary>
      </section>

      <section
        ref={sessionsRef}
        tabIndex={-1}
        aria-labelledby="sessions-title"
        className="flex scroll-mt-20 flex-col gap-4 outline-none"
      >
        <SectionHeading
          id="sessions-title"
          title="Sessions"
          description="Newest first. Summaries appear when the person who ran the session turned them on in the app."
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
          <SessionFilters
            accountKey={accountKey}
            filter={filter}
            onChange={applyFilter}
            members={members}
            pooled={pooled}
          />
          <SessionsList
            accountKey={accountKey}
            filter={shownFilter}
            stale={shownFilter !== filter}
            pooled={pooled}
            onClear={() => applyFilter({})}
          />
        </QueryBoundary>
      </section>
    </>
  );
}

function Projects({
  accountKey,
  showOwner,
  selectedId,
  onSelect,
}: {
  accountKey: string;
  showOwner: boolean;
  selectedId?: string;
  onSelect: (projectId: string) => void;
}) {
  const [projects] = api.projects.list.useSuspenseQuery({ accountKey });
  if (projects.length === 0) {
    return (
      <p className="card p-5 text-sm text-ink-2">
        No projects yet. They appear after the first session on this account
        syncs.
      </p>
    );
  }
  return (
    <ProjectsTable
      projects={projects}
      showOwner={showOwner}
      selectedId={selectedId}
      onSelect={onSelect}
    />
  );
}

function projectOptionLabel(
  project: Project,
  projects: Project[],
  pooled: boolean,
) {
  const sameName = projects.filter((p) => p.name === project.name).length > 1;
  if (pooled && sameName) {
    return `${project.name} (${project.owner.isViewer ? "you" : project.owner.displayName})`;
  }
  return project.name;
}

function ProjectsTable({
  projects,
  showOwner,
  selectedId,
  onSelect,
}: {
  projects: Project[];
  showOwner: boolean;
  selectedId?: string;
  onSelect: (projectId: string) => void;
}) {
  const header = "px-3 py-2.5 text-xs font-medium text-ink-2 whitespace-nowrap";
  const num = "px-3 py-3 text-right tabular-nums whitespace-nowrap";
  return (
    <div
      className="card overflow-x-auto"
      role="region"
      aria-labelledby="projects-title"
      tabIndex={0}
    >
      <table className="w-full min-w-[56rem] text-sm">
        <thead className="border-b border-line text-left">
          <tr>
            <th scope="col" className={cx(header, "pl-5")}>
              Project
            </th>
            {showOwner ? (
              <th scope="col" className={header}>
                Member
              </th>
            ) : null}
            <th scope="col" className={cx(header, "text-right")}>
              Sessions
            </th>
            <th scope="col" className={cx(header, "text-right")}>
              Total tokens
            </th>
            <th scope="col" className={cx(header, "text-right")}>
              Input
            </th>
            <th scope="col" className={cx(header, "text-right")}>
              Output
            </th>
            <th scope="col" className={cx(header, "text-right")}>
              Cache write
            </th>
            <th scope="col" className={cx(header, "text-right")}>
              Cache read
            </th>
            <th scope="col" className={cx(header, "text-right")}>
              Cost
            </th>
            <th scope="col" className={cx(header, "pr-5 text-right")}>
              Last used
            </th>
          </tr>
        </thead>
        <tbody className="divide-y divide-line">
          {projects.map((project) => {
            const selected = project.id === selectedId;
            const summary = project.latestSummaries[0];
            return (
              <tr
                key={project.id}
                className={cx(selected && "bg-accent-soft/60")}
                aria-current={selected ? "true" : undefined}
              >
                <th
                  scope="row"
                  className="max-w-80 py-3 pr-3 pl-5 text-left align-top font-normal"
                >
                  <button
                    type="button"
                    onClick={() => onSelect(project.id)}
                    className="text-left font-medium text-accent-ink underline decoration-1 underline-offset-3 hover:decoration-2"
                    aria-label={`Show sessions in ${project.name}`}
                  >
                    {project.name}
                  </button>
                  {summary ? (
                    <p
                      className="mt-1 line-clamp-2 text-xs text-ink-3"
                      title={summary.text}
                    >
                      Latest: {summary.text}
                    </p>
                  ) : null}
                </th>
                {showOwner ? (
                  <td
                    className="px-3 py-3 align-top whitespace-nowrap text-ink-2"
                    title={project.owner.name ?? undefined}
                  >
                    {project.owner.isViewer ? "You" : project.owner.displayName}
                  </td>
                ) : null}
                <td className={cx(num, "align-top")}>
                  {formatExact(project.sessionCount)}
                </td>
                <td
                  className={cx(num, "align-top font-semibold")}
                  title={`${formatExact(project.tokens.total)} tokens`}
                >
                  {formatTokens(project.tokens.total)}
                </td>
                <td
                  className={cx(num, "align-top text-ink-2")}
                  title={formatExact(project.tokens.input)}
                >
                  {formatTokens(project.tokens.input)}
                </td>
                <td
                  className={cx(num, "align-top text-ink-2")}
                  title={formatExact(project.tokens.output)}
                >
                  {formatTokens(project.tokens.output)}
                </td>
                <td
                  className={cx(num, "align-top text-ink-2")}
                  title={formatExact(project.tokens.cacheCreation)}
                >
                  {formatTokens(project.tokens.cacheCreation)}
                </td>
                <td
                  className={cx(num, "align-top text-ink-2")}
                  title={formatExact(project.tokens.cacheRead)}
                >
                  {formatTokens(project.tokens.cacheRead)}
                </td>
                <td className={cx(num, "align-top text-ink-2")}>
                  {formatCost(project.costUsd) ?? (
                    <span className="text-ink-3">–</span>
                  )}
                </td>
                <td className={cx(num, "pr-5 align-top text-ink-2")}>
                  {project.lastUsedAt ? (
                    <RelativeTime date={project.lastUsedAt} />
                  ) : (
                    "–"
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function SessionFilters({
  accountKey,
  filter,
  onChange,
  members,
  pooled,
}: {
  accountKey: string;
  filter: SessionFilter;
  onChange: (next: SessionFilter) => void;
  members: Person[];
  pooled: boolean;
}) {
  // The same query as the projects table, so no second request.
  const [projects] = api.projects.list.useSuspenseQuery({ accountKey });
  const id = useId();
  const active = Boolean(filter.projectId ?? filter.ownerId);
  const knownProject =
    !filter.projectId || projects.some((p) => p.id === filter.projectId);
  const knownMember =
    !filter.ownerId || members.some((m) => m.id === filter.ownerId);

  return (
    <div
      className="flex flex-wrap items-end gap-3"
      role="group"
      aria-label="Filter sessions"
    >
      <div className="flex min-w-48 flex-1 flex-col gap-1 sm:flex-none">
        <label
          htmlFor={`${id}-project`}
          className="text-xs font-medium text-ink-2"
        >
          Project
        </label>
        <select
          id={`${id}-project`}
          className="field sm:w-64"
          value={filter.projectId ?? ""}
          onChange={(e) =>
            onChange({ ...filter, projectId: e.target.value || undefined })
          }
        >
          <option value="">All projects</option>
          {!knownProject ? (
            <option value={filter.projectId}>Selected project</option>
          ) : null}
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {projectOptionLabel(p, projects, pooled)}
            </option>
          ))}
        </select>
      </div>
      {pooled && members.length > 1 ? (
        <div className="flex min-w-40 flex-1 flex-col gap-1 sm:flex-none">
          <label
            htmlFor={`${id}-member`}
            className="text-xs font-medium text-ink-2"
          >
            Member
          </label>
          <select
            id={`${id}-member`}
            className="field sm:w-52"
            value={filter.ownerId ?? ""}
            onChange={(e) =>
              onChange({ ...filter, ownerId: e.target.value || undefined })
            }
          >
            <option value="">
              Everyone ({plural(members.length, "person", "people")})
            </option>
            {!knownMember ? (
              <option value={filter.ownerId}>Selected member</option>
            ) : null}
            {members.map((m) => (
              <option key={m.id} value={m.id}>
                {m.isViewer
                  ? "You"
                  : m.name
                    ? `${m.displayName} (${m.name})`
                    : m.displayName}
              </option>
            ))}
          </select>
        </div>
      ) : null}
      {active ? (
        <button
          type="button"
          className="btn btn-ghost"
          onClick={() => onChange({})}
        >
          Clear filters
        </button>
      ) : null}
    </div>
  );
}

function SessionsList({
  accountKey,
  filter,
  stale,
  pooled,
  onClear,
}: {
  accountKey: string;
  filter: SessionFilter;
  /** The filter changed and this list is still showing the previous one. */
  stale: boolean;
  pooled: boolean;
  onClear: () => void;
}) {
  const [data, sessions] = api.sessions.list.useSuspenseInfiniteQuery(
    sessionsInput(accountKey, filter),
    { getNextPageParam: (page) => page.nextCursor },
  );

  const items = data.pages.flatMap((page) => page.items);
  const filtered = Boolean(filter.projectId ?? filter.ownerId);

  if (items.length === 0) {
    return (
      <div
        className={cx(
          "card flex flex-col items-start gap-3 p-5 text-sm text-ink-2 transition-opacity",
          stale && "opacity-60",
        )}
      >
        {filtered ? (
          <>
            <p>No sessions match these filters.</p>
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={onClear}
            >
              Clear filters
            </button>
          </>
        ) : (
          <p>No sessions on this account yet.</p>
        )}
      </div>
    );
  }

  return (
    <div className="card flex flex-col gap-5 p-5">
      <SessionList items={items} showOwner={pooled} dimmed={stale} />
      <div className="flex flex-wrap items-center justify-between gap-3 border-t border-line pt-4 text-sm text-ink-2">
        <p aria-live="polite">
          {stale
            ? "Updating…"
            : `Showing ${plural(items.length, "session", "sessions")}${sessions.hasNextPage ? "" : ", all of them"}.`}
        </p>
        {sessions.hasNextPage ? (
          <button
            type="button"
            className="btn btn-secondary btn-sm"
            onClick={() => void sessions.fetchNextPage()}
            disabled={sessions.isFetchingNextPage || stale}
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
