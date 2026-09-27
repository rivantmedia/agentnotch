import { type Metadata } from "next";
import { notFound } from "next/navigation";

import { periodFromParam } from "~/lib/usage-period";
import { requireViewer } from "~/server/pages";
import { api, HydrateClient } from "~/trpc/server";

import {
  isProjectId,
  projectDetailInput,
  projectSessionsInput,
} from "../queries";
import { ProjectView } from "./project-view";

export const metadata: Metadata = { title: "Project" };

export default async function ProjectPage({
  params,
  searchParams,
}: {
  params: Promise<{ id: string }>;
  searchParams: Promise<{ period?: string | string[] }>;
}) {
  // layout.tsx already answered 404 for a project the viewer can't see, before anything
  // streamed. Access lost in between shows as a section that couldn't load.
  const { id } = await params;
  if (!isProjectId(id)) notFound();
  await requireViewer(`/projects/${id}`);
  const period = periodFromParam((await searchParams).period);
  // Started here, finished in the browser: the client components pick these up without asking
  // again.
  void api.accounts.list.prefetch();
  void api.projects.detail.prefetch(projectDetailInput(id, period));
  void api.sessions.list.prefetchInfinite(projectSessionsInput(id));

  return (
    <HydrateClient>
      <ProjectView id={id} initialPeriod={period} />
    </HydrateClient>
  );
}
