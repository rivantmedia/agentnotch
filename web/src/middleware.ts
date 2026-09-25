import { type NextRequest } from "next/server";

import { updateSession } from "~/lib/supabase/middleware";

export async function middleware(request: NextRequest) {
  return updateSession(request);
}

export const config = {
  matcher: [
    /*
     * Everything except static files, images and the Mac app's API (/api/app/*), which takes
     * bearer tokens only and never reads or sets the website's cookies.
     */
    "/((?!_next/static|_next/image|favicon.ico|api/app/|.*\\.(?:svg|png|jpg|jpeg|gif|webp|ico)$).*)",
  ],
};
