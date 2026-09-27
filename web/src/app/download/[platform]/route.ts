import { NextResponse, type NextRequest } from "next/server";

import {
  isPlatform,
  isReleaseDownloadUrl,
  latestReleasePageUrl,
} from "~/lib/releases";
import { requestOrigin } from "~/lib/site-address";
import { getLatestRelease, releasesRepo } from "~/server/releases";

export const dynamic = "force-dynamic";

/**
 * GET /download/<mac|windows|linux>: a stable link to the latest release's file for a platform
 * (the download buttons point here). Redirects to the file on GitHub; to /download, which says
 * so, when the release has none; and to GitHub's releases page when GitHub's API can't say.
 * Nothing here is cached: GitHub's answer already is, server-side.
 */
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ platform: string }> },
): Promise<NextResponse> {
  const { platform } = await params;
  if (!isPlatform(platform)) {
    return noStore(new NextResponse("Not found", { status: 404 }));
  }

  const repo = releasesRepo();
  const latest = await getLatestRelease();
  if (latest.kind === "unavailable") {
    return noStore(NextResponse.redirect(latestReleasePageUrl(repo), 302));
  }

  const asset = latest.kind === "ok" ? latest.downloads[platform] : null;
  if (asset) {
    // Only a file on this repository's releases; anything else goes to its releases page.
    return noStore(
      NextResponse.redirect(
        isReleaseDownloadUrl(asset.url, repo)
          ? asset.url
          : latestReleasePageUrl(repo),
        302,
      ),
    );
  }

  const page = new URL(
    "/download",
    requestOrigin(request.headers, request.nextUrl.origin),
  );
  page.searchParams.set("unavailable", platform);
  return noStore(NextResponse.redirect(page, 302));
}

function noStore(response: NextResponse): NextResponse {
  response.headers.set("Cache-Control", "private, no-store");
  return response;
}
