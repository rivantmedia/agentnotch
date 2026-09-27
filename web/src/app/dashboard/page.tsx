import { type Metadata } from "next";

import { periodFromParam } from "~/lib/usage-period";
import { requireViewer, siteAddress } from "~/server/pages";
import { api, HydrateClient } from "~/trpc/server";

import { DashboardView } from "./dashboard-view";
import { dashboardProjectsInput, RECENT_SESSIONS_INPUT } from "./queries";

export const metadata: Metadata = { title: "Dashboard" };

export default async function DashboardPage({
  searchParams,
}: {
  searchParams: Promise<{ period?: string | string[] }>;
}) {
  await requireViewer("/dashboard");
  const period = periodFromParam((await searchParams).period);
  // Started here, finished in the browser: the client components pick these up without asking
  // again.
  void api.accounts.list.prefetch();
  void api.projects.usage.prefetch(dashboardProjectsInput(period));
  void api.sessions.list.prefetch(RECENT_SESSIONS_INPUT);

  return (
    <HydrateClient>
      <DashboardView siteAddress={await siteAddress()} period={period} />
    </HydrateClient>
  );
}
