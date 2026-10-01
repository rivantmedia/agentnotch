'use strict';
// Every scene of scenes.json in a real browser, through tools/render-scenes.cjs. Skipped unless
// AGENTNOTCH_UI_BROWSER names a Chromium headless shell (or Chrome); a plain `node --test` run
// never needs one. With it, each scene is rendered at both widths, dark and light, at device
// scale 1, 1.25 and 1.5 (the scales the sealed self-test runs the real app at, DESIGN-WIN §5.6),
// and the run fails on a page error, a CSP violation, a call the contract refuses, a showScene
// that answers false, a failing layoutReport() or a PNG that is missing, empty or the wrong size.
//
//   AGENTNOTCH_UI_BROWSER=<headless_shell> [AGENTNOTCH_UI_SNAPSHOTS=<folder to keep the PNGs in>] \
//     node --test agentnotch-ui-tests/snapshots.test.cjs
//
// The PNGs go to <folder>/<theme>-<scale>/ (a temporary folder, removed afterwards, when the
// variable is not set). They are for looking at, never baselines: see baselines/README.md.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const compare = require('./tools/compare-png.cjs');

const BROWSER = process.env.AGENTNOTCH_UI_BROWSER;
const KEEP = process.env.AGENTNOTCH_UI_SNAPSHOTS;
const SCENES = JSON.parse(fs.readFileSync(path.join(__dirname, 'scenes.json'), 'utf8')).scenes;
const WIDTHS = [400, 520];
const THEMES = ['dark', 'light'];
const SCALES = [1, 1.25, 1.5];

/** The files the tool writes for a scene: `<name>[-<width>][-light].png`, from scenes.json's `file`. */
function expectedFiles(theme) {
  const files = [];
  for (const s of SCENES) {
    for (const width of s.widths ? WIDTHS : [null]) {
      const base = s.file.replace(/\.png$/, '').replace('{width}', String(width));
      const name = theme === 'light' ? `${base}-light` : base;
      files.push({ scene: s, width: width || s.width, png: `${name}.png`, json: `${name}.json` });
    }
  }
  return files;
}

for (const theme of THEMES) {
  for (const scale of SCALES) {
    test(`every scene renders in ${theme} at device scale ${scale}: no page error, no CSP violation, a clean layoutReport`, { skip: BROWSER ? false : 'set AGENTNOTCH_UI_BROWSER to a headless Chromium to run the browser pass', timeout: 30 * 60 * 1000 }, () => {
      const root = KEEP || fs.mkdtempSync(path.join(os.tmpdir(), 'an-snapshots-'));
      const out = path.join(root, `${theme}-${scale}`);
      try {
        fs.rmSync(out, { recursive: true, force: true });
        const run = spawnSync(process.execPath, [
          path.join(__dirname, 'tools', 'render-scenes.cjs'),
          '--browser', BROWSER, '--out', out, '--widths', WIDTHS.join(','), '--theme', theme, '--dpr', String(scale),
        ], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, timeout: 25 * 60 * 1000 });
        const failed = run.stdout.split('\n').filter((l) => /^FAIL|^ {7}\S/.test(l));
        assert.equal(run.status, 0, `${theme} @${scale}: render-scenes exited ${run.status}\n${failed.join('\n')}\n${run.stderr}`);
        for (const f of expectedFiles(theme)) {
          const png = path.join(out, f.png);
          assert.ok(fs.existsSync(png) && fs.statSync(png).size > 200, `${f.png} is missing or empty`);
          const image = compare.decode(fs.readFileSync(png));
          assert.ok(Math.abs(image.width - f.width * scale) <= 1, `${f.png}: ${image.width} px wide at scale ${scale}`);
          const report = JSON.parse(fs.readFileSync(path.join(out, f.json), 'utf8'));
          assert.deepEqual(report.problems, [], f.png);
          assert.notEqual(report.layout && report.layout.ok, false, `${f.png}: ${(report.layout.failures || []).join('; ')}`);
          assert.ok(report.layout && typeof report.layout.ok === 'boolean', `${f.png}: the page answered no layoutReport`);
        }
      } finally {
        if (!KEEP) fs.rmSync(root, { recursive: true, force: true });
      }
    });
  }
}
