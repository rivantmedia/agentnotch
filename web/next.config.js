/**
 * Run `build` or `dev` with `SKIP_ENV_VALIDATION` to skip env validation. This is especially useful
 * for Docker builds.
 */
import "./src/env.js";

import { securityHeaderRules } from "./security-headers.js";

/** @type {import("next").NextConfig} */
const config = {
  async redirects() {
    return [
      // The Mac app opens `<dashboardUrl>/pools` (contract/README.md, "Pooling"); dashboardUrl
      // is /dashboard, and the pools page lives at /pools.
      { source: "/dashboard/pools", destination: "/pools", permanent: false },
    ];
  },
  // Anti-framing and related headers on every response (security-headers.js).
  async headers() {
    return securityHeaderRules();
  },
};

export default config;
