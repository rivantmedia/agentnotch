import { notFound } from "next/navigation";
import { type ReactNode } from "react";

import { requireViewer } from "~/server/pages";
import { api } from "~/trpc/server";

import { isAccountKey } from "./queries";

/**
 * Checks the viewer may see the account before anything of the page is sent. The page below
 * streams behind loading.tsx, and once that fallback has gone out the response is a 200: a
 * notFound() from the page would only swap in the not-found UI. From here, above that boundary,
 * a missing, malformed or invisible account answers a real 404 (and signed-out visitors a
 * redirect). The check is cheap; the page loads the account itself.
 */
export default async function AccountLayout({
  children,
  params,
}: {
  children: ReactNode;
  params: Promise<{ key: string }>;
}) {
  const { key } = await params;
  if (!isAccountKey(key)) notFound();
  await requireViewer(`/accounts/${key}`);
  // Not visible reads the same as missing, so an account's existence doesn't leak.
  if (!(await api.accounts.visible({ accountKey: key }))) notFound();
  return children;
}
