import { NextResponse, type NextRequest } from "next/server";

import { safeNextPath } from "~/lib/auth-paths";
import { requestOrigin } from "~/lib/site-address";
import { createClient } from "~/lib/supabase/server";

export const dynamic = "force-dynamic";

/**
 * GET /auth/callback: where Google (through Supabase) sends the browser after sign-in. Exchanges
 * the PKCE code for a session, which the Supabase client stores in cookies, then continues to
 * `next` (a path on this site only).
 */
export async function GET(request: NextRequest): Promise<NextResponse> {
  const { searchParams } = request.nextUrl;
  const origin = requestOrigin(request.headers, request.nextUrl.origin);
  const next = safeNextPath(searchParams.get("next"));
  const code = searchParams.get("code");

  if (code) {
    const supabase = await createClient();
    const { error } = await supabase.auth.exchangeCodeForSession(code);
    if (!error) return noStore(NextResponse.redirect(`${origin}${next}`));
  }

  // No code (the user cancelled, or Google/Supabase reported an error) or a failed exchange.
  const login = new URL("/login", origin);
  login.searchParams.set("error", "signin");
  login.searchParams.set("next", next);
  return noStore(NextResponse.redirect(login));
}

function noStore(response: NextResponse): NextResponse {
  response.headers.set("Cache-Control", "private, no-store");
  return response;
}
