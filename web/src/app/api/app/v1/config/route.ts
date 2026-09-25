import { env } from "~/env";
import { buildConfig } from "~/server/app-api/handlers";

// Read at request time: a build without env values (SKIP_ENV_VALIDATION) must not bake in blanks.
export const dynamic = "force-dynamic";

/** GET /api/app/v1/config: what the app needs to start a sign-in. No auth. */
export function GET(): Response {
  return Response.json(
    buildConfig({
      supabaseUrl: env.NEXT_PUBLIC_SUPABASE_URL,
      supabasePublishableKey: env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY,
      siteUrl: env.NEXT_PUBLIC_SITE_URL,
    }),
    { headers: { "Cache-Control": "public, max-age=300" } },
  );
}
