/**
 * Input pieces shared by the routers.
 */
import { z } from "zod";

import { USAGE_PERIODS } from "~/lib/usage-period";
import {
  hexKey,
  sessionSourceSchema,
  usageSourceSchema,
} from "~/server/app-api/schema";

export const accountKeyInput = hexKey;

/**
 * Free text Postgres can take: no NUL (which it refuses outright, turning a typo into a server
 * error) and no unpaired surrogates (not valid UTF-8).
 */
export function storableText(schema: z.ZodString) {
  return schema.refine(
    (value) =>
      !/\u0000|[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(
        value,
      ),
    "must not contain NUL or broken characters",
  );
}

/** A row id (cuid or UUID) or user id. */
export const idInput = storableText(z.string().min(1).max(128));
/** A search term. */
export const searchInput = storableText(z.string().max(200));
export const sessionSourceInput = sessionSourceSchema;
export const usageSourceInput = usageSourceSchema;
/** A usage breakdown's period (lib/usage-period.ts). */
export const usagePeriodInput = z.enum(USAGE_PERIODS);
