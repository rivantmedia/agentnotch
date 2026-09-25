import { type Metadata } from "next";

import { requireViewer } from "~/server/pages";
import { api, HydrateClient } from "~/trpc/server";

import { PoolsView } from "./pools-view";

export const metadata: Metadata = { title: "Pools" };

export default async function PoolsPage() {
  await requireViewer("/pools");
  void api.pools.list.prefetch();
  void api.accounts.list.prefetch();

  return (
    <HydrateClient>
      <PoolsView />
    </HydrateClient>
  );
}
