import { type NextRequest } from "next/server";

import { updateSession } from "~/lib/supabase/middleware";

export async function middleware(request: NextRequest) {
  return updateSession(request);
}

export const config = {
  matcher: [
    /*
     * Everything except static files, images, the Mac app's API (/api/app/*) and the Release
     * workflow's refresh (/api/releases/*), which take bearer tokens only and never read or set
     * the website's cookies.
     */
    "/((?!_next/static|_next/image|favicon.ico|api/app/|api/releases/|.*\\.(?:svg|png|jpg|jpeg|gif|webp|ico)$).*)",
  ],
};
