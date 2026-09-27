#!/usr/bin/env node
/**
 * Checks the site's database connection strings (DATABASE_URL, DIRECT_URL) for the mistakes
 * behind Prisma's P1000 ("authentication failed") and P1001 ("can't reach database server"),
 * without ever showing the password.
 *
 * From web/:
 *   node --env-file=.env scripts/check-database-url.mjs              both variables
 *   node --env-file=.env scripts/check-database-url.mjs --clipboard  one value copied to the
 *                                                                    clipboard, as DATABASE_URL
 *   … --connect   also log in with the local Prisma CLI (`prisma migrate status`)
 *
 * Values never come from the command line, which lands in shell history, and this script never
 * opens .env itself: `--env-file` has Node load it. A password is only ever reported as its
 * length, whether it is percent-encoded, and "same" or "different" from another one. Every line
 * printed, Prisma's included, is scrubbed of it (raw, percent-decoded and percent-encoded) and of
 * any URL's `user:password@` first. A value whose password breaks the URL is never handed to
 * Prisma: a parser could take part of the password for the host and send the rest there.
 *
 * An unquoted '#' in .env ends the value there (Node's --env-file, like dotenv), and what is left
 * of the password can look like a host, port, database or parameters. So a value with no '@'
 * shows nothing after its scheme, and one whose "host" looks like a piece of the password shows
 * nothing after its last '@' and is never sent anywhere: an empty host, one with characters no
 * host name has, brackets around something that isn't an IPv6 address, no dot, no top-level
 * domain, a port that isn't a number, nothing after it, or any host but Supabase's shared pooler
 * under a `prisma.<ref>` user (only that pooler takes one). These are heuristics: under a user
 * without `.<ref>`, a piece that happens to look like `name.tld:port/db` still shows, so quote
 * .env values that hold a '#'. It is never sent anywhere either: --connect logs in only at
 * Supabase's hosts and this computer. A value longer than MAX_VALUE_LENGTH isn't read at all.
 *
 * --connect is not entirely read-only: `prisma migrate status` reads the migrations table, but it
 * also creates the schema it checks when that doesn't exist. The default, public, exists in every
 * Supabase database; a value with `?schema=` naming any other schema is not tested, so a typo
 * there can't leave a stray schema on the server.
 *
 * Exit code 0 when nothing is a problem, 1 otherwise. The checks are pure (`analyze`,
 * `comparePair`, `interpretPrismaStatus`), and `main` takes the environment, the clipboard and
 * Prisma as arguments, so tests/unit/check-database-url.test.ts drives all of it with fakes.
 */
import { spawnSync } from "node:child_process";
import { existsSync, realpathSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** web/: Prisma runs here, against prisma/schema.prisma and prisma/migrations. */
export const WEB_DIR = path.resolve(import.meta.dirname, "..");
export const ROLES = /** @type {const} */ (["DATABASE_URL", "DIRECT_URL"]);
const CONNECT_TIMEOUT_MS = 60_000;
/**
 * Longer than any real connection string (Supabase's are under 200 characters). A longer value
 * (a log still on the clipboard, say) is refused before anything reads it, and is never a scrub
 * form: nothing of it is printed, so there is nothing to scrub.
 */
export const MAX_VALUE_LENGTH = 2048;
/** Why --connect doesn't log in with a value that names a schema other than public. */
const OTHER_SCHEMA =
  "the value names a schema other than public (schema=…), and prisma migrate status would create that schema if it doesn't exist: test a copy without ?schema=, or with schema=public";
/** Why --connect doesn't log in with a value that breaks, or may be cut short. */
const FIX_FIRST =
  "fix the problems above first (a parser could send part of the password to the wrong host)";
/** Why --connect doesn't log in with a value some of which didn't decode. */
const UNDECODABLE =
  "part of the value didn't decode as UTF-8, so the server would get a different password";
/**
 * Why --connect doesn't log in at any other host: a value cut short by an unquoted '#' can leave
 * a piece of the password that looks like `name.tld/db`, and logging in there would send the
 * start of the password to whoever answers for that name.
 */
const HOST_ELSEWHERE =
  "--connect logs in only at Supabase's hosts and this computer, and this host is neither (were it a piece of a password an unquoted '#' cut short, logging in would send it the start of the password)";
const HOST_PARAM =
  "the value has a host parameter (?host= or ?hostaddr=), and Prisma would log in there instead of at the host checked above";
/**
 * The cut reason for a `prisma.<ref>` user with a host other than the shared pooler: Supavisor's
 * shared pooler is the only host that takes a `.<ref>` user, so the "host" is more likely a piece
 * of the password than a real one.
 */
const REF_ELSEWHERE =
  "it isn't Supabase's shared pooler, the only host a user ending in .<ref> logs in at";
/** Printed instead of any unexpected error, whose message or stack could hold a value. */
const FAILED =
  "The check stopped on an unexpected error (not shown: it could hold a password).\n";

/** @typedef {"DATABASE_URL" | "DIRECT_URL"} Role */
/** @typedef {"ok" | "warn" | "problem"} Level */
/**
 * @typedef {object} Finding
 * @property {Level} level
 * @property {string} check Stable id of the check (tests and tooling match on it).
 * @property {string} message Plain English. Never holds the password.
 * @property {Role} [only] Set when the check applies to this variable only.
 */
/** @typedef {"pooler" | "direct" | "api" | "local" | "other" | "none"} HostKind */
/**
 * @typedef {object} Facts
 * @property {string} [scheme]
 * @property {string} [user]
 * @property {string} [role] The user without its `.<ref>` suffix.
 * @property {string} [userRef]
 * @property {string} [host]
 * @property {HostKind} hostKind
 * @property {string} [hostRef]
 * @property {string} [region]
 * @property {number} [port]
 * @property {string} [database]
 * @property {Record<string, string>} params
 * @property {number} [passwordLength] In characters, once percent-decoded.
 * @property {boolean} [passwordEncoded]
 * @property {boolean} parses Node's URL parser (WHATWG, like the Rust `url` crate Prisma's
 *   engines use) reads the same user, password, host, port and database as this check does.
 * @property {boolean} [cut] What follows the last '@' may be a piece of the password (the value
 *   looks cut short by an unquoted '#' in .env): none of it is shown, and --connect doesn't
 *   send the value anywhere.
 * @property {boolean} [otherSchema] The value names a `schema` other than public, which
 *   `prisma migrate status` would create: --connect doesn't log in with it.
 * @property {boolean} [undecodable] The value holds U+FFFD: some of it didn't decode as UTF-8
 *   (a clipboard or file in another encoding), so its password isn't the one meant, and
 *   --connect doesn't log in with it.
 */
/**
 * @typedef {object} Analysis
 * @property {Role} role
 * @property {boolean} present Whether there is a value to check: set, not blank, and not too
 *   long to be a connection string. Nothing reads a value that isn't.
 * @property {Finding[]} findings
 * @property {Facts} facts
 */
/**
 * @typedef {object} Section
 * @property {string} title
 * @property {Finding[]} findings
 * @property {boolean} counted Whether its findings count towards the summary and exit code.
 */
/**
 * @typedef {object} PrismaRun
 * @property {number | null} status
 * @property {string} stdout
 * @property {string} stderr
 * @property {boolean} [timedOut]
 * @property {string} [error] Why Prisma couldn't be run at all (this script's own words).
 */
/**
 * @typedef {object} Deps
 * @property {Record<string, string | undefined>} env
 * @property {() => string} readClipboard
 * @property {(value: string) => PrismaRun} runPrisma
 * @property {(text: string) => void} write
 */

const SCHEME = /^([A-Za-z][A-Za-z0-9+.-]*):\/\//;
// Supavisor's shared pooler: aws-<n>-<region>.pooler.supabase.com.
const POOLER_HOST = /^(?:aws-\d+-)?([a-z0-9-]+)\.pooler\.supabase\.com$/;
const DIRECT_HOST = /^db\.([a-z0-9]+)\.supabase\.co$/;
const API_HOST = /^([a-z0-9]+)\.supabase\.co$/;
const LOCAL_HOST =
  /^(?:localhost|127(?:\.\d{1,3}){3}|\[::1\]|host\.docker\.internal)$/;
const PROJECT_REF = /^[a-z0-9]{20}$/;
// A Postgres role name, optionally with Supavisor's `.<ref>`. Anything else isn't printed: a
// value missing its `user:` could have the password there.
const PRINTABLE_USER =
  /^[A-Za-z_][A-Za-z0-9_$-]{0,62}(?:\.[A-Za-z0-9]{1,40})?$/;
// A host name (lowercased). An IPv6 address in brackets is checked apart, by isIPv6Literal.
const PRINTABLE_HOST = /^[a-z0-9.-]{1,253}$/;
// Parameters whose values are safe to print (sslpassword=…, say, isn't).
const SAFE_PARAMS = new Set([
  "pgbouncer",
  "connection_limit",
  "pool_timeout",
  "connect_timeout",
  "socket_timeout",
  "sslmode",
  "sslaccept",
  "schema",
  "statement_cache_size",
  "application_name",
]);
const QUOTE_PAIRS = [
  ['"', '"'],
  ["'", "'"],
  ["`", "`"],
  ["\u201C", "\u201D"],
  ["\u2018", "\u2019"],
  ["\u201E", "\u201C"],
  ["\u00AB", "\u00BB"],
];
const QUOTE_MARK = /^["'`\u2018\u2019\u201A\u201C\u201D\u201E\u00AB\u00BB]$/;
const EDGE_SPACE_START = /^[\s\u200B-\u200D\u2060\uFEFF]+/;
const EDGE_SPACE_END = /[\s\u200B-\u200D\u2060\uFEFF]+$/;
const INVISIBLE = new RegExp(
  "[\\u0000-\\u0009\\u000B\\u000C\\u000E-\\u001F\\u007F\\u00A0\\u00AD\\u200B-\\u200F\\u2028\\u2029\\u202F\\u2060\\uFEFF]",
);
const ANSI_COLOUR = new RegExp("\\u001b\\[[0-9;]*m", "g");
/** Characters that end the password in a URL parser: [check, character, name, encoding, effect]. */
const BREAKING = /** @type {const} */ ([
  ["password-hash", "#", "'#' (hash)", "%23", "the rest is read as a fragment"],
  [
    "password-question",
    "?",
    "'?' (question mark)",
    "%3F",
    "the rest is read as parameters",
  ],
  [
    "password-slash",
    "/",
    "'/' (slash)",
    "%2F",
    "the rest is read as the database name",
  ],
]);
/** Characters a URL may not hold unencoded, which parsers mostly encode for you. */
const UNSAFE_NAMES = /** @type {Record<string, string>} */ ({
  "[": "brackets",
  "]": "brackets",
  "{": "braces",
  "}": "braces",
  "<": "angle brackets",
  ">": "angle brackets",
  '"': "double quotes",
  "\\": "backslashes",
  "^": "carets",
  "|": "pipes",
  "`": "backticks",
});

/**
 * Percent-decodes the valid %XX runs of `text` and leaves everything else as written, as the
 * server ends up receiving it.
 * @param {string} text
 */
export function percentDecode(text) {
  return text.replace(/(?:%[0-9A-Fa-f]{2})+/g, (run) => {
    try {
      return decodeURIComponent(run);
    } catch {
      return run;
    }
  });
}

/**
 * Trims the value and takes off surrounding quotes, reporting both through `add` when given.
 * @param {string} value
 * @param {(level: Level, check: string, message: string) => void} [add]
 */
function unwrap(value, add) {
  const report = add ?? (() => undefined);
  const lead = EDGE_SPACE_START.exec(value)?.[0] ?? "";
  const trail =
    lead.length === value.length ? "" : (EDGE_SPACE_END.exec(value)?.[0] ?? "");
  if (lead)
    report(
      "problem",
      "whitespace",
      `starts with ${spaceKind(lead)}: remove it`,
    );
  if (trail) {
    report(
      "problem",
      /^[\r\n]+$/.test(trail) ? "trailing-line-break" : "whitespace",
      `ends with ${spaceKind(trail)}: remove it (a stored value keeps it)`,
    );
  }
  let inner = value.slice(lead.length, value.length - trail.length);
  if (/[\r\n]/.test(inner)) {
    report(
      "problem",
      "line-break",
      "has a line break inside: the value must be one line",
    );
  }
  if (INVISIBLE.test(inner.replace(/[\r\n]/g, ""))) {
    report(
      "problem",
      "invisible",
      "has an invisible character inside (a tab, or a non-breaking or zero-width space): retype it",
    );
  }
  const first = inner.charAt(0);
  const last = inner.charAt(inner.length - 1);
  if (
    inner.length >= 2 &&
    QUOTE_PAIRS.some(([o, c]) => first === o && last === c)
  ) {
    report(
      "problem",
      "quotes",
      "is wrapped in quotes: store the value without them (Vercel keeps them as part of it)",
    );
    inner = inner.slice(1, -1).trim();
  } else if (QUOTE_MARK.test(first) || QUOTE_MARK.test(last)) {
    report(
      "problem",
      "quotes",
      "starts or ends with a stray quote mark: remove it",
    );
    if (QUOTE_MARK.test(first)) inner = inner.slice(1);
    if (QUOTE_MARK.test(inner.charAt(inner.length - 1)))
      inner = inner.slice(0, -1);
    inner = inner.trim();
  }
  return inner;
}

/** @param {string} text */
function spaceKind(text) {
  if (/[\r\n]/.test(text)) return "a line break";
  if (/^ +$/.test(text)) return "a space";
  return "an invisible character (tab, non-breaking or zero-width space)";
}

/**
 * Splits a value the way it was meant, before any URL parser sees it: the host follows the
 * LAST '@', the password is everything between the first ':' and that '@'. A password with '#',
 * '?' or '/' in it is read wrongly by URL parsers, so they can't be trusted to find it.
 * @param {string} cleaned
 */
function split(cleaned) {
  const scheme = SCHEME.exec(cleaned);
  if (!scheme) return undefined;
  const rest = cleaned.slice(scheme[0].length);
  const at = rest.lastIndexOf("@");
  const userinfo = at < 0 ? undefined : rest.slice(0, at);
  const after = at < 0 ? rest : rest.slice(at + 1);
  const colon = userinfo === undefined ? -1 : userinfo.indexOf(":");
  const user =
    userinfo === undefined
      ? ""
      : colon < 0
        ? userinfo
        : userinfo.slice(0, colon);
  const rawPassword =
    userinfo === undefined || colon < 0 ? undefined : userinfo.slice(colon + 1);
  const end = after.search(/[/?#]/);
  const hostPort = end < 0 ? after : after.slice(0, end);
  let tail = end < 0 ? "" : after.slice(end);
  const hash = tail.indexOf("#");
  const fragment = hash < 0 ? undefined : tail.slice(hash + 1);
  if (hash >= 0) tail = tail.slice(0, hash);
  const question = tail.indexOf("?");
  const pathname = question < 0 ? tail : tail.slice(0, question);
  const query = question < 0 ? undefined : tail.slice(question + 1);
  const hp = /^(\[[^\]]*\]|[^:]*)(?::([\s\S]*))?$/.exec(hostPort);
  return {
    scheme: scheme[1] ?? "",
    userinfo,
    user,
    rawPassword,
    host: hp?.[1] ?? hostPort,
    portText: hp?.[2],
    pathname,
    query,
    fragment,
  };
}

/** @typedef {NonNullable<ReturnType<typeof split>>} Parts */

/**
 * Whether Node's URL parser (WHATWG, as Prisma's engines parse) reads what `split` reads. When it
 * doesn't, the password has broken the URL.
 * @param {string} cleaned
 * @param {Parts} parts
 */
function parserAgrees(cleaned, parts) {
  let url;
  try {
    url = new URL(cleaned);
  } catch {
    return false;
  }
  const port =
    parts.portText === undefined || parts.portText === ""
      ? ""
      : String(Number(parts.portText));
  return (
    url.hostname.toLowerCase() === parts.host.toLowerCase() &&
    url.port === port &&
    percentDecode(url.username) === percentDecode(parts.user) &&
    percentDecode(url.password) === percentDecode(parts.rawPassword ?? "") &&
    percentDecode(url.pathname) === percentDecode(parts.pathname)
  );
}

/**
 * Whether `host` (lowercased) is an IPv6 address in brackets that a URL parser accepts.
 * @param {string} host
 */
function isIPv6Literal(host) {
  if (!/^\[[0-9a-f:.]{2,45}\]$/.test(host)) return false;
  try {
    return new URL(`http://${host}/`).hostname.startsWith("[");
  } catch {
    return false;
  }
}

/**
 * Why what follows the last '@' may be a piece of the password rather than a host, or undefined
 * when it looks like a host. Node's --env-file (and dotenv) end an unquoted value at its first
 * '#', so a password with an '@' before its '#' leaves `user:start@piece`: the piece is taken
 * for the host, and a '/', '?' or ':' in it for the database, parameters or port. Decided before
 * any of those is read, so none of them is ever shown for a piece.
 * @param {Parts} parts
 */
function cutReason(parts) {
  const host = parts.host.toLowerCase();
  const noPort = parts.portText === undefined || parts.portText === "";
  const noDatabase = parts.pathname === "" || parts.pathname === "/";
  // A password with "@/", "@?" or "@:" in it leaves no host at all.
  if (!host) return "there is no host";
  if (noPort && noDatabase) return "no port or database follows it";
  if (!noPort && !/^\d+$/.test(parts.portText ?? "")) {
    return "the port after it isn't a number";
  }
  if (host.startsWith("[")) {
    return isIPv6Literal(host)
      ? undefined
      : "it is in brackets but isn't an IPv6 address";
  }
  if (!PRINTABLE_HOST.test(host)) {
    return "it has characters a host name can't have";
  }
  if (LOCAL_HOST.test(host)) return undefined;
  if (!host.includes(".")) return "it has no dot and isn't this computer";
  // A host name ends in a top-level domain (letters, or punycode), or is an IPv4 address; a
  // piece of a password with a '.' in it mostly doesn't.
  const top = host.replace(/\.$/, "").split(".").at(-1) ?? "";
  const ipv4 = /^\d{1,3}(?:\.\d{1,3}){3}$/.test(host);
  if (!ipv4 && !/^(?:[a-z]{2,63}|xn--[a-z0-9-]{1,59})$/.test(top)) {
    return "it doesn't end in a top-level domain";
  }
  return undefined;
}

/**
 * Whether a query string names a schema other than public. `prisma migrate status` creates the
 * schema it checks when that doesn't exist, so such a value is never handed to it.
 * @param {string | undefined} query
 */
function namesOtherSchema(query) {
  if (query === undefined) return false;
  return new URLSearchParams(query)
    .getAll("schema")
    .some((schema) => schema !== "public");
}

/**
 * The role and project a value logs in as, where they can be printed.
 * @param {Parts | undefined} parts
 */
function identity(parts) {
  const user = parts?.user ?? "";
  // Only a host that follows the password: anything else could be a piece of it.
  const host =
    parts && parts.userinfo !== undefined && !cutReason(parts)
      ? parts.host.toLowerCase()
      : "";
  const printable = PRINTABLE_USER.test(user);
  const userMatch = printable ? /^(.*)\.([A-Za-z0-9]+)$/.exec(user) : null;
  const hostRef = DIRECT_HOST.exec(host)?.[1] ?? API_HOST.exec(host)?.[1];
  const userRef = userMatch?.[2];
  return {
    role: printable ? (userMatch?.[1] ?? user) : undefined,
    userRef,
    hostRef,
    ref: userRef ?? hostRef,
  };
}

/**
 * What kind of host a (lowercased, printable) host name is.
 * @param {string} host
 * @returns {Exclude<HostKind, "none">}
 */
function hostKindOf(host) {
  if (POOLER_HOST.test(host)) return "pooler";
  if (DIRECT_HOST.test(host)) return "direct";
  if (API_HOST.test(host)) return "api";
  if (LOCAL_HOST.test(host)) return "local";
  return "other";
}

/**
 * Whether the user is one only Supavisor's shared pooler takes: a role with a `.<ref>` that is a
 * project ref, or prisma/postgres with any `.<ref>`. A role that merely has a dot (john.doe) isn't.
 * @param {ReturnType<typeof identity>} who
 */
function poolerOnlyUser(who) {
  if (!who.userRef) return false;
  return (
    PROJECT_REF.test(who.userRef) ||
    who.role === "prisma" ||
    who.role === "postgres"
  );
}

/** @param {number} n @param {string} word */
function plural(n, word) {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

/**
 * Checks one connection string as the given variable. Pure: the password never leaves it,
 * except as its length, whether it is percent-encoded, and the names of characters it holds.
 * @param {string | undefined} value
 * @param {Role} role
 * @returns {Analysis}
 */
export function analyze(value, role) {
  /** @type {Finding[]} */
  const findings = [];
  /** @type {Facts} */
  const facts = { hostKind: "none", params: {}, parses: false };
  /** @type {Analysis} */
  const result = { role, present: false, findings, facts };
  /**
   * @param {Level} level
   * @param {string} check
   * @param {string} message
   * @param {Role} [only]
   */
  const add = (level, check, message, only) => {
    findings.push(
      only ? { level, check, message, only } : { level, check, message },
    );
  };

  if (value === undefined) {
    add(
      "problem",
      "empty",
      "not set (from web/: node --env-file=.env scripts/check-database-url.mjs)",
    );
    return result;
  }
  if (value.length > MAX_VALUE_LENGTH) {
    add(
      "problem",
      "too-long",
      `too long to be a connection string (over ${MAX_VALUE_LENGTH} characters): not checked, and none of it is shown`,
    );
    return result;
  }
  if (value.trim() === "") {
    add(
      "problem",
      "empty",
      value === "" ? "empty" : "only spaces or line breaks",
    );
    return result;
  }
  result.present = true;

  // The whole value, before any parsing.
  const cleaned = unwrap(value, add);
  if (value.includes("�")) {
    // What a decoder puts in place of bytes that aren't UTF-8 (pbpaste without a UTF-8 locale,
    // a file saved as Latin-1): the character that was there is lost, so the password is not the
    // one meant, and a login with it fails as if the password were wrong.
    facts.undecodable = true;
    add(
      "problem",
      "encoding",
      "has a U+FFFD replacement character: part of it didn't decode as UTF-8, so a character in it is lost (copy it again, or save the file as UTF-8)",
    );
  }
  const parts = split(cleaned);
  if (!parts) {
    add("problem", "scheme", "doesn't start with postgresql://");
    return result;
  }
  const scheme = parts.scheme.toLowerCase();
  facts.scheme = scheme;
  if (scheme !== "postgresql" && scheme !== "postgres") {
    add(
      "problem",
      "scheme",
      `scheme ${parts.scheme}:// isn't one Prisma's PostgreSQL provider takes: use postgresql://`,
    );
  } else if (parts.scheme !== scheme) {
    add("warn", "scheme", `write the scheme in lowercase: ${scheme}://`);
  } else {
    add("ok", "scheme", `scheme ${scheme}://`);
  }

  // Without an '@' there is no password to find, so nothing after the scheme is shown: a value
  // cut short at an unquoted '#' leaves `user:start-of-password`, and a '/', '?' or ':' in that
  // start would otherwise print as the database, parameters or port.
  if (parts.userinfo === undefined) {
    add(
      "problem",
      "userinfo",
      "no '@', so no user and password (expected user:password@host after the scheme); the rest isn't shown, as it may hold the password. In a .env file an unquoted '#' starts a comment and cuts the value short: quote the value or write '#' as %23",
    );
    return result;
  }

  // Host first: what the user must look like depends on it. When it may be a piece of the
  // password, it is neither shown nor classified.
  const who = identity(parts);
  let cut = cutReason(parts);
  /** @type {HostKind} */
  let hostKind = "none";
  if (!cut) {
    const kind = hostKindOf(parts.host.toLowerCase());
    // Only the shared pooler takes prisma.<ref>, so with any other host a piece of the password
    // (Xk2@Lp2.Rk/Mx9Wd4 cut short at its '#') is likelier than a real host.
    if (poolerOnlyUser(who) && (kind === "other" || kind === "local"))
      cut = REF_ELSEWHERE;
    else hostKind = kind;
  }
  const host = hostKind === "none" ? "" : parts.host.toLowerCase();
  if (host) facts.host = host;
  facts.hostKind = hostKind;
  facts.hostRef = who.hostRef;
  if (hostKind === "pooler") facts.region = POOLER_HOST.exec(host)?.[1];
  const supabase = hostKind === "pooler" || hostKind === "direct";

  // User and password: raw-string checks, since a bad password breaks parsing silently.
  if (parts.rawPassword === undefined) {
    add(
      "problem",
      "password",
      "no password: expected user:password@host (the part before '@' isn't shown, in case it is the password)",
    );
  } else {
    const user = parts.user;
    if (user === "") {
      add("problem", "user", "no user before the password");
    } else if (!PRINTABLE_USER.test(user)) {
      add(
        "problem",
        "user",
        user.includes("@")
          ? "more than one '@' before the host: the user has an unencoded '@' (write it as %40)"
          : "the user has characters a role name can't have (not shown)",
      );
    } else {
      facts.user = user;
      facts.role = who.role;
      facts.userRef = who.userRef;
      userChecks(add, hostKind, user, who, cut === REF_ELSEWHERE);
    }

    const raw = parts.rawPassword;
    const decoded = percentDecode(raw);
    facts.passwordLength = [...decoded].length;
    facts.passwordEncoded = /%[0-9A-Fa-f]{2}/.test(raw);
    if (raw === "") {
      add("problem", "password", "the password is empty");
    } else {
      add(
        "ok",
        "password",
        `password <${plural(facts.passwordLength, "character")}>, ${facts.passwordEncoded ? "percent-encoded" : "not percent-encoded"}`,
      );
    }
    // Supabase's template reads postgres.<ref>:[YOUR-PASSWORD]@…: the brackets mark where the
    // password goes and aren't part of it. Kept (even percent-encoded), they reach the server, and
    // the login fails with P1000. Case and encoding don't matter: decoded is what the server gets.
    const placeholder = /^\[?your-password\]?$/i.test(decoded);
    const bracketed =
      placeholder ||
      (decoded.length >= 2 && decoded.startsWith("[") && decoded.endsWith("]"));
    if (placeholder) {
      add(
        "problem",
        "password-placeholder",
        "the password is still the placeholder from Supabase's template: put the database password in its place, without the brackets",
      );
    } else if (bracketed) {
      add(
        "problem",
        "password-brackets",
        "the password is wrapped in [ ]: the brackets in Supabase's template aren't part of the password; remove them",
      );
    }
    if (raw.includes("@")) {
      add(
        "problem",
        "password-at",
        "more than one '@' before the host: the password has an unencoded '@' (write it as %40)",
      );
    }
    for (const [check, char, name, code, effect] of BREAKING) {
      if (raw.includes(char)) {
        add(
          "problem",
          check,
          `the password has an unencoded ${name}: write it as ${code} (unencoded, ${effect})`,
        );
      }
    }
    if (raw.includes(" ")) {
      add(
        "problem",
        "password-space",
        "the password has an unencoded space: write it as %20",
      );
    }
    if (raw.includes(":")) {
      add(
        "warn",
        "password-colon",
        "the password has an unencoded ':' (colon): most parsers accept it, but write it as %3A to be safe",
      );
    }
    if (/%(?![0-9A-Fa-f]{2})/.test(raw)) {
      add(
        "problem",
        "password-percent",
        "the password has a '%' not followed by two hex digits: write a literal '%' as %25",
      );
    }
    const unsafe = new Set();
    // Brackets that are to be removed need no encoding advice: encoded, they'd still be sent.
    const kept = bracketed ? raw.replace(/^\[/, "").replace(/\]$/, "") : raw;
    for (const char of kept) {
      const name = UNSAFE_NAMES[char];
      if (name) unsafe.add(name);
      else if (char.charCodeAt(0) > 0x7e) unsafe.add("non-ASCII characters");
    }
    if (unsafe.size > 0) {
      add(
        "warn",
        "password-unsafe",
        `the password has characters a URL can't hold unencoded (${[...unsafe].join(", ")}): percent-encode them`,
      );
    }
    if (raw.includes("$")) {
      add(
        "warn",
        "password-dollar",
        "the password has a '$': tools that expand variables in .env files (Next.js; Prisma for ${…}) can change it, so write it as %24",
      );
    }
    if (facts.passwordEncoded && /%[0-9A-Fa-f]{2}/.test(decoded)) {
      add(
        "warn",
        "password-double-encoded",
        "the password still has %XX in it once decoded: it may be percent-encoded twice",
      );
    }
  }

  // Parsed: does a real URL parser read what was meant?
  facts.parses = parserAgrees(cleaned, parts);
  if (!facts.parses) {
    const broken = findings.some(
      (f) => f.check.startsWith("password-") && f.level === "problem",
    );
    add(
      "problem",
      "parse",
      broken
        ? "because of those characters, URL parsers (Prisma's too) read a different password, host or database: encode them"
        : "URL parsers (Prisma's too) can't read this value as intended",
    );
  }

  // Host.
  if (parts.fragment !== undefined) {
    add(
      "problem",
      "fragment",
      "has a '#' after the host: remove it and what follows",
    );
  }
  if (cut) {
    // Nothing after the last '@' is shown or sent anywhere: port, database and parameters could
    // all be pieces of the password.
    facts.cut = true;
    const what =
      parts.host === ""
        ? "no host after '@' (what follows isn't shown: it may be part of the password)"
        : `the host isn't shown: ${cut}, so it may be part of the password`;
    add(
      "problem",
      "host",
      `${what}. Was the value cut short by an unquoted '#' in .env? Quote the value there, or write '#' as %23 and '@' as %40`,
    );
    if (!/^\d*$/.test(parts.portText ?? "")) {
      add("problem", "port", "the port isn't a number (not shown)");
    }
    return result;
  }
  // Not cut: the host is a printable host name or IPv6 address (cutReason saw to it).
  if (hostKind === "pooler") {
    add("ok", "host", `host ${host}: Supabase shared pooler (${facts.region})`);
  } else if (hostKind === "direct") {
    add(
      "ok",
      "host",
      `host ${host}: Supabase direct host (project ${who.hostRef})`,
    );
  } else if (hostKind === "api") {
    add(
      "problem",
      "host",
      `host ${host} is the project's API address, not its database: use the pooler host (Supabase > Connect > ORMs > Prisma)`,
    );
  } else if (hostKind === "local") {
    add("warn", "host", `host ${host}: this computer, not Supabase`);
  } else {
    add(
      "warn",
      "host",
      host.includes("supabase")
        ? `host ${host} isn't a Supabase database host (those are aws-N-<region>.pooler.supabase.com and db.<ref>.supabase.co)`
        : `host ${host} isn't a Supabase host`,
    );
  }

  // Parameters (read before the port: transaction mode needs pgbouncer=true).
  const params = /** @type {Record<string, string>} */ ({});
  const shown = [];
  const repeated = new Set();
  if (parts.query) {
    for (const [key, val] of new URLSearchParams(parts.query)) {
      const keyPrintable = /^[A-Za-z0-9_.-]{1,40}$/.test(key);
      if (key in params && keyPrintable) repeated.add(key);
      params[key] = val;
      if (!keyPrintable) shown.push("a parameter (not shown)");
      else if (SAFE_PARAMS.has(key) && /^[\w.,:-]{0,40}$/.test(val))
        shown.push(`${key}=${val}`);
      else shown.push(`${key}=…`);
    }
  }
  facts.params = params;
  if (namesOtherSchema(parts.query)) facts.otherSchema = true;

  // Port and pooler mode.
  const portText = parts.portText;
  /** @type {number | undefined} */
  let port;
  let portOk = true;
  if (portText !== undefined && portText !== "") {
    if (/^\d{1,5}$/.test(portText) && Number(portText) <= 65535)
      port = Number(portText);
    else portOk = false;
  }
  facts.port = port;
  const effective = port ?? 5432;
  const portName =
    port === undefined
      ? "port 5432 (none given, so the default)"
      : `port ${port}`;
  /** @type {"transaction" | "session" | "direct" | "dedicated" | "plain" | "bad"} */
  let mode = "plain";
  if (!portOk) mode = "bad";
  else if (hostKind === "pooler")
    mode =
      effective === 6543
        ? "transaction"
        : effective === 5432
          ? "session"
          : "bad";
  else if (hostKind === "direct")
    mode =
      effective === 5432 ? "direct" : effective === 6543 ? "dedicated" : "bad";

  if (!portOk) {
    add("problem", "port", "the port isn't a number");
  } else if (mode === "bad") {
    add(
      "problem",
      "port",
      hostKind === "pooler"
        ? `${portName}: the pooler listens on 6543 (transaction mode) and 5432 (session mode)`
        : `${portName}: the direct host listens on 5432 (and on 6543 for the dedicated pooler)`,
    );
  } else if (role === "DATABASE_URL") {
    const transaction = mode === "transaction" || mode === "dedicated";
    const label =
      mode === "dedicated"
        ? "dedicated pooler, transaction mode"
        : "transaction mode";
    if (transaction && params.pgbouncer === "true") {
      add("ok", "port", `${portName}: ${label}, with pgbouncer=true`, role);
    } else if (transaction && params.pgbouncer === undefined) {
      add(
        "problem",
        "pgbouncer",
        `${portName} is ${label}: add pgbouncer=true, or queries fail with "prepared statement … already exists"`,
        role,
      );
    } else if (transaction) {
      add(
        "problem",
        "pgbouncer",
        `${portName} is ${label}: pgbouncer must be true`,
        role,
      );
    } else if (mode === "session") {
      add(
        "warn",
        "port",
        `${portName} is session mode: it works, but on Vercel use transaction mode (port 6543 with pgbouncer=true)`,
        role,
      );
    } else if (mode === "direct") {
      add("ok", "port", `${portName}: direct connection`, role);
    } else {
      add("ok", "port", portName, role);
    }
    if (supabase) {
      const limit = params.connection_limit;
      if (limit === undefined) {
        add(
          "warn",
          "connection-limit",
          "no connection_limit: add connection_limit=1 for Vercel, where every function instance opens its own pool",
          role,
        );
      } else if (limit === "1") {
        add("ok", "connection-limit", "connection_limit=1", role);
      } else {
        add(
          "warn",
          "connection-limit",
          "connection_limit isn't 1: 1 is recommended on Vercel",
          role,
        );
      }
    }
  } else {
    if (mode === "transaction" || mode === "dedicated") {
      add(
        "problem",
        "port",
        `${portName} is transaction mode: prisma migrate needs session mode (port 5432 on the pooler host) or the direct host`,
        role,
      );
    } else if (mode === "session") {
      add(
        "ok",
        "port",
        `${portName}: session mode, right for migrations`,
        role,
      );
    } else if (mode === "direct") {
      add(
        "ok",
        "port",
        `${portName}: direct connection, right for migrations`,
        role,
      );
    } else {
      add("ok", "port", portName, role);
    }
  }
  if (hostKind === "direct") {
    add(
      "warn",
      "ipv6",
      role === "DATABASE_URL"
        ? "the direct host has only an IPv6 address (without Supabase's IPv4 add-on): Vercel can't reach it (P1001), so use the pooler host"
        : "the direct host has only an IPv6 address (without Supabase's IPv4 add-on): Vercel, most CI and IPv4-only networks can't reach it; the pooler's session mode (port 5432) works everywhere",
      role,
    );
  }

  // Database.
  if (parts.pathname === "" || parts.pathname === "/") {
    add("warn", "database", "no database name: add /postgres after the port");
  } else {
    const name = percentDecode(parts.pathname.slice(1));
    if (!/^[A-Za-z0-9_.-]{1,63}$/.test(name)) {
      add(
        "warn",
        "database",
        "the database name has unusual characters (not shown)",
      );
    } else {
      facts.database = name;
      if (supabase && name !== "postgres") {
        add(
          "warn",
          "database",
          `database ${name}: Supabase's database is postgres`,
        );
      } else {
        add("ok", "database", `database ${name}`);
      }
    }
  }

  add(
    "ok",
    "params",
    shown.length ? `parameters ${shown.join(", ")}` : "no parameters",
  );
  for (const key of repeated) {
    add("warn", "params", `${key} is given more than once: keep one`);
  }
  return result;
}

/**
 * Checks the user against the host: Supavisor's shared pooler names the project in the user
 * (`prisma.<ref>`), the direct host names it in the host name.
 * @param {(level: Level, check: string, message: string) => void} add
 * @param {HostKind} hostKind
 * @param {string} user
 * @param {ReturnType<typeof identity>} who
 * @param {boolean} refElsewhere The user is one only the shared pooler takes, and the host
 *   (hidden) isn't the shared pooler.
 */
function userChecks(add, hostKind, user, who, refElsewhere) {
  const role = who.role ?? user;
  if (who.userRef && !PROJECT_REF.test(who.userRef)) {
    add(
      "warn",
      "ref",
      `${who.userRef} after the role doesn't look like a project ref (20 lowercase letters)`,
    );
  }
  if (refElsewhere) {
    add(
      "problem",
      "user",
      `user ${user}: a user ending in .<ref> logs in only through Supabase's shared pooler (aws-N-<region>.pooler.supabase.com), and the host isn't it`,
    );
  } else if (hostKind === "pooler") {
    if (!who.userRef) {
      add(
        "problem",
        "user",
        role === "postgres"
          ? `user postgres: the shared pooler can't tell which project that is and refuses the login ("Tenant or user not found"): use prisma.<ref> (README step 4) or postgres.<ref>`
          : `user ${role}: on the shared pooler the user must end in .<project ref>, like prisma.<ref>`,
      );
    } else if (role === "prisma") {
      add("ok", "user", `user ${user} (role prisma, project ${who.userRef})`);
    } else if (role === "postgres") {
      add(
        "ok",
        "user",
        `user ${user} (role postgres, project ${who.userRef}; README sets up a prisma role for the site)`,
      );
    } else {
      add(
        "warn",
        "user",
        `user ${user}: role ${role}, but the site expects prisma (README step 4) or postgres`,
      );
    }
  } else if (hostKind === "direct") {
    if (who.userRef) {
      add(
        "problem",
        "user",
        `user ${user}: on the direct host the user is the role alone (${role}), without .<ref>`,
      );
      if (who.hostRef && who.userRef !== who.hostRef) {
        add(
          "problem",
          "ref",
          `the project in the user (${who.userRef}) isn't the host's (${who.hostRef})`,
        );
      }
    } else {
      add("ok", "user", `user ${user} (project ${who.hostRef}, from the host)`);
    }
  } else {
    add("ok", "user", `user ${user}`);
  }
}

/**
 * Compares two connection strings: the same password (printed only as "same" or "different"),
 * the same role, the same project.
 * @param {string} first
 * @param {string} second
 * @param {readonly [string, string]} [labels]
 * @returns {Finding[]}
 */
export function comparePair(first, second, labels = ROLES) {
  // Not a connection string: nothing of it is read (see analyze).
  if (first.length > MAX_VALUE_LENGTH || second.length > MAX_VALUE_LENGTH)
    return [];
  const [firstLabel, secondLabel] = labels;
  const a = split(unwrap(first));
  const b = split(unwrap(second));
  // Without a ':', what stands before '@' may be the password itself (see analyze): no role or
  // project is read from it.
  /** @param {Parts | undefined} parts */
  const who = (parts) =>
    parts?.rawPassword !== undefined
      ? identity(parts)
      : {
          role: undefined,
          userRef: undefined,
          hostRef: undefined,
          ref: undefined,
        };
  const whoA = who(a);
  const whoB = who(b);
  /** @type {Finding[]} */
  const findings = [];
  const sameRole = whoA.role !== undefined && whoA.role === whoB.role;
  const sameProject = !whoA.ref || !whoB.ref || whoA.ref === whoB.ref;
  if (a?.rawPassword !== undefined && b?.rawPassword !== undefined) {
    const same = percentDecode(a.rawPassword) === percentDecode(b.rawPassword);
    // A password with U+FFFD lost a character in decoding (see analyze): that it differs says
    // nothing about which is out of date.
    const undecodable = `${a.rawPassword}${b.rawPassword}`.includes("�");
    if (same) {
      findings.push({
        level: "ok",
        check: "same-password",
        message:
          a.rawPassword === b.rawPassword
            ? "same password in both"
            : "same password in both (written differently, the same once percent-decoded)",
      });
    } else if (undecodable) {
      findings.push({
        level: "warn",
        check: "same-password",
        message:
          "different passwords, but one didn't decode as UTF-8 (see above): compare them again once it does",
      });
    } else if (sameRole && sameProject) {
      findings.push({
        level: "problem",
        check: "same-password",
        message: `different passwords, though both log in as ${whoA.role}${whoA.ref ? ` on project ${whoA.ref}` : ""}: one of them is out of date`,
      });
    } else {
      findings.push({
        level: "warn",
        check: "same-password",
        message:
          "different passwords (for different roles or projects, so that may be right)",
      });
    }
  }
  if (whoA.role !== undefined && whoB.role !== undefined) {
    findings.push(
      sameRole
        ? {
            level: "ok",
            check: "same-role",
            message: `both log in as role ${whoA.role}`,
          }
        : {
            level: "warn",
            check: "same-role",
            message: `${firstLabel} logs in as ${whoA.role}, ${secondLabel} as ${whoB.role}: use the same role in both`,
          },
    );
  }
  if (whoA.ref && whoB.ref) {
    findings.push(
      whoA.ref === whoB.ref
        ? {
            level: "ok",
            check: "same-project",
            message: `both use project ${whoA.ref}`,
          }
        : {
            level: "problem",
            check: "same-project",
            message: `${firstLabel} is on project ${whoA.ref}, ${secondLabel} on project ${whoB.ref}`,
          },
    );
  }
  return findings;
}

/**
 * Lowercases each UTF-16 code unit on its own (leaving one whose lowercase is longer as it is),
 * so every index into the result is the same index into `text`.
 * @param {string} text
 */
function foldCase(text) {
  let folded = "";
  for (let i = 0; i < text.length; i += 1) {
    const unit = text.charAt(i);
    const lower = unit.toLowerCase();
    folded += lower.length === 1 ? lower : unit;
  }
  return folded;
}

/**
 * Replaces every stretch of `text` covered by any occurrence of any (case-folded) form with
 * "***". Plain string search, never one RegExp built from the forms: V8 refuses a pattern that
 * large, and the error it throws would carry every form in its message.
 * @param {string} text
 * @param {readonly string[]} folded
 */
function hideForms(text, folded) {
  if (!text || folded.length === 0) return text;
  const haystack = foldCase(text);
  const hidden = new Uint8Array(text.length);
  for (const form of folded) {
    for (
      let at = haystack.indexOf(form);
      at >= 0;
      at = haystack.indexOf(form, at + 1)
    ) {
      hidden.fill(1, at, at + form.length);
    }
  }
  let out = "";
  let start = 0;
  while (start < text.length) {
    let end = start;
    const isHidden = hidden[start] === 1;
    while (end < text.length && (hidden[end] === 1) === isHidden) end += 1;
    out += isHidden ? "***" : text.slice(start, end);
    start = end;
  }
  return out;
}

/**
 * A function that takes out of any text every form of the given values' passwords (as
 * written, percent-decoded, percent-encoded in either case, JSON-escaped, and the pieces
 * between URL delimiters), the values themselves, and any `scheme://user:password@`. A value
 * over MAX_VALUE_LENGTH isn't a connection string and adds no form: nothing of it is printed.
 * @param {ReadonlyArray<string | undefined>} values
 * @returns {(text: string) => string}
 */
export function makeScrubber(values) {
  /** @type {Set<string>} */
  const forms = new Set();
  /** @param {string} form */
  const addForm = (form) => {
    forms.add(form);
    forms.add(form.replace(/%[0-9A-Fa-f]{2}/g, (s) => s.toUpperCase()));
    forms.add(form.replace(/%[0-9A-Fa-f]{2}/g, (s) => s.toLowerCase()));
  };
  for (const value of values) {
    if (!value || value.length > MAX_VALUE_LENGTH) continue;
    forms.add(value);
    forms.add(value.trim());
    const cleaned = unwrap(value);
    forms.add(cleaned);
    const parts = split(cleaned);
    // Without a ':', what stands before '@' may well be the password itself.
    const raw = parts?.rawPassword ?? parts?.userinfo;
    if (!raw) continue;
    const decoded = percentDecode(raw);
    addForm(raw);
    addForm(decoded);
    try {
      addForm(encodeURIComponent(decoded));
      addForm(encodeURI(decoded));
    } catch {
      // A lone surrogate can't be encoded, so it can't be printed encoded either.
    }
    try {
      addForm(new URL(cleaned).password);
    } catch {
      // Unparseable: nothing parsed it this way.
    }
    addForm(JSON.stringify(decoded).slice(1, -1));
    // A '.' splits too when there is no ':': a user@host-looking password reads as role.project.
    const delimiters =
      parts?.rawPassword === undefined ? /[@#/?:% .]+/ : /[@#/?:% ]+/;
    for (const piece of [
      ...raw.split(delimiters),
      ...decoded.split(delimiters),
    ]) {
      if (piece.length >= 5) forms.add(piece);
    }
  }
  forms.delete("");
  // Case-insensitive: a tool that changes case (hex digits, say) still gets scrubbed.
  const folded = [...new Set([...forms].map(foldCase))];
  return (text) =>
    hideForms(text, folded).replace(
      /([A-Za-z][A-Za-z0-9+.-]*:\/\/)\S*@/g,
      "$1***@",
    );
}

/**
 * Why --connect won't log in with a value, or undefined when it may. Only a value that parses
 * as meant, isn't cut short, decoded as UTF-8, has a user, a password and a Supabase or local
 * host, and names no schema but public is ever handed to Prisma.
 * @param {Facts} facts
 * @returns {string | undefined}
 */
function connectRefusal(facts) {
  if (
    !facts.parses ||
    facts.cut ||
    !facts.host ||
    !facts.user ||
    facts.passwordLength === undefined
  ) {
    return FIX_FIRST;
  }
  if (facts.undecodable) return UNDECODABLE;
  if (Object.keys(facts.params).some((key) => /^host(?:addr)?$/i.test(key)))
    return HOST_PARAM;
  if (facts.hostKind === "other") return HOST_ELSEWHERE;
  if (facts.otherSchema) return OTHER_SCHEMA;
  return undefined;
}

/** What CLIENT_PING prints when the login and the query worked. */
const PING_OK = "select 1: OK";

/**
 * Run with `node -e` in web/: the site's own generated Prisma client logs in with DATABASE_URL, as
 * the site does, and runs `select 1`. Prints PING_OK, or the error's code and message, which
 * interpretPrismaStatus reads (and scrubs before showing any of it).
 */
const CLIENT_PING = `
const { PrismaClient } = require(require("node:path").join(process.cwd(), "generated", "prisma"));
const db = new PrismaClient();
db.$queryRawUnsafe("select 1")
  .then(() => { process.stdout.write(${JSON.stringify(PING_OK)} + "\\n"); })
  .catch((error) => {
    const code = (error && (error.errorCode || error.code)) || "";
    process.stdout.write(String(code) + "\\n" + String((error && error.message) || error) + "\\n");
    process.exitCode = 1;
  })
  .finally(() => db.$disconnect());
`;

/**
 * How --connect logs in with a value. Prisma Migrate hangs or fails through Supavisor's
 * transaction mode (port 6543, pgbouncer=true: no prepared statements across a session), so a
 * transaction-mode value is tested the way the site uses it: its Prisma client runs `select 1`.
 * Any other value goes through `migrate status`, which reports the migrations too.
 * @param {Pick<Facts, "port" | "params">} facts
 * @returns {"client" | "migrate"}
 */
export function loginMethod(facts) {
  const pgbouncer = Object.entries(facts.params).some(
    ([key, val]) =>
      key.toLowerCase() === "pgbouncer" && val.toLowerCase() === "true",
  );
  return facts.port === 6543 || pgbouncer ? "client" : "migrate";
}

/**
 * Runs Node with `args` in `cwd`, both variables set to `value` (migrate uses DIRECT_URL when it
 * is set, the client DATABASE_URL). Prisma's own update check is off, so the only network contact
 * is the database login.
 * @param {string[]} args
 * @param {string} cwd
 * @param {string} value
 * @returns {PrismaRun}
 */
function runNode(args, cwd, value) {
  const result = spawnSync(process.execPath, args, {
    cwd,
    encoding: "utf8",
    timeout: CONNECT_TIMEOUT_MS,
    maxBuffer: 4 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      DATABASE_URL: value,
      DIRECT_URL: value,
      CHECKPOINT_DISABLE: "1",
      PRISMA_HIDE_UPDATE_MESSAGE: "1",
      NO_COLOR: "1",
      FORCE_COLOR: "0",
    },
  });
  const code = /** @type {NodeJS.ErrnoException | undefined} */ (result.error)
    ?.code;
  return {
    status: result.status,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    timedOut: code === "ETIMEDOUT",
    error:
      result.error && code !== "ETIMEDOUT" ? "Prisma didn't start" : undefined,
  };
}

/**
 * Logs in with `value` (see loginMethod): the local Prisma CLI's `migrate status`, or for a
 * transaction-mode value the site's Prisma client with `select 1`.
 *
 * `migrate status` reads the migrations table and changes nothing, with one exception: it
 * creates the schema it checks (`?schema=`, public when not given) if that doesn't exist. So a
 * value naming any other schema is refused here as well as in `main`: a typo there would
 * otherwise leave a stray schema on the server. So is any value `main` wouldn't log in with
 * (connectRefusal): one that may be cut short, or whose host is neither Supabase's nor local.
 * @param {string} value
 * @param {string} [cwd]
 * @returns {PrismaRun}
 */
export function runPrismaStatus(value, cwd = WEB_DIR) {
  const cleaned = unwrap(value);
  let parsedQuery;
  try {
    parsedQuery = new URL(cleaned).search.slice(1);
  } catch {
    parsedQuery = undefined;
  }
  if (
    namesOtherSchema(split(cleaned)?.query) ||
    namesOtherSchema(parsedQuery)
  ) {
    return {
      status: null,
      stdout: "",
      stderr: "",
      error: OTHER_SCHEMA,
    };
  }
  const { facts } = analyze(value, "DIRECT_URL");
  const refusal = connectRefusal(facts);
  if (refusal) return { status: null, stdout: "", stderr: "", error: refusal };
  if (loginMethod(facts) === "client") {
    if (!existsSync(path.join(cwd, "generated", "prisma", "index.js"))) {
      return {
        status: null,
        stdout: "",
        stderr: "",
        error:
          "the site's Prisma client isn't generated (run npm install in web/)",
      };
    }
    return runNode(["-e", CLIENT_PING], cwd, value);
  }
  const cli = path.join(cwd, "node_modules", "prisma", "build", "index.js");
  if (!existsSync(cli)) {
    return {
      status: null,
      stdout: "",
      stderr: "",
      error: "the Prisma CLI isn't installed (run npm install in web/)",
    };
  }
  return runNode(
    [
      cli,
      "migrate",
      "status",
      "--schema",
      path.join(cwd, "prisma", "schema.prisma"),
    ],
    cwd,
    value,
  );
}

/**
 * Turns the output of a login (`prisma migrate status`, or CLIENT_PING) into findings: the login first, then, when it got in,
 * the migrations. Any line of Prisma's that is printed goes through `scrub` first.
 * @param {PrismaRun} run
 * @param {(text: string) => string} scrub
 * @param {Pick<Facts, "role" | "hostKind">} [facts]
 * @returns {Finding[]}
 */
export function interpretPrismaStatus(run, scrub, facts) {
  if (run.error)
    return [
      { level: "problem", check: "login", message: `not tested: ${run.error}` },
    ];
  if (run.timedOut) {
    return [
      {
        level: "problem",
        check: "login",
        message: `no answer within ${CONNECT_TIMEOUT_MS / 1000} s: host unreachable?`,
      },
    ];
  }
  const text = `${run.stdout}\n${run.stderr}`.replace(ANSI_COLOUR, "");
  const code = /\b(P\d{4})\b/.exec(text)?.[1];
  /** @type {Finding[]} */
  const findings = [];
  if (
    code === "P1000" ||
    /Authentication failed against database server|password authentication failed/i.test(
      text,
    )
  ) {
    findings.push({
      level: "problem",
      check: "login",
      message:
        "login refused (P1000): the server rejected this user and password",
    });
    if (facts?.role === "prisma") {
      findings.push({
        level: "warn",
        check: "login-hint",
        message:
          "the prisma role has its own password: Supabase's \"Reset database password\" changes only postgres's (set prisma's in the SQL editor: alter user prisma with password '…')",
      });
    }
    return findings;
  }
  if (/Tenant or user not found/i.test(text)) {
    return [
      {
        level: "problem",
        check: "login",
        message: `login refused: the pooler knows no such user ("Tenant or user not found"): check the .<ref> after the role`,
      },
    ];
  }
  if (code === "P1001" || /Can't reach database server/i.test(text)) {
    findings.push({
      level: "problem",
      check: "login",
      message:
        "host unreachable (P1001): nothing answered at this host and port",
    });
    if (facts?.hostKind === "direct") {
      findings.push({
        level: "warn",
        check: "login-hint",
        message:
          "the direct host has only an IPv6 address: this network may have no IPv6 route",
      });
    }
    return findings;
  }
  if (text.includes(PING_OK)) {
    return [
      {
        level: "ok",
        check: "login",
        message: "login OK (the site's Prisma client ran select 1)",
      },
      {
        level: "ok",
        check: "migrations",
        message:
          "migrations not read with this value: Prisma Migrate can't run through transaction mode (the session or direct value's check reads them)",
      },
    ];
  }
  const migrations = migrationFinding(text, scrub);
  if (migrations || run.status === 0) {
    findings.push({ level: "ok", check: "login", message: "login OK" });
    findings.push(
      migrations ?? {
        level: "warn",
        check: "migrations",
        message: "migration status unknown",
      },
    );
    return findings;
  }
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  const line =
    lines.find((l) => /error|P\d{4}|FATAL/i.test(l)) ??
    lines.at(-1) ??
    "no message";
  // Scrubbed before it is shortened: a cut could split a password or URL so that neither
  // matches any more.
  const scrubbed = scrub(line);
  const short = scrubbed.length > 200 ? `${scrubbed.slice(0, 200)}…` : scrubbed;
  return [
    {
      level: "problem",
      check: "login",
      message: `other error${code ? ` (${code})` : ""}: ${short}`,
    },
  ];
}

/**
 * @param {string} text
 * @param {(text: string) => string} scrub
 * @returns {Finding | undefined}
 */
function migrationFinding(text, scrub) {
  if (/Database schema is up to date!/.test(text)) {
    return {
      level: "ok",
      check: "migrations",
      message: "migrations up to date",
    };
  }
  const pending =
    /Following migrations? have not yet been applied:\s*\n([\s\S]*?)(?:\n\s*\n|\nTo apply|$)/.exec(
      text,
    );
  if (pending) {
    const names = (pending[1] ?? "").split(/\s+/).filter(Boolean);
    const listed =
      names.slice(0, 5).join(", ") + (names.length > 5 ? ", …" : "");
    return {
      level: "warn",
      check: "migrations",
      message: `${plural(names.length, "migration")} pending (${scrub(listed)}): apply with npm run db:migrate`,
    };
  }
  if (/have failed:/.test(text)) {
    return {
      level: "problem",
      check: "migrations",
      message:
        "a migration failed on this database (npx prisma migrate status says which)",
    };
  }
  if (
    /migration history and the migrations table from your database are different/.test(
      text,
    )
  ) {
    return {
      level: "problem",
      check: "migrations",
      message:
        "the database's migration history differs from prisma/migrations",
    };
  }
  if (/not managed by Prisma Migrate/.test(text)) {
    return {
      level: "warn",
      check: "migrations",
      message: "no migrations applied yet: run npm run db:migrate",
    };
  }
  return undefined;
}

/**
 * Lays the sections out, one line per finding, and scrubs the whole text.
 * @param {Section[]} sections
 * @param {(text: string) => string} scrub
 */
export function formatReport(sections, scrub) {
  const lines = [];
  let problems = 0;
  let warnings = 0;
  for (const section of sections) {
    // A title over nothing (two values with nothing to compare, say) only adds noise.
    if (section.findings.length === 0) continue;
    lines.push(section.title);
    for (const finding of section.findings) {
      lines.push(`  ${finding.level.padEnd(8)}${finding.message}`);
      if (!section.counted) continue;
      if (finding.level === "problem") problems += 1;
      if (finding.level === "warn") warnings += 1;
    }
    lines.push("");
  }
  const warned = warnings ? plural(warnings, "warning") : "";
  lines.push(
    problems
      ? `${plural(problems, "problem")}${warned ? `, ${warned}` : ""}.`
      : `No problems${warned ? ` (${warned})` : ""}.`,
  );
  return { text: `${scrub(lines.join("\n"))}\n`, problems, warnings };
}

const USAGE = `Checks the site's database connection strings without showing the password.

From web/:
  node --env-file=.env scripts/check-database-url.mjs              DATABASE_URL and DIRECT_URL
  node --env-file=.env scripts/check-database-url.mjs --clipboard  a value copied to the clipboard
                                                                   (e.g. from Vercel), as DATABASE_URL
Add --connect to also log in, at Supabase's hosts and this computer only: a transaction-mode
value (port 6543, pgbouncer=true) with the site's own Prisma client (select 1), which is how the
site connects; any other with the local Prisma CLI (prisma migrate status), which also reports the
migrations. migrate status creates the schema it checks if it is missing, so a value whose
?schema= isn't public is not tested.

Values are never taken from the command line. Exit code 0: no problems; 1: problems.
`;

/**
 * The command line. Returns the exit code. Any error is reported only as a fixed message: its
 * own message or stack could hold a value (a RegExp's source, a URL, Prisma's output).
 * @param {string[]} argv
 * @param {Deps} deps
 * @returns {number}
 */
export function main(argv, deps) {
  try {
    return check(argv, deps);
  } catch {
    try {
      deps.write(FAILED);
    } catch {
      // Nowhere left to say it; the exit code still does.
    }
    return 1;
  }
}

/**
 * Whether a value is one this script reads at all: set, not blank, not too long.
 * @param {string | undefined} value
 * @returns {value is string}
 */
function checkable(value) {
  return !!value?.trim() && value.length <= MAX_VALUE_LENGTH;
}

/**
 * `main`'s body.
 * @param {string[]} argv
 * @param {Deps} deps
 * @returns {number}
 */
function check(argv, deps) {
  let clipboard = false;
  let connect = false;
  for (const arg of argv) {
    if (arg === "--clipboard") clipboard = true;
    else if (arg === "--connect") connect = true;
    else if (arg === "--help" || arg === "-h") {
      deps.write(USAGE);
      return 0;
    } else {
      // Not echoed unless it looks like a flag: it could be a pasted connection string.
      const shown = /^--?[a-z][a-z-]{0,30}$/.test(arg)
        ? ` ${arg}`
        : " (not shown)";
      deps.write(
        `Unknown argument${shown}. Values are read from the environment or the clipboard, never the command line.\n\n${USAGE}`,
      );
      return 1;
    }
  }

  /** @type {Section[]} */
  const sections = [];
  /** @type {string[]} */
  const secrets = [];
  for (const role of ROLES) {
    const value = deps.env[role];
    if (checkable(value)) secrets.push(value);
  }
  /** @type {Map<string, Finding[]>} */
  const tested = new Map();
  /**
   * @param {string} value
   * @param {Analysis} analysis
   * @param {string} label
   * @returns {Finding[]}
   */
  const connectFindings = (value, analysis, label) => {
    if (!connect || !analysis.present) return [];
    const refusal = connectRefusal(analysis.facts);
    if (refusal) {
      return [
        { level: "problem", check: "login", message: `not tested: ${refusal}` },
      ];
    }
    const cleaned = unwrap(value);
    let found = tested.get(cleaned);
    if (!found) {
      deps.write(`Logging in with ${label}…\n`);
      found = interpretPrismaStatus(
        deps.runPrisma(cleaned),
        makeScrubber(secrets),
        analysis.facts,
      );
      tested.set(cleaned, found);
    }
    return found;
  };

  if (clipboard) {
    let value;
    try {
      value = deps.readClipboard();
    } catch {
      deps.write("Couldn't read the clipboard (pbpaste, macOS only).\n");
      return 1;
    }
    // A value too long to be a connection string is never read, so it is no scrub form either.
    if (checkable(value)) secrets.push(value);
    const analysis = analyze(value, "DATABASE_URL");
    sections.push({
      title: "Clipboard, checked as DATABASE_URL",
      findings: [
        // Copying a line often takes its line break along; only the stored value matters.
        ...analysis.findings.map((f) =>
          f.check === "trailing-line-break"
            ? {
                ...f,
                level: /** @type {Level} */ ("warn"),
                message:
                  "ends with a line break: often only in the copy, but if the stored value has it, remove it there",
              }
            : f,
        ),
        ...connectFindings(value, analysis, "the clipboard"),
      ],
      counted: true,
    });
    if (analysis.present) {
      const directOnly = analyze(value, "DIRECT_URL").findings.filter(
        (f) => f.only === "DIRECT_URL",
      );
      sections.push({
        title: "If it is meant for DIRECT_URL instead (not counted)",
        findings: directOnly,
        counted: false,
      });
      for (const role of ROLES) {
        const other = deps.env[role];
        if (!checkable(other)) continue;
        sections.push({
          title: `Compared with ${role} in the environment`,
          // Either could be the stale one, or they could be meant to differ: never a problem.
          findings: comparePair(value, other, ["the clipboard", role]).map(
            (f) => (f.level === "problem" ? { ...f, level: "warn" } : f),
          ),
          counted: true,
        });
      }
    }
  } else {
    for (const role of ROLES) {
      const value = deps.env[role];
      const analysis = analyze(value, role);
      sections.push({
        title: role,
        findings: [
          ...analysis.findings,
          ...(value === undefined
            ? []
            : connectFindings(value, analysis, role)),
        ],
        counted: true,
      });
    }
    const [database, direct] = ROLES.map((role) => deps.env[role]);
    if (checkable(database) && checkable(direct)) {
      sections.push({
        title: "DATABASE_URL and DIRECT_URL",
        findings: comparePair(database, direct),
        counted: true,
      });
    }
  }

  const { text, problems } = formatReport(sections, makeScrubber(secrets));
  deps.write(text);
  return problems > 0 ? 1 : 0;
}

/** @returns {Deps} */
function defaultDeps() {
  return {
    env: process.env,
    readClipboard: () => {
      // pbpaste writes in the locale's encoding: MacRoman when no UTF-8 locale is set (LANG
      // unset, or LC_ALL=C), which garbles every non-ASCII character of a password read as
      // UTF-8, and a login with it then fails as if the password were wrong. LC_ALL outranks
      // every other locale variable.
      const result = spawnSync("pbpaste", [], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
        env: { ...process.env, LC_ALL: "en_US.UTF-8" },
      });
      if (result.error || result.status !== 0)
        throw new Error("pbpaste failed");
      return result.stdout;
    },
    runPrisma: (value) => runPrismaStatus(value),
    write: (text) => {
      process.stdout.write(text);
    },
  };
}

function isMain() {
  try {
    const entry = process.argv[1];
    return (
      !!entry &&
      realpathSync(entry) === realpathSync(fileURLToPath(import.meta.url))
    );
  } catch {
    return false;
  }
}

if (isMain()) {
  // A last resort behind main's own catch: Node would print an uncaught error in full, message
  // and stack, and either could hold a value.
  process.on("uncaughtException", () => {
    process.stderr.write(FAILED);
    process.exit(1);
  });
  process.exitCode = main(process.argv.slice(2), defaultDeps());
}
