import { appApiDeps } from "~/server/app-api/deps";
import { handleSync } from "~/server/app-api/handlers";

export const dynamic = "force-dynamic";
// A full batch (200 sessions, 500 readings) on a cold serverless instance can take a while.
export const maxDuration = 60;

/** POST /api/app/v1/sync: stores a batch of sessions and usage readings (bearer token). */
export function POST(request: Request): Promise<Response> {
  return handleSync(request, appApiDeps());
}
