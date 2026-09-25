import { type Metadata } from "next";

import { requireViewer, siteAddress } from "~/server/pages";
import { api, HydrateClient } from "~/trpc/server";

import { DashboardView } from "./dashboard-view";
import { RECENT_SESSIONS_INPUT } from "./queries";

export const metadata: Metadata = { title: "Dashboard" };

export default async function DashboardPage() {
  await requireViewer("/dashboard");
  // Started here, finished in the browser: the client components pick these up without asking
  // again.
  void api.accounts.list.prefetch();
  void api.sessions.list.prefetch(RECENT_SESSIONS_INPUT);

  return (
    <HydrateClient>
      <DashboardView siteAddress={await siteAddress()} />
    </HydrateClient>
  );
}
