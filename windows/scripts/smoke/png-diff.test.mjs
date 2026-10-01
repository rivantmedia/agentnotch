// Tests for png-diff.mjs on PNGs generated here (no fixtures, no Mac renders).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { deflateSync } from 'node:zlib';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { decodePng, encodePng, comparePng, compareDirectories, DEFAULT_TOLERANCE } from './png-diff.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const cli = join(here, 'png-diff.mjs');

// --- a tiny independent encoder: any colour type, any filter per row ---------------------------

const SIG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
function crc(buf) {
  let c = 0xffffffff;
  for (const b of buf) { c ^= b; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; }
  return (c ^ 0xffffffff) >>> 0;
}
function ck(type, body) {
  const out = Buffer.alloc(12 + body.length);
  out.writeUInt32BE(body.length, 0); out.write(type, 4, 'latin1'); body.copy(out, 8);
  out.writeUInt32BE(crc(out.subarray(4, 8 + body.length)), 8 + body.length);
  return out;
}
function paeth(a, b, c) {
  const p = a + b - c; const pa = Math.abs(p - a); const pb = Math.abs(p - b); const pc = Math.abs(p - c);
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
}
// samples: Uint8Array of width*height*channels; filters: one filter type per row (cycled).
function makePng(width, height, colorType, samples, filters = [0], { split = false, interlace = 0, depth = 8 } = {}) {
  const bpp = { 0: 1, 2: 3, 4: 2, 6: 4 }[colorType];
  const stride = width * bpp;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    const f = filters[y % filters.length];
    raw[y * (stride + 1)] = f;
    for (let x = 0; x < stride; x++) {
      const v = samples[y * stride + x];
      const left = x >= bpp ? samples[y * stride + x - bpp] : 0;
      const up = y > 0 ? samples[(y - 1) * stride + x] : 0;
      const ul = y > 0 && x >= bpp ? samples[(y - 1) * stride + x - bpp] : 0;
      const pred = [0, left, up, (left + up) >> 1, paeth(left, up, ul)][f];
      raw[y * (stride + 1) + 1 + x] = (v - pred) & 0xff;
    }
  }
  const z = deflateSync(raw);
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0); header.writeUInt32BE(height, 4); header[8] = depth; header[9] = colorType; header[12] = interlace;
  const idats = split ? [z.subarray(0, 7), z.subarray(7)] : [z];
  return Buffer.concat([SIG, ck('IHDR', header), ...idats.map((d) => ck('IDAT', d)), ck('tEXt', Buffer.from('k\0v')), ck('IEND', Buffer.alloc(0))]);
}

// A deterministic picture with gradients and noise so every filter has something to predict.
function picture(width, height, channels, seed = 1) {
  const out = new Uint8Array(width * height * channels);
  let s = seed;
  for (let i = 0; i < out.length; i++) {
    s = (s * 1103515245 + 12345) & 0x7fffffff;
    out[i] = ((i * 7) + (s >> 16)) & 0xff;
  }
  return out;
}
function rgbaOf(samples, channels) {
  const n = samples.length / channels;
  const rgba = new Uint8Array(n * 4);
  for (let i = 0; i < n; i++) {
    const s = samples.subarray(i * channels, i * channels + channels);
    const o = i * 4;
    if (channels === 4) rgba.set(s, o);
    else if (channels === 3) { rgba.set(s, o); rgba[o + 3] = 255; }
    else if (channels === 1) { rgba[o] = rgba[o + 1] = rgba[o + 2] = s[0]; rgba[o + 3] = 255; }
    else { rgba[o] = rgba[o + 1] = rgba[o + 2] = s[0]; rgba[o + 3] = s[1]; }
  }
  return rgba;
}
function flat(width, height, [r, g, b, a = 255]) {
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0; i < width * height; i++) rgba.set([r, g, b, a], i * 4);
  return rgba;
}

// --- decoding ------------------------------------------------------------------------------------

for (const [filter, name] of [[0, 'none'], [1, 'sub'], [2, 'up'], [3, 'average'], [4, 'paeth']]) {
  test(`decodes RGBA rows filtered with ${name}`, () => {
    const samples = picture(13, 9, 4, filter + 1);
    const png = makePng(13, 9, 6, samples, [filter]);
    const decoded = decodePng(png);
    assert.equal(decoded.width, 13);
    assert.equal(decoded.height, 9);
    assert.deepEqual(Buffer.from(decoded.rgba), Buffer.from(samples));
  });
  test(`decodes RGB rows filtered with ${name}`, () => {
    const samples = picture(11, 7, 3, filter + 9);
    const decoded = decodePng(makePng(11, 7, 2, samples, [filter]));
    assert.deepEqual(Buffer.from(decoded.rgba), Buffer.from(rgbaOf(samples, 3)));
  });
}

test('decodes a picture whose rows use all five filters in turn, in two IDAT chunks, past an ancillary chunk', () => {
  const samples = picture(17, 15, 4, 5);
  const decoded = decodePng(makePng(17, 15, 6, samples, [0, 1, 2, 3, 4], { split: true }));
  assert.deepEqual(Buffer.from(decoded.rgba), Buffer.from(samples));
});

test('RGB and RGBA of the same picture decode to the same pixels', () => {
  const rgb = picture(8, 8, 3, 3);
  const rgba = rgbaOf(rgb, 3);
  const a = decodePng(makePng(8, 8, 2, rgb, [4]));
  const b = decodePng(makePng(8, 8, 6, rgba, [1]));
  assert.deepEqual(Buffer.from(a.rgba), Buffer.from(b.rgba));
  assert.equal(comparePng(a, b, { tolerance: 0, maxRatio: 0 }).ok, true);
});

test('grey and grey+alpha decode to RGBA', () => {
  const grey = picture(5, 5, 1, 2);
  assert.deepEqual(Buffer.from(decodePng(makePng(5, 5, 0, grey, [3])).rgba), Buffer.from(rgbaOf(grey, 1)));
  const ga = picture(5, 5, 2, 4);
  assert.deepEqual(Buffer.from(decodePng(makePng(5, 5, 4, ga, [2])).rgba), Buffer.from(rgbaOf(ga, 2)));
});

test('refuses what it cannot read, with a reason', () => {
  assert.throws(() => decodePng(Buffer.from('not a png at all')), /signature/);
  const good = makePng(2, 2, 6, picture(2, 2, 4), [0]);
  assert.throws(() => decodePng(good.subarray(0, good.length - 12)), /no IEND|runs past/);
  assert.throws(() => decodePng(makePng(2, 2, 6, picture(2, 2, 4), [0], { interlace: 1 })), /interlaced/);
  assert.throws(() => decodePng(makePng(2, 2, 6, picture(2, 2, 4), [0], { depth: 16 })), /bit depth/);
  const palette = makePng(2, 2, 0, picture(2, 2, 1), [0]);
  palette[8 + 8 + 9] = 3; // colour type 3
  assert.throws(() => decodePng(palette), /colour type/);
});

test('encode then decode gives the same pixels back', () => {
  const rgba = picture(31, 17, 4, 6);
  const back = decodePng(encodePng(31, 17, rgba));
  assert.equal(back.width, 31);
  assert.equal(back.height, 17);
  assert.deepEqual(Buffer.from(back.rgba), Buffer.from(rgba));
});

// --- comparing -----------------------------------------------------------------------------------

test('identical images match with nothing changed', () => {
  const png = encodePng(20, 10, picture(20, 10, 4, 7));
  const r = comparePng(png, png, { diff: true });
  assert.equal(r.ok, true);
  assert.equal(r.status, 'same');
  assert.equal(r.changed, 0);
  assert.equal(r.maxDelta, 0);
  assert.equal(r.diff, undefined);
});

test('a difference inside the channel tolerance is not a change', () => {
  const base = flat(20, 10, [100, 100, 100]);
  const near = flat(20, 10, [100 + DEFAULT_TOLERANCE, 100 - DEFAULT_TOLERANCE, 100]);
  const r = comparePng(encodePng(20, 10, near), encodePng(20, 10, base), { maxRatio: 0 });
  assert.equal(r.ok, true);
  assert.equal(r.changed, 0);
  assert.equal(r.maxDelta, DEFAULT_TOLERANCE);
});

test('a difference beyond the tolerance counts its pixels, and the budget decides', () => {
  const base = flat(20, 10, [100, 100, 100]);
  const other = Uint8Array.from(base);
  for (let i = 0; i < 4; i++) other[i * 4] = 100 + DEFAULT_TOLERANCE + 1; // 4 of 200 pixels: 2 %
  const a = encodePng(20, 10, other);
  const b = encodePng(20, 10, base);
  const tight = comparePng(a, b, { maxRatio: 0.005, diff: true });
  assert.equal(tight.ok, false);
  assert.equal(tight.status, 'different');
  assert.equal(tight.changed, 4);
  assert.equal(tight.total, 200);
  assert.equal(tight.ratio, 0.02);
  assert.equal(tight.maxDelta, DEFAULT_TOLERANCE + 1);
  assert.equal(comparePng(a, b, { maxRatio: 0.02 }).ok, true);
  assert.equal(comparePng(a, b, { maxRatio: 0.02, tolerance: DEFAULT_TOLERANCE + 1 }).changed, 0);
  // The diff image is a PNG of the same size with the changed pixels red.
  const diff = decodePng(tight.diff);
  assert.equal(diff.width, 20);
  assert.equal(diff.height, 10);
  assert.deepEqual(Array.from(diff.rgba.subarray(0, 4)), [255, 0, 0, 255]);
  assert.deepEqual(Array.from(diff.rgba.subarray(16, 20)), [Math.round(100 * 0.25 + 191.25), Math.round(100 * 0.25 + 191.25), Math.round(100 * 0.25 + 191.25), 255]);
});

test('alpha counts as a channel', () => {
  const a = encodePng(4, 4, flat(4, 4, [10, 10, 10, 255]));
  const b = encodePng(4, 4, flat(4, 4, [10, 10, 10, 0]));
  assert.equal(comparePng(a, b).ok, false);
});

test('images of different sizes never match, and say both sizes', () => {
  const r = comparePng(encodePng(20, 10, flat(20, 10, [1, 2, 3])), encodePng(20, 11, flat(20, 11, [1, 2, 3])), { diff: true });
  assert.equal(r.ok, false);
  assert.equal(r.status, 'size-mismatch');
  assert.deepEqual([r.width, r.height, r.baselineWidth, r.baselineHeight], [20, 10, 20, 11]);
  assert.equal(r.diff, undefined);
});

test('a file in RGB compares with its RGBA twin', () => {
  const rgb = picture(9, 9, 3, 8);
  const a = makePng(9, 9, 2, rgb, [2]);
  const b = encodePng(9, 9, rgbaOf(rgb, 3));
  assert.equal(comparePng(a, b, { tolerance: 0, maxRatio: 0 }).ok, true);
});

// --- directories and the command line ---------------------------------------------------------------

function scratch() { return mkdtempSync(join(tmpdir(), 'png-diff-test-')); }

test('a folder is compared by name: same, different with a diff file, size mismatch, no baseline, unreadable', () => {
  const dir = scratch();
  try {
    const actual = join(dir, 'actual'); const base = join(dir, 'base'); const diffs = join(dir, 'diffs');
    mkdirSync(actual); mkdirSync(base);
    const grey = flat(10, 10, [50, 50, 50]);
    const odd = Uint8Array.from(grey); for (let i = 0; i < 20; i++) odd[i * 4] = 255;
    writeFileSync(join(actual, 'same.png'), encodePng(10, 10, grey));
    writeFileSync(join(base, 'same.png'), encodePng(10, 10, grey));
    writeFileSync(join(actual, 'diff.png'), encodePng(10, 10, odd));
    writeFileSync(join(base, 'diff.png'), encodePng(10, 10, grey));
    writeFileSync(join(actual, 'size.png'), encodePng(10, 10, grey));
    writeFileSync(join(base, 'size.png'), encodePng(11, 10, flat(11, 10, [50, 50, 50])));
    writeFileSync(join(actual, 'new.png'), encodePng(10, 10, grey));
    writeFileSync(join(actual, 'bad.png'), 'garbage');
    writeFileSync(join(base, 'bad.png'), encodePng(10, 10, grey));
    writeFileSync(join(actual, 'manifest.json'), '[]');
    const results = compareDirectories(actual, base, diffs);
    const by = Object.fromEntries(results.map((r) => [r.name, r]));
    assert.deepEqual(Object.keys(by).sort(), ['bad.png', 'diff.png', 'new.png', 'same.png', 'size.png']);
    assert.equal(by['same.png'].status, 'same');
    assert.equal(by['diff.png'].status, 'different');
    assert.equal(by['diff.png'].changed, 20);
    assert.ok(existsSync(by['diff.png'].diffFile));
    assert.equal(by['size.png'].status, 'size-mismatch');
    assert.equal(by['new.png'].status, 'no-baseline');
    assert.equal(by['new.png'].ok, true);
    assert.equal(by['bad.png'].status, 'error');
    assert.match(by['bad.png'].error, /signature/);
    assert.equal(existsSync(join(diffs, 'same.diff.png')), false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the command line: exit 0 for a match, 1 for a difference with a diff file, 2 for bad usage', () => {
  const dir = scratch();
  try {
    const a = join(dir, 'a.png'); const b = join(dir, 'b.png'); const out = join(dir, 'out.png');
    writeFileSync(a, encodePng(6, 6, flat(6, 6, [0, 0, 0])));
    writeFileSync(b, encodePng(6, 6, flat(6, 6, [255, 255, 255])));
    const same = spawnSync(process.execPath, [cli, a, a], { encoding: 'utf8' });
    assert.equal(same.status, 0);
    assert.equal(JSON.parse(same.stdout).status, 'same');
    const different = spawnSync(process.execPath, [cli, a, b, '--diff', out], { encoding: 'utf8' });
    assert.equal(different.status, 1);
    const parsed = JSON.parse(different.stdout);
    assert.equal(parsed.status, 'different');
    assert.equal(parsed.diffFile, out);
    assert.equal(decodePng(readFileSync(out)).width, 6);
    const loose = spawnSync(process.execPath, [cli, a, b, '--tolerance', '255'], { encoding: 'utf8' });
    assert.equal(loose.status, 0);
    assert.equal(spawnSync(process.execPath, [cli, a], { encoding: 'utf8' }).status, 2);
    assert.equal(spawnSync(process.execPath, [cli, a, b, '--tolerance', 'lots'], { encoding: 'utf8' }).status, 2);
    const missing = spawnSync(process.execPath, [cli, a, join(dir, 'none.png')], { encoding: 'utf8' });
    assert.equal(missing.status, 1);
    assert.equal(JSON.parse(missing.stdout).status, 'error');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the folder form of the command line reports a missing baseline without failing, and a difference with a failure', () => {
  const dir = scratch();
  try {
    const actual = join(dir, 'actual'); const base = join(dir, 'base');
    mkdirSync(actual); mkdirSync(base);
    writeFileSync(join(actual, 'x.png'), encodePng(4, 4, flat(4, 4, [9, 9, 9])));
    const none = spawnSync(process.execPath, [cli, '--actual-dir', actual, '--baseline-dir', base], { encoding: 'utf8' });
    assert.equal(none.status, 0);
    assert.equal(JSON.parse(none.stdout).results[0].status, 'no-baseline');
    writeFileSync(join(base, 'x.png'), encodePng(4, 4, flat(4, 4, [200, 9, 9])));
    const diffDir = join(dir, 'diffs');
    const bad = spawnSync(process.execPath, [cli, '--actual-dir', actual, '--baseline-dir', base, '--diff-dir', diffDir], { encoding: 'utf8' });
    assert.equal(bad.status, 1);
    assert.ok(existsSync(join(diffDir, 'x.diff.png')));
    assert.equal(spawnSync(process.execPath, [cli, '--actual-dir', actual], { encoding: 'utf8' }).status, 2);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
