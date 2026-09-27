import { notFound } from "next/navigation";
import { type ReactNode } from "react";

import { requireViewer } from "~/server/pages";
import { api } from "~/trpc/server";

import { isProjectId } from "../queries";

/**
 * Checks the viewer may see the project before anything of the page is sent, as the account
 * route's layout does for accounts: from here, above the route's loading.tsx, a missing,
 * malformed or invisible project answers a real 404 (and signed-out visitors a redirect).
 */
export default async function ProjectLayout({
  children,
  params,
}: {
  children: ReactNode;
  params: Promise<{ id: string }>;
}) {
  const { id } = await params;
  if (!isProjectId(id)) notFound();
  await requireViewer(`/projects/${id}`);
  // Not visible reads the same as missing, so a project's existence doesn't leak.
  if (!(await api.projects.visible({ id }))) notFound();
  return children;
}
