import { type Metadata } from "next";

import { periodFromParam } from "~/lib/usage-period";
import { requireViewer } from "~/server/pages";
import { api, HydrateClient } from "~/trpc/server";

import { ProjectsView } from "./projects-view";
import { allProjectsInput } from "../queries";

export const metadata: Metadata = { title: "Projects" };

export default async function ProjectsPage({
  searchParams,
}: {
  searchParams: Promise<{ period?: string | string[] }>;
}) {
  await requireViewer("/projects");
  const period = periodFromParam((await searchParams).period);
  // Started here, finished in the browser: the client components pick these up without asking
  // again.
  void api.accounts.list.prefetch();
  void api.projects.usage.prefetch(allProjectsInput(period));

  return (
    <HydrateClient>
      <ProjectsView period={period} />
    </HydrateClient>
  );
}
