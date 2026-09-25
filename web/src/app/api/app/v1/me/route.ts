import { appApiDeps } from "~/server/app-api/deps";
import { handleMe } from "~/server/app-api/handlers";

export const dynamic = "force-dynamic";

/** GET /api/app/v1/me: the signed-in user (bearer token). */
export function GET(request: Request): Promise<Response> {
  return handleMe(request, appApiDeps());
}
