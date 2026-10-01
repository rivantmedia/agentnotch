#!/usr/bin/env node
'use strict';
// Compares two PNGs the way the smoke test compares a WebView2 capture with its baseline
// (DESIGN-WIN §5.6, layer 2): a pixel counts as changed when any of its channels differs by more
// than `tolerance` (anti-aliasing and font smoothing move edges a little), and the images match
// while the changed pixels stay within `budget`, a fraction of the whole image.
//
//   node compare-png.cjs <expected.png> <actual.png> [--tolerance 8] [--budget 0.002] [--diff <out.png>] [--json]
//   node compare-png.cjs <expected-dir> <actual-dir> [same options; --diff names a folder]
//
// Exit 0: within the budget. Exit 1: beyond it, a different size, or (directories) a file that is
// missing on either side. Exit 2: bad arguments or a file that is not a PNG this tool reads.
// The diff image is the actual image faded, with every changed pixel in red; it is written
// whenever a pixel changed or the sizes differ. Output: one line per image, `ok` or `FAIL`, then
// the counts (with --json one JSON object per line instead).
//
// Node built-ins only (zlib). It reads the PNGs Chromium and WebView2 write (8 or 16 bit grey,
// grey+alpha, RGB, RGBA and palette images, not interlaced) and writes 8-bit RGBA.

const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');

const SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const DEFAULT_TOLERANCE = 8;
const DEFAULT_BUDGET = 0.002;

// ---- PNG ----------------------------------------------------------------------------------------

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 255] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, body) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(body.length, 0);
  head.write(type, 4, 'latin1');
  const tail = Buffer.alloc(4);
  tail.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), body])), 0);
  return Buffer.concat([head, body, tail]);
}

/** `{width, height, data}` (data: width*height*4 bytes, RGBA, 8 bit) as a PNG file. */
function encode(image) {
  const { width, height, data } = image;
  if (!(width > 0 && height > 0) || data.length !== width * height * 4) throw new Error('encode: the pixel data does not fit the size');
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0; // filter: none
    Buffer.from(data.buffer, data.byteOffset + y * stride, stride).copy(raw, y * (stride + 1) + 1);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  return Buffer.concat([SIGNATURE, chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

function paeth(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
}

/** A PNG file as `{width, height, data}` (RGBA, 8 bit). Throws an Error naming what it cannot read. */
function decode(buf) {
  if (!Buffer.isBuffer(buf) || buf.length < 33 || !buf.subarray(0, 8).equals(SIGNATURE)) throw new Error('not a PNG file');
  let at = 8;
  let header = null;
  let palette = null;
  let transparency = null;
  const parts = [];
  while (at + 12 <= buf.length) {
    const length = buf.readUInt32BE(at);
    const type = buf.toString('latin1', at + 4, at + 8);
    const body = buf.subarray(at + 8, at + 8 + length);
    if (body.length !== length) throw new Error('truncated PNG');
    if (buf.readUInt32BE(at + 8 + length) !== crc32(buf.subarray(at + 4, at + 8 + length))) throw new Error(`bad checksum in the ${type} chunk`);
    if (type === 'IHDR') {
      header = { width: body.readUInt32BE(0), height: body.readUInt32BE(4), depth: body[8], colour: body[9], interlace: body[12] };
    } else if (type === 'PLTE') palette = body;
    else if (type === 'tRNS') transparency = body;
    else if (type === 'IDAT') parts.push(body);
    else if (type === 'IEND') break;
    at += 12 + length;
  }
  if (!header) throw new Error('no IHDR chunk');
  const { width, height, depth, colour } = header;
  if (header.interlace) throw new Error('interlaced PNGs are not supported');
  if (!(width > 0 && height > 0) || width * height > 64e6) throw new Error('unreasonable image size');
  const channels = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 }[colour];
  if (!channels || !(depth === 8 || (depth === 16 && colour !== 3))) throw new Error(`unsupported PNG (colour type ${colour}, ${depth} bit)`);
  const bytes = (channels * depth) / 8;
  const stride = width * bytes;
  const raw = zlib.inflateSync(Buffer.concat(parts));
  if (raw.length < (stride + 1) * height) throw new Error('truncated image data');
  const rows = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const src = y * (stride + 1) + 1;
    const dst = y * stride;
    for (let x = 0; x < stride; x++) {
      const left = x >= bytes ? rows[dst + x - bytes] : 0;
      const up = y > 0 ? rows[dst - stride + x] : 0;
      const upLeft = y > 0 && x >= bytes ? rows[dst - stride + x - bytes] : 0;
      let value = raw[src + x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) value += paeth(left, up, upLeft);
      else if (filter !== 0) throw new Error(`bad filter type ${filter}`);
      rows[dst + x] = value & 255;
    }
  }
  const data = new Uint8Array(width * height * 4);
  const step = depth / 8; // bytes per sample
  const sample = (i) => rows[i];
  for (let p = 0; p < width * height; p++) {
    const s = p * bytes;
    const o = p * 4;
    if (colour === 6) {
      data[o] = sample(s); data[o + 1] = sample(s + step); data[o + 2] = sample(s + 2 * step); data[o + 3] = sample(s + 3 * step);
    } else if (colour === 2) {
      data[o] = sample(s); data[o + 1] = sample(s + step); data[o + 2] = sample(s + 2 * step); data[o + 3] = 255;
    } else if (colour === 0) {
      data[o] = data[o + 1] = data[o + 2] = sample(s); data[o + 3] = 255;
    } else if (colour === 4) {
      data[o] = data[o + 1] = data[o + 2] = sample(s); data[o + 3] = sample(s + step);
    } else {
      const index = sample(s);
      if (!palette || index * 3 + 2 >= palette.length) throw new Error('palette index out of range');
      data[o] = palette[index * 3]; data[o + 1] = palette[index * 3 + 1]; data[o + 2] = palette[index * 3 + 2];
      data[o + 3] = transparency && index < transparency.length ? transparency[index] : 255;
    }
  }
  return { width, height, data };
}

// ---- the comparison ---------------------------------------------------------------------------------

/**
 * `expected` and `actual` are `{width, height, data}`. A pixel changed when one of its four
 * channels differs by more than `tolerance`; the images match while `changed / pixels <= budget`
 * and have the same size. `diff` is the image to write, or null when nothing changed.
 */
function compare(expected, actual, options) {
  const tolerance = options && options.tolerance !== undefined ? Number(options.tolerance) : DEFAULT_TOLERANCE;
  const budget = options && options.budget !== undefined ? Number(options.budget) : DEFAULT_BUDGET;
  if (!(tolerance >= 0 && tolerance <= 255)) throw new Error('tolerance is 0 to 255');
  if (!(budget >= 0 && budget <= 1)) throw new Error('budget is a fraction, 0 to 1');
  if (expected.width !== actual.width || expected.height !== actual.height) {
    const width = Math.max(expected.width, actual.width);
    const height = Math.max(expected.height, actual.height);
    const data = new Uint8Array(width * height * 4);
    for (let i = 0; i < data.length; i += 4) {
      data[i] = 255; data[i + 3] = 255;
    }
    return {
      ok: false, sizeMismatch: true, expectedSize: [expected.width, expected.height], actualSize: [actual.width, actual.height],
      changed: width * height, pixels: width * height, fraction: 1, maxDelta: 255, tolerance, budget, diff: { width, height, data },
    };
  }
  const pixels = actual.width * actual.height;
  const diff = new Uint8Array(pixels * 4);
  let changed = 0;
  let maxDelta = 0;
  for (let p = 0; p < pixels; p++) {
    const o = p * 4;
    let worst = 0;
    for (let c = 0; c < 4; c++) {
      const d = Math.abs(expected.data[o + c] - actual.data[o + c]);
      if (d > worst) worst = d;
    }
    if (worst > maxDelta) maxDelta = worst;
    if (worst > tolerance) {
      changed += 1;
      diff[o] = 255; diff[o + 1] = 0; diff[o + 2] = 0; diff[o + 3] = 255;
    } else {
      // The image itself, faded towards white, so a red pixel is placed in its picture.
      const a = actual.data[o + 3] / 255;
      for (let c = 0; c < 3; c++) diff[o + c] = Math.round(255 - (255 - actual.data[o + c]) * a * 0.35);
      diff[o + 3] = 255;
    }
  }
  const fraction = pixels ? changed / pixels : 0;
  return {
    ok: fraction <= budget, sizeMismatch: false, changed, pixels, fraction, maxDelta, tolerance, budget,
    diff: changed > 0 ? { width: actual.width, height: actual.height, data: diff } : null,
  };
}

/** Compares two PNG files; writes the diff image to `options.diff` when there is one. */
function compareFiles(expectedPath, actualPath, options) {
  const result = compare(decode(fs.readFileSync(expectedPath)), decode(fs.readFileSync(actualPath)), options);
  if (result.diff && options && options.diff) {
    fs.mkdirSync(path.dirname(options.diff), { recursive: true });
    fs.writeFileSync(options.diff, encode(result.diff));
    result.diffFile = options.diff;
  }
  delete result.diff;
  return result;
}

// ---- command line ----------------------------------------------------------------------------------------

const USAGE = `usage: compare-png.cjs <expected.png|dir> <actual.png|dir> [--tolerance ${DEFAULT_TOLERANCE}] [--budget ${DEFAULT_BUDGET}] [--diff <out.png|dir>] [--json]`;

function parseArgs(argv) {
  const args = { positional: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--json') args.json = true;
    else if (a === '--help') args.help = true;
    else if (a === '--tolerance' || a === '--budget' || a === '--diff') {
      if (i + 1 >= argv.length) throw new Error(`${a} needs a value`);
      args[a.slice(2)] = argv[++i];
    } else if (a.startsWith('--')) throw new Error(`unknown option ${a}`);
    else args.positional.push(a);
  }
  if (args.positional.length !== 2 && !args.help) throw new Error('two paths are needed');
  return args;
}

function line(name, r, json) {
  if (json) return JSON.stringify(Object.assign({ name }, r));
  if (r.missing) return `FAIL ${name}: ${r.missing}`;
  if (r.sizeMismatch) return `FAIL ${name}: the size is ${r.actualSize.join('x')}, the baseline is ${r.expectedSize.join('x')}`;
  const share = `${(r.fraction * 100).toFixed(3)} %`;
  return `${r.ok ? 'ok  ' : 'FAIL'} ${name}: ${r.changed} of ${r.pixels} pixels changed (${share}, budget ${(r.budget * 100).toFixed(3)} %), largest channel difference ${r.maxDelta}${r.diffFile ? `, diff ${r.diffFile}` : ''}`;
}

function main(argv) {
  const args = parseArgs(argv);
  if (args.help) {
    console.log(USAGE);
    return 0;
  }
  const options = { tolerance: args.tolerance === undefined ? DEFAULT_TOLERANCE : Number(args.tolerance), budget: args.budget === undefined ? DEFAULT_BUDGET : Number(args.budget) };
  if (Number.isNaN(options.tolerance) || Number.isNaN(options.budget)) throw new Error('--tolerance and --budget are numbers');
  const [expected, actual] = args.positional;
  if (fs.statSync(expected).isDirectory() !== fs.statSync(actual).isDirectory()) throw new Error('give two files or two folders');
  let failed = 0;
  if (fs.statSync(expected).isDirectory()) {
    const names = (dir) => fs.readdirSync(dir).filter((n) => n.endsWith('.png'));
    const both = new Set([...names(expected), ...names(actual)]);
    if (both.size === 0) throw new Error('no PNG in either folder');
    for (const name of [...both].sort()) {
      let r;
      if (!fs.existsSync(path.join(actual, name))) r = { ok: false, missing: 'no capture' };
      else if (!fs.existsSync(path.join(expected, name))) r = { ok: false, missing: 'no baseline' };
      else r = compareFiles(path.join(expected, name), path.join(actual, name), Object.assign({}, options, args.diff ? { diff: path.join(args.diff, name) } : {}));
      if (!r.ok) failed += 1;
      console.log(line(name, r, args.json));
    }
  } else {
    const r = compareFiles(expected, actual, Object.assign({}, options, args.diff ? { diff: args.diff } : {}));
    if (!r.ok) failed += 1;
    console.log(line(path.basename(actual), r, args.json));
  }
  return failed ? 1 : 0;
}

module.exports = { encode, decode, compare, compareFiles, DEFAULT_TOLERANCE, DEFAULT_BUDGET };

if (require.main === module) {
  try {
    process.exit(main(process.argv.slice(2)));
  } catch (error) {
    console.error(`${error.message}\n${USAGE}`);
    process.exit(2);
  }
}
