'use strict';
// The scene list (scenes.json) and the two tools that use it, without a browser:
//   - the list is well formed and holds the porting notes' inventory (ui.md section 10) plus the
//     notch scenes and the panel's chrome per placement, each with the file the app's snapshot
//     run writes;
//   - every scene shows in the harness (the real pages, the scene's own request, events and
//     replies) and leaves no page error and no call the contract refuses; an unknown name makes
//     every page's showScene answer false; every page's layoutReport answers {ok, failures};
//   - tools/compare-png.cjs on generated images (tolerance, budget, size, diff, exit codes).
// What node cannot know is geometry: snapshots.test.cjs renders every scene in a real browser
// when AGENTNOTCH_UI_BROWSER is set and holds the same scenes to layoutReport() there.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const harness = require('./lib/harness.cjs');
const compare = require('./tools/compare-png.cjs');

const SCENES = JSON.parse(fs.readFileSync(path.join(__dirname, 'scenes.json'), 'utf8')).scenes;
const NOTES = fs.readFileSync(path.join(__dirname, '..', '..', 'docs', 'design', 'windows-port-notes', 'ui.md'), 'utf8');
const plain = (value) => (value === undefined ? undefined : JSON.parse(JSON.stringify(value)));

const GLOBALS = { 'notch.html': 'agentnotch', 'settings.html': 'agentnotchSettings', 'agentnotch/panel.html': 'agentnotchPanel' };
const WIDTHS = [400, 520];
const EDGES = ['right', 'left', 'top', 'bottom'];

// ---- the list -------------------------------------------------------------------------------------------

/** The names of ui.md section 10, the inventory of the Mac's sheets (each is rendered at -400 and -520). */
function inventory() {
  const start = NOTES.indexOf('10. SNAPSHOT INVENTORY');
  const end = NOTES.indexOf('Not rendered (they need a sealed app launch)', start);
  assert.ok(start > 0 && end > start, 'ui.md still has its section 10');
  const names = [];
  for (const row of NOTES.slice(start, end).split('\n')) {
    const m = /^ {2}([a-z][a-z0-9-]*)(,.*)?(\s|$)/.exec(row);
    if (!m) continue;
    if (m[2]) {
      // "settings-cloud-signed-out, -signed-in, -overridden, -no-website"
      const prefix = m[1].replace(/-[a-z]+(-[a-z]+)?$/, '');
      names.push(m[1]);
      for (const part of m[2].split(',').map((p) => p.trim()).filter(Boolean)) names.push(`${prefix}${part.split(/\s/)[0]}`);
    } else names.push(m[1]);
  }
  return names;
}

test('the inventory of ui.md section 10 is read as 28 sheets (the reader itself is checked)', () => {
  const names = inventory();
  assert.equal(names.length, 28, names.join(' '));
  for (const name of ['panel-every-state', 'panel-empty', 'chat-no-route', 'settings', 'settings-cloud-signed-in', 'settings-cloud-no-website']) {
    assert.ok(names.includes(name), name);
  }
});

test('scenes.json holds the inventory, the notch scenes and the panel chrome, and nothing else', () => {
  const names = SCENES.map((s) => s.name);
  assert.equal(new Set(names).size, names.length, 'a scene is listed twice');
  for (const name of inventory()) assert.ok(names.includes(name), `${name} (ui.md section 10) is not in scenes.json`);
  const extra = new Set(['panel-menu', 'panel-single-account', 'panel-pinned']);
  const expected = new Set(inventory());
  for (const edge of EDGES) for (const part of ['open', 'card', 'folded']) expected.add(`notch-${edge}-${part}`);
  for (const edge of [...EDGES, 'floating']) expected.add(`panel-chrome-${edge}`);
  for (const name of extra) expected.add(name);
  assert.deepEqual([...names].sort(), [...expected].sort());
});

test('every scene names its page, global, scene, size and the file the app\'s snapshot run writes', () => {
  for (const s of SCENES) {
    assert.ok(Object.prototype.hasOwnProperty.call(GLOBALS, s.page), `${s.name}: page ${s.page}`);
    assert.equal(s.global, GLOBALS[s.page], `${s.name}: global`);
    assert.match(s.scene, /^[a-z]+(-[a-z]+)+$/, `${s.name}: scene`);
    assert.ok(s.widths === true || Number(s.width) > 0, `${s.name}: a width`);
    assert.ok(s.height === 'fit' || Number(s.height) > 0 || s.widths === true && s.height === undefined, `${s.name}: a height`);
    // The file: <name>.png, or <name>-{width}.png for the scenes drawn at both widths.
    assert.equal(s.file, `${s.name}${s.widths ? '-{width}' : ''}.png`, `${s.name}: file`);
    if (s.widths) assert.equal(s.width, undefined, `${s.name}: widths and a fixed width`);
    if (s.mac !== undefined) assert.equal(s.mac, s.name, `${s.name}: the Mac sheet has the same name`);
  }
});

test('the notch scenes cover every edge with its window size; the chrome covers every placement', () => {
  for (const edge of EDGES) {
    for (const part of ['open', 'card', 'folded']) {
      const s = SCENES.find((x) => x.name === `notch-${edge}-${part}`);
      assert.ok(s, `notch-${edge}-${part}`);
      assert.equal(s.scene, `notch-${part}`);
      assert.equal(s.edge, edge);
      // notch_window_size in main.rs: 360 x 650 beside, 650 x 650 flat.
      assert.deepEqual([s.width, s.height], edge === 'left' || edge === 'right' ? [360, 650] : [650, 650]);
    }
  }
  const tails = { right: 'right', left: 'left', top: 'top', bottom: 'bottom' };
  for (const [edge, tail] of Object.entries(tails)) {
    const s = SCENES.find((x) => x.name === `panel-chrome-${edge}`);
    assert.equal(s.panel.placement.tail_edge, tail, edge);
    assert.equal(s.panel.placement.kind, edge === 'left' || edge === 'right' ? 'beside' : 'above_or_below', edge);
  }
  assert.equal(SCENES.find((x) => x.name === 'panel-chrome-floating').panel.placement, undefined);
});

// ---- every scene shows in the harness -----------------------------------------------------------------

async function showing(s, width) {
  const options = {
    edge: s.edge || 'right',
    width: width || s.width || 400,
    height: Number(s.height) || 700,
    replies: s.replies ? Object.assign({}, ...Object.entries(s.replies).map(([k, v]) => ({ [k]: () => v }))) : undefined,
    before: s.panel ? (window) => { window.__AGENTNOTCH_PANEL__ = JSON.parse(JSON.stringify(s.panel)); } : undefined,
  };
  const page = harness.loadPage(s.page, options);
  await page.settle();
  for (const ev of s.events || []) {
    page.emit(ev.event, ev.fixture ? harness.fixture(ev.fixture) : ev.payload);
    await page.settle();
  }
  return page;
}

for (const s of SCENES) {
  test(`scene ${s.name}: showScene('${s.scene}') is true, leaves no page error and no refused call, and layoutReport answers`, async () => {
    for (const width of s.widths ? WIDTHS : [null]) {
      const page = await showing(s, width);
      const g = `window.${s.global}`;
      assert.equal(page.run(`${g}.showScene(${JSON.stringify(s.scene)})`), true, `${s.name}@${width}`);
      await page.settle();
      assert.deepEqual(page.errors.map(String), [], `${s.name}@${width}: page errors`);
      assert.deepEqual(page.hub.violations, [], `${s.name}@${width}: contract violations`);
      const report = plain(page.run(`${g}.layoutReport()`));
      assert.equal(typeof report.ok, 'boolean', `${s.name}: ok`);
      assert.ok(Array.isArray(report.failures), `${s.name}: failures`);
      assert.equal(report.ok, report.failures.length === 0, `${s.name}: ok means no failures`);
      assert.equal(page.document.documentElement.classList.contains('an-static'), true, `${s.name}: the static mode is on`);
    }
  });
}

test('an unknown scene makes every page\'s showScene answer false, and shows nothing', async () => {
  for (const [file, name] of Object.entries(GLOBALS)) {
    const page = harness.loadPage(file, {});
    await page.settle();
    for (const bad of ['nope', '', 'constructor', '__proto__', 'hasOwnProperty', 'toString', 'panel-every-state-', 'notch']) {
      assert.equal(page.run(`window.${name}.showScene(${JSON.stringify(bad)})`), false, `${file}: ${JSON.stringify(bad)}`);
    }
    assert.equal(page.document.documentElement.classList.contains('an-static'), false, `${file}: an unknown name does not set the static mode`);
    assert.deepEqual(page.errors.map(String), [], file);
  }
});

test('layoutReport answers {ok, failures} on all four globals, before any scene, and the chat\'s is the panel\'s', async () => {
  for (const [file, name] of Object.entries(GLOBALS)) {
    const page = harness.loadPage(file, {});
    await page.settle();
    const report = plain(page.run(`window.${name}.layoutReport()`));
    assert.equal(typeof report.ok, 'boolean', name);
    assert.ok(Array.isArray(report.failures), name);
  }
  const page = await showing(SCENES.find((s) => s.name === 'chat-approval'), 400);
  page.run("agentnotchPanel.showScene('chat-approval')");
  await page.settle();
  const chat = plain(page.run('agentnotchChat.layoutReport()'));
  assert.deepEqual(chat, plain(page.run('agentnotchPanel.layoutReport()')));
  assert.equal(chat.route.startsWith('session:'), true);
});

test('layoutReport fails what the browser would measure: clipped text, a control outside the card, a tail in a corner', async () => {
  const placement = SCENES.find((s) => s.name === 'panel-chrome-right');
  const page = await showing(placement, null);
  page.run("agentnotchPanel.showScene('panel-every-state')");
  const card = page.$('#an-card');
  card.__rect = { left: 0, top: 0, width: 400, height: 500 };
  assert.equal(plain(page.run('agentnotchPanel.layoutReport()')).ok, true);
  // a clipped text
  const text = page.$('[data-an-text]');
  assert.ok(text, 'the panel has [data-an-text] elements');
  text.__rect = { left: 10, top: 10, width: 100, height: 16 };
  text.__scrollWidth = 300;
  const clipped = plain(page.run('agentnotchPanel.layoutReport()'));
  assert.equal(clipped.ok, false);
  assert.match(clipped.failures.join(' '), /text is clipped/);
  text.__scrollWidth = 100;
  // a control outside the card
  const button = page.$('#an-card button');
  button.__rect = { left: 390, top: 10, width: 60, height: 20 };
  const outside = plain(page.run('agentnotchPanel.layoutReport()'));
  assert.match(outside.failures.join(' '), /control outside the card/);
  button.__rect = { left: 10, top: 10, width: 20, height: 20 };
  // a tail whose base meets a rounded corner (the card's corner is 16 px)
  page.$('#an-tail').__rect = { left: 399, top: 4, width: 33, height: 36 };
  const corner = plain(page.run('agentnotchPanel.layoutReport()'));
  assert.match(corner.failures.join(' '), /tail reaches into a corner/);
  page.$('#an-tail').__rect = { left: 399, top: 200, width: 33, height: 36 };
  page.$('#an-panel').__rect = { left: 0, top: 0, width: 440, height: 500 };
  assert.doesNotMatch(plain(page.run('agentnotchPanel.layoutReport()')).failures.join(' '), /corner/);
});

// ---- compare-png ---------------------------------------------------------------------------------------------

/** A w x h image filled with one colour; `paint(x, y)` may return another [r, g, b, a]. */
function image(w, h, fill, paint) {
  const data = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const c = (paint && paint(x, y)) || fill;
      data.set(c, (y * w + x) * 4);
    }
  }
  return { width: w, height: h, data };
}

const GREY = [128, 128, 128, 255];

test('compare-png: the PNG writer and reader agree, and the reader takes the other colour types', () => {
  const img = image(7, 5, GREY, (x, y) => (x === 3 && y === 2 ? [10, 20, 30, 40] : null));
  const back = compare.decode(compare.encode(img));
  assert.equal(back.width, 7);
  assert.equal(back.height, 5);
  assert.deepEqual([...back.data], [...img.data]);
  assert.throws(() => compare.decode(Buffer.from('not a png at all, and rather long than a header')), /not a PNG/);
  const png = compare.encode(img);
  const broken = Buffer.from(png);
  broken[40] ^= 0xff;
  assert.throws(() => compare.decode(broken), /checksum|truncated|inflate|data/i);
  // grey+alpha 8 bit and RGB 16 bit, written by hand with the filters the encoder never uses
  const zlib = require('node:zlib');
  const make = (colour, depth, rows) => {
    const raw = Buffer.concat(rows.map((r) => Buffer.from(r)));
    const head = Buffer.alloc(13);
    head.writeUInt32BE(2, 0);
    head.writeUInt32BE(rows.length, 4);
    head[8] = depth;
    head[9] = colour;
    const chunk = (type, body) => {
      const h = Buffer.alloc(8);
      h.writeUInt32BE(body.length, 0);
      h.write(type, 4, 'latin1');
      const c = Buffer.alloc(4);
      c.writeUInt32BE(crc(Buffer.concat([h.subarray(4), body])), 0);
      return Buffer.concat([h, body, c]);
    };
    return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', head), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
  };
  const crc = (buf) => zlib.crc32 ? zlib.crc32(buf) : manualCrc(buf);
  const manualCrc = (buf) => {
    let c = 0xffffffff;
    for (const byte of buf) {
      c ^= byte;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    return (c ^ 0xffffffff) >>> 0;
  };
  // grey+alpha, filter 1 (sub): pixel 1 is pixel 0 plus the stored bytes
  const ga = compare.decode(make(4, 8, [[1, 100, 200, 5, 7]]));
  assert.deepEqual([...ga.data], [100, 100, 100, 200, 105, 105, 105, 207]);
  // RGB 16 bit, filter 2 (up) on the second row: the high bytes are what is read
  const rgb = compare.decode(make(2, 16, [[0, 10, 0, 20, 0, 30, 0, 40, 0, 50, 0, 60, 0], [2, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0]]));
  assert.deepEqual([...rgb.data.subarray(0, 4)], [10, 20, 30, 255]);
  assert.deepEqual([...rgb.data.subarray(8, 12)], [11, 21, 31, 255]);
});

test('compare-png: identical images match; a change within the tolerance does not count; one beyond it does', () => {
  const a = image(100, 100, GREY);
  assert.equal(compare.compare(a, image(100, 100, GREY)).changed, 0);
  assert.equal(compare.compare(a, image(100, 100, GREY)).diff, null);
  // every pixel 8 off (the default tolerance): unchanged; 9 off: changed
  const near = compare.compare(a, image(100, 100, [136, 128, 120, 255]));
  assert.equal(near.changed, 0);
  assert.equal(near.ok, true);
  assert.equal(near.maxDelta, 8);
  const far = compare.compare(a, image(100, 100, [137, 128, 128, 255]));
  assert.equal(far.changed, 10000);
  assert.equal(far.ok, false);
  // a difference in the alpha channel alone counts
  assert.equal(compare.compare(a, image(100, 100, [128, 128, 128, 200])).changed, 10000);
  // the tolerance is an option, 0 to 255
  assert.equal(compare.compare(a, image(100, 100, [137, 128, 128, 255]), { tolerance: 9 }).changed, 0);
  assert.equal(compare.compare(a, image(100, 100, [129, 128, 128, 255]), { tolerance: 0 }).changed, 10000);
  assert.throws(() => compare.compare(a, a, { tolerance: 300 }), /tolerance/);
  assert.throws(() => compare.compare(a, a, { budget: 2 }), /budget/);
});

test('compare-png: the changed-area budget (0.2 % of the pixels by default) and what it counts', () => {
  const a = image(100, 100, GREY); // 10 000 pixels: the default budget allows 20
  const changed = (n) => compare.compare(a, image(100, 100, GREY, (x, y) => (y * 100 + x < n ? [0, 0, 0, 255] : null)));
  assert.equal(changed(20).ok, true);
  assert.equal(changed(20).changed, 20);
  assert.equal(changed(21).ok, false);
  assert.ok(Math.abs(changed(21).fraction - 0.0021) < 1e-12);
  // the budget is an option
  assert.equal(compare.compare(a, image(100, 100, [0, 0, 0, 255]), { budget: 1 }).ok, true);
  assert.equal(compare.compare(a, image(100, 100, GREY, (x, y) => (y * 100 + x < 100 ? [0, 0, 0, 255] : null)), { budget: 0.01 }).ok, true);
  assert.equal(compare.compare(a, image(100, 100, GREY, (x, y) => (y * 100 + x < 101 ? [0, 0, 0, 255] : null)), { budget: 0.01 }).ok, false);
  assert.equal(compare.compare(a, changedImage(), { budget: 0 }).ok, false);
  function changedImage() {
    return image(100, 100, GREY, (x, y) => (x === 5 && y === 5 ? [0, 0, 0, 255] : null));
  }
});

test('compare-png: a different size never matches, and the diff marks every changed pixel in red', () => {
  const mismatch = compare.compare(image(100, 100, GREY), image(100, 101, GREY), { budget: 1 });
  assert.equal(mismatch.ok, false);
  assert.equal(mismatch.sizeMismatch, true);
  assert.deepEqual([mismatch.expectedSize, mismatch.actualSize], [[100, 100], [100, 101]]);
  const r = compare.compare(image(10, 10, GREY), image(10, 10, GREY, (x, y) => (x === 2 && y === 3 ? [255, 255, 255, 255] : null)));
  assert.equal(r.changed, 1);
  const px = (x, y) => [...r.diff.data.subarray((y * 10 + x) * 4, (y * 10 + x) * 4 + 4)];
  assert.deepEqual(px(2, 3), [255, 0, 0, 255]);
  assert.notDeepEqual(px(3, 3), [255, 0, 0, 255]);
  assert.equal(px(3, 3)[3], 255);
});

test('compare-png: the command line writes a diff PNG and exits 0 within the budget, 1 beyond it, 2 for bad input', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'an-compare-'));
  try {
    const write = (name, img) => {
      const file = path.join(dir, name);
      fs.writeFileSync(file, compare.encode(img));
      return file;
    };
    const tool = path.join(__dirname, 'tools', 'compare-png.cjs');
    const run = (...args) => spawnSync(process.execPath, [tool, ...args], { encoding: 'utf8' });
    const base = write('base.png', image(50, 50, GREY));
    const same = write('same.png', image(50, 50, GREY));
    const small = write('small.png', image(50, 50, GREY, (x, y) => (x === 0 && y < 5 ? [0, 0, 0, 255] : null)));
    const big = write('big.png', image(50, 50, GREY, (x) => (x < 25 ? [0, 0, 0, 255] : null)));
    const wide = write('wide.png', image(60, 50, GREY));

    let r = run(base, same, '--diff', path.join(dir, 'd0.png'));
    assert.equal(r.status, 0, r.stdout + r.stderr);
    assert.match(r.stdout, /^ok /);
    assert.equal(fs.existsSync(path.join(dir, 'd0.png')), false, 'no diff when nothing changed');

    r = run(base, small, '--budget', '0.01', '--diff', path.join(dir, 'd1.png'));
    assert.equal(r.status, 0, r.stdout);
    assert.match(r.stdout, /5 of 2500 pixels changed/);
    assert.equal(compare.decode(fs.readFileSync(path.join(dir, 'd1.png'))).width, 50, 'a diff is written even within the budget');

    r = run(base, big, '--diff', path.join(dir, 'd2.png'), '--json');
    assert.equal(r.status, 1);
    const json = JSON.parse(r.stdout.trim());
    assert.equal(json.ok, false);
    assert.equal(json.changed, 1250);
    assert.equal(json.diffFile, path.join(dir, 'd2.png'));
    const diff = compare.decode(fs.readFileSync(path.join(dir, 'd2.png')));
    assert.deepEqual([...diff.data.subarray(0, 4)], [255, 0, 0, 255]);

    r = run(base, wide);
    assert.equal(r.status, 1);
    assert.match(r.stdout, /FAIL .*size is 60x50, the baseline is 50x50/);

    // folders: same names are compared, a missing file on either side fails
    const exp = path.join(dir, 'exp');
    const act = path.join(dir, 'act');
    fs.mkdirSync(exp);
    fs.mkdirSync(act);
    fs.copyFileSync(base, path.join(exp, 'a.png'));
    fs.copyFileSync(same, path.join(act, 'a.png'));
    r = run(exp, act);
    assert.equal(r.status, 0, r.stdout);
    fs.copyFileSync(base, path.join(exp, 'b.png'));
    r = run(exp, act, '--diff', path.join(dir, 'diffs'));
    assert.equal(r.status, 1);
    assert.match(r.stdout, /FAIL b\.png: no capture/);
    fs.copyFileSync(big, path.join(act, 'c.png'));
    r = run(exp, act);
    assert.match(r.stdout, /FAIL c\.png: no baseline/);

    assert.equal(run(base).status, 2);
    assert.equal(run(base, path.join(dir, 'nothing.png')).status, 2);
    assert.equal(run(base, __filename).status, 2);
    assert.equal(run(base, same, '--tolerance').status, 2);
    assert.equal(run(base, same, '--budget', '7').status, 2);
    assert.equal(run(base, dir).status, 2);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
