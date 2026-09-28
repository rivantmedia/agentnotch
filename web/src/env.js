import { createEnv } from "@t3-oss/env-nextjs";
import { z } from "zod";

import { siteUrlFrom } from "./lib/site-url.js";

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
    // Optional: the GitHub repository (`owner/name`) whose latest release /download offers.
    // Unset means rivantmedia/agentnotch (src/lib/releases.ts, which holds the same rule).
    RELEASES_REPO: z
      .string()
      .regex(/^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})\/(?!\.\.?$)[\w.-]{1,100}$/)
      .optional(),
    // Optional: a GitHub token for reading those releases. Public releases need none; a token
    // lifts GitHub's limit of 60 unauthenticated requests an hour per IP address.
    GITHUB_RELEASES_TOKEN: z.string().min(1).optional(),
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
    // The site's public address, e.g. https://agentnotch.example.com: NEXT_PUBLIC_SITE_URL when
    // set, else the Vercel project's production domain (src/lib/site-url.js). Unset elsewhere
    // (local development without it, a self-hosted site that doesn't set it): the app API then
    // uses the address each request came in on.
    NEXT_PUBLIC_SITE_URL: z.string().url().optional(),
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
    RELEASES_REPO: process.env.RELEASES_REPO,
    GITHUB_RELEASES_TOKEN: process.env.GITHUB_RELEASES_TOKEN,
    NODE_ENV: process.env.NODE_ENV,
    NEXT_PUBLIC_SUPABASE_URL: process.env.NEXT_PUBLIC_SUPABASE_URL,
    NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY:
      process.env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY,
    // Vercel sets both production domain variables at build and at run time; browser code gets
    // only the NEXT_PUBLIC_ one, which Next.js builds in.
    NEXT_PUBLIC_SITE_URL: siteUrlFrom(
      process.env.NEXT_PUBLIC_SITE_URL,
      process.env.VERCEL_PROJECT_PRODUCTION_URL,
      process.env.NEXT_PUBLIC_VERCEL_PROJECT_PRODUCTION_URL,
    ),
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
