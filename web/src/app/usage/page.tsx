import { type Metadata } from "next";

import { selectionFromParam } from "~/lib/account-selection";
import { periodFromParam } from "~/lib/usage-period";
import { requireViewer } from "~/server/pages";
import { api, HydrateClient } from "~/trpc/server";

import {
  asksForAccounts,
  combinedUsageInput,
  usageProjectsInput,
} from "./queries";
import { UsageView } from "./usage-view";

export const metadata: Metadata = { title: "Usage" };

export default async function UsagePage({
  searchParams,
}: {
  searchParams: Promise<{
    period?: string | string[];
    accounts?: string | string[];
  }>;
}) {
  await requireViewer("/usage");
  const query = await searchParams;
  const period = periodFromParam(query.period);
  const selection = selectionFromParam(query.accounts);
  // Started here, finished in the browser: the client components pick these up without asking
  // again. Usage over time waits for the browser, which alone knows the viewer's time zone.
  void api.accounts.list.prefetch();
  if (asksForAccounts(selection)) {
    void api.usage.combined.prefetch(combinedUsageInput(period, selection));
    void api.projects.usage.prefetch(usageProjectsInput(period, selection));
  }

  return (
    <HydrateClient>
      <UsageView period={period} selection={selection} />
    </HydrateClient>
  );
}
