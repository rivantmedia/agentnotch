/**
 * Supabase in the browser: only for starting a sign-in (the session lives in cookies that the
 * server reads). Uses the publishable key, never a secret one.
 */
import { createBrowserClient } from "@supabase/ssr";

import { env } from "~/env";

export function createClient() {
  return createBrowserClient(
    env.NEXT_PUBLIC_SUPABASE_URL,
    env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY,
  );
}
