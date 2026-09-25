/**
 * Supabase in Server Components, Server Actions and Route Handlers, reading the session cookies.
 *
 * Never trust `auth.getSession()` here: it returns whatever the cookie says. Use
 * `auth.getClaims()` (verifies the JWT) or `auth.getUser()` (asks Supabase).
 */
import "server-only";

import { createServerClient } from "@supabase/ssr";
import { cookies } from "next/headers";

import { env } from "~/env";

export async function createClient() {
  const cookieStore = await cookies();
  return createServerClient(
    env.NEXT_PUBLIC_SUPABASE_URL,
    env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY,
    {
      cookies: {
        getAll: () => cookieStore.getAll(),
        setAll(cookiesToSet) {
          try {
            for (const { name, value, options } of cookiesToSet) {
              cookieStore.set(name, value, options);
            }
          } catch {
            // Server Components can't set cookies. The middleware refreshes the session before
            // any of them runs, so there is nothing to write here.
          }
        },
      },
    },
  );
}
