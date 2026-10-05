// A dependency-free PNG comparison for the smoke test's snapshot baselines (DESIGN-WIN §5.6).
//
// Why our own: the runner needs nothing installed beyond Node, and the comparison has to be a
// tolerance (anti-aliasing differs by a few levels between two captures of the same page) plus a
// small changed-area budget (a caret, a clock), which no byte comparison gives.
//
// A pixel is "changed" when any of its four channels differs by more than `tolerance` (0-255).
// Two images match when the changed pixels are at most `maxRatio` of all pixels. Images of
// different sizes never match: a re-layout or a scale change is a real difference, and a diff of
// two sizes would be meaningless.
//
// Module:
//   decodePng(buffer)                    -> { width, height, rgba }   8-bit grey, grey+alpha, RGB, RGBA; not interlaced
//   encodePng(width, height, rgba)       -> Buffer                    8-bit RGBA, filter 0
//   comparePng(actual, baseline, opts)   -> { ok, status, width, height, changed, total, ratio, maxDelta, diff? }
//   compareDirectories(actualDir, baselineDir, diffDir, opts) -> [{ name, status, ... }]
//
// CLI (JSON on stdout, exit 0 = everything matches, 1 = a difference or a missing/bad file, 2 = usage):
//   node png-diff.mjs <actual.png> <baseline.png> [--diff out.png] [--tolerance 24] [--max-ratio 0.005]
//   node png-diff.mjs --actual-dir D --baseline-dir B [--diff-dir O] [--tolerance 24] [--max-ratio 0.005]
//     the directory form compares every *.png in D with the same name in B; a file with no
//     baseline gets status "no-baseline", which is reported and does not fail (until baselines exist).

import { inflateSync, deflateSync } from 'node:zlib';
import { readFileSync, writeFileSync, readdirSync, mkdirSync, existsSync } from 'node:fs';
import { basename, join } from 'node:path';
import { pathToFileURL } from 'node:url';

export const DEFAULT_TOLERANCE = 24; // per channel, of 255: sub-pixel text smoothing stays under it
export const DEFAULT_MAX_RATIO = 0.005; // of all pixels: half a percent

const SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const CHANNELS = { 0: 1, 2: 3, 4: 2, 6: 4 }; // colour type -> samples per pixel

// --- decoding --------------------------------------------------------------------------------

export function decodePng(buffer) {
  if (!Buffer.isBuffer(buffer)) buffer = Buffer.from(buffer);
  if (buffer.length < 8 || !buffer.subarray(0, 8).equals(SIGNATURE)) throw new Error('not a PNG (bad signature)');
  let header = null;
  const data = [];
  let at = 8;
  let ended = false;
  while (at + 12 <= buffer.length) {
    const length = buffer.readUInt32BE(at);
    const type = buffer.toString('latin1', at + 4, at + 8);
    const start = at + 8;
    if (start + length + 4 > buffer.length) throw new Error(`PNG chunk ${type} runs past the end of the file`);
    const body = buffer.subarray(start, start + length);
    if (type === 'IHDR') {
      if (length !== 13) throw new Error('PNG IHDR has the wrong length');
      header = {
        width: body.readUInt32BE(0), height: body.readUInt32BE(4), depth: body[8],
        colorType: body[9], compression: body[10], filter: body[11], interlace: body[12],
      };
    } else if (type === 'IDAT') {
      data.push(body);
    } else if (type === 'IEND') {
      ended = true;
      break;
    }
    at = start + length + 4;
  }
  if (!header) throw new Error('PNG has no IHDR');
  if (!ended) throw new Error('PNG has no IEND (truncated)');
  if (header.depth !== 8) throw new Error(`unsupported PNG bit depth ${header.depth} (8 only)`);
  if (!(header.colorType in CHANNELS)) throw new Error(`unsupported PNG colour type ${header.colorType} (grey, grey+alpha, RGB, RGBA only)`);
  if (header.interlace !== 0) throw new Error('interlaced PNGs are not supported');
  if (header.compression !== 0 || header.filter !== 0) throw new Error('unknown PNG compression or filter method');
  if (header.width === 0 || header.height === 0) throw new Error('PNG has no pixels');
  if (!data.length) throw new Error('PNG has no IDAT');

  const { width, height, colorType } = header;
  const bpp = CHANNELS[colorType];
  const stride = width * bpp;
  const raw = inflateSync(Buffer.concat(data));
  if (raw.length !== (stride + 1) * height) throw new Error(`PNG pixel data is ${raw.length} bytes, expected ${(stride + 1) * height}`);

  // Un-filter in place, one scanline at a time (the previous line is the one just un-filtered).
  const pixels = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const from = y * (stride + 1) + 1;
    const to = y * stride;
    for (let x = 0; x < stride; x++) {
      const value = raw[from + x];
      const left = x >= bpp ? pixels[to + x - bpp] : 0;
      const up = y > 0 ? pixels[to - stride + x] : 0;
      const upLeft = y > 0 && x >= bpp ? pixels[to - stride + x - bpp] : 0;
      let predicted;
      switch (filter) {
        case 0: predicted = 0; break;
        case 1: predicted = left; break;
        case 2: predicted = up; break;
        case 3: predicted = (left + up) >> 1; break;
        case 4: predicted = paeth(left, up, upLeft); break;
        default: throw new Error(`PNG scanline ${y} has the unknown filter type ${filter}`);
      }
      pixels[to + x] = (value + predicted) & 0xff;
    }
  }

  // To RGBA, whatever the file held.
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0, p = 0, o = 0; i < width * height; i++, o += 4) {
    if (colorType === 6) { rgba[o] = pixels[p]; rgba[o + 1] = pixels[p + 1]; rgba[o + 2] = pixels[p + 2]; rgba[o + 3] = pixels[p + 3]; p += 4; }
    else if (colorType === 2) { rgba[o] = pixels[p]; rgba[o + 1] = pixels[p + 1]; rgba[o + 2] = pixels[p + 2]; rgba[o + 3] = 255; p += 3; }
    else if (colorType === 0) { rgba[o] = rgba[o + 1] = rgba[o + 2] = pixels[p]; rgba[o + 3] = 255; p += 1; }
    else { rgba[o] = rgba[o + 1] = rgba[o + 2] = pixels[p]; rgba[o + 3] = pixels[p + 1]; p += 2; }
  }
  return { width, height, rgba };
}

function paeth(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  if (pa <= pb && pa <= pc) return a;
  return pb <= pc ? b : c;
}

// --- encoding --------------------------------------------------------------------------------

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buffer) {
  let c = 0xffffffff;
  for (const byte of buffer) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, body) {
  const out = Buffer.alloc(12 + body.length);
  out.writeUInt32BE(body.length, 0);
  out.write(type, 4, 'latin1');
  body.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + body.length)), 8 + body.length);
  return out;
}

export function encodePng(width, height, rgba) {
  if (rgba.length !== width * height * 4) throw new Error('encodePng: the pixel array does not match the size');
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // depth
  header[9] = 6; // RGBA
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0;
    Buffer.from(rgba.buffer, rgba.byteOffset + y * stride, stride).copy(raw, y * (stride + 1) + 1);
  }
  return Buffer.concat([SIGNATURE, chunk('IHDR', header), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

// --- comparing -------------------------------------------------------------------------------

// `actual` and `baseline` are PNG files' bytes (or decoded { width, height, rgba }). With
// `diff: true` a different pair of the same size also carries `diff`, the PNG to look at:
// changed pixels in red over a faded copy of the actual image.
export function comparePng(actual, baseline, { tolerance = DEFAULT_TOLERANCE, maxRatio = DEFAULT_MAX_RATIO, diff = false } = {}) {
  const a = actual.rgba ? actual : decodePng(actual);
  const b = baseline.rgba ? baseline : decodePng(baseline);
  if (a.width !== b.width || a.height !== b.height) {
    return {
      ok: false, status: 'size-mismatch', width: a.width, height: a.height,
      baselineWidth: b.width, baselineHeight: b.height, changed: null, total: null, ratio: null, maxDelta: null,
    };
  }
  const total = a.width * a.height;
  let changed = 0;
  let maxDelta = 0;
  const marks = new Uint8Array(total);
  for (let i = 0; i < total; i++) {
    let worst = 0;
    for (let c = 0; c < 4; c++) {
      const delta = Math.abs(a.rgba[i * 4 + c] - b.rgba[i * 4 + c]);
      if (delta > worst) worst = delta;
    }
    if (worst > maxDelta) maxDelta = worst;
    if (worst > tolerance) { marks[i] = 1; changed++; }
  }
  const ratio = changed / total;
  const ok = ratio <= maxRatio;
  const result = { ok, status: ok ? 'same' : 'different', width: a.width, height: a.height, changed, total, ratio, maxDelta };
  if (diff && changed > 0) {
    const out = new Uint8Array(total * 4);
    for (let i = 0; i < total; i++) {
      if (marks[i]) { out[i * 4] = 255; out[i * 4 + 1] = 0; out[i * 4 + 2] = 0; out[i * 4 + 3] = 255; continue; }
      // Faded: 25 % of the actual pixel over white, so the red stands out on any page colour.
      for (let c = 0; c < 3; c++) out[i * 4 + c] = Math.round(a.rgba[i * 4 + c] * 0.25 + 255 * 0.75);
      out[i * 4 + 3] = 255;
    }
    result.diff = encodePng(a.width, a.height, out);
  }
  return result;
}

// Every *.png in actualDir against the same name in baselineDir. A diff image is written to
// diffDir for each difference (and a size mismatch gets none: there is nothing to overlay).
export function compareDirectories(actualDir, baselineDir, diffDir, opts = {}) {
  const names = readdirSync(actualDir).filter((n) => n.toLowerCase().endsWith('.png')).sort();
  const results = [];
  for (const name of names) {
    const baselinePath = join(baselineDir, name);
    if (!existsSync(baselinePath)) { results.push({ name, status: 'no-baseline', ok: true }); continue; }
    try {
      const r = comparePng(readFileSync(join(actualDir, name)), readFileSync(baselinePath), { ...opts, diff: Boolean(diffDir) });
      if (r.diff && diffDir) {
        mkdirSync(diffDir, { recursive: true });
        const diffPath = join(diffDir, name.replace(/\.png$/i, '') + '.diff.png');
        writeFileSync(diffPath, r.diff);
        r.diffFile = diffPath;
      }
      delete r.diff;
      results.push({ name, ...r });
    } catch (e) {
      results.push({ name, status: 'error', ok: false, error: e.message });
    }
  }
  return results;
}

// --- command line ----------------------------------------------------------------------------

function parseArgs(argv) {
  const options = {};
  const positional = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) { positional.push(arg); continue; }
    const key = arg.slice(2);
    const value = argv[++i];
    if (value === undefined) throw new Error(`--${key} needs a value`);
    options[key] = value;
  }
  return { options, positional };
}

function numberOption(options, key, fallback, max) {
  if (options[key] === undefined) return fallback;
  const value = Number(options[key]);
  if (!Number.isFinite(value) || value < 0 || value > max) throw new Error(`--${key} must be a number from 0 to ${max}`);
  return value;
}

function main(argv) {
  let options; let positional;
  try { ({ options, positional } = parseArgs(argv)); } catch (e) { console.error(e.message); return 2; }
  let opts;
  try {
    opts = { tolerance: numberOption(options, 'tolerance', DEFAULT_TOLERANCE, 255), maxRatio: numberOption(options, 'max-ratio', DEFAULT_MAX_RATIO, 1) };
  } catch (e) { console.error(e.message); return 2; }

  if (options['actual-dir'] !== undefined || options['baseline-dir'] !== undefined) {
    if (!options['actual-dir'] || !options['baseline-dir'] || positional.length) {
      console.error('usage: png-diff.mjs --actual-dir D --baseline-dir B [--diff-dir O]'); return 2;
    }
    if (!existsSync(options['actual-dir'])) { console.error(`no such folder: ${options['actual-dir']}`); return 2; }
    const results = compareDirectories(options['actual-dir'], options['baseline-dir'], options['diff-dir'], opts);
    console.log(JSON.stringify({ results }, null, 2));
    return results.every((r) => r.ok) ? 0 : 1;
  }

  if (positional.length !== 2) {
    console.error('usage: png-diff.mjs <actual.png> <baseline.png> [--diff out.png] [--tolerance N] [--max-ratio R]\n   or: png-diff.mjs --actual-dir D --baseline-dir B [--diff-dir O]');
    return 2;
  }
  const name = basename(positional[0]);
  let r;
  try {
    r = comparePng(readFileSync(positional[0]), readFileSync(positional[1]), { ...opts, diff: Boolean(options.diff) });
  } catch (e) {
    console.log(JSON.stringify({ name, status: 'error', ok: false, error: e.message }));
    return 1;
  }
  if (r.diff && options.diff) { writeFileSync(options.diff, r.diff); r.diffFile = options.diff; }
  delete r.diff;
  console.log(JSON.stringify({ name, ...r }));
  return r.ok ? 0 : 1;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const code = main(process.argv.slice(2));
  // Exit after stdout is flushed, whatever else is pending.
  process.stdout.write('', () => process.exit(code));
}
