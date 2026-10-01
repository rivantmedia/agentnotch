# Baselines

WebView2 captures of every scene in `../scenes.json`, compared by the smoke test (WP11) on CI with
`../tools/compare-png.cjs` (a per-pixel tolerance and a small changed-area budget).

This folder holds no PNGs yet, on purpose: a render made on the Mac (headless Chromium, another
font) would fail that comparison. `../tools/render-scenes.cjs` output is for looking at while writing
pages, never a baseline.

## The file names

Each scene's `file` field in `../scenes.json` is the name the app's snapshot run
(`AGENTNOTCH_SNAPSHOT_CLAUDE=<dir>`) writes and the name expected here:

- `<name>.png` for a scene with a fixed size (the notch scenes, `panel-chrome-*`, `settings*`), for example
  `notch-right-card.png`, `panel-chrome-top.png`, `settings-cloud-signed-in.png`;
- `<name>-400.png` and `<name>-520.png` for a scene drawn at both widths (every `panel-*` state and every `chat-*`
  scene), for example `panel-every-state-520.png`, `chat-no-route-400.png`.

48 scenes make 70 files at 100 % page scale. The sealed run at 125 % and 150 % checks the layout invariants only; it compares no pixels.

## How a baseline is made

1. Push the branch; the Windows workflow's smoke job runs the snapshot run on the Windows runner and uploads the
   PNGs (`AGENTNOTCH_SNAPSHOT_CLAUDE`) as an artifact (`gh run download <id> -R rivantmedia/agentnotch -n <artifact>`).
2. **Look at every PNG** (agents and people can read PNGs) next to the Mac sheet of the same name
   (`Scripts/spm-snapshots.sh <dir>`, `mac` in `scenes.json`) and fix what is wrong; Segoe UI and Cascadia differ from the Mac's San Francisco, so
   judge layout, spacing, colour, truncation and copy.
3. Copy the PNGs of that run into this folder with the names above, in the same commit as the change that made them. Never copy a Mac render,
   and never a render made by `tools/render-scenes.cjs`: only a capture by the app on a Windows runner is a baseline.

## How a baseline is re-made

A deliberate UI change, or a runner image roll that moves anti-aliasing beyond the tolerance, re-makes the affected baselines the same way
(download the artifact of the failing run, look at the diff images CI uploads beside it, look at the new PNGs) in the commit that causes the
difference: never silently, never to make a red run green without looking. `compare-png.cjs <baselines> <captures> --diff <dir>` shows the
difference locally.
