import { revalidateTag } from "next/cache";

import { env } from "~/env";
import { refreshReleases, releaseRunVerifier } from "~/server/releases-refresh";
import { releasesRepo } from "~/server/releases";

export const dynamic = "force-dynamic";

/** The Release workflow clears the site's cached release list (src/server/releases-refresh.ts). */
export async function POST(request: Request): Promise<Response> {
  return refreshReleases(request, {
    // The tokens' audience is the origin of the site's address (src/env.js): NEXT_PUBLIC_SITE_URL,
    // else the Vercel project's production domain. Null when it has none.
    verify: releaseRunVerifier(env.NEXT_PUBLIC_SITE_URL, releasesRepo()),
    revalidate: revalidateTag,
  });
}
