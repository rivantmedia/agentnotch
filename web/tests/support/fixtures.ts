/**
 * The contract fixtures (contract/fixtures), read as plain JSON the way a request body arrives.
 * Each call returns a fresh copy, so a test can change it freely.
 */
import { readFileSync } from "node:fs";
import path from "node:path";

const dir = path.resolve(import.meta.dirname, "../../contract/fixtures");

export const FIXTURE_NAMES = [
  "config.json",
  "error.json",
  "keys.json",
  "me.json",
  "sync-request.json",
  "sync-response.json",
] as const;

export type FixtureName = (typeof FIXTURE_NAMES)[number];

export function fixtureText(name: FixtureName): string {
  return readFileSync(path.join(dir, name), "utf8");
}

export function fixture(name: FixtureName): unknown {
  return JSON.parse(fixtureText(name)) as unknown;
}

/** The sync request fixture, typed loosely enough to edit in tests. */
export type SyncFixture = {
  schemaVersion: number;
  device: { id: string; name: string; appVersion: string };
  accounts: Array<{
    key: string;
    email: string | null;
    organizationName: string | null;
    plan: string | null;
    label: string | null;
  }>;
  sessions: Array<{
    accountKey: string;
    sessionId: string;
    project: { key: string; name: string };
    title: string | null;
    source: string;
    models: string[];
    startedAt: string;
    lastActivityAt: string;
    endedAt: string | null;
    messageCount: number;
    tokens: {
      input: number;
      output: number;
      cacheCreation: number;
      cacheRead: number;
    };
    costUsd: number | null;
    summary?: { text: string; model: string; generatedAt: string } | null;
    [extra: string]: unknown;
  }>;
  usage: Array<{
    accountKey: string;
    source: string;
    observedAt: string;
    windows: Array<{
      id: string;
      utilization: number;
      resetsAt: string | null;
    }>;
  }>;
  [extra: string]: unknown;
};

export function syncFixture(): SyncFixture {
  return fixture("sync-request.json") as SyncFixture;
}

export type KeysFixture = {
  accounts: Array<{
    accountUuid: string;
    organizationUuid: string | null;
    key: string;
  }>;
  projects: Array<{ accountKey: string; path: string; key: string }>;
  /** The install secret the project keys were made with (tests only; never sent). */
  installSecretHex: string;
};

export function keysFixture(): KeysFixture {
  return fixture("keys.json") as KeysFixture;
}
