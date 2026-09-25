/**
 * Reads a JSON request body without ever holding more than `maxBytes` of it.
 */
import { AppApiError } from "./errors";

export async function readJsonBody(
  request: Request,
  maxBytes: number,
): Promise<unknown> {
  const declared = request.headers.get("content-length");
  if (declared !== null && Number(declared) > maxBytes) {
    throw tooLarge(maxBytes);
  }
  if (!request.body) {
    throw new AppApiError("BAD_REQUEST", "The request has no body.");
  }

  // Content-Length can be absent (chunked) or wrong, so count what actually arrives.
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let received = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    received += value.byteLength;
    if (received > maxBytes) {
      await reader.cancel().catch(() => undefined);
      throw tooLarge(maxBytes);
    }
    chunks.push(value);
  }

  const bytes = new Uint8Array(received);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }

  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    throw new AppApiError("BAD_REQUEST", "The body is not valid UTF-8.");
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    throw new AppApiError("BAD_REQUEST", "The body is not valid JSON.");
  }
}

function tooLarge(maxBytes: number): AppApiError {
  const mb = Math.round((maxBytes / (1024 * 1024)) * 10) / 10;
  return new AppApiError(
    "PAYLOAD_TOO_LARGE",
    `The body is larger than ${mb} MB. Send fewer sessions or readings per request.`,
  );
}
