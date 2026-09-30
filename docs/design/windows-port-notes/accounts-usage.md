ACCOUNTS AND USAGE: Mac engine to Windows porting spec
(Paths are repo-relative unless absolute. E = Packages/ClaudeControl/Sources/ClaudeControl/Engine. B = Sources/ClaudeBridge. X = ../claude-parallel-accounts-vsc-extension/src. W = windows/codenotch/src, which is upstream's Rust/Tauri port. Anything not read from code or docs is marked UNVERIFIED.)

====================================================================
0. FINDINGS THAT CHANGE THE PLAN
====================================================================
0.1 The upstream Windows port's Claude usage breaks the fork's no-token rule and must be removed, not extended.
  - W/usage.rs:1-21 reads `~/.claude/.credentials.json` (`claudeAiOauth.accessToken`), calls `GET api.anthropic.com/api/oauth/usage` and renews tokens by running `claude -p`.
  - W/usage.rs:97-122 `profiles()` finds accounts by the presence of a credential file (`has_credential`, line 93).
  - W/watcher.rs:78-81 gets its roots from that list.
  - Replace all of it with the engine's pipeline (sections 3-12): discovery through `.claude.json`'s `oauthAccount`; usage from get_usage, `cachedUsageUtilization`, status line `rate_limits` and Claude Desktop's cache.
  - The Rust tree needs its own equivalent of Scripts/verify-token-free.sh. Forbidden patterns: `.credentials.json`, `claudeAiOauth`, `accessToken`, `api/oauth/usage`, `sessions/*.key`, and any Credential Manager API such as `CredRead`.
0.2 Claude Parallel Profiles does nothing on native Windows.
  - X/extension.ts:29: `const SUPPORTED_PLATFORMS = ['linux','darwin']`. On win32 it goes inert (extension.ts:31-39: "no files are read or written … open your folder in a WSL window").
  - README.md:92-104 says the same: "Native Windows | Not supported | Inert".
  - So on native Windows these never exist: `%USERPROFILE%\.claude-windows\<hex>`, `.manifest.json`, `.parallel-accounts-store`, the `.claude-shared` links, and mirroring into `~/.claude`.
  - They exist only inside a WSL distro's Linux home (for example `\\wsl.localhost\<distro>\home\<u>\.claude-windows\…`). There the Claude Code processes are Linux processes with Linux paths.
  - Port the classifier and grouping logic unchanged: it is pure, cheap, and handles plain `CLAUDE_CONFIG_DIR` profiles (`%USERPROFILE%\.claude-<name>`). On native Windows its extension branches never fire.
  - Supporting WSL is a separate project: hooks inside WSL must reach the Windows app, and pids, paths and homes are Linux. Recommend leaving it out of v1.
0.3 Claude Code on Windows keeps the token in a plaintext file.
  - The file is `<dir>\.credentials.json` (W/usage.rs:5; X/credentials.ts:12 for Linux/WSL).
  - The Mac notion of a "login conflict" by exact `CLAUDE_CONFIG_DIR` spelling (the Keychain item name hashes the string, E/Models/AccountModels.swift:29-39) does not apply. On Windows two spellings of one folder are one login (NTFS is case-insensitive).
  - `CLAUDE_SECURESTORAGE_CONFIG_DIR` overrides where the token is read. Per X/extension.ts:74-84, "Claude Code's extension only sets it itself on Windows". The probe must strip it (section 10).

====================================================================
1. PATHS (E/Core/AccountPaths.swift)
====================================================================
1.1 Mac rules
  - `normalize(path)` (34-48):
    - expand `~` and `~/` against the configured home;
    - `standardizingPath`, which resolves `.` and `..` lexically; symlinks are NOT resolved;
    - drop the trailing `/`.
  - Account (folder) id = normalized absolute config folder (51-53).
  - defaultConfigDir = `~/.claude` (27-29).
  - `configDir(fromTranscriptPath:)` (62-74):
    - search the components from the end for "projects" at index ≤ count-3;
    - the prefix before it is the config folder;
    - main transcripts are `<cfg>/projects/<slug>/<sid>.jsonl`; subagent transcripts sit deeper.
  - `globalConfigFile(configDir, env)` (83-91) mirrors Claude Code, `join(CLAUDE_CONFIG_DIR || homedir, ".claude.json")`:
    - env set: `<normalize(env)>/.claude.json`, even when env == ~/.claude, which then reads `~/.claude/.claude.json`;
    - else the default folder: `~/.claude.json`;
    - else `<dir>/.claude.json`.
  - `isInfrastructureDir` (106-114): true for `~/.claude-shared`, `~/.claude-windows`, or any folder the registry classified as infrastructure (a lock-guarded set).
  - `shortName` = last path component.
  - `AccountPathDisplayName.abbreviated` (E/Services/Accounts/AccountNaming.swift:201-208) prints `~/…`.
1.2 Windows mapping
  - home = `%USERPROFILE%`, falling back to `SHGetKnownFolderPath(FOLDERID_Profile)`. Never use `$HOME`: Git Bash sets it to `/c/Users/...`.
    - Claude Code uses Node/Bun `os.homedir()`. libuv on Windows reads USERPROFILE first. Bun behaving the same is UNVERIFIED but expected.
  - default config folder `%USERPROFILE%\.claude`; default identity file `%USERPROFILE%\.claude.json`; other folders `<dir>\.claude.json`.
  - normalize():
    - accept `/` and `\`;
    - strip `\\?\` (and turn `\\?\UNC\` into `\\`);
    - resolve `.` and `..` lexically;
    - uppercase the drive letter;
    - drop the trailing `\` except at a root (`C:\`);
    - optionally expand 8.3 short names (`GetLongPathNameW`) when a component contains `~`.
  - Compare case-insensitively (ordinal ignore-case). Recommended: map keys use a folded form; ids keep the case for display.
  - Any hash of a path (the `claude-dir-` ring id, 5.4) must be taken over one canonical form. Recommend lowercase with backslashes, so ids stay stable.
  - The transcript "projects" search compares case-insensitively.
  - Transcript slug: every non-alphanumeric character becomes `-` (CLAUDE.md facts). So `C:\Users\me\proj` becomes `C--Users-me-proj`. Prefer the hook's `transcript_path`.
  - Display: `~\.claude-work`.
  - `DevFlags.pathList` (E/Core/DevFlags.swift:57-62) splits `AGENTNOTCH_EXTRA_CONFIG_DIRS` on ":". Windows must split on ";" because drive letters contain ":".
  - `AccountRegistry.canBeAccount` / `isAncestor` (E/Services/Accounts/AccountRegistry.swift:901-912) special-case "/". Windows needs drive roots and UNC roots, case-insensitive prefixes, and link resolution via `std::fs::canonicalize` (strip its `\\?\`).

====================================================================
2. FOLDER MODEL AND PARALLEL PROFILES LAYOUT
====================================================================
2.1 ClaudeAccount, i.e. one config folder (E/Models/AccountModels.swift:24-155)
  - id, configDir: normalized.
  - configDirEnv: raw `CLAUDE_CONFIG_DIR`, nil for the default.
  - customLabel.
  - seenConfigDirEnvs: every spelling seen, "" meaning unset.
  - Identity fields from `oauthAccount`: email, displayName, organizationName, organizationUuid, accountUuid, subscriptionType, rateLimitTier.
  - colorIndex (palette of 8), source (discovered|hook|manual), lastSeenAt, isHidden.
  - kind (run|store|infrastructure; not persisted).
  - defaultLabel / defaultMonogram (computed).
  - isDefault = configDirEnv empty AND configDir == ~/.claude.
  - planName (131-144): tier contains "max_20x" gives "Max 20x"; "max_5x" gives "Max 5x"; else subscriptionType max/pro/team/enterprise capitalized.
  - launchCommand (148-151): `claude`, or `CLAUDE_CONFIG_DIR='<dir>' claude` (POSIX single quotes).
    - Windows needs a PowerShell form, `$env:CLAUDE_CONFIG_DIR='C:\…\.claude-work'; claude`, or cmd `set "CLAUDE_CONFIG_DIR=…" && claude`.
    - This string feeds the sign-in guidance "Run <cmd> once, then /login." (B/ClaudeUsageProvider.swift:75-77).
2.2 Names (E/Services/Accounts/ConfigDirClassifier.swift:54-80)
  - `.claude-windows`, `.claude-shared`, `.parallel-accounts-store` (marker), `.manifest.json` (inside `.claude-windows`).
  - isWindowDir = a direct child of `~/.claude-windows` whose name doesn't start with ".".
2.3 Manifest `~/.claude-windows/.manifest.json`
  - Written by X/accounts.ts:195-212 as `JSON.stringify({stores:[…abs paths], created:[…], customOAuth: bool, defaultConfigDir: string|null}, null, 2)`, mode 0600.
  - The engine reads only `stores` and `created`: arrays of non-empty strings, each normalized (ConfigDirClassifier.swift:90-96).
  - `isUnreadable` (105-110): the file exists but doesn't parse. The extension writes it in place (not atomically), so a reader can catch it mid-write.
2.4 What the extension does (Linux/macOS/WSL only)
  - Stores are `~/.claude-<name>`, created with the marker (X/accounts.ts:94-102).
  - Per-window working copy: `~/.claude-windows/<first 12 hex of sha1(workspaceFile URI string || first folder fsPath)>`; a folderless window gets random 6 bytes (X/workdir.ts:70-82).
  - Shared history: `~/.claude-shared/{projects,sessions,session-env,shell-snapshots,file-history,plans,todos}` plus `history.jsonl`, symlinked into every folder (X/sharedHistory.ts:20-30).
  - Mirrors the focused window's account into `~/.claude.json`'s `oauthAccount` and its token into `~/.claude` (X/capture.ts:118-181).
  - Version 1.4.x copies the whole `oauthAccount` block from the account's own file (X/oauthAccount.ts:36-43). Older versions patched only email, displayName and organizationName and kept the stale `accountUuid`, which is why the engine lets the email decide (5.2).
  - Its own discovery regex is `/^\.claude[-_](.+)$/`, skipping `windows` and `shared` (X/accounts.ts:338-347).
  - Mac engine counterpart: E/Services/Accounts/WindowFolderNames.swift:19-21 (`Insecure.SHA1…prefix(6)` = 12 hex). It names a window folder "VS Code · <project>" by hashing session cwds and their parents up to home (26-51). Windows: dormant, or WSL only.
2.5 classify(snapshot, previousStores) is pure (ConfigDirClassifier.swift:219-262). For each folder except home itself:
  1) `~/.claude-windows` or `~/.claude-shared`: infrastructure.
  2) Has the marker, OR is in manifest.created, OR (manifest unreadable AND in previousStores, "sticky"): store.
  3) Some other folder's `projects` or `sessions` symlink targets `<owner>/{projects|sessions}` (owner ≠ itself) AND it is not `~/.claude` AND it is not signed in: infrastructure.
  4) Otherwise run. It is flagged adoptedByExtension if it is in manifest.stores, not the default, and not a window folder.
  - extensionDetected = manifest present || manifest unreadable || any marker found.
2.6 readSnapshot (271-307) is read-only. Candidates:
  - `~/.claude` if it is a directory;
  - every home child named `.claude-*` or `.claude_*` that is a directory;
  - every non-dot child of `~/.claude-windows`;
  - manifest.stores that exist;
  - explicitDirs (known accounts, `AGENTNOTCH_EXTRA_CONFIG_DIRS`, previous stores).
  folderFacts (310-343):
  - hasGlobalConfig = `<dir>/.claude.json` exists;
  - isSignedIn = the identity file (`~/.claude.json` for the default folder) has a login (byte scan, 4.4);
  - hasStoreMarker, hasProjects, hasSessions: is-dir checks;
  - linkTargets: `readlink` of `projects` and `sessions`, made absolute;
  - hasLiveSession: only when `sessions/` is not a link (4.5).
2.7 Windows mapping
  - Symlinks and junctions: `std::fs::symlink_metadata` + `read_link`, which handles both.
  - Relative link targets resolve against the folder.
  - Directory listings: `read_dir`. Hidden or dot names have no special meaning on Windows, so keep the dot-prefix string rules.
2.8 WindowDirWatch (E/Services/Accounts/WindowDirWatcher.swift). Every 5 s it takes a fingerprint:
  - windows: {child name: stamp of `<child>/.claude.json`} (mtime, size, exists);
  - manifest stamp;
  - `~/.claude.json` stamp, only while the manifest exists.
  change():
  - a set of names differs, or the manifest differs: `.folders`, which triggers a rediscovery;
  - a window's stamp changes and exists flips: `.folders`;
  - a window's stamp changes otherwise: `.identities`, which re-reads identities;
  - the default config changed: `.identities`.
  Windows: keep it as a cheap poll, or replace it with ReadDirectoryChangesW / the `notify` crate. On native Windows it effectively watches nothing.

====================================================================
3. DISCOVERY AND REGISTRY (E/Services/Accounts/AccountRegistry.swift)
====================================================================
3.1 Schedule
  - A synchronous discovery when the registry is created (249-270), so the rings made at launch and the hook manager's first pass never mistake a store for a run folder.
  - Then `discoverNow` every 5 min (120, 309-314).
  - The window watcher (321-337).
  - A new folder seen in a hook or status line triggers a discovery (866).
3.2 discover(snapshot) (504-555): the order is default, then others sorted, then windows sorted, then stores sorted.
  - Default: added if `~/.claude` is in the snapshot.
  - Skip folders that `canBeAccount` rejects (home or an ancestor of home, as written or with links resolved).
  - store: added if hasGlobalConfig.
  - run + window folder: added if hasGlobalConfig.
  - run + in extras: added.
  - run + a direct home child named `.claude-*` / `.claude_*`, and (hasProjects || hasSessions || hasGlobalConfig):
    - looksLikeBackup(name): suggestion `.looksLikeBackup`;
    - isSignedIn || hasLiveSession: added;
    - else: suggestion `.found`.
  - Other known run folders (by hand, from a hook) are classified but not rediscovered.
  - looksLikeBackup (582-600): strip `.claude-`, `.claude_` or `.`; split on non-alphanumerics. It is a backup if:
    - any word is in {backup, backups, bak, bkp, old, copy, orig, original, archive, archived, tmp, temp, save, saved, prev, previous}; or
    - a word starts with "backup" or ends with "backup"/"bak"; or
    - a numeric word has 8 digits, or 4 digits in 2019...2099.
3.3 applyLayout (274-293)
  - Drops folders now classified infrastructure.
  - Drops window folders and stores that have gone from disk.
  - Keeps other vanished folders ("Folder missing").
  - Publishes the infrastructure set to AccountPaths.
3.4 New folders
  - A discovered folder is created with configDirEnv = its path; for the default folder, nil.
  - colorIndex = nextColorIndex (1280-1287): the first unused of 8, else the least used, lowest index on ties.
3.5 Sightings: record(AccountSighting{configDir, configDirEnv raw or nil, sessionId, at}) (830-897)
  - Ignored for infrastructure, sharedStore and windowsRoot.
  - variant = rawEnv ?? ("" if the default folder else nil).
  - Unknown folder: add it with source .hook, unless forgotten; a forgotten folder becomes a "seenAgain" suggestion instead. Then discover.
  - Known folder:
    - the first spelling seen sets configDirEnv;
    - later spellings are appended to seenConfigDirEnvs;
    - lastSeenAt updates at most once per 60 s (123);
    - if the env changed, identities are refreshed.
  - SightingThrottle (E/Services/Session/HookEventPipeline.swift ~188-218): at most one sighting per session per 60 s, or at once when the session's config folder changes. It needs a transcript_path or an env value.
  - configDir resolution (E/Services/Hooks/HookEvent.swift:475-486): the transcript path's folder, unless that is infrastructure; else normalize(CLAUDE_CONFIG_DIR); else `~/.claude`.
  - Windows: the same logic. The Mac per-spelling logic (performIdentityRefresh, 739-781) moves to a signed-in spelling when the current one has no login. On Windows collapse spellings by normalized path; conflicts can't arise.
3.6 User actions
  - checkFolder (928-953) rejects:
    - missing or not a folder;
    - home, or a link to home;
    - a folder containing home or ~/.claude;
    - a folder inside another account's folder;
    - infrastructure.
  - addAccount (958-990): source .manual; configDirEnv = the normalized path.
  - createAccount(name) (1013-1053):
    - slug = sanitizedAccountName: ASCII letters, digits, `-`, `_`, `.`; spaces become `-`; strip leading `.`/`-`; lowercase;
    - mkdir `~/.claude-<slug>` with 0700;
    - an existing folder is adopted only if it is clearly a Claude folder (projects/sessions/.claude.json/settings.json);
    - the typed name becomes customLabel.
  - rename; setHidden (per identity when the folder belongs to one); remove ("forget": a whole identity, or a folder; nothing is deleted on disk).
  - Windows: create the folder without special ACLs (the profile folder is user-private by default).
3.7 accounts.json at `<support>/accounts.json` (1291-1407), version 2
  - JSONEncoder: pretty, sortedKeys, withoutEscapingSlashes; dates ISO 8601 (whole seconds); nil optionals omitted; atomic write; parent folder 0700.
  ```
  {"version":2,
   "accounts":[{"id","configDir","configDirEnv"?,"customLabel"?,"colorIndex":Int,"isHidden":Bool,
                "source":"discovered|hook|manual","lastSeenAt"?:ISO,"seenConfigDirEnvs"?:[String]}],
   "removedIds":[String],
   "identities"?:{"uuid:…|email:…|dir:…":{"customLabel"?,"colorIndex":Int,"isHidden":Bool}},
   "forgottenIdentities"?:[String],
   "defaultIdentityTimeline"?:{"spans":[{"identity"?,"rawUuid"?,"isLogin":Bool,"from":ISO,"after"?:ISO,"lastSeen":ISO}]}}
  ```
  - On load, canBeAccount is re-checked and ids deduplicated.
  - Windows: `%LOCALAPPDATA%\Agent Notch\Claude\accounts.json`. Recommend LOCALAPPDATA over APPDATA: the content holds machine-local absolute paths and pids, which a roaming profile would carry to other PCs. Upstream uses `%APPDATA%\codenotch`.
  - Atomic write = temp file + `fs::rename` (MoveFileEx replace). Retry on sharing violations (antivirus).
  - VibeNotchImport.swift (Superpowered Vibe Notch accounts.json import) is Mac-only; skip it.

====================================================================
4. READING .claude.json
====================================================================
4.1 ClaudeGlobalConfigReader (E/Services/Accounts/ClaudeGlobalConfigReader.swift:122-179)
  - One shared cache per path, keyed by (mtime, size).
  - A reparse that fails (caught mid-write) returns the last good parse.
  - A missing file evicts the entry and returns nil.
  - The registry and UsageStore share it.
4.2 Only two top-level keys are read: `oauthAccount` and `cachedUsageUtilization` (94).
  - JSONFieldScanner (E/Services/Accounts/JSONFieldScanner.swift) walks the top-level object byte by byte:
    - skips a BOM;
    - skips values: strings with escapes, nested {}/[] tracked by depth, scalars up to the next delimiter;
    - returns the raw bytes of the wanted values only (the last occurrence wins);
    - returns nil when the text is not an object or is truncated.
  - Rust: `#[derive(Deserialize)] struct G { #[serde(rename="oauthAccount")] o: Option<Box<RawValue>>, #[serde(rename="cachedUsageUtilization")] c: Option<Box<RawValue>> }`. Unknown fields go through IgnoredAny, which builds no value.
    - One difference: serde rejects a duplicated known key, where Swift keeps the last. Acceptable, or port the scanner.
  - The file can be megabytes.
4.3 oauthAccount → identity (45-60); the identity is nil unless accountUuid or email is present.
  - accountUuid ← "accountUuid"
  - email ← "emailAddress"
  - displayName ← "displayName"
  - organizationName ← "organizationName"
  - organizationUuid ← "organizationUuid"
  - organizationType ← "organizationType" (for example "claude_max")
  - rateLimitTier ← "organizationRateLimitTier" ?? "userRateLimitTier"
  - billingType ← "billingType"
  - hasExtraUsageEnabled ← "hasExtraUsageEnabled" (lenient bool)
  - Empty strings are treated as absent.
  - subscriptionType (35-42): organizationType lowercased with the `claude_` prefix stripped.
  - Applied to a folder (800-814): every field is overwritten; subscriptionType is kept if the identity has none, cleared when signed out.
  - noteSubscriptionType (818-825): the plan from get_usage fills a missing one.
4.4 hasLogin (618-641) is a byte scan of the file:
  - find `"oauthAccount"`, whitespace, `:`, whitespace, `{`, whitespace, then any character except `}`;
  - `{}` counts as no login;
  - the Mac reads `mappedIfSafe`.
  - Windows: do NOT memory-map. A mapped view can make Claude Code's replace or truncate of the file fail (ERROR_USER_MAPPED_FILE). Read it into memory. Rust `File::open` already uses FILE_SHARE_READ|WRITE|DELETE, so Claude Code's rename isn't blocked.
4.5 hasLiveSession (645-655): `sessions/` holds some `<pid>.json` (pid > 0) whose process is alive. The file is never opened; `.key` files are never touched.
  - isProcessAlive (658-660) = `kill(pid,0)==0 || errno==EPERM`.
  - Windows: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` then `GetExitCodeProcess == STILL_ACTIVE`. ERROR_ACCESS_DENIED counts as alive; ERROR_INVALID_PARAMETER as gone.
  - Windows reuses pids aggressively. Where a start time is known, pair the pid with process creation time (14.2).
4.6 cachedUsageUtilization (E/Services/Usage/UsageParser.swift:239-252)
  - Shape: `{"fetchedAtMs": number, "accountUuid": String, "utilization": <usage body 8.1>}`.
  - Snapshot = fetchedAt (ms/1000), accountUuid, the parsed body.
  - Nil unless the body has at least one window.
  - matchingCachedUsage (ClaudeGlobalConfigReader.swift:114-119): used only when cached.accountUuid == oauthAccount.accountUuid. After a `/login` the old snapshot stays behind in the file.

====================================================================
5. IDENTITY GROUPING, RING IDS AND ATTRIBUTION
====================================================================
All in E/Services/Accounts/AccountIdentities.swift and pure.
5.1 Key prefixes
  - `uuid:`, `email:`, `dir:`.
  - baseKey (154-162) = `uuid:<trimmed lowercased accountUuid>`, else `email:<lowercased email>`.
  - Split organization key = `uuid:<acct>/<org>` (separator "/", 247). Helpers: accountUuid(ofKey:) and organization(ofKey:) (250-261).
5.2 identityKeys(folders, mirroredDefault) (178-244)
  Folders with a uuid or email ("logins") are compared against the other logins.
  - uuids(forEmail e): the uuids that other logins with email e have, by count descending, then uuid ascending.
  - (uuid, email):
    - uuidElsewhereAsOthers = another login has the same uuid with a different non-nil email;
    - emailElsewhere = uuids(forEmail) minus this uuid;
    - staleMirror = (folder == mirroredDefault) && no other login has the same uuid AND email;
    - if (uuidElsewhereAsOthers || staleMirror) and emailElsewhere is non-empty: key `uuid:<emailElsewhere[0]>`, corrected=true. **The email decides.**
    - else `uuid:<uuid>`.
  - (uuid, nil): `uuid:<uuid>`.
  - (nil, email): `uuid:<first uuid for that email elsewhere>`, else `email:<email>`.
  - Organization split: for each `uuid:` key, own = the non-nil organizationUuids of the members not corrected.
    - If more than one distinct: common = the most frequent (ties go to the ascending uuid).
    - Every member becomes `key + "/" + (corrected ? common : own org ?? common)`.
5.3 group(folders, prefs, forgotten, savedFolders, mirrorsDefault, home) (333-473)
  - usable = kind ≠ infrastructure.
  - Keys are computed with mirroredDefault = defaultFolder.id when mirrorsDefault.
  - Buckets by key; forgotten keys' folders go to `forgottenFolders`.
  - Folders nobody signed in to:
    - only a run folder can get a ring;
    - `dir:<folderId>` if (source manual, not a window, not the default), or (the default and no signed-in identity exists anywhere);
    - else `unsignedFolders` (listed as "not signed in");
    - an unsigned store is dropped.
  - defaultOwner(defaultFolder, keys) (267-278):
    - own = baseKey(accountUuid, email only when there is no uuid);
    - own if present;
    - else the single `own/…` split key;
    - else the split key matching the folder's organization;
    - else nil.
  - Prefs derivation for keys without prefs:
    - order: identities holding the default folder first, then key ascending;
    - sources = orderedFolders. If the default folder is mirrored and corrected, it is removed, and inserted first for the owner's key;
    - saved folders (in accounts.json) come first;
    - derivedPrefs (502-511): label = the first non-empty customLabel; colour = the first source's colorIndex unless another bucket's identity has it (mod 8), then nextColorIndex(taken); isHidden = the first source's.
  - Identity fields: the first non-empty value over the members sorted by trustRank (491-497): store 0, window 1, standalone 2, default 3, corrected 9.
    - organizationUuid = the split scope, keeping the member's original casing when one matches, else field().
    - accountUuid = from the key, else field().
  - runDirs / storeDirs = orderedFolders (477-489): default 0, window 1, other run folders 2, stores 3; then by path.
  - includesDefault, organizationScope, windowDirIds.
  - Naming: AccountRegistry.named(representatives) (section 6).
  - Final order: label (localizedCaseInsensitiveCompare), then id. It deliberately does not follow which account `~/.claude` holds, so the rings don't reorder whenever the mirrored account changes.
  - ClaudeIdentityAccount (45-146):
    - primaryDir = runDirs.first ?? storeDirs.first;
    - terminalLaunchCommand = "claude" if includesDefault, else the launchCommand of the first non-window run folder with a set configDirEnv, else nil;
    - canBeForgotten = !includesDefault || it has stores or windows.
5.4 Ring ids
  - Identity ring (283-296, 517-520): for `uuid:`/`email:` keys, strip the prefix (the value may be `<acct>/<org>`), then `"claude-acct-" + hex(sha256(lowercased(value)).prefix(6))`, i.e. 12 hex.
  - A `dir:` identity uses the per-folder id (E/Public/ClaudeSummaries.swift:380-415):
    - `~/.claude` → `claude`;
    - `~/.claude-<slug>` → `claude-<slug>`;
    - anything else → `claude-dir-<first 4 bytes, 8 hex, of sha256(normalized path)>`.
  - Windows: identical for `claude-acct-`. For `claude-dir-`, hash the canonical Windows form (1.2).
5.5 FolderRings (526-597)
  - A lock-guarded global snapshot the registry publishes (observeDefault, AccountRegistry.swift:1223-1245): folder→ring, folder→identity, identity→ring, untracked identities and folders, defaultFolder, mirrorsDefault, defaultTimeline.
  - It serves the socket server and the pure mappers, from any thread.
  - `isUntracked(folder,pid)` uses the process start time.
5.6 FolderIdentityTimeline (E/Services/Accounts/FolderIdentityTimeline.swift)
  - Records who `~/.claude` ran as over time. Used only while mirroring (extensionDetected), so it is dormant on native Windows.
  - Span{identity?, rawUuid?, isLogin, from, after?, lastSeen}; at most 24 spans (60).
  - observe(identity, rawUuid, modifiedAt, now, resumed) (80-97):
    - first look: span from min(modifiedAt ?? now, now);
    - same identity and rawUuid, and not (resumed && written > lastSeen): only extend lastSeen;
    - else append Span(isLogin: rawUuid changed, from: min(max(written, last.lastSeen), now), after: last.lastSeen).
  - identity(at:) (100-111):
    - before the first span: `.unknown`;
    - within a span: `.identity`, unless a later span isLogin, then `.switching`;
    - in a gap (after, from): `.switching`.
  - FolderAttribution.attribute(current, timeline, startedAt, mirrored) (141-149):
    - not mirrored: `.known(current)`;
    - no start time or no timeline: `.unsure`;
    - `.identity(id?)`: `.known(id)`;
    - otherwise `.unsure(current)`.
  - registry.attribution(forFolderId, startedAt) (AccountRegistry.swift:392-399) uses the timeline only for the default folder.

====================================================================
6. NAMES, BADGES AND NICKNAMES
====================================================================
6.1 AccountNaming (E/Services/Accounts/AccountNaming.swift)
  - baseLabel: "Claude <Domain>", where Domain = the domain's first label, capitalized, with at least one letter; else "Claude <email>"; else "Claude (<folderWord>)"; else "Claude".
    - folderWord = `.claude-work` → "work".
  - assign(accounts) (99-165), sorted by id:
    - clashes (case-insensitive) are lengthened in up to 3 levels, skipping custom names:
      - L1 "Claude <email>";
      - L2 "Claude <email> · <organizationName|planName>";
      - L3 "Claude <email> · <~path>" or "Claude (<~path>)".
  - Monograms: candidates from customLabel, domain label, email local part, organization initials (or the name), displayName, folderWord; "CC" as the fallback.
    - Custom names pick first.
    - An account whose first choice is unique keeps it.
    - The rest take their other candidates, then the shared one, then `<letter><1..99>`.
  - Hidden accounts are named as if they joined the tracked set (AccountRegistry.swift:1250-1262).
6.2 Bridge naming (B/ClaudeRingNames.swift): ring name = the engine's ownLabel (the identity's label), else Codenotch's rule:
  - "Claude <Domain>", or the full email when two collide;
  - folderName: `claude-acct-*` or the default ring → "Claude"; `claude-<slug>` → "Claude (<slug>)"; else the folder name without dots.
  - source text (110-123):
    - "Claude Parallel Profiles account (no window open)";
    - "Claude Code";
    - "Claude Code in ~/x and N VS Code workspaces".
    - It uses `contains("/.claude-windows/")`; Windows needs a separator-agnostic check.
6.3 Nicknames are upstream Codenotch's.
  - `Preferences.accountNicknames: [ringID: String]` in UserDefaults key "accountNicknames" (Sources/Settings/Preferences.swift:77-79, 492, 921-933).
  - setNickname trims; blank removes.
  - The hub gets them through `setNicknames(byRingID)` (E/Public/ClaudeControlHub.swift:760-764). recompute() replaces the account label with a non-empty nickname (298-302).
  - The Windows port has no nicknames (W: no hits). Add `accountNicknames: {ringId: name}` to the fork's Windows config.json.
6.4 Ring migration (E/Public/ClaudeRingMigration.swift, B/ClaudeRingMigrator.swift)
  - Moves nickname, on/off, order, archive, muted state and menu-bar choice from the per-folder ring ids of older Mac versions to `claude-acct-*`, once per old id (defaults key "claudeControl.migratedRingIDs").
  - Mac-upgrade only. A fresh Windows install has no legacy rings: do not port it. Only port ringID(configDir:) for `dir:` identities.

====================================================================
7. PUBLIC PROJECTION AND HUB (per account)
====================================================================
  - ClaudeAccountSummary (E/Public/ClaudeSummaries.swift:56-153): id, ringID, configDir (the primary folder), label, email, planName, organizationUuid, isDefault (includesDefault), isTracked, colorIndex, hooks, launchCommand, runDirs, storeDirs, windowCount, isSignedIn, formerRingIDs, uncertainFormerRingIDs, canForget, hasTerminalLaunch, adoptedDirs, ownLabel.
    - Built by ClaudeHostProjections.account(identity:…) (E/Public/ClaudeHostProjections.swift:67-114).
  - Hub.launchAccounts() (ClaudeControlHub.swift:142-153): the visible identities, synchronously, so the rings exist at launch.
  - recompute() (282-420): accounts + nicknames; session attribution (known / unsure / waiting; Desktop-hosted 13.x); per-identity ClaudeRingReading (381-392); retiredRingIDs; paused rings.
  - pausedAccountIds (771-779): accounts whose ring is hidden. `UsageStore.setPausedAccounts` then never probes them or reads Desktop for them.
  - The session's ring when its folder is unknown: the account `~/.claude` runs as, else the first tracked account (443-447).

====================================================================
8. USAGE DATA FORMATS (E/Services/Usage/UsageParser.swift)
====================================================================
8.1 The usage body (the /api/oauth/usage shape inside cachedUsageUtilization.utilization and get_usage rate_limits) (132-174)
  - fiveHour = `five_hour{utilization 0-100, resets_at}` ?? `limits[kind=="session"]{percent|utilization, resets_at}`; duration 18000 s.
  - sevenDay = `seven_day` ?? `limits[kind=="weekly_all"]`; duration 604800 s.
  - scoped, in order, the first name winning case-insensitively:
    - `limits[kind=="weekly_scoped"]` named by `scope.model.display_name`;
    - `model_scoped[]{display_name, utilization|percent, resets_at}`;
    - `seven_day_opus` → "Opus";
    - `seven_day_sonnet` → "Sonnet".
  - extra_usage{is_enabled, monthly_limit, used_credits, utilization, currency}; credits are in minor units.
  - subscription_type.
  - A real body carries extra keys that are ignored (UsageParserTests.swift:19-31): `limit_dollars`, `iguana_necktie`, `spend`, `seven_day_cowork`, …
  - Lenient scalars (276-320):
    - numbers from int, double or numeric string; bools are not numbers; NaN and infinity are rejected;
    - dates: ISO 8601 with or without fractional seconds; no zone means UTC; epoch numbers above 1e12 are ms, else seconds.
8.2 get_usage response (206-234)
  - `rate_limits_available == false` → unavailable("Usage limits aren't available for this login").
  - `rate_limits` not an object (null) → malformed("Claude Code couldn't load usage right now"). Claude Code 2.1.280 answers null when its own fetch failed.
  - `rate_limits.error{type:"rate_limit_error"}` → rateLimited; another error → malformed(message).
  - Else parse the body; `subscription_type` overrides.
  - isPossiblySeeded = `rate_limits.limits` is NOT an array. Claude Code's 429 fallback answers with its saved snapshot, up to an hour old, with `limits` stripped and no fetch time.
  - No windows → malformed.
8.3 Status line rate_limits (257-270): `{five_hour|seven_day: {used_percentage (or utilization) 0-100, resets_at epoch s}}`.
  - CLAUDE.md facts (2.1.282):
    - one set per process; the latest response replaces it, even a lower one;
    - empty until the process's first response;
    - windows past `resets_at` are omitted;
    - re-runs repeat the set with no timestamp;
    - headers arrive at the start of a response but are applied at its end.
8.4 Models (E/Models/UsageModels.swift)
  - UsageWindow{utilization, resetsAt?, duration}; it reads 0 after its reset.
  - AccountUsage{accountId, fiveHour?, sevenDay?, scoped[{name, window}], extraUsage?, subscriptionType?, source probe|statusLine|cache, updatedAt, takenAfter?}.
  - limitHit (174-188): of the windows at ≥100% not yet reset, the one with the latest reset; nil reset counts as the farthest.
  - staleThreshold(minutes) (157-160): 3600 s when probes are off, else max(900, minutes·60·1.5).
8.5 Engine window ids (E/Services/Usage/UsageRingWindows.swift)
  - `session`, `weekly_all`, `weekly_<slug>`: lowercased; runs of non-ASCII-alphanumerics become `_`; trimmed; "Sonnet 4.5" → `weekly_sonnet_4_5`; empty → `weekly_scoped`.
  - `extra_usage`: fraction = utilization/100, else used/limit; money in major units using the currency's fraction digits; "USD" by default.
  - Order: session, weekly_all, then by id.
  - accountUsage(from external reading) (140-175): an extra_usage window is converted back to minor units. Nil without session or weekly_all.

====================================================================
9. USAGE STORE (E/Services/Usage/UsageStore.swift): per identity, four token-free sources
====================================================================
9.1 Constants (76-110)
  - cache poll 20 s; scheduler 30 s;
  - minimumProbeInterval 300 s; forced refresh ≥ 60 s apart; ring click probes only if the newest data is > 120 s old; refresh wait ≤ 20 s;
  - backoff 120 s·2^(n-1), capped at 1800 s (508-512); unavailable → next attempt in 1800 s;
  - fullSnapshotMaxAge 900 s;
  - Desktop read ≤ every 60 s, 300 s after a miss; a forced read keeps ≥ 5 s between reads;
  - seeded-fresh window 90 s;
  - state save 2 s after a change, 30 s when only a status line changed;
  - Desktop clock allowance 60 s; discard retry 60 s;
  - statusLineRetention 7 days (722).
  - Settings (E/Core/ClaudeControlSettings.swift):
    - `claudeControl.usageProbeIntervalMinutes`: default 5; 0 turns the probe off; effective = max(min·60, 300) (502-505);
    - `claudeControl.readsDesktopUsageCache`: default true;
    - `claudeControl.claudeBinaryPath`.
  - Automatic probes are off in dev runs (`--no-install`) unless `AGENTNOTCH_USAGE_PROBE=1`, and off when sealed (125-130).
9.2 Loops (287-325)
  - pollCycle every 20 s: pruneStatusLine, pollCaches, pollExternal, scheduleProbes.
  - The scheduler runs every 30 s.
  - Registry identity changes, debounced 1 s, run accountsChanged (973-998). It:
    - moves folder-keyed state to identities;
    - prunes unknown ids;
    - drops a folder's status lines once it has changed hands (except a mirrored ~/.claude);
    - triggers a poll.
9.3 Reading = {window, at (the latest the data can be from), notBefore?} (136-154). A full snapshot's Reading: at = updatedAt, notBefore = min(takenAfter ?? updatedAt, updatedAt).
9.4 Merge algorithm (pure; port exactly)
  - isSameWindow(a,b) (581-584): both have resets and |Δ| < min(duration)/4 (75 min for 5 h, 42 h for 7 d).
  - isSmallDrop(higher, lower) (614-618): same window and 0 < drop ≤ 5 points (resetDropMinimum, 593).
  - supersedes(c, o) (603-610): c.notBefore ≥ o.at and c.at > o.at, unless:
    - c's window had reset by c.notBefore while o's reset is later, or
    - isSmallDrop(o→c).
  - isMoreCurrent(c, o) (631-641):
    - both have resets and are different windows: the later reset wins;
    - same window with different utilization: the higher wins;
    - else the later `at` wins.
  - mostCurrentIndex (653-660): drop readings superseded by any other, then reduce with isMoreCurrent; ties keep the earlier one.
  - advance(record, five, seven, receivedAt, startedAt) (676-694): one Claude Code process's record.
    - notBefore = (record.lastReportAt ?? startedAt) if < receivedAt.
    - Per window: nil keeps the stored reading; isRepeat (same utilization and duration, resets within 1 s, 699-706) or a small drop keeps the stored reading and its time; else Reading(window, at: receivedAt, notBefore).
    - lastReportAt = max.
  - statusLineKey (712-718): `pid:<pid>@<epochSecondsOfStart>`, else `pid:<pid>`, else `session:<id>`.
  - statusCandidates (405-412): with mirroring, readings via ~/.claude count only if newer than the newest reading from the account's own folders.
  - merge (783-809): start from the full snapshot (or empty). For fiveHour and sevenDay, winningStatus(status readings, over: snapshot reading) (814-818): the snapshot is candidate 0 and a status reading must win outright. updatedAt and source reflect the newest reading used. Scoped windows, extra usage and plan come only from the full snapshot.
  - Full snapshots replace each other only by a newer updatedAt (835-838), whatever the source.
9.5 ingest(StatusLineUpdate) (517-566)
  - folderId = normalize(update.accountId ?? CLAUDE_CONFIG_DIR ?? ~/.claude).
  - registry.attribution(folder, sessionStartedAt(sessionId)):
    - known(id) → id;
    - known(nil) → registry.identityId(for: folder) ?? folderId;
    - unsure → DROP.
  - pid is trusted only if the session's hook pid equals it (trustedProcessId, 747-751).
  - startedAt = the kernel start time of the pid.
  - Store under [identity][folder][processKey]; publish; save after 30 s on change.
  - Emits UsageObservation(source statusLine) for the cloud when the reading is new and shown.
9.6 pollCaches (854-899)
  - Every registry folder's global config, stores included, read off the main thread → registry.applyIdentity.
  - Then per identity:
    - signedIn and fetchState: not signed in → unavailable("Not signed in to Claude"); signing back in → idle (923-931);
    - the organization (only when the Desktop source is on);
    - freshestCachedUsage (904-917): the max fetchedAt over the identity's folders (filtered to organizationScope when split) whose cachedUsage.accountUuid == the identity's uuid, lowercased → full snapshot (source cache, updatedAt fetchedAt).
9.7 pollExternal (943-971)
  - Visible, not paused, signedIn == true, organization known, due (934-938).
  - reading → snapshot with takenAfter = observedAt − 60 s → accept. A miss is remembered.
9.8 Probe scheduling (1058-1110)
  - Candidates = visible identities with a run folder.
  - Due = signedIn, not paused, past nextAttemptAt, last probe ≥ 300 s ago, and (now − newestData ≥ interval OR now − newestFull ≥ max(interval, 900 s)).
  - Pick the smallest newestData.
  - One probe at a time; a FIFO forced queue (1112-1146). A forced probe clears the backoff and is skipped if probed < 60 s ago.
  - refresh(accountId, reason) (453-471): the caches and Desktop are re-read first. ringClick respects freshness and backoff; waits ≤ 20 s.
9.9 startProbe (1148-1244)
  - Refuses when probes are disabled ("Usage probes are off") or there is no run folder ("Not checked: no VS Code workspace or terminal folder runs this account now").
  - Steps:
    1) pollCaches;
    2) activity per run folder = max(folder.lastSeenAt, mtime of its .claude.json), ignoring the mtime for a mirrored ~/.claude;
    3) UsageProbePlanner.probeFolder (E/Services/Usage/UsageProbePlanner.swift:29-54): kind==run only. With mirroring, drop ~/.claude if the identity has other run folders. Take the newest activity (ties: default first, then the smaller path); else the default; else the first;
    4) before: folderRuns(the folder's current login, identity) and not HookInstaller.isNeverInstallTarget (E/Services/Hooks/HookInstaller.swift:254-273). A store → unavailable; changed hands → discardProbe (no backoff, retry ≥ 60 s);
    5) run with configDirEnv = the folder's (nil for the default), configDirs = all run folders (for binary lookup), cwd = `<support>/usage-probe`;
    6) after: re-read the config; changed hands → discard; a seeded answer is dated by matchingCachedUsage.
  - folderRuns (1249-1263): split-org mismatch → false; else the emails match (lowercased); else the uuids match.
  - interpretProbeAnswer (1284-1303):
    - not seeded → snapshot(source probe, updatedAt now, takenAfter min(launchedAt, now));
    - seeded and the cached copy hasSameWindows (|Δutil| < 0.5, |Δreset| < 60 s, same scoped set; ParsedUsage 69-90) → snapshot(source cache, updatedAt fetchedAt), rateLimited = now − fetchedAt > 90 s;
    - otherwise nil and rateLimited.
  - finishProbe (1312-1360):
    - usage: accept, record the plan, reset the backoff (or noteRateLimited);
    - unavailable: next attempt in 30 min;
    - rateLimited: retryAt = now + max(backoff(n), 300 s), status "Usage check paused (too many requests), retrying in N min";
    - failed: backoff(n), failed(reason).
9.10 usage-state.json (E/Services/Usage/UsageStateStore.swift). Written 0600, atomically, pretty, sorted keys, ISO 8601 whole seconds.
  ```
  {"version":1,"accounts":{"<identityId>":{"lastProbeAt"?,"failureCount":Int,"nextAttemptAt"?,
    "lastFullReading"?:{AccountUsage: accountId,fiveHour?{utilization,resetsAt?,duration},sevenDay?,
       scoped:[{name,window}],extraUsage?{isEnabled,monthlyLimit?,usedCredits?,utilization?,currency?},
       subscriptionType?,source,updatedAt,takenAfter?},
    "statusLines"?:[{"folder","key":"pid:<pid>@<start>","readings":{"lastReportAt","fiveHour"?:{"window","at","notBefore"?},"sevenDay"?}}]}}}
  ```
  - Restore (1425-1478):
    - a lastFullReading older than 8 days is dropped;
    - folder-keyed entries map to identities through registry.owner(ofSavedFolder:);
    - only `pid:…@…` status lines are restored, and only if younger than 7 days and the folder still belongs to the identity.
    - A file from a newer version is ignored.
  - The "within 1 s" rule in isRepeat exists because whole-second ISO dates round the reset times.
9.11 Ring reading (E/Public/ClaudeHostProjections.swift:395-425; E/Public/ClaudeSummaries.swift:420-510)
  - Status:
    - fetchState == notSignedIn → signInNeeded;
    - else windows → ok;
    - else unavailable / failed / waitingForFirstReading.
  - isStale: ok and older than the threshold, unless a window is exhausted (usedFraction ≥ 1, not yet reset, not money).
  - Bridge to a Codenotch ring (B/ClaudeUsageProvider.swift:91-126):
    - ok/failed-with-windows → ok or stale(since);
    - waiting → no windows, stale(distantPast);
    - signInNeeded/unavailable → unsupported(text);
    - failed without windows → throws apiError.
  - Push policy (B/ClaudeProviderSync.swift): at most one ingest per ring per 5 s, with a trailing push; a launch ring is kept 20 s while waiting for the hub; one ring per tracked account (274-277); push only on a change of windows, status, plan, name or reset credits (290-297).
  - Tauri mapping: W/usage.rs:130-157's LimitWindow{id,label,used 0-1,resets_at ms,group} and UsageSnapshot{status ok|stale|needsAuth|backoff|error, windows, fetched_at, note, backoff_until}.
    - Upstream draws one Claude cell with accounts as `group`s. The fork needs one ring per identity, so the UI must support N Claude providers. That is UI work.

====================================================================
10. get_usage PROBE (E/Services/Usage/UsageProbe.swift)
====================================================================
10.1 Command (48-56), with a `CLAUDE_CONFIG_DIR` of the account's raw value (removed for the default):
    `claude -p --input-format stream-json --output-format stream-json --verbose --no-session-persistence --strict-mcp-config --settings {"disableAllHooks":true}`
  - No model request and no transcript; hooks are off, so the probe never shows up as a session.
10.2 stdin: one JSON line each, keys sorted, "\n"-terminated (58-71).
  - First `{"request":{"subtype":"initialize"},"request_id":"agentnotch-init","type":"control_request"}`.
  - After the init response succeeds: `{"request":{"skip_behaviors":true,"subtype":"get_usage"},"request_id":"agentnotch-usage","type":"control_request"}`.
10.3 stdout parsing (106-137): only lines with `{"type":"control_response","response":{"subtype":"success"|…,"request_id":…,"response":{…}|"error":"…"}}` count.
  - An init error ends the probe.
  - A usage success goes through 8.2.
  - Error strings (141-150): "rate limit" or "429" → rateLimited; "not logged in", "please run /login" or "claude.ai" → unavailable("Not signed in to Claude"); else failed(first 200 characters).
  - Sample fixture (Tests/…/UsageProbeTests.swift:19): `{"type":"control_response","response":{"subtype":"success","request_id":"agentnotch-usage","response":{"subscription_type":"max","rate_limits_available":true,"rate_limits":{…},"behaviors":null}}}`.
10.4 Lifecycle (186-408)
  - Timeout 20 s.
  - Lines over 8 MiB fail the probe; the last 4 KiB of stderr are kept for "Claude Code exited (<code>): <last line>"; after an exit, 0.3 s grace for late stdout.
  - finish: close stdin; after 2 s terminate; 3 s later SIGKILL; SIGPIPE disabled on stdin.
10.5 Environment (77-91)
  - Remove CLAUDECODE, CLAUDE_PID, CLAUDE_EFFORT, AI_AGENT, CLAUDE_CONFIG_DIR, CLAUDE_CODE_*, CLAUDE_AGENT_SDK_*.
  - Set CLAUDE_CONFIG_DIR for a non-default folder.
  - PATH gains the binary's folder and its link-resolved folder, then the standard folders (E/Services/Hooks/ClaudeBinaryLocator.swift:255-275).
10.6 Binary (ClaudeBinaryLocator.swift:122-176) is looked up in this order:
  - the Settings choice;
  - the remembered path;
  - `~/.local/bin/claude`, `/opt/homebrew/bin`, `/usr/local/bin`, `<cfg>/local/claude` for each folder, `~/.bun/bin`, …;
  - finally the login shell's `command -v claude` (5 s, cooldown 24 h).
10.7 Windows mapping
  - Candidates, from W/usage.rs:261-282:
    - `%USERPROFILE%\.local\bin\claude.exe` (native installer);
    - `%APPDATA%\npm\claude.cmd`;
    - `%LOCALAPPDATA%\pnpm\claude.cmd`;
    - `%USERPROFILE%\.volta\bin\claude.exe`;
    - PATH `claude.exe` / `claude.cmd`.
  - Exclude Desktop-owned copies (W/usage.rs:255-258): paths containing `\AnthropicClaude\`, `\Claude\claude-code\` (e.g. `%APPDATA%\Claude\claude-code\<ver>\claude.exe`), or `\WindowsApps\`.
  - Spawn with CREATE_NO_WINDOW (0x08000000), piped stdio, cwd `<support>\usage-probe`.
  - Put the child in a Job object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, so a `.cmd` shim's node grandchild dies with it. Terminate = TerminateJobObject; there is no SIGTERM phase.
  - `.cmd` shims go through cmd.exe. Rust ≥ 1.77.2 escapes batch arguments (the BatBadBut fix) and errors when it cannot. Safer alternatives:
    - resolve the shim to `node.exe <prefix>\node_modules\@anthropic-ai\claude-code\cli.js`; or
    - write `{"disableAllHooks":true}` to `<support>\usage-probe\settings.json` and pass the path. `--settings` accepting a file path is UNVERIFIED on the current CLI; check `claude --help`.
  - Environment variable names are case-insensitive on Windows. Rust's Command env handling is case-insensitive there. Filter the prefixes case-insensitively.
  - Also strip CLAUDE_SECURESTORAGE_CONFIG_DIR (0.3).
  - Strip a trailing '\r' from stdout lines.
  - Get_usage is in the same JS bundle, so it should exist on Windows builds (UNVERIFIED; test it).

====================================================================
11. STATUS LINE PATH
====================================================================
11.1 The Mac wrapper is Packages/ClaudeControl/Scripts/agentnotch-statusline.py. It is installed as the account's `statusLine` command and chains to the previous command saved in `agentnotch-statusline.previous.json` `{"command": "..."}` beside it.
  - It refuses to chain to its own or former wrappers.
  - The previous command runs with shell=True in its own process group, with a 30 s timeout; its output and exit code pass through.
  - Message (71-100), sent fire-and-forget to the Unix socket (0.3 s timeout, socket ownership checked):
  ```
  {"event":"StatusLine","session_id","transcript_path","cwd","config_dir_env": $CLAUDE_CONFIG_DIR|null,
   "pid": int($CLAUDE_PID) in 1..2^31-1 | null,
   "status_line":{"rate_limits": <raw>,"context_window":{"used_percentage","context_window_size"},
                  "model": <raw {id,display_name}>,"cost":{"total_cost_usd"},"session_name","version"}}
  ```
  - Decoded by StatusLineMessage (E/Services/Hooks/HookEvent.swift:494-535). accountId = SessionFilter.accountId(transcriptPath, configDirEnv).
11.2 Windows mapping
  - A native exe, or a `statusline` subcommand of the hook exe, since Python can't be assumed.
  - Transport: upstream uses loopback HTTP on 127.0.0.1:48666 (W/../codenotch-hook/src/main.rs:6-64; port read from `%APPDATA%\codenotch\config.json`). Alternatively a per-user named pipe.
  - The previous command runs via the same shell Claude Code would use. Which shell Claude Code uses on Windows for hooks and status line (Git Bash vs cmd) is UNVERIFIED. The quoted `"C:\…\x.exe" arg` form upstream writes (W/hooks_install.rs:86) works in both.
  - Kill the previous command's tree with a Job object.
  - That Claude Code on Windows exports CLAUDE_PID and CLAUDE_CONFIG_DIR to hook and status-line children is UNVERIFIED; CLAUDE.md lists them for Mac/Linux 2.1.x. Without CLAUDE_PID, readings key by session (`session:<id>`) and restarts lose process identity.

====================================================================
12. CLAUDE DESKTOP USAGE CACHE
====================================================================
Sources/Providers/ClaudeDesktopUsageCache.swift + B/DesktopUsageSource.swift
12.1 Mac algorithm
  - Folder `~/Library/Application Support/Claude/Cache/Cache_Data` (92-95).
  - Candidates (187-220): non-hidden files, not recursive, name ending "_0" (the stream-0/1 file of a Simple Cache entry), regular, 24 < size ≤ 512 KiB; mtime descending; at most 400.
  - Per candidate (223-298):
    - open once and read 24 + 8192 bytes;
    - header: u64 LE magic `0xfcfb6d1ba7725c30` at offset 0; u32 LE key length at offset 12; header size 24, which includes 4 bytes of alignment padding;
    - key = UTF-8 bytes [24, 24+len), len ≤ 8 KiB;
    - usageOrganization(key) (393-404): the key must contain "claude.ai" or "anthropic.com"; take the text after "/api/organizations/" up to ? or #; it must split into exactly [org, "usage"]. Keys look like `1/0/https://claude.ai/api/organizations/<org>/usage?skip_spend=1`;
    - the org must equal the requested org. Then seek to 0, read ≤ 512 KiB, and re-parse, checking the org again;
    - modification time from fstat on the SAME descriptor.
  - Body at 24+keyLen:
    - must start with the zstd magic `28 b5 2f fd`;
    - ZSTD_findFrameCompressedSize; a declared content size must be ≤ 256 KiB;
    - decompress into a 256 KiB buffer (441-475);
    - trailer = the bytes after the frame. Find `\0date:` or `\0Date:`; the value runs to the next NUL; RFC 7231 IMF-fixdate `EEE, dd MMM yyyy HH:mm:ss zzz`, GMT (413-425).
  - requestsResetCredits = the query contains `cedar_ember=1`.
  - JSON is decoded as UsageResponse (Sources/Providers/ClaudeOAuthProvider.swift:601-718): snake_case, ISO dates with or without fractional seconds.
    - limitWindows: each `limits[]{kind, percent, resets_at (required), scope.model.display_name}` → window id = kind, label = model or kind wording, fraction = percent/100, duration 5 h for session, 7 d for weekly_*;
    - then `five_hour` / `seven_day` are merged in as session / weekly_all when missing. A just-rolled window drops out of `limits` but not the named fields.
    - The cedar_ember reset-credits block (Sources/Providers/ClaudeResetCredits.swift): `{eligible, ineligibleReason?, grants[{id,resetsLeft,startsAt,endsAt,paused}]}`.
  - capturedAt = the Date header ?? the file mtime ?? now.
  - Scan: the newest usable entry gives the windows. Keep scanning up to index 20 for the newest reset block (163-184); Desktop refreshes `…/usage` and `…/usage?skip_spend=1` alternately, so there is no caching of the "winning" file.
  - DesktopUsageSource (B/DesktopUsageSource.swift):
    - off when sealed or the setting is off;
    - rescans 5 min after a miss;
    - translate (72-77) → nil if ANY window's resetsAt ≤ now;
    - a window's label is dropped when it equals the standard label.
  - Engine contract: `ClaudeExternalUsageSource.reading(organizationUuid:now:) async -> {windows:[ClaudeRingReading.Window], observedAt}` (E/Public/ClaudeControlConfiguration.swift:248-260). It is keyed by the identity's organizationUuid (CloudKeys.organization, E/Services/Cloud/CloudModels.swift:550-561, skips corrected folders). Desktop is signed in to exactly one account, so the org match is what keeps another account's numbers off a ring.
12.2 Windows mapping
  - userData: `%APPDATA%\Claude`. For an MSIX package: `%LOCALAPPDATA%\Packages\<name containing claude/anthropic>\LocalCache\Roaming\Claude` (W/watcher.rs:84-101). Check both.
  - Cache: `<userData>\Cache\Cache_Data` (UNVERIFIED; the Electron/Chromium default).
  - HARD, UNVERIFIED: which disk-cache backend Chromium uses on Windows.
    - To my knowledge Chromium on Windows uses the "blockfile" backend (`index`, `data_0..data_3`, `f_XXXXXX`), not Simple Cache (`<16hex>_0`).
    - If Cache_Data holds `*_0` files, port the Mac reader as is.
    - If it holds data_N files, write a blockfile reader (the format below is from memory; check it against Chromium `net/disk_cache/blockfile/disk_format.h` and `addr.h`):
      - an EntryStore (256 bytes) in the block files: hash, next, rankings, reuse, refetch, state, creation_time u64, key_len i32, long_key addr, data_size[4], data_addr[4], flags, self_hash, then inline key[160];
      - a CacheAddr (u32): bit 31 = initialized; bits 28-30 = file type (0 = external `f_%06x`, 2 = 256 B blocks, 3 = 1 KiB, 4 = 4 KiB); bits 24-25 = block count − 1; bits 16-23 = file number; low 16 bits = start block;
      - block files have an 8 KiB header;
      - stream 0 = the pickled response headers (the same `\0date:` scan works); stream 1 = the body (zstd as received);
      - practical approach: scan the data_1..data_3 blocks for EntryStores whose key matches, then follow data_addr[1].
  - File sharing: Chromium may hold the files open. Open with FILE_SHARE_READ|WRITE|DELETE (Rust's default). If sharing violations still occur while Desktop runs, treat that as a miss. UNVERIFIED.
  - Read without memory-mapping; take the timestamp from the handle (`File::metadata()`).
  - zstd: the `zstd` crate (`zstd::bulk::Decompressor::decompress(src, 256*1024)`, with `zstd_safe::find_frame_compressed_size` for the trailer offset), or pure-Rust `ruzstd`. Keep the 256 KiB cap and the declared-size check.
  - The Mac vendors the C decoder as `CodenotchZstd` through a fork seam.

====================================================================
13. DESKTOP-HOSTED SESSIONS
====================================================================
E/Services/Session/DesktopHostedSessions.swift
  - Desktop-hosted entrypoints: `claude-desktop`, `claude-desktop-3p`, `local-agent`, or any entrypoint containing "desktop" (37-45).
  - The registry entry carries `hostSessionId` matching `^local_[0-9a-f-]{8,72}$` (49-55; lowercase hex only).
  - Record root: `~/Library/Application Support/Claude/claude-code-sessions/<accountUuid>/<organizationUuid>/<hostSessionId>.json` (58-60).
  - identity(hostSessionId, candidates, root) (73-98): candidates = identities with `uuid:` keys (account uuid from the key, org from CloudKeys.organization; ClaudeControlHub.swift:424-431).
    - A known org gives exact lookups (lowercase and the given spelling).
    - An unknown org lists the account's folder for UUID-named subfolders.
    - The result must be exactly one identity, else nil.
  - Link safety (112-133):
    - lstat root, account and organization must be real directories and the record a regular file, checked top-down;
    - a link anywhere means no record;
    - the record is NEVER opened.
  - Result (139-143): hosted and found → known(identity); hosted and not found → unsure(folder's bestGuess).
  - DesktopSessionAttributor caches a found answer until the candidates change; a miss is retried after 15 s (150-183).
  - Registry entry fields (E/Services/Session/SessionRegistryScanner.swift:29-110), from `<dir>/sessions/<pid>.json`, epoch-ms times: pid, sessionId, cwd, kind, entrypoint, name, nameSource, version, status, waitingFor, startedAt, updatedAt, statusUpdatedAt, procStart (`ps -o lstart` UTC), hostSessionId.
  - Windows mapping:
    - root `%APPDATA%\Claude\claude-code-sessions\…`, plus the MSIX variant (UNVERIFIED; the name comes from the Mac bundle);
    - "no link" = `symlink_metadata` and reject FILE_ATTRIBUTE_REPARSE_POINT (symlinks AND junctions);
    - NTFS is case-insensitive, so one spelling is enough;
    - that Claude Code writes the sessions registry on Windows at all, and that `procStart` is present, is UNVERIFIED. Liveness then falls back to startedAt versus the process creation time (SessionRegistryScanner.swift:456-468: live if creation ≤ startedAt + 5 s, or lstart within 2 s).

====================================================================
14. PROCESS-LEVEL HELPERS
====================================================================
14.1 CLAUDE_CONFIG_DIR of a running process (E/Services/Session/ProcessConfigDir.swift)
  - Mac: `sysctl KERN_PROCARGS2`, same uid only.
  - Parse: argc i32, exec path, NUL padding, argc arguments, then environment entries until an empty one.
  - Byte-compare `CLAUDE_CONFIG_DIR=`; extract only that value.
  - No environment → unreadable. Empty value → unset.
  - The buffer is zeroed with memset_s.
  - Cached per (pid, start time) (109-149).
  - Used by SessionRegistryScanner.attribute (387-419) only when a `sessions/` folder is shared through links (Parallel Profiles). Otherwise a folder's entries belong to it.
  - Windows:
    - on native Windows, sessions folders aren't shared, so this can be left at "unreadable";
    - if needed: `OpenProcess(PROCESS_QUERY_INFORMATION|PROCESS_VM_READ)`, `NtQueryInformationProcess(ProcessBasicInformation)` → PEB → ProcessParameters → Environment and EnvironmentSize, via `ReadProcessMemory`;
    - the block is UTF-16LE; match the name case-insensitively; SecureZeroMemory the buffer;
    - fails for elevated targets from a non-elevated app.
14.2 Process start time
  - ProcessInspector.startDate (Mac `proc_pidinfo` `pbi_start_tvsec`) is used for statusLineKey, the attribution timeline and pid-reuse checks.
  - Windows: `GetProcessTimes` creation FILETIME → Unix seconds as (ft − 116444736000000000) / 1e7.
  - ProcessID.valid: 1...Int32.max (E/Services/Hooks/HookEvent.swift:441-451).

====================================================================
15. WHAT HAPPENS TO PARALLEL PROFILES ON WINDOWS
====================================================================
  - Native: the classifier yields only run folders plus infrastructure names. ExtensionDetected is false, so these stay dormant: the mirror correction (staleMirror), prefersOwnFolders in the probe planner, the timeline and statusCandidates filtering. Keep the code; it is pure and tested.
  - The general UUID/email correction (uuidElsewhereAsOthers) still applies to hand-copied folders.
  - WSL (future):
    - the home is `\\wsl.localhost\<distro>\home\<u>`;
    - classification works over UNC paths;
    - symlink targets are Linux-absolute (`/home/u/.claude-shared/projects`) and need translating to UNC before comparison;
    - pids are Linux, so liveness and start times need `wsl.exe`/procfs, or `isAlive: false` as the inspector does (E/Public/ClaudeAccountInspection.swift:110);
    - hooks inside WSL must reach the Windows app (TCP on the host or a relay);
    - the probe must run inside WSL (`wsl.exe -d <distro> --cd … env CLAUDE_CONFIG_DIR=… claude …`).
    - All of this makes WSL a separate phase.

====================================================================
16. INSPECTOR AND TESTS WORTH PORTING
====================================================================
  - The read-only inspector (E/Public/ClaudeAccountInspection.swift; binary `agentnotch-inspect-accounts`) prints:
    - accounts, rings, run and store folders;
    - the freshest cached usage (reading only `accountUuid` and `fetchedAtMs` of cachedUsageUtilization);
    - the probe folder, install targets, infrastructure and ring moves.
    Port it as `agentnotch-inspect-accounts.exe` for checking a real Windows setup.
  - Test suites whose vectors to reuse (Packages/ClaudeControl/Tests/ClaudeControlTests/Engine/): AccountPathsTests, AccountRegistryTests, PP_LayoutTests, PP_IdentityTests, PP_UsageTests, PP_ProjectionTests, A2_AccountNamingTests, A3_RingIdentityTests, A3_RingReadingTests, A3_UsageStoreTests, UsageParserTests (it has the real get_usage and cachedUsageUtilization bodies), UsageProbeTests, DesktopHostedSessionsTests, Fix_ForgottenAccountTests, Fix_AccountOwnLabelTests.
  - Bridge: Tests/ForkSPM/RingNameTests.swift.
  - Sealed fixtures: E/Fixtures/SampleLayout.swift, SampleSessions.swift, SampleData.swift.

====================================================================
17. HARD PARTS AND RISKS (most severe first)
====================================================================
  1. The token-free rule versus upstream's Windows Claude provider (0.1): delete it, and add a Rust grep check to CI.
  2. The Claude Desktop cache format on Windows is probably Chromium blockfile, not Simple Cache, and may hit sharing violations (12.2). Verify on a real PC before building. It is the least certain source; the other three work without it.
  3. Spawning the probe: `.cmd` shim quoting, a hidden console, killing the whole tree through a Job object, stripping CLAUDE_SECURESTORAGE_CONFIG_DIR, and checking get_usage on the Windows CLI (10.7).
  4. Path identity: case-insensitivity, `\\?\`, 8.3 names, drive letters, `;` as the list separator, and stable hashing for `claude-dir-` ids (1.2).
  5. Claude Code behaviour on Windows that is UNVERIFIED:
    - hook and status-line environment (CLAUDE_PID, CLAUDE_CONFIG_DIR);
    - the shell used for hook and statusLine commands;
    - whether the sessions registry and `procStart` exist;
    - the `claude-code-sessions` path;
    - the MSIX userData path.
    Write a small Windows probe script or checklist and run it once on a real machine.
  6. PID reuse on Windows: pair every pid with its creation time (statusLineKey, liveness, the ProcessConfigDir cache).
  7. File I/O etiquette: never mmap files Claude Code or Chromium own; read with full share mode; use atomic temp+rename writes with retry; put machine-local state under %LOCALAPPDATA%.
  8. WSL / Parallel Profiles is out of scope for v1 (0.2, 15). Say in the UI that WSL sessions are not tracked.
