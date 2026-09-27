/**
 * scripts/check-database-url.mjs: every check, the Prisma verdicts, and above all that no form
 * of a password (as written, percent-decoded, percent-encoded) ever reaches the output. Every
 * password here is made up. Prisma and the clipboard are fakes: nothing here connects anywhere.
 * The last tests run the script itself with Node, on a made-up .env in a temporary folder and a
 * fake pbpaste, never with --connect.
 */
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { parseEnv } from "node:util";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  MAX_VALUE_LENGTH,
  analyze,
  comparePair,
  interpretPrismaStatus,
  loginMethod,
  main,
  makeScrubber,
  percentDecode,
  runPrismaStatus,
} from "../../scripts/check-database-url.mjs";

const SCRIPT = path.resolve(
  import.meta.dirname,
  "../../scripts/check-database-url.mjs",
);

type Analysis = ReturnType<typeof analyze>;
type Finding = Analysis["findings"][number];
type PrismaRun = Parameters<typeof interpretPrismaStatus>[0];

const REF = "abcdefghijklmnopqrst";
const OTHER_REF = "tsrqponmlkjihgfedcba";
const POOLER = "aws-0-eu-west-1.pooler.supabase.com";
const DIRECT = `db.${REF}.supabase.co`;

/** Made up, with every character that needs encoding: @ # / ? % : and a space. */
const SPECIAL = "Zq8#vW/k?p@x:3 m%Lr";
const SPECIAL_ENCODED = encodeURIComponent(SPECIAL);
/** Made up, with nothing to encode. */
const PLAIN = "made-up-Hunter2-Staple9";
const OTHER_PLAIN = "another-made-up-Kestrel7";

type UrlParts = {
  user?: string;
  password?: string;
  host?: string;
  /** null leaves the port out. */
  port?: string | null;
  db?: string;
  query?: string;
};

function url({
  user = `prisma.${REF}`,
  password = PLAIN,
  host = POOLER,
  port = "6543",
  db = "postgres",
  query = "pgbouncer=true&connection_limit=1",
}: UrlParts = {}) {
  return `postgresql://${user}:${password}@${host}${port === null ? "" : `:${port}`}/${db}${query ? `?${query}` : ""}`;
}

const DATABASE_URL = url();
const DIRECT_URL = url({ port: "5432", query: "" });

function levels(findings: Finding[], check: string) {
  return findings.filter((f) => f.check === check).map((f) => f.level);
}

function problems(findings: Finding[]) {
  return findings.filter((f) => f.level === "problem").map((f) => f.check);
}

function messages(findings: Finding[]) {
  return findings.map((f) => f.message).join("\n");
}

const UP_TO_DATE: PrismaRun = {
  status: 0,
  stdout:
    'Prisma schema loaded from prisma/schema.prisma\nDatasource "db": PostgreSQL database "postgres", schema "public" at "127.0.0.1:55438"\n\n3 migrations found in prisma/migrations\n\nDatabase schema is up to date!\n',
  stderr: "",
};

function run(
  argv: string[],
  env: Record<string, string | undefined>,
  options: { clipboard?: string; prisma?: (value: string) => PrismaRun } = {},
) {
  let out = "";
  const prismaCalls: string[] = [];
  const code = main(argv, {
    env,
    readClipboard: () => {
      if (options.clipboard === undefined) throw new Error("no clipboard");
      return options.clipboard;
    },
    runPrisma: (value) => {
      prismaCalls.push(value);
      return options.prisma?.(value) ?? UP_TO_DATE;
    },
    write: (text) => {
      out += text;
    },
  });
  return { code, out, prismaCalls };
}

/** Every way `password` could be printed: as written, decoded, encoded (both hex cases). */
function forms(password: string) {
  const decoded = percentDecode(password);
  const encoded = encodeURIComponent(decoded);
  const lowerHex = encoded.replace(/%[0-9A-F]{2}/g, (s) => s.toLowerCase());
  return [password, decoded, encoded, lowerHex];
}

function expectNoPassword(output: string, password: string) {
  for (const form of forms(password)) {
    expect(output).not.toContain(form);
    // Nor any sizeable piece of it.
    for (let i = 0; i + 6 <= form.length; i++) {
      expect(output).not.toContain(form.slice(i, i + 6));
    }
  }
}

/** Like expectNoPassword, in any case too: a host is printed lowercased. */
function expectNoPiece(output: string, password: string) {
  expectNoPassword(output, password);
  const lower = output.toLowerCase();
  for (let i = 0; i + 6 <= password.length; i++) {
    expect(lower, password).not.toContain(
      password.slice(i, i + 6).toLowerCase(),
    );
  }
}

/** What Node's --env-file loads from a .env text: its own parser, which ends an unquoted
 * value at its first '#'. */
function loadEnv(text: string) {
  return parseEnv(text) as Record<string, string | undefined>;
}

/** The two variables as a .env file would hold them, unquoted. */
function envFile(password: string) {
  return `DATABASE_URL=${url({ password })}\nDIRECT_URL=${url({ password, port: "5432", query: "" })}\n`;
}

/**
 * Made up, as someone might paste them into .env without percent-encoding: a '/', '?', ':' or
 * '@' before the '#', so what is left after the cut reads as a database, parameters, a port or a
 * host.
 */
const CUT_PASSWORDS = [
  "Xk9pQ/Lm3nR7vT#Zz8",
  "Tr7vQ?Kp9wLx2#m",
  "Tr7vQ?sslmode=Kp9wLx2#m",
  "Tr7vQ:Kp9wLx2/Qm4sT8#z",
  "Ab3c@Qw9Rt5Yh#Zz8",
  "Tr7vQ@Kp9wLx2#mZ",
  "Tr7vQ@Kp9wLx2/Qm4sT8#z",
  "Tr7vQ@Kp9wLx2?sslmode=Qm4sT8#z",
  "Tr7vQ@Kp9wLx2:6543/Qm4sT8#z",
  "Tr7vQ@Kp.9wLx2#z",
  "Tr7vQ@Kp.9wLx2/Qm4sT8#z",
  "Tr7vQ@Kp.9wLx2:Qm4sT8/Hn5bV2#z",
  // A "host" with a character no host name has, none at all, or brackets around no IPv6
  // address: each once printed the port, database or parameters that followed it.
  "Xk2@c!d.ef/Gh7Jk9Pq4#Zz8",
  "Xk2@c!d.ef:6543?Tk5Wn8=Vb3Lm#Zz8",
  "Xk2@/Gh7Jk9Pq4#Zz8",
  "Xk2@:6543/Gh7Jk9Pq4#Zz8",
  "Xk2@[ab:cd]/Gh7Jk9Pq4#Zz8",
  // A piece shaped like name.tld/db, under a prisma.<ref> user only the shared pooler takes.
  "7vQ@Lp2.Rk/Mx9Wd4#Tz4",
];

describe("analyze: the whole value", () => {
  it("reports a missing or empty value", () => {
    expect(problems(analyze(undefined, "DATABASE_URL").findings)).toEqual([
      "empty",
    ]);
    expect(analyze("", "DIRECT_URL").findings[0]?.message).toBe("empty");
    expect(analyze(" \n", "DIRECT_URL").findings[0]?.message).toBe(
      "only spaces or line breaks",
    );
    expect(analyze(undefined, "DATABASE_URL").present).toBe(false);
  });

  it("reports leading and trailing whitespace and line breaks", () => {
    const leading = analyze(` ${DATABASE_URL}`, "DATABASE_URL").findings;
    expect(levels(leading, "whitespace")).toEqual(["problem"]);
    expect(messages(leading)).toContain("starts with a space");

    const trailing = analyze(`${DATABASE_URL}\n`, "DATABASE_URL").findings;
    expect(messages(trailing)).toContain("ends with a line break");
    // What follows is still checked, on the trimmed value.
    expect(levels(trailing, "port")).toEqual(["ok"]);

    expect(levels(trailing, "trailing-line-break")).toEqual(["problem"]);
    expect(
      levels(
        analyze(`${DATABASE_URL}\r\n`, "DATABASE_URL").findings,
        "trailing-line-break",
      ),
    ).toEqual(["problem"]);
    expect(
      levels(
        analyze(`${DATABASE_URL} `, "DATABASE_URL").findings,
        "whitespace",
      ),
    ).toEqual(["problem"]);
  });

  it("reports a value that didn't decode as UTF-8 (U+FFFD)", () => {
    const { findings, facts } = analyze(
      url({ password: "F�ke-Pw9" }),
      "DATABASE_URL",
    );
    expect(problems(findings)).toEqual(["encoding"]);
    expect(messages(findings)).toContain(
      "has a U+FFFD replacement character: part of it didn't decode as UTF-8",
    );
    expect(facts.undecodable).toBe(true);
    expect(analyze(DATABASE_URL, "DATABASE_URL").facts.undecodable).toBe(
      undefined,
    );
  });

  it("reports a line break or an invisible character inside", () => {
    const [head, tail] = [DATABASE_URL.slice(0, 20), DATABASE_URL.slice(20)];
    expect(
      problems(analyze(`${head}\n${tail}`, "DATABASE_URL").findings),
    ).toContain("line-break");
    for (const invisible of ["\u00A0", "\u200B", "\t", "\uFEFF"]) {
      expect(
        problems(
          analyze(`${head}${invisible}${tail}`, "DATABASE_URL").findings,
        ),
      ).toContain("invisible");
    }
  });

  it("reports surrounding quotes, straight or curly, and checks what is inside", () => {
    for (const [open, close] of [
      ['"', '"'],
      ["'", "'"],
      ["\u201C", "\u201D"],
    ] as const) {
      const findings = analyze(
        `${open}${DATABASE_URL}${close}`,
        "DATABASE_URL",
      ).findings;
      expect(problems(findings)).toEqual(["quotes"]);
      expect(messages(findings)).toContain("wrapped in quotes");
      expect(levels(findings, "port")).toEqual(["ok"]);
    }
    const stray = analyze(`${DATABASE_URL}"`, "DATABASE_URL").findings;
    expect(problems(stray)).toEqual(["quotes"]);
    expect(messages(stray)).toContain("stray quote");
    // Quotes and a line break together, as a copied .env line might have.
    expect(
      problems(analyze(`"${DATABASE_URL}"\n`, "DATABASE_URL").findings),
    ).toEqual(["trailing-line-break", "quotes"]);
  });

  it("finds nothing wrong with a good pair", () => {
    expect(problems(analyze(DATABASE_URL, "DATABASE_URL").findings)).toEqual(
      [],
    );
    expect(problems(analyze(DIRECT_URL, "DIRECT_URL").findings)).toEqual([]);
    expect(
      analyze(DATABASE_URL, "DATABASE_URL").findings.every(
        (f) => f.level === "ok",
      ),
    ).toBe(true);
  });
});

describe("analyze: the password, before any URL parsing", () => {
  it("names each unencoded character that breaks the URL", () => {
    const { findings, facts } = analyze(
      url({ password: SPECIAL }),
      "DATABASE_URL",
    );
    expect(problems(findings)).toEqual([
      "password-at",
      "password-hash",
      "password-question",
      "password-slash",
      "password-space",
      "password-percent",
      "parse",
    ]);
    expect(levels(findings, "password-colon")).toEqual(["warn"]);
    expect(messages(findings)).toContain("more than one '@' before the host");
    expect(messages(findings)).toContain("write it as %23");
    expect(messages(findings)).toContain("write it as %2F");
    expect(messages(findings)).toContain("write it as %3F");
    expect(messages(findings)).toContain("write it as %20");
    expect(messages(findings)).toContain("write a literal '%' as %25");
    // The host, port and database are still read from after the last '@'.
    expect(facts.host).toBe(POOLER);
    expect(facts.port).toBe(6543);
    expect(facts.database).toBe("postgres");
    expect(facts.parses).toBe(false);
    expect(facts.passwordEncoded).toBe(false);
    expect(facts.passwordLength).toBe(SPECIAL.length);
  });

  it("accepts the same password percent-encoded", () => {
    const { findings, facts } = analyze(
      url({ password: SPECIAL_ENCODED }),
      "DATABASE_URL",
    );
    expect(problems(findings)).toEqual([]);
    expect(findings.filter((f) => f.check.startsWith("password-"))).toEqual([]);
    expect(facts.parses).toBe(true);
    expect(facts.passwordEncoded).toBe(true);
    expect(facts.passwordLength).toBe(SPECIAL.length);
    expect(messages(findings)).toContain(
      `password <${SPECIAL.length} characters>, percent-encoded`,
    );
  });

  it("reports each breaking character on its own", () => {
    const cases: [string, string][] = [
      ["made@up-pw-1", "password-at"],
      ["made#up-pw-1", "password-hash"],
      ["made?up-pw-1", "password-question"],
      ["made/up-pw-1", "password-slash"],
      ["made up-pw-1", "password-space"],
      ["made%zzup-pw", "password-percent"],
      ["made-up-pw-1%", "password-percent"],
    ];
    for (const [password, check] of cases) {
      const findings = analyze(url({ password }), "DATABASE_URL").findings;
      expect(levels(findings, check), password).toEqual(["problem"]);
    }
    expect(
      levels(
        analyze(url({ password: "made:up-pw-1" }), "DATABASE_URL").findings,
        "password-colon",
      ),
    ).toEqual(["warn"]);
  });

  it("tells a password that breaks parsing from one parsers still read right", () => {
    // A raw '@' alone: URL parsers take the last '@', so they still find the host.
    expect(
      analyze(url({ password: "made@up-pw-1" }), "DATABASE_URL").facts.parses,
    ).toBe(true);
    for (const password of [
      "made/up-pw-1",
      "made#up-pw-1",
      "made?up-pw-1",
      "a@b.co/x",
    ]) {
      const { facts, findings } = analyze(url({ password }), "DATABASE_URL");
      expect(facts.parses, password).toBe(false);
      expect(problems(findings)).toContain("parse");
    }
  });

  it("says the brackets of Supabase's [YOUR-PASSWORD] aren't part of the password", () => {
    for (const password of ["[Pass123]", "%5BPass123%5D", "%5bPass123]"]) {
      for (const role of ["DATABASE_URL", "DIRECT_URL"] as const) {
        const { findings } = analyze(
          url({ user: `postgres.${REF}`, password, port: "5432" }),
          role,
        );
        expect(levels(findings, "password-brackets"), password).toEqual([
          "problem",
        ]);
        expect(messages(findings)).toContain(
          "the password is wrapped in [ ]: the brackets in Supabase's template aren't part of the password; remove them",
        );
        // Encoding them would still send them: no advice to.
        expect(levels(findings, "password-unsafe"), password).toEqual([]);
        expectNoPiece(messages(findings), percentDecode(password));
      }
    }
    // The main check fails on it, where it once passed with two warnings.
    const env = {
      DATABASE_URL: url({ user: `postgres.${REF}`, password: "[Pass123]" }),
      DIRECT_URL: url({
        user: `postgres.${REF}`,
        password: "[Pass123]",
        port: "5432",
        query: "",
      }),
    };
    const { code, out } = run([], env);
    expect(code).toBe(1);
    expect(out).not.toContain("percent-encode them");
    expect(out.trimEnd().endsWith("2 problems.")).toBe(true);
  });

  it("says the password is still Supabase's placeholder, with or without its brackets", () => {
    for (const password of [
      "[YOUR-PASSWORD]",
      "YOUR-PASSWORD",
      "[your-password]",
      "%5BYOUR-PASSWORD%5D",
    ]) {
      const { findings } = analyze(url({ password }), "DATABASE_URL");
      expect(problems(findings), password).toEqual(["password-placeholder"]);
      expect(messages(findings)).toContain(
        "the password is still the placeholder from Supabase's template: put the database password in its place, without the brackets",
      );
      expect(levels(findings, "password-unsafe")).toEqual([]);
      // Not echoed (the report says "password" everywhere, so pieces of it can't be checked).
      expect(messages(findings).toLowerCase()).not.toContain("your-password");
    }
  });

  it("still advises encoding for brackets inside a password, and other characters beside wrapping ones", () => {
    const inside = analyze(url({ password: "made[up]pw" }), "DATABASE_URL");
    expect(levels(inside.findings, "password-brackets")).toEqual([]);
    expect(levels(inside.findings, "password-unsafe")).toEqual(["warn"]);
    expect(messages(inside.findings)).toContain("(brackets)");
    const wrapped = analyze(url({ password: "[made{up]" }), "DATABASE_URL");
    expect(levels(wrapped.findings, "password-brackets")).toEqual(["problem"]);
    expect(messages(wrapped.findings)).toContain(
      "can't hold unencoded (braces)",
    );
  });

  it("warns about '$', characters a URL can't hold and double encoding", () => {
    expect(
      levels(
        analyze(url({ password: "made$up-pw-1" }), "DATABASE_URL").findings,
        "password-dollar",
      ),
    ).toEqual(["warn"]);
    const unsafe = analyze(
      url({ password: "made{up}pwé" }),
      "DATABASE_URL",
    ).findings;
    expect(levels(unsafe, "password-unsafe")).toEqual(["warn"]);
    expect(messages(unsafe)).toContain("braces, non-ASCII characters");
    expect(
      levels(
        analyze(url({ password: "made%2540up-pw" }), "DATABASE_URL").findings,
        "password-double-encoded",
      ),
    ).toEqual(["warn"]);
  });

  it("reports a missing or empty password without printing what stands before '@'", () => {
    const noPassword = analyze(
      `postgresql://${PLAIN}@${POOLER}:6543/postgres`,
      "DATABASE_URL",
    );
    expect(problems(noPassword.findings)).toContain("password");
    expect(messages(noPassword.findings)).not.toContain(PLAIN);
    expect(noPassword.facts.user).toBeUndefined();

    expect(
      problems(analyze(url({ password: "" }), "DATABASE_URL").findings),
    ).toContain("password");
    // A '#' in an unquoted .env line cuts the value short, leaving no '@'.
    const cut = analyze(
      "postgresql://prisma.x:made-up",
      "DATABASE_URL",
    ).findings;
    expect(problems(cut)).toContain("userinfo");
    expect(messages(cut)).toContain("unquoted '#' starts a comment");
  });
});

describe("analyze: a value an unquoted '#' in .env cut short", () => {
  it("shows nothing after the scheme when no '@' is left", () => {
    for (const password of ["Xk9pQ/Lm3nR7vT#Zz8", "Tr7vQ?sslmode=Kp9wLx2#m"]) {
      const value = loadEnv(envFile(password)).DATABASE_URL;
      expect(value).not.toContain("@");
      const { findings, facts } = analyze(value, "DATABASE_URL");
      expect(findings.map((f) => f.check)).toEqual(["scheme", "userinfo"]);
      expect(messages(findings)).toContain("unquoted '#' starts a comment");
      expect(facts).toMatchObject({
        hostKind: "none",
        params: {},
        parses: false,
      });
      expect(facts.host).toBeUndefined();
      expect(facts.port).toBeUndefined();
      expect(facts.database).toBeUndefined();
      expectNoPiece(messages(findings), password);
    }
  });

  it("hides a host that may be a piece of the password, and what follows it", () => {
    const cases: [string, string][] = [
      ["Ab3c@Qw9Rt5Yh#Zz8", "no port or database follows it"],
      ["Tr7vQ@Kp9wLx2/Qm4sT8#z", "no dot"],
      ["Tr7vQ@Kp9wLx2?sslmode=Qm4sT8#z", "no port or database follows it"],
      ["Tr7vQ@Kp9wLx2:6543/Qm4sT8#z", "no dot"],
      ["Tr7vQ@Kp.9wLx2#z", "no port or database follows it"],
      ["Tr7vQ@Kp.9wLx2/Qm4sT8#z", "top-level domain"],
      ["Tr7vQ@Kp.9wLx2:Qm4sT8/Hn5bV2#z", "port after it isn't a number"],
      ["Xk2@c!d.ef/Gh7Jk9Pq4#Zz8", "characters a host name can't have"],
      ["Xk2@c!d.ef:6543?Tk5Wn8=Vb3Lm#Zz8", "characters a host name can't have"],
      ["Xk2@[ab:cd]/Gh7Jk9Pq4#Zz8", "in brackets but isn't an IPv6 address"],
      ["Xk2@[::1x]/Gh7Jk9Pq4#Zz8", "in brackets but isn't an IPv6 address"],
      ["7vQ@Lp2.Rk/Mx9Wd4#Tz4", "the only host a user ending in .<ref>"],
    ];
    for (const [password, reason] of cases) {
      const value = loadEnv(envFile(password)).DIRECT_URL;
      const { findings, facts } = analyze(value, "DIRECT_URL");
      expect(levels(findings, "host"), password).toEqual(["problem"]);
      const host = findings.find((f) => f.check === "host")?.message ?? "";
      expect(host).toContain("the host isn't shown");
      expect(host).toContain(reason);
      expect(host).toContain("cut short by an unquoted '#' in .env?");
      expect(facts.cut).toBe(true);
      expect(facts.host).toBeUndefined();
      expect(facts.port).toBeUndefined();
      expect(facts.database).toBeUndefined();
      expect(facts.params).toEqual({});
      expect(levels(findings, "database")).toEqual([]);
      expect(levels(findings, "params")).toEqual([]);
      expectNoPiece(messages(findings), password);
    }
  });

  it("hides what follows an '@' with no host after it, and says it may be cut short", () => {
    for (const password of ["Xk2@/Gh7Jk9Pq4#Zz8", "Xk2@:6543/Gh7Jk9Pq4#Zz8"]) {
      for (const role of ["DATABASE_URL", "DIRECT_URL"] as const) {
        const value = loadEnv(envFile(password))[role];
        const { findings, facts } = analyze(value, role);
        const host = findings.find((f) => f.check === "host")?.message ?? "";
        expect(levels(findings, "host"), password).toEqual(["problem"]);
        expect(host).toContain("no host after '@'");
        expect(host).toContain("cut short by an unquoted '#' in .env?");
        expect(facts).toMatchObject({
          cut: true,
          hostKind: "none",
          params: {},
        });
        expect(facts.port).toBeUndefined();
        expect(facts.database).toBeUndefined();
        expect(levels(findings, "port")).toEqual([]);
        expect(levels(findings, "database")).toEqual([]);
        expect(levels(findings, "params")).toEqual([]);
        expectNoPiece(messages(findings), password);
      }
    }
  });

  it("still reads real hosts, local names and addresses", () => {
    for (const host of [
      "db.example.com",
      "localhost",
      "127.0.0.1",
      "10.0.0.7",
      "[::1]",
      "[2001:db8::1]",
      "[::]",
      "host.docker.internal",
      "db.example.xn--p1ai",
    ]) {
      const { facts } = analyze(url({ host, user: "site" }), "DATABASE_URL");
      expect(facts.cut, host).toBeUndefined();
      expect(facts.host, host).toBe(host);
    }
    // No port but a database, as a value for another host may be written.
    expect(
      analyze(
        url({ host: "db.example.com", port: null, user: "site" }),
        "DATABASE_URL",
      ).facts.cut,
    ).toBeUndefined();
  });
});

describe("analyze: a prisma.<ref> user with a host other than the shared pooler", () => {
  it("hides the host and what follows, and says the user needs the pooler", () => {
    for (const [host, port] of [
      ["db.example.com", "5432"],
      ["lp2.rk", null],
      ["127.0.0.1", "55438"],
      ["localhost", "5432"],
    ] as const) {
      for (const user of [`prisma.${REF}`, `postgres.${REF}`, "prisma.short"]) {
        const { findings, facts } = analyze(
          url({ user, host, port, db: "Mx9Wd4" }),
          "DATABASE_URL",
        );
        const label = `${user}@${host}`;
        expect(facts.cut, label).toBe(true);
        expect(facts.host, label).toBeUndefined();
        expect(facts.hostKind, label).toBe("none");
        expect(facts.port, label).toBeUndefined();
        expect(facts.database, label).toBeUndefined();
        expect(facts.params, label).toEqual({});
        expect(levels(findings, "user"), label).toEqual(["problem"]);
        expect(messages(findings)).toContain(
          "a user ending in .<ref> logs in only through Supabase's shared pooler",
        );
        expect(messages(findings)).toContain(
          "cut short by an unquoted '#' in .env?",
        );
        expect(messages(findings)).not.toContain("Mx9Wd4");
        expect(messages(findings).toLowerCase()).not.toContain(host);
      }
    }
  });

  it("leaves Supabase's own hosts, and roles that merely have a dot, alone", () => {
    expect(analyze(DATABASE_URL, "DATABASE_URL").facts.cut).toBeUndefined();
    const direct = analyze(url({ host: DIRECT, port: "5432" }), "DIRECT_URL");
    expect(direct.facts).toMatchObject({ hostKind: "direct", host: DIRECT });
    expect(direct.facts.cut).toBeUndefined();
    const api = analyze(url({ host: `${REF}.supabase.co` }), "DATABASE_URL");
    expect(api.facts).toMatchObject({ hostKind: "api" });
    const dotted = analyze(
      url({ user: "john.doe", host: "db.example.com", port: "5432" }),
      "DATABASE_URL",
    );
    expect(dotted.facts).toMatchObject({
      hostKind: "other",
      host: "db.example.com",
    });
    expect(dotted.facts.cut).toBeUndefined();
    expect(levels(dotted.findings, "user")).toEqual(["ok"]);
  });

  it("no longer passes a cut value whose piece looks like name.tld/db", () => {
    // Once "No problems" and exit 0, and --connect logged in at a host made from the password.
    const env = loadEnv(envFile("7vQ@Lp2.Rk/Mx9Wd4#Tz4"));
    expect(env.DIRECT_URL).toBe(`postgresql://prisma.${REF}:7vQ@Lp2.Rk/Mx9Wd4`);
    for (const argv of [[], ["--connect"]]) {
      const { code, out, prismaCalls } = run(argv, env);
      expect(code).toBe(1);
      expect(prismaCalls).toEqual([]);
      expect(out).not.toContain("No problems");
      expectNoPiece(out, "7vQ@Lp2.Rk/Mx9Wd4#Tz4");
    }
    const loaded = `postgresql://prisma.${REF}:Xk2@mp7.qz/Gh7Jk9Pq4`;
    const connect = run(["--connect"], {
      DATABASE_URL: loaded,
      DIRECT_URL: loaded,
    });
    expect(connect.code).toBe(1);
    expect(connect.prismaCalls).toEqual([]);
    expect(connect.out).not.toContain("login OK");
    expectNoPiece(connect.out, "Xk2@mp7.qz/Gh7Jk9Pq4");
  });
});

describe("analyze: the parsed parts", () => {
  it("reads scheme, user, host, port, database and parameters", () => {
    const { facts, findings } = analyze(DATABASE_URL, "DATABASE_URL");
    expect(facts).toMatchObject({
      scheme: "postgresql",
      user: `prisma.${REF}`,
      role: "prisma",
      userRef: REF,
      host: POOLER,
      hostKind: "pooler",
      region: "eu-west-1",
      port: 6543,
      database: "postgres",
      params: { pgbouncer: "true", connection_limit: "1" },
      passwordLength: PLAIN.length,
      passwordEncoded: false,
      parses: true,
    });
    expect(messages(findings)).toContain(
      `password <${PLAIN.length} characters>, not percent-encoded`,
    );
    expect(messages(findings)).toContain(
      "parameters pgbouncer=true, connection_limit=1",
    );
  });

  it("takes postgres:// and refuses other schemes", () => {
    expect(
      levels(
        analyze(
          DATABASE_URL.replace("postgresql:", "postgres:"),
          "DATABASE_URL",
        ).findings,
        "scheme",
      ),
    ).toEqual(["ok"]);
    expect(
      levels(
        analyze(DATABASE_URL.replace("postgresql:", "mysql:"), "DATABASE_URL")
          .findings,
        "scheme",
      ),
    ).toEqual(["problem"]);
    expect(
      levels(
        analyze(
          DATABASE_URL.replace("postgresql:", "PostgreSQL:"),
          "DATABASE_URL",
        ).findings,
        "scheme",
      ),
    ).toEqual(["warn"]);
    expect(
      problems(analyze(`${POOLER}:6543/postgres`, "DATABASE_URL").findings),
    ).toEqual(["scheme"]);
  });

  it("never prints a parameter value that could be a secret", () => {
    const findings = analyze(
      url({
        query: `pgbouncer=true&connection_limit=1&sslpassword=${OTHER_PLAIN}`,
      }),
      "DATABASE_URL",
    ).findings;
    expect(messages(findings)).toContain("sslpassword=…");
    expect(messages(findings)).not.toContain(OTHER_PLAIN);
  });

  it("warns about repeated parameters, a fragment, a bad port and a missing database", () => {
    expect(
      levels(
        analyze(
          url({ query: "pgbouncer=true&pgbouncer=true&connection_limit=1" }),
          "DATABASE_URL",
        ).findings,
        "params",
      ),
    ).toEqual(["ok", "warn"]);
    expect(
      problems(analyze(`${DATABASE_URL}#x`, "DATABASE_URL").findings),
    ).toContain("fragment");
    expect(
      problems(analyze(url({ port: "65x" }), "DATABASE_URL").findings),
    ).toContain("port");
    expect(
      levels(analyze(url({ db: "" }), "DATABASE_URL").findings, "database"),
    ).toEqual(["warn"]);
    expect(
      levels(analyze(url({ db: "site" }), "DATABASE_URL").findings, "database"),
    ).toEqual(["warn"]);
  });
});

describe("analyze: Supabase's shape", () => {
  it("accepts prisma.<ref> and postgres.<ref> on the shared pooler", () => {
    expect(
      levels(analyze(DATABASE_URL, "DATABASE_URL").findings, "user"),
    ).toEqual(["ok"]);
    expect(
      levels(
        analyze(url({ user: `postgres.${REF}` }), "DATABASE_URL").findings,
        "user",
      ),
    ).toEqual(["ok"]);
    expect(
      levels(
        analyze(url({ user: `site.${REF}` }), "DATABASE_URL").findings,
        "user",
      ),
    ).toEqual(["warn"]);
  });

  it("refuses plain postgres on the shared pooler", () => {
    for (const role of ["DATABASE_URL", "DIRECT_URL"] as const) {
      const findings = analyze(url({ user: "postgres" }), role).findings;
      expect(levels(findings, "user")).toEqual(["problem"]);
      expect(messages(findings)).toContain("Tenant or user not found");
    }
    expect(
      levels(analyze(url({ user: "prisma" }), "DATABASE_URL").findings, "user"),
    ).toEqual(["problem"]);
  });

  it("warns when the ref doesn't look like one", () => {
    expect(
      levels(
        analyze(url({ user: "prisma.short" }), "DATABASE_URL").findings,
        "ref",
      ),
    ).toEqual(["warn"]);
  });

  it("wants the bare role on the direct host, and warns it is IPv6-only", () => {
    const good = analyze(
      url({ user: "prisma", host: DIRECT, port: "5432", query: "" }),
      "DIRECT_URL",
    );
    expect(problems(good.findings)).toEqual([]);
    expect(good.facts).toMatchObject({ hostKind: "direct", hostRef: REF });
    expect(levels(good.findings, "ipv6")).toEqual(["warn"]);
    expect(messages(good.findings)).toContain(
      "session mode (port 5432) works everywhere",
    );

    const suffixed = analyze(
      url({ host: DIRECT, port: "5432", query: "" }),
      "DIRECT_URL",
    );
    expect(levels(suffixed.findings, "user")).toEqual(["problem"]);
    expect(levels(suffixed.findings, "ref")).toEqual([]);

    const otherProject = analyze(
      url({
        user: `prisma.${OTHER_REF}`,
        host: DIRECT,
        port: "5432",
        query: "",
      }),
      "DIRECT_URL",
    );
    expect(problems(otherProject.findings)).toEqual(["user", "ref"]);

    const asDatabaseUrl = analyze(
      url({
        user: "prisma",
        host: DIRECT,
        port: "5432",
        query: "connection_limit=1",
      }),
      "DATABASE_URL",
    );
    expect(messages(asDatabaseUrl.findings)).toContain(
      "Vercel can't reach it (P1001)",
    );
    expect(levels(asDatabaseUrl.findings, "ipv6")).toEqual(["warn"]);
  });

  it("refuses the project's API address", () => {
    expect(
      levels(
        analyze(url({ host: `${REF}.supabase.co` }), "DATABASE_URL").findings,
        "host",
      ),
    ).toEqual(["problem"]);
  });

  it("warns about a host that isn't Supabase", () => {
    expect(
      levels(
        analyze(url({ host: "db.example.com", user: "site" }), "DATABASE_URL")
          .findings,
        "host",
      ),
    ).toEqual(["warn"]);
    expect(
      levels(
        analyze(url({ host: "127.0.0.1", user: "site" }), "DATABASE_URL")
          .findings,
        "host",
      ),
    ).toEqual(["warn"]);
  });

  it("needs pgbouncer=true in transaction mode (port 6543) for DATABASE_URL", () => {
    const missing = analyze(
      url({ query: "connection_limit=1" }),
      "DATABASE_URL",
    ).findings;
    expect(problems(missing)).toEqual(["pgbouncer"]);
    expect(missing.find((f) => f.check === "pgbouncer")?.only).toBe(
      "DATABASE_URL",
    );
    expect(
      problems(
        analyze(
          url({ query: "pgbouncer=false&connection_limit=1" }),
          "DATABASE_URL",
        ).findings,
      ),
    ).toEqual(["pgbouncer"]);
    // The dedicated pooler on the direct host is transaction mode too.
    expect(
      problems(
        analyze(
          url({ user: "prisma", host: DIRECT, query: "connection_limit=1" }),
          "DATABASE_URL",
        ).findings,
      ),
    ).toEqual(["pgbouncer"]);
  });

  it("says session mode (5432) works for DATABASE_URL but transaction mode is better on Vercel", () => {
    const findings = analyze(url({ port: "5432" }), "DATABASE_URL").findings;
    expect(levels(findings, "port")).toEqual(["warn"]);
    expect(messages(findings)).toContain(
      "it works, but on Vercel use transaction mode",
    );
    const noPort = analyze(url({ port: null }), "DATABASE_URL").findings;
    expect(messages(noPort)).toContain(
      "port 5432 (none given, so the default) is session mode",
    );
  });

  it("wants session mode or the direct host for DIRECT_URL", () => {
    expect(levels(analyze(DIRECT_URL, "DIRECT_URL").findings, "port")).toEqual([
      "ok",
    ]);
    const transaction = analyze(url(), "DIRECT_URL").findings;
    expect(problems(transaction)).toEqual(["port"]);
    expect(messages(transaction)).toContain(
      "prisma migrate needs session mode",
    );
    // DATABASE_URL's own checks don't apply.
    expect(levels(transaction, "pgbouncer")).toEqual([]);
    expect(levels(transaction, "connection-limit")).toEqual([]);
  });

  it("refuses a port the pooler doesn't listen on", () => {
    expect(
      problems(analyze(url({ port: "5433" }), "DATABASE_URL").findings),
    ).toEqual(["port"]);
  });

  it("recommends connection_limit=1 for DATABASE_URL", () => {
    expect(
      levels(
        analyze(url({ query: "pgbouncer=true" }), "DATABASE_URL").findings,
        "connection-limit",
      ),
    ).toEqual(["warn"]);
    expect(
      levels(
        analyze(
          url({ query: "pgbouncer=true&connection_limit=5" }),
          "DATABASE_URL",
        ).findings,
        "connection-limit",
      ),
    ).toEqual(["warn"]);
    expect(
      levels(
        analyze(DATABASE_URL, "DATABASE_URL").findings,
        "connection-limit",
      ),
    ).toEqual(["ok"]);
  });
});

describe("comparePair", () => {
  it("says same for identical passwords, however they are written", () => {
    const same = comparePair(DATABASE_URL, DIRECT_URL);
    expect(same.map((f) => [f.check, f.level])).toEqual([
      ["same-password", "ok"],
      ["same-role", "ok"],
      ["same-project", "ok"],
    ]);
    expect(same[0]?.message).toBe("same password in both");

    for (const [first, second] of [
      [
        SPECIAL_ENCODED,
        SPECIAL_ENCODED.replace(/%[0-9A-F]{2}/g, (s) => s.toLowerCase()),
      ],
      ["made@up-pw-1", "made%40up-pw-1"],
    ] as const) {
      const written = comparePair(
        url({ password: first }),
        url({ password: second, port: "5432" }),
      );
      expect(written[0]?.level).toBe("ok");
      expect(written[0]?.message).toContain("written differently");
    }
    // One letter's case is a different password.
    expect(
      comparePair(DATABASE_URL, url({ password: PLAIN.toLowerCase() }))[0]
        ?.level,
    ).toBe("problem");
  });

  it("says different, as a problem when both log in as the same role", () => {
    const different = comparePair(
      DATABASE_URL,
      url({ password: OTHER_PLAIN, port: "5432" }),
    );
    expect(different[0]).toMatchObject({
      check: "same-password",
      level: "problem",
    });
    expect(different[0]?.message).toContain("one of them is out of date");

    const roles = comparePair(
      DATABASE_URL,
      url({ user: `postgres.${REF}`, password: OTHER_PLAIN, port: "5432" }),
    );
    expect(roles.map((f) => [f.check, f.level])).toEqual([
      ["same-password", "warn"],
      ["same-role", "warn"],
      ["same-project", "ok"],
    ]);
  });

  it("doesn't call a password out of date when one of them didn't decode", () => {
    const findings = comparePair(
      url({ password: "F�ke-Pw9" }),
      url({ password: "Fäke-Pw9", port: "5432" }),
    );
    expect(findings[0]).toMatchObject({
      check: "same-password",
      level: "warn",
    });
    expect(findings[0]?.message).toContain("didn't decode as UTF-8");
    expect(findings[0]?.message).not.toContain("out of date");
  });

  it("says when the two are on different projects", () => {
    const findings = comparePair(
      DATABASE_URL,
      url({ user: `prisma.${OTHER_REF}`, port: "5432" }),
    );
    expect(findings.find((f) => f.check === "same-project")?.level).toBe(
      "problem",
    );
  });

  it("finds the project of a direct-host value in its host", () => {
    const findings = comparePair(
      DATABASE_URL,
      url({
        user: "prisma",
        host: `db.${OTHER_REF}.supabase.co`,
        port: "5432",
        query: "",
      }),
    );
    expect(findings.find((f) => f.check === "same-project")?.message).toContain(
      OTHER_REF,
    );
  });
});

describe("interpretPrismaStatus", () => {
  const scrub = makeScrubber([url({ password: SPECIAL_ENCODED })]);

  it("reads a refused login (P1000), with a hint for the prisma role", () => {
    const refused: PrismaRun = {
      status: 1,
      stdout:
        'Datasource "db": PostgreSQL database "postgres", schema "public" at "x:6543"\n',
      stderr:
        "Error: P1000: Authentication failed against database server, the provided database credentials for `prisma.abcdefghijklmnopqrst` are not valid.\n",
    };
    const findings = interpretPrismaStatus(refused, scrub, {
      role: "prisma",
      hostKind: "pooler",
    });
    expect(findings.map((f) => [f.level, f.check])).toEqual([
      ["problem", "login"],
      ["warn", "login-hint"],
    ]);
    expect(findings[0]?.message).toBe(
      "login refused (P1000): the server rejected this user and password",
    );
    expect(
      interpretPrismaStatus(refused, scrub, {
        role: "postgres",
        hostKind: "pooler",
      }),
    ).toHaveLength(1);
  });

  it("reads an unreachable host (P1001) and the pooler's unknown user", () => {
    const unreachable = interpretPrismaStatus(
      {
        status: 1,
        stdout: "",
        stderr: "Error: P1001: Can't reach database server at `x:5432`\n",
      },
      scrub,
      { hostKind: "direct" },
    );
    expect(unreachable[0]?.message).toBe(
      "host unreachable (P1001): nothing answered at this host and port",
    );
    expect(unreachable[1]?.message).toContain("IPv6");
    const tenant = interpretPrismaStatus(
      {
        status: 1,
        stdout: "",
        stderr:
          "Error: Schema engine error:\nFATAL: Tenant or user not found\n",
      },
      scrub,
    );
    expect(tenant[0]?.message).toContain(
      "login refused: the pooler knows no such user",
    );
  });

  it("reads migrations up to date, pending, failed and never applied", () => {
    expect(
      interpretPrismaStatus(UP_TO_DATE, scrub).map((f) => f.message),
    ).toEqual(["login OK", "migrations up to date"]);
    const pending = interpretPrismaStatus(
      {
        status: 1,
        stdout:
          "3 migrations found in prisma/migrations\nFollowing migrations have not yet been applied:\n0002_usage_quotas\n0003_ip_rate_limits\n\nTo apply migrations in development run prisma migrate dev.\n",
        stderr: "",
      },
      scrub,
    );
    expect(pending.map((f) => [f.level, f.message])).toEqual([
      ["ok", "login OK"],
      [
        "warn",
        "2 migrations pending (0002_usage_quotas, 0003_ip_rate_limits): apply with npm run db:migrate",
      ],
    ]);
    const failed = interpretPrismaStatus(
      {
        status: 1,
        stdout: "",
        stderr: "Following migration have failed:\n0003_ip_rate_limits\n",
      },
      scrub,
    );
    expect(failed[1]?.level).toBe("problem");
    const unmanaged = interpretPrismaStatus(
      {
        status: 1,
        stdout: "",
        stderr: "The current database is not managed by Prisma Migrate.\n",
      },
      scrub,
    );
    expect(unmanaged[1]?.message).toBe(
      "no migrations applied yet: run npm run db:migrate",
    );
  });

  it("reports any other error by its code, scrubbed", () => {
    const other = interpretPrismaStatus(
      {
        status: 1,
        stdout: "",
        stderr: `Error: P1013: The provided database string is invalid. ${url({ password: SPECIAL_ENCODED })} (${SPECIAL})\n`,
      },
      scrub,
    );
    expect(other).toHaveLength(1);
    expect(other[0]?.message).toMatch(/^other error \(P1013\): /);
    expectNoPassword(other[0]?.message ?? "", SPECIAL);
  });

  it("reports a timeout and a CLI that can't run", () => {
    expect(
      interpretPrismaStatus(
        { status: null, stdout: "", stderr: "", timedOut: true },
        scrub,
      )[0]?.message,
    ).toContain("no answer");
    expect(
      interpretPrismaStatus(
        { status: null, stdout: "", stderr: "", error: "no CLI" },
        scrub,
      )[0]?.message,
    ).toBe("not tested: no CLI");
  });
});

describe("makeScrubber", () => {
  it("removes the password in every form, and any URL's user:password@", () => {
    const scrub = makeScrubber([url({ password: SPECIAL_ENCODED })]);
    const text = [
      `raw ${SPECIAL}`,
      `decoded ${percentDecode(SPECIAL_ENCODED)}`,
      `encoded ${SPECIAL_ENCODED} ${SPECIAL_ENCODED.toLowerCase()}`,
      `json ${JSON.stringify(SPECIAL)}`,
      "url postgresql://someone:else@example.com:5432/db",
    ].join("\n");
    const scrubbed = scrub(text);
    expectNoPassword(scrubbed, SPECIAL);
    expect(scrubbed).toContain("postgresql://***@example.com:5432/db");
    expect(scrubbed).not.toContain("someone:else");
  });

  it("scrubs a raw password too", () => {
    const scrubbed = makeScrubber([url({ password: SPECIAL })])(
      `x ${SPECIAL} y ${SPECIAL_ENCODED}`,
    );
    expectNoPassword(scrubbed, SPECIAL);
  });

  it("takes out overlapping forms in full, whatever their case", () => {
    const scrub = makeScrubber([url({ password: "Ab12Cd34%2FEf56" })]);
    const scrubbed = scrub("a ab12cd34/EF56 b AB12CD34%2fef56 c");
    expect(scrubbed).toBe("a *** b *** c");
  });

  it("adds no form for a value too long to be a connection string, and never throws", () => {
    // V8 refuses a RegExp literal of 32,767 characters or more; the old scrubber built one.
    const log =
      `postgresql://prisma.${REF}:${PLAIN}@${POOLER}:6543/postgres `.repeat(
        700,
      );
    expect(log.length).toBeGreaterThan(40_000);
    const scrub = makeScrubber([log]);
    // Words of the log would be pieces of a "password" if it were read as a value.
    expect(scrub("pool postgres ready")).toBe("pool postgres ready");
    expect(scrub(`x ${log.slice(0, 50_000)}`)).not.toContain(PLAIN);
  });
});

describe("main", () => {
  it("exits 0 for a good pair and 1 when anything is a problem", () => {
    const good = run([], { DATABASE_URL, DIRECT_URL });
    expect(good.code).toBe(0);
    expect(good.out).toContain("DATABASE_URL and DIRECT_URL");
    expect(good.out).toContain("  ok      same password in both");
    expect(good.out.trimEnd().endsWith("No problems.")).toBe(true);
    expect(good.prismaCalls).toEqual([]);

    const bad = run([], {
      DATABASE_URL: url({ user: "postgres" }),
      DIRECT_URL,
    });
    expect(bad.code).toBe(1);
    expect(bad.out).toMatch(/^ {2}problem user postgres: /m);
    expect(run([], {}).code).toBe(1);
  });

  it("prints one line per finding, each starting ok, warn or problem", () => {
    const { out } = run([], {
      DATABASE_URL: url({ password: SPECIAL }),
      DIRECT_URL,
    });
    const findingLines = out
      .split("\n")
      .filter((line) => line.startsWith("  "));
    expect(findingLines.length).toBeGreaterThan(10);
    for (const line of findingLines)
      expect(line).toMatch(/^ {2}(ok {6}|warn {4}|problem )\S/);
  });

  it("never echoes an unknown argument that could be a connection string", () => {
    const { code, out } = run([DATABASE_URL], {});
    expect(code).toBe(1);
    expect(out).toContain("Unknown argument (not shown)");
    expectNoPassword(out, PLAIN);
    expect(run(["--verbose"], {}).out).toContain("Unknown argument --verbose");
    expect(run(["--help"], {}).code).toBe(0);
  });

  it("checks the clipboard as DATABASE_URL and lists DIRECT_URL's own checks apart", () => {
    const { code, out } = run(["--clipboard"], {}, { clipboard: url() });
    expect(code).toBe(0);
    expect(out).toContain("Clipboard, checked as DATABASE_URL");
    expect(out).toContain(
      "If it is meant for DIRECT_URL instead (not counted)",
    );
    expect(out).toContain("prisma migrate needs session mode");

    const compared = run(
      ["--clipboard"],
      { DATABASE_URL: url({ password: OTHER_PLAIN }) },
      { clipboard: url() },
    );
    expect(compared.out).toContain(
      "Compared with DATABASE_URL in the environment",
    );
    expect(compared.out).toContain("  warn    different passwords");
    expect(compared.code).toBe(0);
    expectNoPassword(compared.out, OTHER_PLAIN);

    expect(run(["--clipboard"], {}).code).toBe(1);
  });

  it("tests each value with Prisma under --connect, once per distinct value", () => {
    const { code, out, prismaCalls } = run(["--connect"], {
      DATABASE_URL,
      DIRECT_URL: DATABASE_URL,
    });
    expect(prismaCalls).toEqual([DATABASE_URL]);
    expect(out).toContain("  ok      login OK");
    expect(out).toContain("  ok      migrations up to date");
    // The same value as DIRECT_URL is transaction mode, which migrations can't use.
    expect(code).toBe(1);
  });

  it("takes a line break at the end of the clipboard for a warning only", () => {
    const { code, out } = run(["--clipboard"], {}, { clipboard: `${url()}\n` });
    expect(code).toBe(0);
    expect(out).toContain(
      "  warn    ends with a line break: often only in the copy",
    );
    expect(run([], { DATABASE_URL: `${url()}\n`, DIRECT_URL }).code).toBe(1);
  });

  it("tests the value without its quotes and whitespace", () => {
    const clip = run(
      ["--connect", "--clipboard"],
      {},
      { clipboard: ` "${DATABASE_URL}"\n` },
    );
    expect(clip.prismaCalls).toEqual([DATABASE_URL]);
    expect(clip.code).toBe(1);
  });

  it("never hands Prisma a value whose password breaks the URL, or that has none", () => {
    const { out, prismaCalls } = run(["--connect"], {
      DATABASE_URL: url({ password: SPECIAL }),
      DIRECT_URL: url({ password: SPECIAL, port: "5432", query: "" }),
    });
    expect(prismaCalls).toEqual([]);
    expect(out).toContain("not tested: fix the problems above first");
    const noPassword = run(
      ["--connect", "--clipboard"],
      {},
      {
        clipboard: `postgresql://${PLAIN}@${POOLER}:6543/postgres`,
      },
    );
    expect(noPassword.prismaCalls).toEqual([]);
  });

  it("reports Prisma's verdicts per value", () => {
    const { out, code } = run(
      ["--connect"],
      {
        DATABASE_URL,
        DIRECT_URL: url({ password: OTHER_PLAIN, port: "5432", query: "" }),
      },
      {
        prisma: (value) =>
          value.includes(OTHER_PLAIN)
            ? {
                status: 1,
                stdout: "",
                stderr:
                  "Error: P1000: Authentication failed against database server\n",
              }
            : UP_TO_DATE,
      },
    );
    expect(code).toBe(1);
    const direct = out.slice(out.indexOf("\nDIRECT_URL\n"));
    expect(direct).toContain("  problem login refused (P1000)");
    expect(out.slice(0, out.indexOf("\nDIRECT_URL\n"))).toContain(
      "  ok      login OK",
    );
  });

  it("never prints any form of a password, whatever the value or Prisma says", () => {
    const passwords = [
      SPECIAL,
      SPECIAL_ENCODED,
      PLAIN,
      "made-up Pass:word%41$",
      "p@ss#w/rd?x",
      "[made-up-Pw7]",
    ];
    for (const password of passwords) {
      const values = [
        url({ password }),
        url({ password, port: "5432", query: "" }),
        url({ password, user: "postgres" }),
        url({ password, host: DIRECT, user: "prisma", port: "5432" }),
        ` '${url({ password })}'\n`,
        `postgresql://${password}@${POOLER}:6543/postgres`,
      ];
      // Prisma fakes that echo the password back in every form.
      const echo = (value: string): PrismaRun => ({
        status: 1,
        stdout: `Datasource at ${value}\n`,
        stderr: `Error: P1017: Server has closed the connection. ${forms(password).join(" ")} ${value}\n`,
      });
      for (const value of values) {
        for (const argv of [[], ["--connect"]]) {
          const env = { DATABASE_URL: value, DIRECT_URL: value };
          expectNoPassword(run(argv, env, { prisma: echo }).out, password);
          expectNoPassword(
            run([...argv, "--clipboard"], env, {
              clipboard: value,
              prisma: echo,
            }).out,
            password,
          );
        }
      }
    }
  });
});

describe("main: values an unquoted '#' cut short", () => {
  it("prints no piece of the password, exits 1 and never runs Prisma", () => {
    for (const password of CUT_PASSWORDS) {
      const env = loadEnv(envFile(password));
      for (const argv of [[], ["--connect"]]) {
        const fromEnv = run(argv, env);
        expect(fromEnv.code, password).toBe(1);
        expect(fromEnv.prismaCalls, password).toEqual([]);
        expect(fromEnv.out).toMatch(
          /unquoted '#' starts a comment|cut short by an unquoted '#' in \.env\?/,
        );
        expectNoPiece(fromEnv.out, password);

        for (const clipboard of [env.DATABASE_URL, env.DIRECT_URL]) {
          const clip = run([...argv, "--clipboard"], env, { clipboard });
          expect(clip.code, password).toBe(1);
          expect(clip.prismaCalls, password).toEqual([]);
          expectNoPiece(clip.out, password);
        }
      }
    }
  });

  it("no longer passes a cut value whose host is a piece of the password", () => {
    // Once exit 0 ("No problems"), and --connect logged in at a host made from the password.
    const env = loadEnv(envFile("Tr7vQ@Kp9wLx2#mZ"));
    expect(env.DATABASE_URL).toBe(`postgresql://prisma.${REF}:Tr7vQ@Kp9wLx2`);
    const { code, out, prismaCalls } = run(["--connect"], env);
    expect(code).toBe(1);
    expect(prismaCalls).toEqual([]);
    expect(out).not.toContain("No problems");
  });
});

describe("main: a value too long to be a connection string", () => {
  /** A made-up log of about 38 KB that mentions the environment's value. */
  const LOG = Array.from(
    { length: 400 },
    (_, i) =>
      `2026-09-27 12:00:${String(i % 60).padStart(2, "0")} INFO pool ${DATABASE_URL} ready`,
  ).join("\n");

  it("refuses the clipboard with a fixed message, reads none of it and prints nothing of it", () => {
    expect(LOG.length).toBeGreaterThan(32_767);
    for (const argv of [["--clipboard"], ["--clipboard", "--connect"]]) {
      const { code, out, prismaCalls } = run(
        argv,
        { DATABASE_URL, DIRECT_URL },
        { clipboard: LOG },
      );
      expect(code).toBe(1);
      expect(prismaCalls).toEqual([]);
      expect(out).toContain(
        `problem too long to be a connection string (over ${MAX_VALUE_LENGTH} characters)`,
      );
      expect(out).not.toContain("If it is meant for DIRECT_URL");
      expect(out).not.toContain("Compared with");
      expect(out).not.toContain("INFO");
      expectNoPassword(out, PLAIN);
    }
  });

  it("refuses such a value in the environment too, and compares nothing with it", () => {
    const { code, out, prismaCalls } = run(["--connect"], {
      DATABASE_URL: LOG,
      DIRECT_URL,
    });
    expect(code).toBe(1);
    expect(prismaCalls).toEqual([DIRECT_URL]);
    expect(out).toContain("too long to be a connection string");
    expect(out).not.toContain("DATABASE_URL and DIRECT_URL");
    expectNoPassword(out, PLAIN);
    // Just at the limit is still read.
    const padded = `${DATABASE_URL}&application_name=${"a".repeat(MAX_VALUE_LENGTH - DATABASE_URL.length - 18)}`;
    expect(padded.length).toBe(MAX_VALUE_LENGTH);
    expect(analyze(padded, "DATABASE_URL").present).toBe(true);
    expect(analyze(`${padded}a`, "DATABASE_URL").present).toBe(false);
  });
});

describe("main: an unexpected error", () => {
  const FAILED = "The check stopped on an unexpected error";

  it("prints a fixed message, never the error, and exits 1", () => {
    const password = "made-up-Crash9Pw";
    const value = url({ password });
    const prismaThrows = run(
      ["--connect"],
      { DATABASE_URL: value, DIRECT_URL: value },
      {
        prisma: () => {
          throw new Error(`Invalid regular expression: /${value}|${password}/`);
        },
      },
    );
    expect(prismaThrows.code).toBe(1);
    expect(prismaThrows.out).toContain(FAILED);
    expectNoPassword(prismaThrows.out, password);

    let out = "";
    const env = new Proxy(
      {},
      {
        get: () => {
          throw new Error(`can't read ${value}`);
        },
      },
    );
    const code = main([], {
      env,
      readClipboard: () => "",
      runPrisma: () => UP_TO_DATE,
      write: (text) => {
        out += text;
      },
    });
    expect(code).toBe(1);
    expect(out).toBe(
      "The check stopped on an unexpected error (not shown: it could hold a password).\n",
    );
  });

  it("still exits 1 when even the message can't be written", () => {
    let writes = 0;
    const code = main([], {
      env: { DATABASE_URL, DIRECT_URL },
      readClipboard: () => "",
      runPrisma: () => UP_TO_DATE,
      write: () => {
        writes += 1;
        throw new Error("EPIPE");
      },
    });
    expect(code).toBe(1);
    expect(writes).toBe(2);
  });
});

describe("--connect and ?schema=", () => {
  it("marks a value naming a schema other than public", () => {
    for (const query of [
      "schema=pubilc",
      "schema=Public",
      "schema=public&schema=site",
      "pgbouncer=true&schema=",
      "sch%65ma=madeup",
    ]) {
      expect(
        analyze(url({ port: "5432", query }), "DIRECT_URL").facts.otherSchema,
        query,
      ).toBe(true);
    }
    for (const query of [
      "",
      "schema=public",
      "schema=pub%6Cic&pgbouncer=true",
    ]) {
      expect(
        analyze(url({ port: "5432", query }), "DIRECT_URL").facts.otherSchema,
        query,
      ).toBeUndefined();
    }
  });

  it("never hands Prisma such a value: migrate status would create the schema", () => {
    const typo = url({ port: "5432", query: "schema=pubilc" });
    const { code, out, prismaCalls } = run(["--connect"], {
      DATABASE_URL: typo,
      DIRECT_URL: typo,
    });
    expect(prismaCalls).toEqual([]);
    expect(code).toBe(1);
    expect(out).toContain(
      "not tested: the value names a schema other than public (schema=…), and prisma migrate status would create that schema",
    );
    // Without --connect it is only reported as a parameter.
    expect(run([], { DATABASE_URL: typo, DIRECT_URL: typo }).out).toContain(
      "schema=pubilc",
    );

    const publicSchema = url({ port: "5432", query: "schema=public" });
    expect(
      run(["--connect"], {
        DATABASE_URL: publicSchema,
        DIRECT_URL: publicSchema,
      }).prismaCalls,
    ).toEqual([publicSchema]);
  });

  it("refuses such a value in runPrismaStatus too, before looking for Prisma", () => {
    const empty = mkdtempSync(path.join(tmpdir(), "check-database-url-"));
    try {
      expect(
        runPrismaStatus(url({ port: "5432", query: "schema=pubilc" }), empty)
          .error,
      ).toContain("names a schema other than public");
      expect(runPrismaStatus(DIRECT_URL, empty).error).toBe(
        "the Prisma CLI isn't installed (run npm install in web/)",
      );
    } finally {
      rmSync(empty, { recursive: true, force: true });
    }
  });
});

describe("--connect and the host", () => {
  const OTHER_HOST =
    "--connect logs in only at Supabase's hosts and this computer";

  it("logs in only at Supabase's hosts and this computer", () => {
    // Bare roles: a piece of a cut password shaped like name.tld/db still shows, but is never
    // sent anywhere.
    for (const value of [
      "postgresql://prisma:Xk2@mp7.qz/Gh7Jk9Pq4",
      url({ user: "site", host: "db.example.com", port: "5432", query: "" }),
    ]) {
      const { code, out, prismaCalls } = run(["--connect"], {
        DATABASE_URL: value,
        DIRECT_URL: value,
      });
      expect(prismaCalls, value).toEqual([]);
      expect(code).toBe(1);
      expect(out).toContain(`not tested: ${OTHER_HOST}`);
    }
    for (const value of [
      url({ user: "site", host: "127.0.0.1", port: "55438", query: "" }),
      url({ user: "site", host: "localhost", port: "5432", query: "" }),
      url({ user: "prisma", host: DIRECT, port: "5432", query: "" }),
      DIRECT_URL,
    ]) {
      expect(
        run(["--connect"], { DATABASE_URL: value, DIRECT_URL: value })
          .prismaCalls,
        value,
      ).toEqual([value]);
    }
  });

  it("never hands Prisma a value that didn't decode as UTF-8", () => {
    const value = url({
      user: "site",
      host: "127.0.0.1",
      password: "F�ke-Pw9",
      port: "55438",
      query: "",
    });
    for (const { code, out, prismaCalls } of [
      run(["--connect", "--clipboard"], {}, { clipboard: value }),
      run(["--connect"], { DATABASE_URL: value, DIRECT_URL: value }),
    ]) {
      expect(prismaCalls).toEqual([]);
      expect(code).toBe(1);
      expect(out).toContain(
        "not tested: part of the value didn't decode as UTF-8",
      );
    }
  });

  it("refuses such values in runPrismaStatus too, before looking for Prisma", () => {
    const empty = mkdtempSync(path.join(tmpdir(), "check-database-url-"));
    try {
      expect(
        runPrismaStatus("postgresql://prisma:Xk2@mp7.qz/Gh7Jk9Pq4", empty)
          .error,
      ).toContain(OTHER_HOST);
      expect(
        runPrismaStatus(
          loadEnv(envFile("Xk2@/Gh7Jk9Pq4#Zz8")).DIRECT_URL ?? "",
          empty,
        ).error,
      ).toContain("fix the problems above first");
      expect(
        runPrismaStatus(
          `postgresql://prisma.${REF}:Xk2@mp7.qz/Gh7Jk9Pq4`,
          empty,
        ).error,
      ).toContain("fix the problems above first");
      expect(
        runPrismaStatus(
          url({ user: "site", host: "127.0.0.1", port: "55438", query: "" }),
          empty,
        ).error,
      ).toBe("the Prisma CLI isn't installed (run npm install in web/)");
    } finally {
      rmSync(empty, { recursive: true, force: true });
    }
  });
});

describe("the script itself, run by Node", () => {
  let dir: string;

  beforeEach(() => {
    dir = mkdtempSync(path.join(tmpdir(), "check-database-url-"));
    mkdirSync(path.join(dir, "bin"));
  });

  afterEach(() => {
    rmSync(dir, { recursive: true, force: true });
  });

  /**
   * `node <nodeArgs> script <scriptArgs>` with only a made-up environment, never this machine's,
   * and the fake bin first on PATH.
   */
  function node(
    nodeArgs: string[],
    scriptArgs: string[],
    env: Record<string, string> = {},
  ) {
    return spawnSync(process.execPath, [...nodeArgs, SCRIPT, ...scriptArgs], {
      cwd: dir,
      encoding: "utf8",
      timeout: 30_000,
      env: {
        NODE_ENV: "test",
        PATH: `${path.join(dir, "bin")}:/usr/bin:/bin`,
        HOME: dir,
        ...env,
      },
    });
  }

  it("loads a made-up .env with --env-file and prints no piece of a password '#' cut short", () => {
    const envPath = path.join(dir, "fake.env");
    writeFileSync(
      envPath,
      `DATABASE_URL=${url({ password: "Xk9pQ/Lm3nR7vT#Zz8" })}\nDIRECT_URL=${url({ password: "Ab3c@Qw9Rt5Yh#Zz8", port: "5432", query: "" })}\n`,
    );
    const result = node([`--env-file=${envPath}`], []);
    expect(result.status).toBe(1);
    expect(result.stderr).toBe("");
    expect(result.stdout).toContain("unquoted '#' starts a comment");
    expect(result.stdout).toContain("the host isn't shown");
    expectNoPiece(result.stdout, "Xk9pQ/Lm3nR7vT#Zz8");
    expectNoPiece(result.stdout, "Ab3c@Qw9Rt5Yh#Zz8");
  });

  /** Made up: non-ASCII characters MacRoman writes differently from UTF-8. */
  const UNICODE_PW = "Fäke-Ünï-Pw9";
  const UNICODE_URL = `postgresql://uni:${UNICODE_PW}@127.0.0.1:55438/postgres`;
  const MAC_ROMAN: Record<string, number> = { ä: 0x8a, Ü: 0x86, ï: 0x95 };

  /**
   * A fake pbpaste that writes the clipboard as the real one does: UTF-8 when the locale
   * (LC_ALL, else LC_CTYPE, else LANG) names it, MacRoman otherwise, or always MacRoman.
   */
  function fakePbpaste(text: string, honoursLocale = true) {
    const utf8 = path.join(dir, "clipboard.utf8");
    const macRoman = path.join(dir, "clipboard.macroman");
    writeFileSync(utf8, text);
    writeFileSync(
      macRoman,
      Buffer.from([...text].map((c) => MAC_ROMAN[c] ?? c.charCodeAt(0))),
    );
    const pbpaste = path.join(dir, "bin", "pbpaste");
    writeFileSync(
      pbpaste,
      honoursLocale
        ? `#!/bin/sh\ncase "\${LC_ALL:-\${LC_CTYPE:-$LANG}}" in\n  *UTF-8*|*utf8*|*UTF8*|*utf-8*) cat "${utf8}" ;;\n  *) cat "${macRoman}" ;;\nesac\n`
        : `#!/bin/sh\ncat "${macRoman}"\n`,
    );
    chmodSync(pbpaste, 0o755);
  }

  it("reads the clipboard as UTF-8 whatever the locale, so a non-ASCII password stays whole", () => {
    fakePbpaste(UNICODE_URL);
    // No LANG at all (as under env -i), and an inherited LC_ALL=C that would outrank LANG.
    const locales: Record<string, string>[] = [
      {},
      { LC_ALL: "C" },
      { LANG: "C", LC_CTYPE: "C" },
    ];
    for (const locale of locales) {
      const result = node([], ["--clipboard"], {
        DATABASE_URL: UNICODE_URL,
        ...locale,
      });
      expect(result.stderr).toBe("");
      expect(result.stdout).not.toContain("U+FFFD");
      expect(result.stdout).toContain(
        `password <${[...UNICODE_PW].length} characters>`,
      );
      expect(result.stdout).toContain("  ok      same password in both");
      expect(result.status).toBe(0);
      expectNoPassword(result.stdout, UNICODE_PW);
    }
  });

  it("reports a clipboard that still didn't decode as UTF-8", () => {
    fakePbpaste(UNICODE_URL, false);
    const result = node([], ["--clipboard"], { DATABASE_URL: UNICODE_URL });
    expect(result.stderr).toBe("");
    expect(result.status).toBe(1);
    expect(result.stdout).toContain(
      "problem has a U+FFFD replacement character",
    );
    expectNoPassword(result.stdout, UNICODE_PW);
  });

  it("refuses a 38 KB clipboard with a fixed message, and crashes into nothing", () => {
    const clip = path.join(dir, "clipboard.txt");
    const line = `2026-09-27 12:00:00 INFO connecting ${DATABASE_URL}\n`;
    writeFileSync(clip, line.repeat(Math.ceil(38_400 / line.length)));
    const pbpaste = path.join(dir, "bin", "pbpaste");
    writeFileSync(pbpaste, `#!/bin/sh\ncat "${clip}"\n`);
    chmodSync(pbpaste, 0o755);
    const result = node([], ["--clipboard"], { DATABASE_URL, DIRECT_URL });
    expect(result.status).toBe(1);
    expect(result.stderr).toBe("");
    expect(result.stdout).toContain("too long to be a connection string");
    expect(result.stdout).not.toContain("INFO");
    expectNoPassword(result.stdout, PLAIN);
  });
});

describe("--connect's login method", () => {
  it("uses the site's Prisma client for a transaction-mode value, migrate status otherwise", () => {
    expect(loginMethod(analyze(DATABASE_URL, "DATABASE_URL").facts)).toBe(
      "client",
    );
    expect(loginMethod(analyze(DIRECT_URL, "DIRECT_URL").facts)).toBe(
      "migrate",
    );
    const pgbouncerOn5432 = url({ port: "5432", query: "pgbouncer=true" });
    expect(loginMethod(analyze(pgbouncerOn5432, "DATABASE_URL").facts)).toBe(
      "client",
    );
    const direct = url({
      user: "prisma",
      host: DIRECT,
      port: "5432",
      query: "",
    });
    expect(loginMethod(analyze(direct, "DIRECT_URL").facts)).toBe("migrate");
  });

  it("reads the client's select 1 as a good login, and its P1000 as refused", () => {
    const scrub = makeScrubber([DATABASE_URL]);
    const ok = interpretPrismaStatus(
      { status: 0, stdout: "select 1: OK\n", stderr: "" },
      scrub,
    );
    expect(levels(ok, "login")).toEqual(["ok"]);
    expect(levels(ok, "migrations")).toEqual(["ok"]);
    const refused = interpretPrismaStatus(
      {
        status: 1,
        stdout:
          "P1000\nAuthentication failed against database server, the provided database credentials for `postgres` are not valid.\n",
        stderr: "",
      },
      scrub,
    );
    expect(problems(refused)).toEqual(["login"]);
    expect(messages(refused)).toContain("P1000");
  });
});

describe("--connect with a host parameter", () => {
  it("never logs in: Prisma would connect to the parameter's host, not the one checked", () => {
    for (const extra of ["host=127.0.0.1", "hostaddr=10.0.0.1", "HOST=/tmp"]) {
      const value = url({ query: `pgbouncer=true&${extra}` });
      const { out, prismaCalls } = run(["--connect"], { DATABASE_URL: value });
      expect(prismaCalls).toEqual([]);
      expect(out).toContain("host parameter");
      expect(runPrismaStatus(value).error).toContain("host parameter");
    }
  });
});

describe("a value without a ':' before '@'", () => {
  // What stands before '@' may be the password itself: a dot in it must not make it a role and
  // a project in the comparison.
  const password = "Gm4Wz8Tq.Kp7Yx2Lr";
  const first = `postgresql://${password}@${POOLER}:6543/postgres?pgbouncer=true&connection_limit=1`;
  const second = `postgresql://${password}@${POOLER}:5432/postgres`;

  it("gives no role or project to compare", () => {
    expect(comparePair(first, second)).toEqual([]);
  });

  it("shows no piece of it, from the environment or the clipboard", () => {
    for (const argv of [[], ["--clipboard"]]) {
      const { out } = run(
        argv,
        { DATABASE_URL: first, DIRECT_URL: second },
        { clipboard: first },
      );
      expectNoPiece(out, "Gm4Wz8Tq");
      expectNoPiece(out, "Kp7Yx2Lr");
    }
  });
});
