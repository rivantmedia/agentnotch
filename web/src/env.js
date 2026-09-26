import { createEnv } from "@t3-oss/env-nextjs";
import { z } from "zod";

export const env = createEnv({
  /**
   * Server-only variables. Never import these into client components.
   */
  server: {
    // Supabase Postgres through the pooler (port 6543, `?pgbouncer=true`): what the site uses.
    DATABASE_URL: z.string().url(),
    // Supabase Postgres directly (port 5432): what `prisma migrate` uses.
    DIRECT_URL: z.string().url(),
    // Only for projects still signing access tokens with the legacy shared secret (HS256).
    // Projects on asymmetric signing keys are verified against their JWKS and leave it unset.
    SUPABASE_JWT_SECRET: z.string().min(32).optional(),
    // Optional: keys the hash of a client's IP address in the per-IP rate limits, so the stored
    // hashes can't be turned back into addresses by trying every one.
    RATE_LIMIT_PEPPER: z.string().min(16).optional(),
    NODE_ENV: z
      .enum(["development", "test", "production"])
      .default("development"),
  },

  /**
   * Public variables (shipped to the browser).
   */
  client: {
    NEXT_PUBLIC_SUPABASE_URL: z.string().url(),
    // The publishable ("anon") key. Never the secret / service-role key.
    NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY: z.string().min(1),
    // The site's public origin, e.g. https://agentnotch.example.com (no trailing slash needed).
    NEXT_PUBLIC_SITE_URL: z.string().url(),
  },

  /**
   * Next.js inlines only literal `process.env.X` reads in edge and client bundles, so each
   * variable is spelled out.
   */
  runtimeEnv: {
    DATABASE_URL: process.env.DATABASE_URL,
    DIRECT_URL: process.env.DIRECT_URL,
    SUPABASE_JWT_SECRET: process.env.SUPABASE_JWT_SECRET,
    RATE_LIMIT_PEPPER: process.env.RATE_LIMIT_PEPPER,
    NODE_ENV: process.env.NODE_ENV,
    NEXT_PUBLIC_SUPABASE_URL: process.env.NEXT_PUBLIC_SUPABASE_URL,
    NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY:
      process.env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY,
    NEXT_PUBLIC_SITE_URL: process.env.NEXT_PUBLIC_SITE_URL,
  },
  /**
   * `SKIP_ENV_VALIDATION=1 npm run build` builds without real values (CI, Docker).
   */
  skipValidation: !!process.env.SKIP_ENV_VALIDATION,
  /**
   * `SOME_VAR=''` counts as unset.
   */
  emptyStringAsUndefined: true,
});
