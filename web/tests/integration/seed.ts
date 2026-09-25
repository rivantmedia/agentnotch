/**
 * Sync requests for the integration tests, built from the contract fixture so they stay valid.
 */
import { syncRequestSchema, type SyncRequest } from "~/server/app-api/schema";

import { syncFixture, type SyncFixture } from "../support/fixtures";

/** The fixture's two accounts: personal (K1) and company (K2). */
export const K1 =
  "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";
export const K2 =
  "d8485d82cdbceb2582311953e97b1666022b76dcbb98ef283fece56b1b5b8874";
export const S1 = "a1b2c3d4-e5f6-4789-8abc-def012345678"; // on K1, with a summary
export const S2 = "0f9e8d7c-6b5a-4493-8271-605f4e3d2c1b"; // on K2, still running

/** A moment after everything in the fixture, for the "last N days" totals. */
export const AFTER_FIXTURE = new Date("2026-09-26T00:00:00Z");

/** Validates like the route does, so tests only ever apply what the API would accept. */
export function request(
  edit: (r: SyncFixture) => void = () => undefined,
): SyncRequest {
  const raw = syncFixture();
  edit(raw);
  return syncRequestSchema.parse(raw);
}

/** A request from another person's Mac, on one of the fixture's accounts. */
export function requestFor(options: {
  deviceId: string;
  accountKey: string;
  sessionId: string;
  projectKey: string;
  projectName: string;
  observedAt?: string;
  tokens?: {
    input: number;
    output: number;
    cacheCreation: number;
    cacheRead: number;
  };
}): SyncRequest {
  return request((r) => {
    r.device = {
      id: options.deviceId,
      name: "Other Mac",
      appVersion: "1.18.0",
    };
    r.accounts = r.accounts
      .filter((a) => a.key === options.accountKey)
      .map((a) => ({ ...a, label: "Theirs" }));
    const base = r.sessions.find((s) => s.accountKey === options.accountKey)!;
    r.sessions = [
      {
        ...base,
        sessionId: options.sessionId,
        project: { key: options.projectKey, name: options.projectName },
        title: `Work in ${options.projectName}`,
        tokens: options.tokens ?? {
          input: 100,
          output: 200,
          cacheCreation: 300,
          cacheRead: 400,
        },
        summary: undefined,
      },
    ];
    delete r.sessions[0]!.summary;
    r.usage = r.usage
      .filter((u) => u.accountKey === options.accountKey)
      .map((u) => ({
        ...u,
        observedAt: options.observedAt ?? "2026-09-25T12:00:00Z",
      }));
  });
}
