/**
 * What the website must store of a sync request, field by field: the rows the contract check
 * (tests/integration/contract-e2e.test.ts) compares with the database after posting the Mac
 * app's own request. And which of the request's fields those rows account for: every field the
 * app sends must be either read into an expected row here or listed in NOT_STORED with the
 * reason, so a field the website drops without anyone deciding to fails the check
 * (`unstoredFields`, tested in tests/unit/contract-rows.test.ts).
 */
import { type SyncRequest } from "~/server/app-api/schema";

/**
 * Fields of a sync request the website doesn't keep, by path (`[]` for any element of an
 * array), each with why.
 */
export const NOT_STORED: Readonly<Record<string, string>> = {
  schemaVersion:
    "the wire format's version: the route accepts only 1, so there is nothing to keep",
};

/** Sorted by a text key, in JavaScript (never by the database's collation). */
export function sortedBy<T>(
  items: readonly T[],
  key: (item: T) => string,
): T[] {
  return [...items].sort((a, b) =>
    key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0,
  );
}

export const projectKey = (p: { accountKey: string; key: string }) =>
  `${p.accountKey}:${p.key}`;
export const sessionKey = (s: { accountKey: string; sessionId: string }) =>
  `${s.accountKey}:${s.sessionId}`;
export const readingKey = (r: {
  accountKey: string;
  source: string;
  windowId: string;
  observedAt: Date;
}) => `${r.accountKey}|${r.source}|${r.windowId}|${r.observedAt.toISOString()}`;
export const windowKey = (w: { accountKey: string; windowId: string }) =>
  `${w.accountKey}|${w.windowId}`;

const date = (text: string) => new Date(text);

/**
 * The rows `sent` must leave, in the shape the contract check reads them back in: one Mac, the
 * user's report of each account (and each account's key), one project per account and project
 * key, one session per account and session, one reading per reading and window, and the window
 * ids each account has.
 */
export function expectedRows(sent: SyncRequest) {
  const deviceId = sent.device.id.toLowerCase();
  const projects = new Map(
    sent.sessions.map((s) => {
      const project = {
        accountKey: s.accountKey,
        key: s.project.key,
        name: s.project.name,
      };
      return [projectKey(project), project] as const;
    }),
  );
  const readings = sent.usage.flatMap((u) =>
    u.windows.map((w) => ({
      accountKey: u.accountKey,
      source: u.source,
      windowId: w.id,
      utilization: w.utilization,
      resetsAt: w.resetsAt === null ? null : date(w.resetsAt),
      observedAt: date(u.observedAt),
    })),
  );
  return {
    devices: [
      {
        id: deviceId,
        name: sent.device.name,
        appVersion: sent.device.appVersion,
      },
    ],
    accounts: sortedBy(sent.accounts, (a) => a.key).map((a) => ({
      key: a.key,
      email: a.email,
      organizationName: a.organizationName,
      plan: a.plan,
      label: a.label,
    })),
    accountKeys: [...new Set(sent.accounts.map((a) => a.key))].sort(),
    projects: sortedBy([...projects.values()], projectKey),
    sessions: sortedBy(
      sent.sessions.map((s) => ({
        accountKey: s.accountKey,
        sessionId: s.sessionId.toLowerCase(),
        project: { key: s.project.key, name: s.project.name },
        title: s.title,
        source: s.source,
        models: [...s.models],
        startedAt: date(s.startedAt),
        lastActivityAt: date(s.lastActivityAt),
        endedAt: s.endedAt === null ? null : date(s.endedAt),
        messageCount: s.messageCount,
        tokens: {
          input: BigInt(s.tokens.input),
          output: BigInt(s.tokens.output),
          cacheCreation: BigInt(s.tokens.cacheCreation),
          cacheRead: BigInt(s.tokens.cacheRead),
        },
        costUsd: s.costUsd,
        summary: s.summary
          ? {
              text: s.summary.text,
              model: s.summary.model,
              at: date(s.summary.generatedAt),
            }
          : null,
        deviceId,
      })),
      sessionKey,
    ),
    readings: sortedBy(readings, readingKey),
    windows: [
      ...new Set(
        readings.map((r) =>
          windowKey({ accountKey: r.accountKey, windowId: r.windowId }),
        ),
      ),
    ].sort(),
  };
}

export type ExpectedRows = ReturnType<typeof expectedRows>;

/**
 * The fields of `sent` (by path, `[]` for any element of an array) that `build` never read and
 * `notStored` doesn't list: what the website would drop without a check noticing. `build` runs
 * on a copy of `sent` that records every field read, and its result is read in full too.
 */
export function unstoredFields<T>(
  sent: T,
  build: (sent: T) => unknown,
  notStored: Readonly<Record<string, string>> = NOT_STORED,
): string[] {
  const read = new Set<string>();
  readAll(build(recording(sent, "", read)));
  return leafPaths(sent).filter(
    (path) => !read.has(path) && !Object.hasOwn(notStored, path),
  );
}

/** Every field of a JSON value, by path; an empty array or object counts as one. */
export function leafPaths(value: unknown, path = ""): string[] {
  const paths = new Set<string>();
  const walk = (node: unknown, at: string) => {
    if (Array.isArray(node)) {
      if (node.length === 0) paths.add(at);
      for (const item of node) walk(item, `${at}[]`);
    } else if (node !== null && typeof node === "object") {
      const keys = Object.keys(node);
      if (keys.length === 0) paths.add(at);
      for (const key of keys) {
        walk((node as Record<string, unknown>)[key], join(at, key));
      }
    } else {
      paths.add(at);
    }
  };
  walk(value, path);
  return [...paths].sort();
}

function join(path: string, key: string): string {
  return path === "" ? key : `${path}.${key}`;
}

/** `value`, with every field read through it (at any depth) added to `read` by its path. */
function recording<T>(value: T, path: string, read: Set<string>): T {
  if (value === null || typeof value !== "object") return value;
  const isArray = Array.isArray(value);
  return new Proxy(value, {
    get(target, key, receiver) {
      const child: unknown = Reflect.get(target, key, receiver);
      if (typeof key === "symbol") return child;
      if (isArray && key === "length") {
        read.add(path);
        return child;
      }
      // Methods and the like come from the prototype: they aren't fields.
      if (!Object.hasOwn(target, key)) return child;
      const at = isArray ? `${path}[]` : join(path, key);
      read.add(at);
      return recording(child, at, read);
    },
  });
}

/** Reads every array element and object field of `value`, so a recording copy notes them. */
function readAll(value: unknown): void {
  if (value instanceof Date || value === null || typeof value !== "object") {
    return;
  }
  if (value instanceof Map || value instanceof Set) {
    for (const item of value) readAll(item);
    return;
  }
  for (const key of Object.keys(value)) {
    readAll((value as Record<string, unknown>)[key]);
  }
}
