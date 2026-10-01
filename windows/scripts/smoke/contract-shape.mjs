// Checks that a request body has the shape web/contract fixes for the app => website API
// (web/contract/README.md): the fixture's key sets and JSON types, with the few places the
// contract allows null or leaves a key out. Dependency-free; also a CLI that prints JSON.
//
//   node contract-shape.mjs <body.json> [--fixture <file>] [--sorted] [--millis]
//
// Exit 0 with {"ok":true,"problems":[]}, exit 1 with the problems listed.
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
export const DEFAULT_FIXTURE = resolve(here, '../../../web/contract/fixtures/sync-request.json');

// Paths use `[]` for "every element of an array". A path listed here may be null; the value is
// the type it has when it is not.
const NULLABLE = {
  'accounts[].email': 'string',
  'accounts[].organizationName': 'string',
  'accounts[].plan': 'string',
  'accounts[].label': 'string',
  'sessions[].title': 'string',
  'sessions[].endedAt': 'string',
  'sessions[].costUsd': 'number',
  'usage[].windows[].resetsAt': 'string',
};
const OPTIONAL = new Set(['sessions[].summary']);
const DATES = new Set([
  'sessions[].startedAt',
  'sessions[].lastActivityAt',
  'sessions[].endedAt',
  'sessions[].summary.generatedAt',
  'usage[].observedAt',
  'usage[].windows[].resetsAt',
]);
const KEYS = new Set([
  'accounts[].key',
  'sessions[].accountKey',
  'sessions[].project.key',
  'usage[].accountKey',
]);
const INTEGERS = new Set([
  'schemaVersion',
  'sessions[].messageCount',
  'sessions[].tokens.input',
  'sessions[].tokens.output',
  'sessions[].tokens.cacheCreation',
  'sessions[].tokens.cacheRead',
]);

const DAY = 86_400_000;
const MIN_DATE = Date.parse('2023-01-01T00:00:00Z');
const ANY_DATE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,9})?Z$/;
const MILLIS_DATE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;
const HEX64 = /^[0-9a-f]{64}$/;

export function jsonType(v) {
  if (v === null) return 'null';
  if (Array.isArray(v)) return 'array';
  return typeof v;
}

// Options: sortedKeys (the Mac app encodes with sorted keys; the fixture is hand-written, so it
// is not required by default), millisDates (the app always writes `.mmmZ`), now (for the range).
export function validate(body, fixture, options = {}) {
  const { sortedKeys = false, millisDates = false, now = Date.now() } = options;
  const problems = [];
  const walk = (value, shape, path, shown) => {
    const here = shown || '$';
    const nullable = Object.hasOwn(NULLABLE, path);
    if (value === null) {
      if (!nullable && shape !== null) problems.push(`${here}: null where ${jsonType(shape)} is expected`);
      return;
    }
    // A fixture null stands for "null or the nullable type".
    const want = shape === null ? NULLABLE[path] : jsonType(shape);
    if (want === undefined) {
      problems.push(`${here}: expected null, got ${jsonType(value)}`);
      return;
    }
    if (jsonType(value) !== want) {
      problems.push(`${here}: expected ${want}, got ${jsonType(value)}`);
      return;
    }
    if (want === 'array') {
      if (shape.length === 0) return;
      value.forEach((item, i) => walk(item, shape[0], `${path}[]`, `${shown}[${i}]`));
      return;
    }
    if (want === 'object') {
      const keys = Object.keys(value);
      if (sortedKeys && keys.some((k, i) => i > 0 && keys[i - 1] > k)) {
        problems.push(`${here}: keys are not sorted (${keys.join(', ')})`);
      }
      for (const k of Object.keys(shape)) {
        const child = path ? `${path}.${k}` : k;
        const childShown = shown ? `${shown}.${k}` : k;
        if (!Object.hasOwn(value, k)) {
          if (!OPTIONAL.has(child)) problems.push(`${childShown}: missing`);
        } else {
          walk(value[k], shape[k], child, childShown);
        }
      }
      for (const k of keys) {
        if (!Object.hasOwn(shape, k)) problems.push(`${shown ? `${shown}.${k}` : k}: not in the contract`);
      }
      return;
    }
    scalar(value, path, here);
  };
  const scalar = (value, path, here) => {
    if (INTEGERS.has(path) && !Number.isInteger(value)) problems.push(`${here}: not an integer`);
    if (KEYS.has(path) && !HEX64.test(value)) problems.push(`${here}: not 64 lowercase hex characters`);
    if (DATES.has(path)) {
      const re = millisDates ? MILLIS_DATE : ANY_DATE;
      const t = re.test(value) ? Date.parse(value) : NaN;
      if (Number.isNaN(t)) {
        problems.push(`${here}: not a ${millisDates ? 'YYYY-MM-DDTHH:MM:SS.mmmZ' : 'UTC ISO 8601'} date (${JSON.stringify(value)})`);
      } else {
        // resetsAt may lie up to 32 days ahead; every other date may not pass now + 1 day.
        const ahead = path.endsWith('resetsAt') ? 32 * DAY : DAY;
        if (t < MIN_DATE || t > now + ahead) problems.push(`${here}: outside the allowed date range`);
      }
    }
  };
  walk(body, fixture, '', '');
  // Cross reference the contract states in words: every accountKey is declared.
  const declared = new Set((body?.accounts ?? []).map((a) => a?.key));
  for (const kind of ['sessions', 'usage']) {
    (body?.[kind] ?? []).forEach((row, i) => {
      if (row && typeof row.accountKey === 'string' && !declared.has(row.accountKey)) {
        problems.push(`${kind}[${i}].accountKey: not listed in accounts[]`);
      }
    });
  }
  return problems;
}

export function validateFile(bodyFile, fixtureFile = DEFAULT_FIXTURE, options = {}) {
  let body;
  try {
    body = JSON.parse(readFileSync(bodyFile, 'utf8'));
  } catch (e) {
    return [`${bodyFile}: not JSON (${e.message})`];
  }
  return validate(body, JSON.parse(readFileSync(fixtureFile, 'utf8')), options);
}

function main(argv) {
  const args = { files: [], fixture: DEFAULT_FIXTURE, sortedKeys: false, millisDates: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--fixture') args.fixture = argv[++i];
    else if (argv[i] === '--sorted') args.sortedKeys = true;
    else if (argv[i] === '--millis') args.millisDates = true;
    else args.files.push(argv[i]);
  }
  if (args.files.length !== 1) {
    console.log(JSON.stringify({ ok: false, problems: ['usage: contract-shape.mjs <body.json> [--fixture f] [--sorted] [--millis]'] }));
    return 2;
  }
  const problems = validateFile(args.files[0], args.fixture, args);
  console.log(JSON.stringify({ ok: problems.length === 0, problems }));
  return problems.length === 0 ? 0 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.exitCode = main(process.argv.slice(2));
}
