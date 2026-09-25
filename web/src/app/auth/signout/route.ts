import { NextResponse, type NextRequest } from "next/server";

import { requestOrigin } from "~/lib/site-address";
import { createClient } from "~/lib/supabase/server";

export const dynamic = "force-dynamic";

/**
 * POST /auth/signout: ends this browser's website session (a form posts here). The scope is
 * "local" on purpose: Supabase's default ("global") would revoke every session the user has,
 * the Mac app's included, and its sync would stop within the hour. The app signs out on its own.
 */
const SIGN_OUT_SCOPE = "local" as const;

export async function POST(request: NextRequest): Promise<NextResponse> {
  const origin = requestOrigin(request.headers, request.nextUrl.origin);
  // Another site must not be able to sign people out by posting a form here.
  const from = request.headers.get("origin");
  if (from !== null && from !== origin) {
    return NextResponse.json(
      { error: { code: "FORBIDDEN", message: "Cross-site sign-out refused." } },
      { status: 403 },
    );
  }

  const supabase = await createClient();
  await supabase.auth.signOut({ scope: SIGN_OUT_SCOPE });

  // 303: the browser follows with a GET.
  const response = NextResponse.redirect(new URL("/login", origin), 303);
  response.headers.set("Cache-Control", "private, no-store");
  return response;
}
