import { revalidateTag } from "next/cache";

import { env } from "~/env";
import { refreshReleases, releaseRunVerifier } from "~/server/releases-refresh";
import { releasesRepo } from "~/server/releases";

export const dynamic = "force-dynamic";

/** The Release workflow clears the site's cached release list (src/server/releases-refresh.ts). */
export async function POST(request: Request): Promise<Response> {
  return refreshReleases(request, {
    // Optional chaining: a build made with SKIP_ENV_VALIDATION may run without the variable.
    verify: releaseRunVerifier(
      env.NEXT_PUBLIC_SITE_URL ?? undefined,
      releasesRepo(),
    ),
    revalidate: revalidateTag,
  });
}
