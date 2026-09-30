# Baselines

WebView2 captures of every scene in `../scenes.json`, compared by the smoke test on CI.

This folder holds no PNGs yet, on purpose: a render made on the Mac (headless Chromium, another
font) would fail that comparison. The first captures come from CI's Windows job once the glue's
capture (WP9) and the smoke test (WP11) are merged; they are looked at, then committed here.
`../tools/render-scenes.cjs` output is for looking at while writing pages, never a baseline.
