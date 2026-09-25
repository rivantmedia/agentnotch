/**
 * Runs before every page, route and tRPC call (see src/middleware.ts): refreshes the Supabase
 * session cookies, and sends signed-out visitors of protected pages to /login.
 *
 * Refreshed cookies are written to both the request (so the page or route that runs next reads
 * the new session) and the response (so the browser keeps it). Supabase's cache headers go on
 * the response too, so no CDN ever stores a response that sets someone's session.
 */
import { createServerClient } from "@supabase/ssr";
import { NextResponse, type NextRequest } from "next/server";

import { env } from "~/env";
import { isAcceptedSignIn } from "~/lib/auth-claims";
import { isProtectedPath } from "~/lib/auth-paths";
import { requestOrigin } from "~/lib/site-address";

export async function updateSession(
  request: NextRequest,
): Promise<NextResponse> {
  let response = NextResponse.next({ request });

  const supabase = createServerClient(
    env.NEXT_PUBLIC_SUPABASE_URL,
    env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY,
    {
      cookies: {
        getAll: () => request.cookies.getAll(),
        setAll(cookiesToSet, headers) {
          for (const { name, value } of cookiesToSet) {
            request.cookies.set(name, value);
          }
          response = NextResponse.next({ request });
          for (const { name, value, options } of cookiesToSet) {
            response.cookies.set(name, value, options);
          }
          for (const [key, value] of Object.entries(headers)) {
            response.headers.set(key, value);
          }
        },
      },
    },
  );

  // Nothing may run between creating the client and this call: it is what refreshes the session.
  // getClaims() verifies the token; getSession() would only read the cookie.
  const { data } = await supabase.auth.getClaims();
  // The same rule as the server's (Google sign-ins only), so a session the pages would refuse
  // goes to /login instead of bouncing between the two.
  const signedIn = isAcceptedSignIn(data?.claims);

  const { pathname, search } = request.nextUrl;
  if (!signedIn && isProtectedPath(pathname)) {
    const login = new URL(
      "/login",
      requestOrigin(request.headers, request.nextUrl.origin),
    );
    login.searchParams.set("next", `${pathname}${search}`);
    const redirect = NextResponse.redirect(login);
    // Keep whatever the refresh wrote (e.g. clearing a dead session).
    for (const cookie of response.cookies.getAll())
      redirect.cookies.set(cookie);
    redirect.headers.set("Cache-Control", "private, no-store");
    return redirect;
  }
  return response;
}
