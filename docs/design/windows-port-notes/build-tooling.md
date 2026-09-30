SPEC: BUILD, RELEASE AND TOOLING FOR A WINDOWS AGENT NOTCH
(Read-only mapping. No repo file was changed. Scratch root, called S below: <scratch>)

======================================================================
0. STATE OF THE REPO (HEAD 1d1fde8, branch main, clean)
======================================================================
- `VERSION` is `1.0.0`.
- `app-config.json` is `{"websiteURL": "https://agentnotch.rivant.in"}`. Recent commit 1d1fde8 made it the single source for the app's website: `spm-build-app.sh` validates it and writes it into Info.plist as `AgentNotchWebsiteURL`, and release.yml's `website` job reads it.
- Remotes: origin rivantmedia/agentnotch; upstream vinzdg/codenotch. The local upstream/main equals the merge-base 642d329, so there is no unmerged upstream churn in windows/.
- Windows workflows today: `windows.yml` and `windows-package.yml` are upstream's and guarded `if: github.repository == 'vinzdg/codenotch'`. Exception: windows-package also runs on `workflow_dispatch` in any repo. CLAUDE.md says never to ship its output, because the upstream port reads Claude's login token.
- fork.yml and release.yml are macOS only. Nothing Windows is built for the fork.
- Rust was not installed on this Mac. I installed rustup (stable 1.98.1) only under S/rust (see §8).

======================================================================
1. THE MAC RELEASE PIPELINE TODAY (what Windows must slot into)
======================================================================
release.yml:
- Triggers (36-49):
  - a push to main touching `VERSION` or `Scripts/sparkle-public-ed-key.txt`;
  - `workflow_dispatch` with inputs `dry_run` and `rotate_update_key`.
- Top-level `permissions: contents: read`.
- Concurrency (61-64): `group: release`, `queue: max`, `cancel-in-progress: false`.
- Job `checks` (67-69) is `uses: ./.github/workflows/fork.yml`.
- Job `release` (71-376):
  - `if: github.repository == 'rivantmedia/agentnotch'`, `runs-on: macos-26`, `XCODE_APP=/Applications/Xcode_26.6.app`.
  - `contents: write` and `environment: release` (main-only deployment branch; it holds every secret).
- "Plan the release" (117-216):
  - Fails on any ref other than refs/heads/main, even in a dry run.
  - Checks `VERSION` against `^[0-9]+\.[0-9]+\.[0-9]+$`; tag is `agentnotch-v$VERSION`.
  - A published release is a green skip. Its own stale draft (`name == "Agent Notch $version"`) is marked `stale_draft=true`. A foreign draft is a problem.
  - The latest release is found with `gh release list --exclude-drafts --exclude-pre-releases`, then `sed agentnotch-vX.Y.Z | sort -V | tail -1`. `VERSION` below it is a problem.
  - Key guard (183-201): `first_key()` is an awk that takes the first line that isn't a comment. It compares `Scripts/sparkle-public-ed-key.txt` with the same file at `ref=agentnotch-v$latest`, read through `gh api -H 'Accept: application/vnd.github.raw+json' repos/$GH_REPO/contents/...`. Unreadable is a problem. A different key is a problem unless `ROTATE_UPDATE_KEY`.
  - Tag at another commit is a problem.
  - In a dry run, problems are `::warning::` instead of errors.
- "Check the update signing key" (218-268): if `SPARKLE_ED_PRIVATE_KEY` or the committed public key is missing, it writes setup steps (environment `gh api` commands) to the step summary and fails.
- Build (270-283): `Scripts/release-build.sh --out build/release`, then `release-info.env` into `$GITHUB_OUTPUT` (VERSION, TAG, DMG, ZIP, ARCHS, SIGNED_WITH, NOTARIZED).
- Notes (287-315): written to `$RUNNER_TEMP/notes.md`; the Gatekeeper/xattr steps appear only when not notarized.
- Dry run (317-326): `upload-artifact@v7` named `AgentNotch-<V>-dry-run` with dmg, zip and appcast.xml.
- Publish (328-359): `gh release create "$TAG" --draft --target "$GITHUB_SHA" --title "Agent Notch $VERSION" --notes-file … --generate-notes [--notes-start-tag prev] <dmg> <zip> appcast.xml`. Each asset must have `state=="uploaded"` and a size equal to `stat -f%z` (BSD stat), then `gh release edit "$TAG" --draft=false --latest`.
- Feed check (363-376): curls `releases/latest/download/appcast.xml` for `<sparkle:version>$VERSION`, six tries 10 s apart; only a warning on failure.
- Job `website` (395-457):
  - `needs: release`, ubuntu, `continue-on-error`, `id-token: write`, `environment: release`.
  - Sparse-checks out `app-config.json` and POSTs `<origin>/api/releases/refresh` with a GitHub OIDC token whose audience is the site origin.
  - The website (`web/src/server/auth/verify-release-run.ts:87-96`) requires:
    - `repository` == the repo;
    - `ref` == refs/heads/main;
    - `workflow_ref` == `job_workflow_ref` == `<repo>/.github/workflows/release.yml@refs/heads/main`;
    - `environment` == `release`.
  - So Windows publishing must stay inside release.yml (not a separate workflow), or the refresh job must stay there.

release-build.sh:
- Removes stale outputs first (82).
- Reads the key once, unsets `SPARKLE_ED_PRIVATE_KEY` before the build (109-118: "nothing started below … inherits it"), and derives the public key and compares it with the committed file (122-147).
- Builds `--release --universal --with-updates`, checks the Info.plist keys, signing and minos.
- Makes the dmg, then the zip (ditto), signs it with `sign_update`, and runs its own `ed25519 verify` against the committed key (368).
- Writes `appcast.xml` with one item whose enclosure URL is the tag-specific `…/releases/download/<tag>/<zip>` (never latest/download, 374-377), and `release-info.env`.
- Windows should copy this philosophy: the key only in the signing step, and our own verification against the committed key.

release-make-keys.sh:
- `--update-key <dir> [--rotate]`, `--signing-cert <dir>`, `--set-secrets`.
- `key_folder` refuses a folder inside the repo (checked on the realpath). Nothing is ever overwritten. It refuses to replace a committed key without `--rotate` (113-121).
- The key comes from CryptoKit via `release-ed25519.swift`.
- `SECRETS+=("NAME file")`. Printed commands: `gh api -X PUT …/environments/release` with custom_branch_policies, then `POST …/deployment-branch-policies name=main`, then `gh secret set NAME --env release -R rivantmedia/agentnotch < file`.
- `ALL_SECRETS` (218-219) is used to warn about repository-level copies.

fork.yml:
- One macos-26 job:
  - checks: check-seams, deployment targets, token-free, package pins, spm-test, `make test-ci`, verify-deps, the CLT build;
  - "Builds from source carry no update feed": plutil checks, and `--with-updates` must refuse a `.dev` id with exit 2.
- Concurrency `${{ github.workflow }}-${{ github.ref }}`, cancel-in-progress.
- Also `workflow_call` (Release runs it).

The release contract in CLAUDE.md "Releases and updates" (351-418) says the app gate, release-build, release.yml and the website classifier "change together". It lists the assets, the fixed feed URL and the rule "never name a fork asset Codenotch.dmg" (README's upstream half links `releases/latest/download/Codenotch.dmg` and `…/Codenotch-Setup.exe`, README.md:686, 713). Adding Windows means extending that section.

======================================================================
2. UPSTREAM WINDOWS PORT: BUILD FACTS AND WHAT MUST NOT SHIP
======================================================================
Workspace `windows/Cargo.toml`:
- members `codenotch`, `codenotch-hook`; resolver 2; release profile `lto=true`, `codegen-units=1`, `strip=true`, `opt-level="z"`.
- Locked versions: tauri 2.11.5, tauri-build 2.6.3, tauri-plugin-updater 2.12.0 (features `native-tls`, `zip`, no default features), tauri-plugin-single-instance 2.4.4, wry 0.55.1, webview2-com 0.38.2, windows 0.58/0.61, ureq 2.12.1 (default features, so rustls and ring come in too), rusqlite 0.32.1 bundled (libsqlite3-sys 0.30.1), tauri-winres 0.3.6, embed-resource 3.0.11, minisign-verify 0.2.5.

`windows/codenotch/tauri.conf.json`:
- `productName "Codenotch"`, `version "1.18.0"`, `identifier "com.immidi.codenotch"`, `frontendDist "ui"`.
- One window `notch` (transparent, alwaysOnTop).
- `bundle.targets ["nsis"]`, `icon ["icons/icon.ico"]`, `createUpdaterArtifacts false`.
- `plugins.updater.endpoints ["https://github.com/vinzdg/codenotch/releases/latest/download/latest.json"]`, `pubkey "REPLACE_WITH_TAURI_PUBLIC_KEY"`, `windows.installMode "passive"`.

`tauri.bundle.conf.json` is merged only when packaging: `bundle.resources {"../target/hook/release/codenotch-hook.exe": "codenotch-hook.exe"}`.

`codenotch/src/updater.rs`:
- `UNSET_PUBKEY` (57) and `configured()` (64-72): the updater is skipped when the pubkey is the placeholder.
- Checks 20 s after launch (149-155); `download_and_install` (127).

windows.yml (the upstream CI shape to reuse):
- `node scripts/check-ui-scripts.mjs` (a vm.Script parse of the inline `<script>`s in ui/*.html);
- `node --test test-light-surface.cjs`, `node scripts/test-claude-auth-ui.cjs`, `node --test scripts/test-ko-i18n.cjs`;
- `dtolnay/rust-toolchain@stable` + clippy, `Swatinem/rust-cache@v2 (workspaces: windows)`;
- `cargo build --locked`, `cargo test --locked`, `node --test test-codex-headline.cjs`, `cargo clippy --all-targets --locked` (reported, not enforced).

windows-package.yml (the template for the fork's jobs):
- Hook build (54-57): `RUSTFLAGS=-C target-feature=+crt-static cargo build --release --locked -p codenotch-hook --target-dir target/hook`.
  - Its own target dir, because the bundler copies resources next to the app in target/release, so the hook would be copied onto itself.
  - crt-static because Claude Code may run the hook where vcruntime140.dll is missing. The app needs no flag: tauri-build links the runtime statically.
- Signing gate (64-73): the secret must not be empty. Build (77-91): `npx --yes @tauri-apps/cli@2.11.4 build --config tauri.bundle.conf.json [--config '{"bundle":{"createUpdaterArtifacts":true}}'] -- --locked`. The comment warns not to splat `$args`.
- Name (96-100): the single `target/release/bundle/nsis/*-setup.exe` is copied to `Codenotch-Setup.exe`.
- Smoke test (105-122):
  - `Start-Process .\Codenotch-Setup.exe -ArgumentList '/S' -Wait`;
  - find `codenotch-hook.exe` under `$env:LOCALAPPDATA` (depth 3) and check that `codenotch.exe` and `uninstall.exe` are beside it;
  - run `codenotch.exe doctor` with stdout redirected, plus `%APPDATA%\codenotch\doctor.log`, and match `Codenotch doctor`;
  - `uninstall.exe /S -Wait`.
- Feed (152-172): expects `target/release/bundle/nsis/*-setup.nsis.zip` and writes latest.json with only `windows-x86_64`, URL `…/releases/download/v$version/Codenotch-Setup.nsis.zip`.
  - UPSTREAM BUG: with Tauri v2, `createUpdaterArtifacts: true` produces no `.nsis.zip`, so this step would throw "expected one updater archive, found 0". Section 3 shows why.

Token and upstream-only paths in the port (all must stay out of the fork's Windows build):
- `codenotch/src/usage.rs`:
  - `ENDPOINT "https://api.anthropic.com/api/oauth/usage"` (30);
  - `CRED_NAMES [".credentials.json","credentials.json"]` (68);
  - `read_credentials()` reads `claudeAiOauth.accessToken`/`expiresAt`/`subscriptionType` (196-217);
  - `probe_credentials()` (221, which doctor calls);
  - `find_cli()` (261);
  - renewal by running `claude -p` with `CLAUDE_CONFIG_DIR` (312-343; the "ClaudeTokenRefresher" port);
  - the request sends headers `Authorization: Bearer`, `anthropic-beta: oauth-2025-04-20` (473-474).
- `codenotch/src/claude_auth.rs` runs `claude auth login --claudeai`.
- `main.rs`:
  - `claude_sign_in` (667) and `get_claude_auth` (670), registered in `generate_handler!` (~1812);
  - `usage::start(handle.clone())` (1891);
  - `doctor` runs before Tauri (1776-1782) and calls `crate::usage::probe_credentials()` (`doctor.rs:78`), so upstream's doctor reads `.credentials.json`.
- `ui/notch.html:1159` does `invoke('claude_sign_in')`; line 1596 does `invoke('get_claude_auth')`.
- `usage.rs:782` is an `#[ignore]` test that runs the installed `claude`. Never pass `--ignored`.
- `glm.rs:142` reads `ANTHROPIC_AUTH_TOKEN` from Claude Code settings when it points at z.ai. That is a GLM key, not a Claude login, but note it.

Names that collide with an installed upstream Codenotch-for-Windows (the fork must differ in every one):
- `productName`/`identifier` → install dir `%LOCALAPPDATA%\Codenotch`, uninstall key, single-instance mutex, WebView2 data `%LOCALAPPDATA%\com.immidi.codenotch`.
- Port `48666` (`config.rs:229-231`; `codenotch-hook/src/main.rs:9`).
- Hook reads `%APPDATA%\codenotch\config.json` (hook main.rs:39); config is `dirs::config_dir()/codenotch/config.json` (`config.rs:266-270`).
- The hook spawns a sibling `codenotch.exe` (hook main.rs:80).
- `hooks_install.rs:29` `is_ours` = the command contains "codenotch-hook" (or eatbean/pacman). So a fork hook named `*codenotch-hook*` would be deleted by upstream's installer. Name it `agentnotch-hook.exe`.
- Autostart Run value `"Codenotch"` (`autostart.rs:7-8`).
- Hook entries go to `~/.claude/settings.json` only (`hooks_install.rs:19`), with backup `json.codenotch-bak-<ts>`.

======================================================================
3. TAURI v2 FACTS (read from source: tauri-bundler 2.9.4, tauri-cli 2.11.4 and 2.12.0, tauri-plugin-updater 2.12.0; unpacked in S/winmap/tb, tc, tc212 and S/rust/cargo/registry)
======================================================================
Installer name and location:
- `nsis/mod.rs:652-656`: `{productName}_{version}_{arch}-setup.exe` in `target/release/bundle/nsis/`.
- productName "Agent Notch" gives `Agent Notch_1.0.0_x64-setup.exe`. GitHub turns spaces into dots, so rename to `AgentNotch-<V>-Setup.exe` before upload.
- `installer.nsi`:
  - currentUser mode: `INSTDIR = $LOCALAPPDATA\${PRODUCTNAME}` (514), so `%LOCALAPPDATA%\Agent Notch\`;
  - `UNINSTKEY = Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}` under HKCU (66), with DisplayName/DisplayVersion/UninstallString (701-706);
  - `MANUKEY = Software\${MANUFACTURER}` (67). Manufacturer = `bundle.publisher`, else the second label of the identifier (mod.rs:268-270): "rivantmedia".
- Deep-link schemes (needed for `agentnotch://auth-callback`): `installer.nsi:671-675` writes `HKCU\Software\Classes\<scheme>` with `shell\open\command "…exe" "%1"`, via tauri-plugin-deep-link config. The uninstaller removes it (806-809).
- The uninstaller's "delete app data" checkbox removes only `$APPDATA\${BUNDLEID}` and `$LOCALAPPDATA\${BUNDLEID}` (870-883). A fork folder like `%APPDATA%\Agent Notch` is not removed unless an NSIS hook does it.
- NSIS hooks: `bundle.windows.nsis.installerHooks` provides `NSIS_HOOK_PREINSTALL`/`POSTINSTALL`/`PREUNINSTALL`/`POSTUNINSTALL` (641, 733, 778, 886).
- The bundler downloads NSIS 3.11 and nsis_tauri_utils.dll from github.com/tauri-apps/binary-releases, hash-checked (`nsis/mod.rs:38-43`, 87-119). The runner needs network.

Updater artifacts:
- `bundle.rs:206-239`: with `createUpdaterArtifacts: true` (v2), zipping happens only for macOS `.app`. NSIS and MSI are "self contained": the updater artifact is the installer itself. `.nsis.zip` exists only with `"v1Compatible"`, which is deprecated and to be removed in v3.
- tauri-cli `bundle.rs` `sign_updaters` (226-316) signs every NSIS bundle path and writes `<file>.sig` (`updater_signature.rs sign_file`: extension + ".sig").
  - It reads `TAURI_SIGNING_PRIVATE_KEY` (a string or a path) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (defaults to "" when `CI` is set).
  - If `plugins.updater.pubkey` does not match the private key, it ONLY WARNS (~305). Our pipeline must verify against the committed key itself.
- CLI 2.12.0 (npm latest, published 2026-09-26) adds `version:<V>` to the trusted comment (`bundle.rs:312`) and `signer sign --app-version <V>`.
- Plugin 2.12.0 (already locked) supports `plugins.updater.requireSignedVersion: true` (`config.rs:120-135`, `updater.rs:1530-1597`). It rejects a manifest that pairs a new version with an older signed artifact (a downgrade or replay). Recommend CLI 2.12.0 plus `requireSignedVersion: true` from the first Windows release. Turning it on later would reject no earlier signatures, because 2.12.0 signatures carry the version.

Keys:
- `signer generate [-p <pw>] [-w <file>] [--ci] [-f]`.
  - Without `-p` and without `--ci`, minisign prompts for the password on the TTY; `-p` has no environment form.
  - `-w` writes `<file>` (0600) and `<file>.pub`. 2.12.0 refuses if either exists (`updater_signature.rs:95-101`).
  - Private key = base64 of the rsign-encrypted secret key box. Public key = base64 of "untrusted comment: minisign public key: <ID>\nRW…" (single line; goes verbatim into `plugins.updater.pubkey`).
- `signer sign` reads `-k`/`TAURI_SIGNING_PRIVATE_KEY` (the literal string), or `-f`/`TAURI_SIGNING_PRIVATE_KEY_PATH`, and `-p`/`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- Verified here with a throwaway key: `.sig` = base64 of
  "untrusted comment: signature from tauri secret key\n<b64 sig blob>\ntrusted comment: timestamp:<unix>\tfile:<name>\tversion:1.0.0\n<b64 global sig>".
  - Renaming the file after signing does not invalidate it (the file name only sits in the comment).
  - Key id: bytes [2..10] of the decoded pubkey blob equal bytes [2..10] of the decoded signature blob.

Updater manifest and runtime:
- Target search (`updater.rs:608-638`): `windows-x86_64-nsis` first, then `windows-x86_64`.
- Version parse trims a leading "v" (1520-1526). `pub_date` must be RFC 3339 (1479-1485).
- NSIS install args: installMode passive → `/P`, quiet → `/S`, plus `/UPDATE`, plus `/ARGS <current exe args>` (`updater.rs:891-914`; `config.rs:41-49`). A zip is unpacked only if the download is a zip (955). The `zip` feature is not needed for v2 artifacts (harmless).
- The fork's latest.json (served at `https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json`):
    {"version":"1.0.0","notes":"https://github.com/rivantmedia/agentnotch/releases/tag/agentnotch-v1.0.0",
     "pub_date":"2026-09-28T12:00:00Z",
     "platforms":{"windows-x86_64-nsis":{"signature":"<content of .exe.sig>","url":"https://github.com/rivantmedia/agentnotch/releases/download/agentnotch-v1.0.0/AgentNotch-1.0.0-Setup.exe"},
                  "windows-x86_64":{ same }}}
  The URL is tag-specific (as the appcast enclosure is), never latest/download.
- `dirs` on Windows uses `SHGetKnownFolderPath` (FOLDERID_Profile, RoamingAppData, LocalAppData; `dirs-sys-0.4.1/src/lib.rs:151-183`) and IGNORES USERPROFILE/APPDATA environment variables. Tests on a Windows runner cannot sandbox through the environment, so the engine must take injected roots (the analog of AGENTNOTCH_SUPPORT_DIR / AGENTNOTCH_EXTRA_CONFIG_DIRS). `std::env::home_dir()` does honour USERPROFILE (Rust ≥ 1.85).
- tauri-build writes `<crate>/gen/schemas/*.json` into the source tree on every check or build (confirmed in the scratch copy). It is gitignored (`windows/.gitignore: codenotch/gen/`), so local checks run on a copy.

======================================================================
4. WEBSITE CLASSIFIER (web/src/lib/releases.ts)
======================================================================
- `assetKind` (240-271):
  - Any name containing "codenotch" (case-insensitive) is ignored (243).
  - Excluded extensions (218-219): `.xml .json .yml/.yaml .txt .md .sig .asc .minisign .pem .p7s .sha* .md5 .blockmap .delta`.
  - Excluded words (220-231): checksum(s)/sha256sums/sums/dsym(s)/symbols/debug/src/source.
- Windows mapping: `.exe` = rank 0, `.msi` = 1, `.msix`/`.appx` = 2, `.zip` with the word win/windows/win32/win64 = 3.
- Mac zip fallback (263-269): ANY other `.zip` is Mac rank 2. So `AgentNotch-<V>-Setup.nsis.zip` (words agentnotch,1,0,0,setup,nsis,zip) would be classified MAC. On a tie, the first listed wins, so with no dmg it could be offered to Mac users.
- The fork should upload exactly:
  - `AgentNotch-<V>.dmg` (mac 0), `AgentNotch-<V>.zip` (mac 2), `appcast.xml` (null);
  - `AgentNotch-<V>-Setup.exe` (windows 0, pinned by `web/tests/unit/releases.test.ts:74,181-190`);
  - `latest.json` (null); optionally `AgentNotch-<V>-Setup.exe.sig` (null).
  - Never a Windows `.zip` without "windows" in its name. Never anything named `Codenotch*`, which the classifier drops and which also collides with README's upstream links.
- Optional hardening: return null for `/\.(nsis|msi)\.zip$/` and add test cases for `AgentNotch-1.0.0-Setup.exe.sig` and `.nsis.zip`.
- `/download` page copy is Mac-only: metadata "for the Mac", `PlatformCard` says "Agent Notch runs on macOS only for now." (page.tsx:240-245), and the "Installing on a Mac" section. It needs a Windows requirements line and a SmartScreen section.

======================================================================
5. (1) ADDING WINDOWS TO release.yml
======================================================================
Job graph (all inside release.yml, so the OIDC `job_workflow_ref` stays valid):
- `checks`: fork.yml, extended with a Windows job (see §6b).
- `plan` (ubuntu-latest, contents: read, no environment): today's plan script moved out of the Mac job, plus the Windows parts below. Outputs: `version`, `tag`, `skip`, `stale_draft`, `previous_tag`, `windows` (true when `Scripts/tauri-update-public-key.txt` holds a key).
- `mac` (needs checks and plan; `if: needs.plan.outputs.skip != 'true'`):
  - today's steps (Select Xcode, Check the update signing key, `release-build.sh`, notes);
  - uploads artifact `mac-release` (dmg, zip, appcast.xml, release-info.env, retention-days 3);
  - `environment: release`.
- `windows` (needs checks and plan; `if: skip != 'true' && needs.plan.outputs.windows == 'true'`; `runs-on: windows-2025`, pinned rather than windows-latest, like Xcode 26.6; `environment: release`; `contents: read`; `timeout-minutes: 60`; `defaults.run.shell: pwsh`).
- `publish` (ubuntu-latest, contents: write, needs plan, mac and windows):
  - `if: ${{ !cancelled() && needs.plan.outputs.skip != 'true' && needs.mac.result == 'success' && (needs.windows.result == 'success' || (needs.windows.result == 'skipped' && needs.plan.outputs.windows != 'true')) && !(github.event_name == 'workflow_dispatch' && inputs.dry_run) }}`.
  - `!cancelled()` is required; otherwise a skipped Windows job skips publish.
- `website`: `needs: publish`, `if: needs.publish.result == 'success' && !dry_run` (was `needs.release`).
- Dry run: each build job uploads its own artifact (`AgentNotch-<V>-dry-run`, `AgentNotch-<V>-windows-dry-run`), and publish is skipped.
- Each job that names `environment: release` needs its own approval when the environment has a required reviewer (mac, windows, publish, website, so up to four). Keep publish without an environment if that matters. It needs no secrets, and the main-only check in `plan` still applies.

Windows parts of `plan` (bash, mirroring 183-201):
- `wkey=$(first_key < Scripts/tauri-update-public-key.txt)`. Validate: base64 that decodes to "untrusted comment: minisign public key: …\nRW…" and a 42-byte blob starting "Ed".
- Released key: `gh api -H 'Accept: application/vnd.github.raw+json' "repos/$GH_REPO/contents/Scripts/tauri-update-public-key.txt?ref=agentnotch-v$latest"`.
  - An HTTP 404 means "no Windows key at the latest release". That is OK only if that release has no `latest.json` asset (`gh release view agentnotch-v$latest --json assets --jq '.assets[].name' | grep -qx latest.json`); otherwise it is a problem.
  - Any other failure is a problem ("could not read").
  - Distinguish 404 from other failures by capturing stderr (`gh api` prints "HTTP 404").
- `wkey != released_wkey` is a problem unless `rotate_update_key` (reuse it, or add `rotate_windows_update_key` with its own label).
- Stronger optional check: fetch the latest release's `latest.json`, decode `platforms["windows-x86_64"].signature`, and compare key id bytes [2..10] with the committed key's (see §3).
- Continuity: if the previous release carried `latest.json` and this run has `windows != true` (key file removed), it is a problem. A Mac-only release after Windows has shipped would make `releases/latest/download/latest.json` return 404, and every Windows copy would silently stop seeing updates.
- Add `Scripts/tauri-update-public-key.txt` to the `on.push.paths` list.
  - The first Windows release goes out on the commit that adds the key, if VERSION is not yet released.
  - A key-only push on a released version does nothing, as it does for the Mac today. Document "commit the Windows key with a VERSION bump".

Windows job steps:
1. `actions/checkout@v7` with `persist-credentials: false`.
2. `dtolnay/rust-toolchain@<commit sha>` with `toolchain: 1.98.1` (explicit, since a SHA ref carries no toolchain name). No rust-cache in the release job (keeps it reproducible and avoids cache poisoning); a cold release build is slow (lto + opt-level z + sqlite), which 60 minutes covers.
3. "Check the Windows update key": if `secrets.TAURI_SIGNING_PRIVATE_KEY` is empty, write setup steps to `$GITHUB_STEP_SUMMARY` (the same shape as 231-261: `release-make-keys.sh --windows-update-key <dir>`, then `gh secret set TAURI_SIGNING_PRIVATE_KEY --env release …`) and fail.
4. "Release config": write `$RUNNER_TEMP\tauri.release.conf.json`:
     {"version":"<VERSION>",
      "plugins":{"updater":{"pubkey":"<committed key>",
        "endpoints":["https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json"],
        "requireSignedVersion":true,"windows":{"installMode":"passive"}}}}
   Keep `bundle.createUpdaterArtifacts` false in every config; signing is a separate step (below).
5. Build the hooks, with no secrets in the environment: `$env:RUSTFLAGS='-C target-feature=+crt-static'; cargo build --release --locked -p agentnotch-hook [-p agentnotch-statusline] --target-dir target/hook`.
6. Build the installer, with no secrets in the environment: `npx --yes @tauri-apps/cli@2.12.0 build --config tauri.bundle.conf.json --config $env:RUNNER_TEMP\tauri.release.conf.json -- --locked`.
   - Better supply-chain pinning: a committed `windows/tools/package.json` + `package-lock.json` (integrity sha512) and `npm ci`, or `cargo install tauri-cli --version 2.12.0 --locked` (slower).
   - Cargo runs every dependency's build script, which is why the key must not be in this step's environment. This is the release-build.sh "unset before build" rule.
7. Name: exactly one `target/release/bundle/nsis/*-setup.exe`, copied to `out\AgentNotch-$V-Setup.exe`. Assert that `(Get-Item …).VersionInfo.ProductVersion` starts with `$V` (tauri-winres stamps it from the config version).
8. Sign (the only step with the key):
   - env `TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}` (an empty value works as "no password"; tested);
   - `npx --yes @tauri-apps/cli@2.12.0 signer sign --app-version $V out\AgentNotch-$V-Setup.exe` produces `.exe.sig`.
9. Verify against the committed key, the way `ed25519 verify` does for the Mac:
   - a fork-owned bin using `minisign-verify = "0.2.5"` (already in Cargo.lock; the plugin uses the same crate): `verify-update <pubkey file> <file> <sig> <V>`;
   - it checks the signature and that the trusted comment's `version:` equals V;
   - a 25-line prototype passes and rejects a version mismatch: S/winmap/verify-sig/src/main.rs.
10. latest.json:
   - built with ConvertTo-Json as in §3 (both platform keys; `pub_date` `(Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')`; the URL pinned to the tag);
   - read it back and check that the version, URL and signature equal the `.sig` content, then re-verify through the tool with the signature taken from the JSON.
11. Smoke test on the SIGNED file (see §6 for the checks), including doctor printing `updates: on`, the feed URL, and the key id equal to the committed key's.
12. `upload-artifact@v7` named `windows-release` (exe, exe.sig, latest.json, retention-days 3).

Publish job:
- `actions/download-artifact` (the major matching upload-artifact@v7) for both artifacts.
- `gh release delete "$TAG" --yes` if `stale_draft`.
- `gh release create "$TAG" --draft --target "$GITHUB_SHA" --title "Agent Notch $VERSION" "${notes[@]}" "$DMG" "$ZIP" appcast.xml "AgentNotch-$VERSION-Setup.exe" latest.json` (optionally plus `AgentNotch-$VERSION-Setup.exe.sig`).
- Size check per file, using `wc -c < "$file" | tr -d ' '` instead of `stat -f%z`, which is BSD-only (release.yml:353). Assert that no asset name matches `-i codenotch`.
- Then `gh release edit "$TAG" --draft=false --latest`. appcast.xml and latest.json then switch together at publish, and neither `latest/download` URL ever points at a draft.
- Feed checks (warning only): appcast as today, plus `curl -fsSL …/releases/latest/download/latest.json | jq -r .version` == VERSION, with the same retry loop.
- Notes gain a "## Windows" section:
  - download `AgentNotch-<V>-Setup.exe`;
  - it installs per user into `%LOCALAPPDATA%\Agent Notch` with no admin, and fetches WebView2 if needed;
  - it is unsigned, so SmartScreen says "Windows protected your PC": choose More info, then Run anyway;
  - Windows 10/11 x64;
  - updates: checked from the app, verified with the fork's minisign key.

Secrets (all in the `release` environment):
- `TAURI_SIGNING_PRIVATE_KEY`: required once `Scripts/tauri-update-public-key.txt` exists.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: optional; may be empty.
- Optional later:
  - Authenticode: `WINDOWS_SIGNING_PFX_BASE64`/`_PASSWORD`, used through `bundle.windows.certificateThumbprint` after importing into CurrentUser\My, or `bundle.windows.signCommand` (Azure Trusted Signing).
  - A self-signed cert buys nothing against SmartScreen, unlike the Mac's TCC benefit. The updater does not need Authenticode; minisign covers it.
- Add all of these to `ALL_SECRETS` in release-make-keys.sh.

release-make-keys.sh `--windows-update-key <dir> [--rotate]` (no Keychain):
- The same refusals first (the loop at 111-131): `key_folder` checks the folder is outside the repo; `<dir>/tauri-update.key` and `.pub` must not exist; a committed `Scripts/tauri-update-public-key.txt` must not be replaced without `--rotate` (or `--rotate-windows`).
- Then `npx --yes @tauri-apps/cli@2.12.0 signer generate -w "$dir/tauri-update.key"`. It runs interactively: minisign prompts twice for the password on the TTY, so the password is never on a command line or printed. `--ci` gives an empty password; the 0600 key sits in the 0700 dir. Tested here in scratch: the key is written 0600, and the `.pub` is written.
- Write the `.pub` content as the first non-comment line of `Scripts/tauri-update-public-key.txt`, under a header `# Agent Notch's Windows update key (Tauri updater pubkey): base64 minisign public key`, using the same python replace-and-keep-header block as 137-159 (honour an `AGENTNOTCH_WINDOWS_UPDATE_PUBLIC_KEY_FILE` override for tests).
- `SECRETS+=("TAURI_SIGNING_PRIVATE_KEY $dir/tauri-update.key")`.
  - The password secret: `gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --env release -R rivantmedia/agentnotch`, where gh prompts for the value, since the script never saw the password.
  - The secret must be the literal key string; `signer sign` wants the literal and base64-decodes it. Tauri writes it without a trailing newline, so `< file` is fine.
- Warn exactly as for the Sparkle key: losing it strands every Windows install (each checks against the pubkey compiled into it).
- Optional `Scripts/release-build-windows.ps1` (the analog of release-build.sh, "everything but publish"): runnable on a Windows machine, and it is what the workflow calls, so the logic lives in one script, not in YAML.

======================================================================
6. (2) STANDALONE WINDOWS BUILD WORKFLOW (+ fork.yml Windows checks)
======================================================================
6a. New file `.github/workflows/agentnotch-windows.yml`. Do not edit upstream's `windows.yml`/`windows-package.yml`: they are upstream files and would conflict on merges.
    name: Windows Build
    on:
      workflow_dispatch:
      push: { branches-ignore: [main], paths: [windows/**, <fork crates>/**, VERSION, app-config.json, .github/workflows/agentnotch-windows.yml] }
      pull_request: { paths: [same] }
    permissions: { contents: read }
    concurrency: { group: ${{ github.workflow }}-${{ github.ref }}, cancel-in-progress: true }
    jobs:
      installer:
        if: github.repository != 'vinzdg/codenotch'
        runs-on: windows-2025
        timeout-minutes: 60
        defaults: { run: { shell: pwsh, working-directory: windows } }
        steps: checkout (persist-credentials false); rust toolchain (pinned); Swatinem/rust-cache@v2 (fine here: no secrets);
          node UI checks; build hooks (+crt-static, target/hook); tauri build with the SOURCE config only (no pubkey, no endpoints:
          a copy built from source never updates itself); rename to AgentNotch-<V>-Setup.exe;
          smoke test (below); upload-artifact@v7 name AgentNotch-Setup-<V>-${{ github.sha }} (if-no-files-found: error, retention-days 14);
          step summary: "unsigned, no update feed, SmartScreen: More info, then Run anyway; never attach to a release".
    Smoke test (PowerShell):
    - `Start-Process .\AgentNotch-$V-Setup.exe -ArgumentList '/S' -Wait`;
    - `$dir = "$env:LOCALAPPDATA\Agent Notch"`; assert `agentnotch.exe`, `agentnotch-hook.exe` (+ statusline exe) and `uninstall.exe` exist;
    - `Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Agent Notch'`: DisplayName 'Agent Notch', DisplayVersion == VERSION;
    - `Start-Process "$dir\agentnotch.exe" -ArgumentList 'doctor' -RedirectStandardOutput doctor.txt -Wait`. The exe is GUI-subsystem and attaches to its parent's console, so also read the doctor.log the app writes. Match `Agent Notch doctor v$V`, plus `updates: off (built from source)` here or `updates: on …` in the release job. Assert the output never contains `.credentials.json` or `credential[`.
    - Hook fails open: copy the hook exe to `$env:RUNNER_TEMP` (no sibling app, so it cannot launch one) and run it with garbage stdin. It must exit 0 in under 3 s.
    - `Start-Process "$dir\uninstall.exe" -ArgumentList '/S' -Wait`, then poll up to 60 s until `$dir\agentnotch.exe` and the uninstall key are gone. The NSIS uninstaller re-launches a temp copy, so `-Wait` may return early; upstream never checks this.
    - Never run `claude`. The doctor must not run it either.
6b. fork.yml gains a job `windows` (windows-2025, `if: github.repository != 'vinzdg/codenotch'`), which also runs inside Release through `workflow_call`:
    - node UI checks; `cargo build --locked`, `cargo test --locked`, `cargo clippy --all-targets --locked -- -D warnings` for fork crates;
    - "builds from source carry no update feed": the source tauri.conf.json has an empty `pubkey`, no `endpoints`, and `createUpdaterArtifacts` false, plus a unit test of a pure `updates_enabled(config, identifier, sealed)` gate (the analog of `Fork.updatesEnabled(info:bundleID:sealed:underTest:)`);
    - "versions agree": VERSION == the fork tauri config `version` == the fork crates' `version`, or a build.rs that exports `AGENTNOTCH_VERSION` from `../../VERSION` with rerun-if-changed. Upstream's doctor prints `env!("CARGO_PKG_VERSION")`, which is 1.18.0.
    The bash checks (check-seams, verify-token-free) stay in the macOS job.

======================================================================
7. (3) EXTENDING check-seams.sh / fork-seams.txt / verify-token-free.sh
======================================================================
check-seams.sh:
- The diff scope (92-93) currently covers only `Sources Tests`. Add `windows` and upstream's workflow files `.github/workflows/{ci,package,windows,windows-package}.yml`. Those get no ALLOW entry, so any fork edit to them fails.
- ALLOW lines for each upstream windows file the fork edits (`windows/Cargo.toml` and `windows/Cargo.lock` if members are added, `windows/codenotch/**` files at seams). Use `ALLOW windows/<fork-owned dir>/**` if fork crates live under windows/. Fork crates outside windows/ are not scanned.
- FORBID comment detection (68) is `^[0-9]+:[[:space:]]*(#|//|@#)`. Make it per extension:
  - `.rs`: `//` and `/*`/`*` (NOT `#`: `#[cfg…]` attribute lines would otherwise be wrongly exempt);
  - `.html`/`.js`/`.mjs`/`.cjs`: `//`, `<!--`, `/*`, `*`;
  - `.json`: none;
  - `.toml`/`.yml`/`.sh`/Makefile: `#`;
  - `.nsh`/`.nsi`: `;` and `#`.
- SEAM lines are `grep -cF` and work unchanged for Rust, HTML and JSON (e.g. `"identifier": "com.rivantmedia.agentnotch"`).
- New sections:
  (a) "The engine has no UI toolkit", the analog of no SwiftUI (102-108): fail on `grep -nE '^[[:space:]]*(tauri|tauri-build|tauri-plugin-[a-z-]+|wry|tao|webview2-com)[[:space:]]*='` in the engine crate's Cargo.toml, and on `use tauri` in its src.
  (b) Upstream's name: the analog of 115-137 for fork Rust/HTML. Grep `'"[^"]*Codenotch[^"]*"'` in fork-owned `*.rs`/`*.html` and in fork-added lines of windows/, with a NAME_OK list (settings.html alone has 108 occurrences, i18n.rs 12, tray.rs 3), unless the fork routes upstream copy through a rebrand pass like R1.
  (c) If Windows hooks share wire fixtures with the Python hook scripts, a fixture-agreement check (the analog of `embed-scripts.sh --check`).
- Example fork-seams.txt lines. The paths depend on whether the fork edits `windows/codenotch` or owns a new app crate:
    SEAM   WISO  <app>/tauri.conf.json   "identifier": "com.rivantmedia.agentnotch"
    SEAM   WISO  <app>/tauri.conf.json   "productName": "Agent Notch"
    FORBID WISO  <app>/tauri.conf.json   com.immidi.codenotch
    FORBID WISO  <app>/tauri.conf.json   vinzdg/codenotch
    FORBID WISO  <app>/tauri.conf.json   "createUpdaterArtifacts": true
    FORBID WISO  <app>/tauri.conf.json   dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWdu      (a pubkey in the source config: source builds carry no key)
    FORBID WISO  <app>/src/<hooks installer>.rs   codenotch-hook                 (upstream's is_ours substring)
    FORBID WISO  .github/workflows/release.yml    Codenotch-Setup
    FORBID WISO  .github/workflows/release.yml    -p codenotch                 (never package upstream's app)
    FORBID WISO  <app>/**                         taskkill /IM codenotch.exe   (the analog of the `pkill -x Codenotch` rule)
  If upstream's app crate is reused in place (the U1 analog):
    SEAM   WU1   windows/codenotch/src/main.rs   // Fork: WU1 upstream's Claude token reader never starts
    FORBID WU1   windows/codenotch/src/main.rs   usage::start(
    FORBID WU1   windows/codenotch/src/main.rs   claude_auth::start_login
    FORBID WU1   windows/codenotch/src/main.rs   claude_sign_in,
    FORBID WU1   windows/codenotch/src/doctor.rs probe_credentials(
    FORBID WU1   windows/codenotch/ui/notch.html invoke('claude_sign_in')
  Note: FORBID supports exact files only; globs exist only for ALLOW (`is_allowed`, 80-90).
verify-token-free.sh:
- Add to PATTERNS (36-51) the Windows and Rust forms:
  `claudeAiOauth`, `api/oauth/usage`, `oauth-2025-04-20`, `"accessToken"`, `"refreshToken"` (camelCase is Claude's credential file; Supabase uses snake_case `access_token`), `CredReadW`, `CredEnumerateW`, `read_credentials(`, `probe_credentials(`, `run_renewal(`, `claude_auth::`, `auth login`, `setup-token`, `sessions\\*.key`. `.credentials.json` is already there and catches `.credentials.json` in Rust too.
- Fork-owned dirs (the loop at 64-69): add the fork's Rust crates and UI, with `--exclude-dir=target --exclude-dir=gen --exclude-dir=node_modules`.
- The added-lines scan (99-111): `git diff "$BASE" -- Sources Tests windows` plus untracked files under windows.
- The dormant-site rule (the analog of 80-86): `read_credentials(`, `ENDPOINT`, `find_cli(` and `api/oauth/usage` may appear only in `windows/codenotch/src/usage.rs`, `claude_auth.rs` and `doctor.rs` (upstream). The fork app must not name `mod usage`/`mod claude_auth` or `#[path = …usage.rs]` includes, and `usage::start(` must be absent from the fork's main.
- ALLOWED_LINES for fork tests that assert those strings are absent.
- The script's final "OK" message should mention Windows.

======================================================================
8. (4) LOCAL TOOLING ON THIS MAC (done, all under S)
======================================================================
- Install: rustup-init with `RUSTUP_HOME=S/rust/rustup CARGO_HOME=S/rust/cargo --no-modify-path --profile minimal`. Stable is rustc/cargo 1.98.1. Nothing was written to ~/.cargo or ~/.rustup (checked).
- Env file `S/rust/env.sh` exports RUSTUP_HOME, CARGO_HOME, PATH and `CARGO_TARGET_DIR=S/rust/target`.
- Checks ran on a copy: `S/winmap/windows-copy`, made by rsync, because tauri-build writes `codenotch/gen/`. The copy's Cargo.toml currently has the cross-check edits described below.
- Tests used `HOME=S/winmap/fakehome`, which stayed empty.

Host check (aarch64-apple-darwin), `cargo check --locked --workspace` on unmodified windows/ (79 s cold):
- The hook crate is OK.
- The codenotch crate fails with exactly 3 errors:
  - `generate_context!()` panics "failed to open icon …/codenotch/icons/icon.png" (the macOS host wants a PNG; only icon.ico exists);
  - `no method named transparent` at `dropzones.rs:46` and `settings_window.rs:49` (on macOS this needs the tauri feature `macos-private-api` plus `app.macOSPrivateApi: true`).
- Two warnings: unused `Stdio` (claude_auth.rs:4) and `Manager` (topmost.rs:12).
- With a scratch-only patch (icon.png copied from tray-color.png, the feature, and the config flag): check passes (7 warnings).
- `cargo test --workspace`: 129 passed, 0 failed, 3 ignored, in 35 s. 136 tests are declared; 4 are `#[cfg(windows)]`-gated (e.g. agy_cli.rs:640, 666). The hook crate has 0 tests.
- So upstream's pure logic is testable on macOS, but only after edits to upstream files. For fork crates: no Tauri in the engine, so check and test on macOS need nothing extra.

Cross check `--target x86_64-pc-windows-msvc` (`rustup target add` done):
- It fails in C build scripts: `ring 0.17.14` (ureq's default `tls` feature, i.e. rustls) with `'assert.h' file not found`: there are no MSVC CRT/SDK headers. `libsqlite3-sys` (rusqlite `bundled`) would fail the same way.
- Next, `tauri-winres` fails with `NotAttempted("llvm-rc")`: embed-resource wants `llvm-rc` for msvc targets (`embed-resource-3.0.11/src/non_windows.rs:46-55`).
- With scratch-only changes:
  - `ureq = { default-features = false, features = ["json","native-tls","gzip"] }`;
  - rusqlite without `bundled`;
  - a stub `S/winmap/stubbin/llvm-rc` that answers `/?` with "OVERVIEW: LLVM Resource Converter … no-preprocess" and touches the `/fo` output.
  Then `cargo check --workspace --target x86_64-pc-windows-msvc` PASSES with 0 warnings. All cfg(windows) Rust, the windows crate APIs, tauri/wry/webview2-com type-check from macOS.
- Those changes are check-only. Without ureq's `tls`, `ureq::get(https)` fails at runtime unless every agent sets `tls_connector` (usage.rs:472, codex.rs:194 and glm.rs:257 use the default agent).
- Apple clang can emit x86_64 COFF (`clang --target=x86_64-pc-windows-msvc -c` gives "Intel amd64 COFF object"). Symlinked as `clang-cl`, it runs in cl driver mode.
- So a full-dependency cross check needs only headers:
  - `xwin --accept-license splat`, which downloads Microsoft's CRT and SDK (about 1 GB). NOT done: accepting Microsoft's licence is the maintainer's decision.
  - Then `CFLAGS_x86_64_pc_windows_msvc=-imsvc …`, or cargo-xwin.
- Linking (`cargo build`) also needs `lld-link` and `llvm-rc`, which are not in the Command Line Tools (would need `brew install llvm` or an LLVM tarball in scratch).
- Tauri's experimental NSIS cross-build (`brew install nsis llvm`, `cargo install cargo-xwin`, `tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc`) was not tried. The result could not be run here anyway; installers must be built and smoke-tested on the Windows runner.
- Recommendation for fork crates: no C dependencies. Use sha2/hmac (RustCrypto), serde_json, `windows-sys`, and ureq with native-tls only (schannel on Windows, security-framework on macOS). Then `cargo check`/`clippy --target x86_64-pc-windows-msvc` work from this Mac with zero extra tools, and `cargo test` runs on the host.

Other scratch results:
- `npx --yes @tauri-apps/cli@2.12.0` works here (npm cache in S/winmap/npmcache):
  - `signer generate --ci -w S/winmap/throwaway-key/tauri-update.key` wrote the 0600 key and the `.pub`;
  - `signer sign --app-version 1.0.0 -f … <file>` (with `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=` empty) wrote the `.sig` shown in §3.
  - This key is throwaway; never use it.
- The verifier prototype lives at S/winmap/verify-sig.
- Node UI checks (`scripts/check-ui-scripts.mjs` etc.) are pure fs+vm and host-independent. I could not run them in this session because tool calls kept failing, so treat them as unverified here.
- Commands for later workers:
    source S/rust/env.sh; export HOME=S/winmap/fakehome
    rsync -a --exclude target <repo>/windows/ S/winmap/<copy>/
    cargo check --locked --workspace                                       # host
    PATH=S/winmap/stubbin:$PATH cargo check --target x86_64-pc-windows-msvc   # only with no C deps

======================================================================
9. HARD PARTS / RISKS
======================================================================
1. Upstream's Claude path is token-based end to end: `.credentials.json`, api/oauth/usage, `claude -p` renewal, `claude auth login`, and a doctor that reads the credential. It must be excluded wholesale and pinned by the checks in §7.
2. Upstream's feed step targets `.nsis.zip`, which Tauri v2 `createUpdaterArtifacts: true` never makes. Copying it would produce a broken feed.
3. The tauri CLI only warns when the private key does not match the pubkey. Our own minisign verification against the committed key is mandatory.
4. File-in-use during updates: the NSIS updater replaces `agentnotch-hook.exe` and the status-line exe, which Claude Code runs constantly, so an in-use write is likely. It needs an `NSIS_HOOK_PREINSTALL` that renames the in-use exes aside (Windows allows renaming a running image) and cleans them up later.
5. Uninstalling leaves hook entries in `~/.claude*/settings.json` pointing at deleted exes. Those hooks fail with a non-2 exit, so they don't block, but they report errors on every event. `NSIS_HOOK_PREUNINSTALL` could run `agentnotch.exe uninstall-hooks`: a product decision, since it writes settings.json.
6. Coexistence with upstream Codenotch for Windows requires distinct values for the identifier, productName, exe names (no "codenotch-hook" substring), Run value, config dir, and IPC endpoint (port 48666 is taken; prefer a per-user named pipe, the analog of the Mac socket).
7. On Windows, `dirs` ignores environment overrides, so test isolation must come through injected roots.
8. Once Windows ships, every release must carry `latest.json`. The plan step must refuse a Mac-only release after that.
9. Version plumbing: VERSION is the source. The tauri `version` (via `--config` or kept equal and checked), CARGO_PKG_VERSION and the exe's ProductVersion must agree. `requireSignedVersion` also ties the signature to V.
10. Authenticode is absent, so SmartScreen warns on every manual install. Updates are safe through minisign.
11. With a required reviewer on the `release` environment, the multi-job release needs several approvals.
12. The tauri CLI comes from npm into the job that later holds the key. Pin it with a lockfile, and keep the key only in the signing step's environment.
13. Local limits: no cross-link or cross-bundle from this Mac without the MSVC licence and an LLVM install. Upstream's Tauri crate does not check on the macOS host without edits to upstream files. Fork crates should be designed to avoid both problems.
