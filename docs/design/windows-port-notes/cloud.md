CLOUD SYNC (app side): porting spec for Rust/Tauri on Windows

Path abbreviations used below:
- P = Packages/ClaudeControl/Sources/ClaudeControl
- C = P/Engine/Services/Cloud
- T = Packages/ClaudeControl/Tests/ClaudeControlTests/Engine
- F = web/contract/fixtures

Swift line numbers were taken from HEAD (1d1fde8).

======================================================================
0. SCOPE, FILES AND WHAT DEPENDS ON WHAT
======================================================================

Source files:
- web/contract/README.md: the fixed API.
- F/:
  - config.json
  - me.json
  - sync-request.json
  - sync-response.json
  - error.json
  - keys.json
- C/:
  - CloudModels.swift: wire types, JSON, clamping, keys.
  - CloudAuth.swift: Supabase PKCE, refresh, session store.
  - CloudHTTP.swift: transport, API, website validation.
  - CloudFiles.swift: atomic 0600 writes, install secret, throttled state files.
  - CloudSync.swift: service, scheduling, pass, batcher, memory.
  - SessionLedger.swift: ledger, owners, backfill roots, CloudFolderLogins.
  - SessionTokenScanner.swift
  - UsageHistoryRecorder.swift
  - SessionSummarizer.swift: runner, excerpt, redaction, scrub, SessionSummaryStore.
  - CloudLiveEnvironment.swift: engine adapter.
- P/Engine/Public/ClaudeControlHub+Cloud.swift: public state, actions, ledger feed.
- Sources/ClaudeBridge/CloudWebAuthSession.swift: the browser step.
- P/UI/Settings/CloudSection.swift: UI and copy.
- P/Engine/Core/ClaudeControlSettings.swift: switches and device id.
- P/Engine/Core/DevFlags.swift:102: AGENTNOTCH_WEB_URL.
- app-config.json, and Scripts/spm-build-app.sh:112-161, :419-424.
- Tests:
  - T/CloudContractTests.swift
  - T/CloudContractE2ETests.swift
  - T/Cloud_TestSupport.swift
  - T/CloudAuthTests.swift
  - SessionSummarizerTests, SessionLedgerTests, SessionTokenScannerTests, UsageHistoryRecorderTests, CloudSync*Tests
- Scripts/cloud-contract-e2e.sh

What the cloud module needs from the rest of the engine. Other areas must provide these on Windows:
1. The account registry's visible, remembered, signed-in identities: identity id `uuid:<accountUuid>[/<org>]`, email, organizationName, planName, label, folders with organizationUuid, correctedFolders, and whether ~/.claude is mirrored.
2. Live sessions from the hook pipeline. Per session: sessionId, cwd, transcriptPath, configDir, entrypoint, createdAt, lastActivity, model, costUSD (status line), title, pidStartedAt. Attribution per session: certain, unsure or waiting.
3. Usage observations from UsageStore: probe, statusLine, claudeJson, desktop.
4. The 5-hour utilization per identity.
5. The claude binary locator and the usage-probe folder planner.

Recommended split:
- A platform-neutral crate `agentnotch-cloud`. No `windows` crate deps. Pure Rust with serde, sha2, hmac, base64, chrono, fancy-regex, unicode-segmentation, url, and a blocking or async HTTP client behind a trait.
- A thin Windows glue layer for ACLs, deep link, process spawning and device name.

The split lets the contract e2e (Docker and Postgres) run on ubuntu CI, and lets the tests read F/ directly.

======================================================================
1. THE WIRE CONTRACT (byte-level)
======================================================================

1.1 Endpoints (web/contract/README.md)

- The base is the website URL. Endpoints are `<website>/<path>`, joined as appendingPathComponent (CloudHTTP.swift:160). Example: https://example.com/app → https://example.com/app/api/app/v1/sync.
- Paths (CloudModels.swift:64-68):
  - "api/app/v1/config": GET, no auth.
  - "api/app/v1/me": GET, Bearer.
  - "api/app/v1/sync": POST, Bearer.
- Headers on every API request (CloudHTTP.swift:229-240):
  - `Accept: application/json`
  - `User-Agent: AgentNotch/<appVersion>`
  - with a body: `Content-Type: application/json`
  - when authed: `Authorization: Bearer <supabase access token>`
- Transport (CloudHTTP.swift:37-46): ephemeral; no cookies, no cache; request timeout 30 s, resource timeout 60 s.
  - Rust: a fresh client with no cookie store and no cache, 30 s timeout.
  - Refused (networkNotAllowed) when sealed or not bootstrapped (:50).
- Error body: `{"error":{"code","message"}}`. The codes are UNAUTHORIZED 401, FORBIDDEN 403, BAD_REQUEST 400, PAYLOAD_TOO_LARGE 413, RATE_LIMITED 429, and INTERNAL 500/503. A 503 means the website couldn't check the token right now; the session is kept.
- CloudAPIError.from (CloudHTTP.swift:107-111):
  - It decodes the body if it can.
  - Retry-After is parsed only as numeric seconds (TimeInterval(trimmed)). An HTTP-date is ignored.
  - isUnauthorized is status 401 or code UNAUTHORIZED.
  - isRateLimited is 429 or RATE_LIMITED.
- The website's side (web/src/server/app-api/rate-limit.ts):
  - sync is 12-burst per user, refilled at 1 per 10 s, shared by all of a user's devices;
  - 60 per minute per IP;
  - body cap 5 MiB (schema.ts LIMITS.bodyBytes).
  - The app stays under this with at most 5 requests per pass.

1.2 GET config (F/config.json)

- Fields: `{supabaseUrl, supabasePublishableKey, redirectUrl, dashboardUrl}`.
- The website's zod has `redirectUrl: z.literal("agentnotch://auth-callback")` (web/src/server/app-api/schema.ts APP_REDIRECT_URL, configResponseSchema).
- The app refuses sign-in unless `config.redirectUrl == "agentnotch://auth-callback"` (CloudSync.swift:982-984, error badConfig "redirectUrl is …").
- The website strips trailing slashes from supabaseUrl (handlers.ts buildConfig).

1.3 GET me (F/me.json)

- Shape: `{"user":{"id","email","name"|null},"dashboardUrl"}`.
- The app decodes email as optional (CloudModels.swift:148-157).
- It is used only right after sign-in, with failures ignored (`try?`, CloudSync.swift:995-999).

1.4 POST sync (F/sync-request.json, F/sync-response.json)

Request types are in CloudModels.swift:227-374. Encoding rules must be matched exactly:

- Top level: schemaVersion (int 1), device{id,name,appVersion}, accounts[], sessions[], usage[].
- Explicit null. These keys are always present and written as JSON null when absent (custom encode, CloudModels.swift:257-264, 326-341, 359-364):
  - accounts[].email / organizationName / plan / label
  - sessions[].title / endedAt / costUsd
  - usage[].windows[].resetsAt
  - The website's schemas require the keys.
- sessions[].summary is left out when there is none (`encodeIfPresent`, :340). Absent means "keep the stored summary". The website treats null like absent (schema.ts `.nullish()`).
- Keys are sorted (`.sortedKeys`) and slashes are not escaped (`.withoutEscapingSlashes`) (CloudModels.swift:113-121).
  - Rust: serialize through `serde_json::Value`, whose Map is a BTreeMap without the `preserve_order` feature, or order the struct fields alphabetically. serde_json never escapes '/'.
  - Non-ASCII goes out raw UTF-8 on both sides.
- Numbers:
  - Swift writes Double 42.0 as `42`; serde_json writes `42.0`. Both are valid for zod and Postgres, and the fixture comparison compares numerically.
  - This only matters if you hash encoded bytes (§5.5): the hashes are app-internal, so it is fine.
  - Integers (tokens) must be JSON integers up to 2^53-1.
- Dates are written as `YYYY-MM-DDTHH:MM:SS.mmmZ`, always 3 fraction digits, rounded to the nearest millisecond (CloudModels.swift:100-106).
  - Example: parsed ".482" re-encodes as ".482", not ".481".
  - Test vectors (T/CloudContractTests.swift:300-306):
    - "2026-09-25T08:02:11.482Z" round-trips unchanged;
    - "2026-09-25T09:47:03Z" encodes as "2026-09-25T09:47:03.000Z";
    - "yesterday" doesn't parse.
  - Reading accepts with or without fractional seconds, UTC `Z` only. The website refuses offsets.
- device.id:
  - A UUID made once and stored in settings key `claudeControl.cloudDeviceId`.
  - Swift writes it uppercase (UUID().uuidString, ClaudeControlSettings.swift:294-299). The website lowercases it (web/src/server/services/sync.ts:83).
  - It is regenerated if the stored value isn't a UUID.
- device.name: the Mac uses SCDynamicStoreCopyComputerName, else the hostname (CloudSync.swift:720-723).
- device.appVersion: CFBundleShortVersionString, "0" when unknown.
- Response 200: `{"accepted":{"sessions":int,"usage":int},"serverTime":date}`. The app decodes it and ignores the values beyond decoding.

1.5 Limits and clamping

CloudSyncRequest.clamped(now:) (CloudModels.swift:390-446) is applied to every request just before encoding (CloudHTTP.swift:204). Port it exactly.

- Lengths are in UTF-16 units, never splitting a Character (grapheme cluster), via clampedUTF16 (:452-463).
  - Rust: iterate `unicode_segmentation` graphemes and sum `encode_utf16().count()`.
  - Limits (:38-61):
    - deviceName 120, appVersion 40, email 320, organizationName 200, plan 60, label 80;
    - projectName 120, title 200, modelId 200;
    - summaryText 2000, summaryModel 80.
  - Counts: accounts 50, sessions 200, models 10, usage 500, windows 20.
  - Numbers:
    - messageCount clamped to 0…2,147,483,647;
    - tokens clamped to 0…9,007,199,254,740,991;
    - costUsd set to null when NaN, ∞, negative or ≥ 1e8.
- accounts: keep only those whose key isKey (64 lowercase hex, :480), then take the first 50.
- sessions: keep only those where all of these hold:
  - the accountKey is in the kept accounts;
  - project.key isKey;
  - sessionId isUUID (exactly 36 UTF-8 bytes and a parseable UUID in any case, :485). In Rust, enforce the 8-4-4-4-12 hyphenated form: `uuid::parse_str` also accepts simple, braced and urn forms.
  - startedAt, lastActivityAt and (if present) endedAt are accepted.
- Accepted date: ≥ 2023-01-01T00:00:00Z (epoch 1_672_531_200) and ≤ now + 86_400 s (:71-82).
- Then the first 200 sessions. Per session:
  - project.name and title are clamped;
  - models are clamped to 200 units, empty ones dropped, de-duplicated preserving order, first 10 kept;
  - messageCount, tokens and costUsd are clamped as above;
  - the summary text is clamped to 2000 and its model to 80; the summary is dropped (session kept) if generatedAt isn't accepted;
  - if lastActivityAt < startedAt, lastActivityAt = startedAt;
  - if endedAt < lastActivityAt, endedAt = lastActivityAt.
- usage: keep readings whose accountKey is known and whose observedAt is accepted; take the first 500. Per reading:
  - drop windows whose utilization isn't finite or whose id isn't isWindowID;
  - set utilization to max(0, u);
  - set resetsAt to null unless 2023-01-01 ≤ resetsAt ≤ now + 32 days (:84-88);
  - take the first 20 windows;
  - drop the reading if no window is left.
- isWindowID (:493-501): true for exactly "session", "weekly_all" or "extra_usage". Otherwise the id must be "weekly_" followed by 1 to 57 characters: first [a-z0-9], the rest [a-z0-9_.-].
  - This is stricter than the website's `weekly_[a-z0-9._-]{1,60}`.
- contractWindowID (:505-511): lowercase; as-is if valid; for a "weekly_" id, keep only the first 57 characters after the prefix; nil otherwise.
- scopedID(forModel:) (P/Engine/Services/Usage/UsageRingWindows.swift:90-104): "weekly_" + a slug of the lowercased model name. Runs of non-ASCII-alphanumerics fold to "_", with no leading "_", trailing "_" trimmed, and "scoped" when empty.
- CloudSessionSource.from(entrypoint:) (CloudModels.swift:205-215), after trim and lowercase:
  - "" or nil → other;
  - "cli" → cli;
  - contains "vscode" → vscode;
  - contains "desktop", or the entrypoint is desktop-hosted (`claude-desktop`, `claude-desktop-3p`, `local-agent`) → desktop;
  - starts with "sdk" → sdk;
  - otherwise other.

1.6 Keys (CloudModels.swift:469-592; F/keys.json)

- accountKey = lowercase hex SHA-256 of the lowercased text. The text is `<accountUuid>/<organizationUuid>`, both trimmed of spaces, or `<accountUuid>` alone when the organization is nil or blank (:522-531).
  - The separator "/" is AccountIdentityGrouping.organizationSeparator (P/Engine/Services/Accounts/AccountIdentities.swift:247).
  - Fixture vectors (F/keys.json):
    - 3f1f0a3e-8a7b-4c1d-9e2f-5a6b7c8d9e0f, no org → 8ca65b0d…94c0;
    - 9d2c7b1a-0000-4e5f-8a9b-1c2d3e4f5a6b / 7a6b5c4d-3e2f-4a1b-9c8d-0e1f2a3b4c5d → d8485d82…8874.
  - Upper-casing the inputs yields the same key.
- accountKey(identity:) (:540-561): only for `uuid:` identity ids (accountUuid(ofKey:) strips "uuid:" and anything after "/"). The organization is chosen in this order:
  1. identity.organizationScope, the split org;
  2. if the identity has no folders, identity.organizationUuid;
  3. otherwise the first non-corrected folder's organizationUuid (trimmed, lowercased).
  - A mirrored (corrected) folder's stale org is never used.
  - email: and dir: identities give nil and are never sent.
- project.key = lowercase hex HMAC-SHA256. The key is the 32-byte install secret; the message is UTF-8 `<accountKey>:<projectPath>` (:566-569).
  - Fixture: installSecretHex "5f"×32:
    - "/Users/me/code/agentnotch" with key A → b89e2468…02b7;
    - "/Users/me/work/billing-service" with key B → c11c0b20…14e0.
  - It is never equal to a plain sha256 (a test checks this).
- projectPath(forCwd:home:) (:573-584):
  - "~" or "~/…" expanded against home;
  - URL.resolvingSymlinksInPath;
  - standardizingPath;
  - trailing "/" stripped (except for root).
- projectName(forCwd:) (:587-592): trailing "/" stripped, then the last path component. If that is empty, the path itself.
- CloudBackfill.login (SessionLedger.swift:752-757) = sha256hex of `clean(accountUuid)|clean(orgUuid)|clean(email)`, where clean is trim then lowercase and nil becomes "". It is nil when both accountUuid and email are empty. Local only.

WINDOWS MAPPING for keys:
- accountKey and its test vectors are platform-independent. Reuse F/keys.json unchanged.
- projectPath must be defined for Windows. Stability within one install is all that matters, since the secret is per install, but it must be stable across runs and sources. Rules:
  - expand "~", "~\" and "~/" against %USERPROFILE%;
  - `dunce::canonicalize` (GetFinalPathNameByHandleW), which resolves symlinks and junctions, returns the on-disk case and strips `\\?\`;
  - if it fails (the folder is gone), normalize lexically: "/"→"\", upper-case the drive letter, collapse "." and "..", strip trailing "\" except in "C:\".
  - Never lowercase the whole path: canonical case already unifies `c:\code` and `C:\Code`.
- The Windows install's keys will differ from the Mac's for the same folder. That is by design: the secret is per install.
- projectName: split on both "\" and "/"; for a drive root, use "C:".
- The fixture HMAC vectors test only the HMAC. Also add Windows projectPath unit tests: junction, case, trailing slash, `\\?\`.

======================================================================
2. SIGN-IN (Supabase Auth with Google, PKCE), SESSION AND TOKENS
======================================================================

2.1 Flow (CloudAuth.swift:5-35; CloudSync.signIn :961-1026)

The flow:
1. Precondition: effectiveWebsite is non-nil, else lastError = "This build has no website to sign in to." A second call while signingIn returns false. authGeneration is bumped, authState = signingIn, lastError = nil.
2. GET config (no auth) and check that redirectUrl equals the constant.
3. Validate config: supabaseUrl passes validatedLink (https, or http to localhost/127.0.0.1/::1/[::1]; no user or password; path kept), else badConfig("supabaseUrl"). The publishable key must be non-empty.
4. Build the PKCE pair:
   - verifier = base64url with no padding of 32 CSPRNG bytes (43 characters) (:164-176);
   - challenge = base64url(SHA-256(ASCII verifier)) with no padding (:179-181).
   - RFC 7636 App. B vector (T/CloudAuthTests.swift:27-33): verifier "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk" → challenge "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".
5. authorize URL = `<supabaseUrl>/auth/v1/authorize?provider=google&redirect_to=agentnotch://auth-callback&code_challenge=<c>&code_challenge_method=s256` (:266-277). Percent-encoding of redirect_to is not significant.
6. Browser step (host): returns the callback URL.
   - The user cancelling it is CloudAuthError.cancelled, a quiet return to signedOut with no error shown (:1015-1016).
   - Other errors become provider(localizedDescription).
7. authorizationCode(fromCallback:) (:280-309):
   - the scheme must be "agentnotch" and the host "auth-callback", compared case-insensitively, else invalidCallback;
   - pairs are parsed by hand from the percent-encoded query and then the fragment: split on "&", split once on "=", "+"→space, percent-decode, and a bad escape is kept literally;
   - the first non-empty `error` or `error_code` → provider(error_description ?? error, first 300 characters);
   - else the first non-empty `code`, else invalidCallback.
   - Vectors (T/CloudAuthTests.swift:54-74):
     - `agentnotch://auth-callback?code=abc-123` → "abc-123";
     - `#code=frag` works;
     - `#error=access_denied&error_description=Email+link+is+invalid+or+has+expired` → provider("Email link is invalid or has expired");
     - `https://evil.example.com/auth-callback?code=x` → invalid;
     - `?x=%zz&code=ok` → "ok";
     - formPairs("a=%zz&b=two+words&c") values → ["%zz","two words",""].
8. Code exchange: `POST <supabase>/auth/v1/token?grant_type=pkce` with headers `Content-Type: application/json` and `apikey: <publishable>`, no Authorization. Body `{"auth_code":code,"code_verifier":verifier}` (:333-335, 532-558).
9. session(from:) (:561-586):
   - access_token and refresh_token must both be non-empty, else badResponse("no tokens");
   - expiresAt = expires_at (epoch seconds) if > 0, else now + expires_in, else now + 3600;
   - userId = user.id and email = user.email.
10. After the exchange, check isCurrent: still started, the same authGeneration and the same effectiveWebsite. If not, revoke the new session (best-effort logout) and return false (:986-990).
11. adopt: epoch++, save, use (:341-352). A save failure is only logged: signed in for this run only.
12. GET me (errors ignored): email, userId and dashboardUrl override the token's values.
13. isCurrent again. Then:
    - dashboardURL = validatedLink(dashboard);
    - memory.adopt(userId, website, dashboardUrl), which resets cloud-sync-state.json when the user or website changed (CloudSync.swift:173-183);
    - turnSwitchesOff(): consent is per sign-in, so both switches go off;
    - authState = signedIn(email), nextSyncAt = nil, notBefore = nil, failures = 0.
14. On error (if still current): cancelled → signedOut; any other error → authState = .error(message) and lastError = message.

Token-request error parsing (:549-555):
- message = error_description | msg | message | error;
- code = error_code | error;
- the result is CloudAuthError.http(status, code, message).

2.2 Refresh (CloudAuth.swift:370-485)

- refreshMargin is 60 s: a token with ≤ 60 s left is refreshed first.
- `POST <supabase>/auth/v1/token?grant_type=refresh_token`, body `{"refresh_token":…}`, header apikey.
- Single-flight: concurrent callers share one refresh task, because Supabase rotates the refresh token (:442-450).
- refusesRefreshToken (:406-412) is true only when both hold:
  - the status is 400 or 401;
  - the code is one of invalid_grant, refresh_token_not_found, refresh_token_already_used, session_not_found, session_expired, user_not_found or user_banned, OR the message contains "invalid refresh token".
- On a refusal: forget (delete the file) if the session is still the same refresh token, and throw signedOut.
- Anything else (5xx, 429, network) keeps the session and throws.
- If the session changed while the request was out, throw signedOut.
- The new session keeps the old userId and email when the answer lacks them.
- It is SAVED BEFORE USE (:477-483).
- If validatedLink(supabaseUrl) fails for the stored session: forget and throw signedOut.

2.3 Binding a token to its website and sign-in (:386-438)

- accessGrant(for website): no session → signedOut; session.websiteURL != website → otherWebsite. Returns {token, website, signIn: epoch}.
- refreshAfterUnauthorized(failedToken, website, signIn) (:421-430): checks the website and the epoch. If the current token differs from the failed one and has > 60 s left, use it; otherwise refresh, then check again.
- The epoch (signInEpoch) is bumped on adopt, setAside and forget, never on refresh.
- CloudAPI.authorized (CloudHTTP.swift:212-227): grant, send; on 401, one retry with refreshAfterUnauthorized; a non-2xx is then CloudAPIError.server.
- A 401 after a good refresh keeps the session (see §5.8).

2.4 Sign-out and set-aside

- signOut (:489-497): forget first (epoch++, file deleted), then best-effort `POST <supabase>/auth/v1/logout?scope=local` with headers `apikey` and `Authorization: Bearer <access>`. The answer is ignored.
- CloudSync.signOut (CloudSync.swift:1030-1038): authGeneration++, turnSwitchesOff(), auth.signOut(), signedOut, dashboardURL = nil.
- setAside (:516-520): stop using the session without deleting the file. Used when the saved session's websiteURL ≠ the effective website (restoreSession, CloudSync.swift:830-848).
  - The switches are NOT turned off there, but canUpload needs signedIn, and the next sign-in turns them off anyway.

2.5 Session file cloud-session.json (CloudAuth.swift:59-71, 90-135)

- A JSON object, encoded with CloudJSON (sorted keys, ms dates): accessToken, refreshToken, expiresAt, userId?, email?, supabaseUrl, publishableKey, websiteURL.
- Swift's synthesized Codable omits nil optionals.
- A decode failure is treated as no session.
- Read and write are gated: nothing happens when sealed or before bootstrap (a save throws notAllowed, load returns nil).
- Written atomically at 0600 (§4.1). clear() deletes it.
- websiteURL is the website's canonical string (§3.1). String equality decides "same website", so the Rust canonicalizer must be deterministic.

2.6 The Mac browser step (Sources/ClaudeBridge/CloudWebAuthSession.swift:26-104)

- ASWebAuthenticationSession with callback `.customScheme("agentnotch")`.
- prefersEphemeralWebBrowserSession = false, so the browser's Google login is reused.
- Presented over the Settings window.
- Nothing is registered with Launch Services, so no other app can deliver the callback.
- A user close or task cancel → CancellationError, a quiet cancel.
- Failure to start → "The sign-in window couldn't open."
- A nil callback → "The sign-in ended without an answer from the website."
- A sealed run throws CancellationError without opening anything.

WINDOWS MAPPING, sign-in (the hard part)

Option A: custom scheme. This is the direct port and needs NO website or Supabase change. `agentnotch://auth-callback` is already in Supabase's Redirect URLs (web/README.md:150-160), and the contract literal stays.
- Add `tauri-plugin-deep-link = "2"`, with config `"plugins": {"deep-link": {"desktop": {"schemes": ["agentnotch"]}}}`.
- The Tauri NSIS bundle registers the scheme at install: HKCU\Software\Classes\agentnotch with "URL Protocol" and shell\open\command `"…\Agent Notch.exe" "%1"`. It is HKLM for a per-machine install.
- Dev and portable runs: call `app.deep_link().register_all()` at startup. Windows and Linux only.
- Upstream already uses `tauri-plugin-single-instance = "2"` (windows/codenotch/Cargo.toml; main.rs:1792). Enable its `deep-link` feature and register single-instance FIRST.
  - Windows starts a NEW process with the URL in argv[1]. The plugin forwards it to the running instance, where `app.deep_link().on_open_url(...)` fires.
  - The existing single-instance callback (settings_window::open) must not also pop Settings for a deep-link launch; it can inspect args for "agentnotch://".
- Pending-flow gate: keep {verifier, website, authGeneration, started_at} in memory only, never on disk.
  - Accept a callback only while a sign-in is pending, and only once.
  - A cold-start callback (the app had quit) or a stray URL is ignored, optionally with the note "Sign-in expired; try again".
  - Validate it with authorizationCode exactly as on the Mac.
  - Tolerate `agentnotch://auth-callback/?code=…`: the host is still auth-callback. `url::Url` gives `host_str()=="auth-callback"` for this non-special scheme.
- Security. Another app or web page can register or trigger agentnotch:// (the last HKCU writer wins). PKCE makes an intercepted or injected code useless: the exchange needs this process's verifier, and a login-CSRF code minted with another challenge fails the exchange. Keep the one-shot pending gate anyway.
- Cancellation: the Mac learns when the sheet closes; Windows cannot tell when a browser tab closes. Needed:
  - a "Cancel" button while signingIn;
  - a timeout (about 10 min) resolving to a quiet cancel (signedOut, no error);
  - "Sign in" clicked while pending cancels the old flow and restarts.
  - Without these the Mac guard `if case .signingIn { return false }` would leave the user stuck.
- Opening the browser: `tauri-plugin-opener` / ShellExecuteW to the default browser. This is the equivalent of non-ephemeral: it reuses the Google session. Refuse when sealed.
- UX: the browser shows "Open Agent Notch?" (Chrome and Edge let the user tick "always allow"). The tab stays on Supabase or blank afterwards. After the callback, focus the Settings window.

Option B: loopback (RFC 8252). This needs website changes.
- The redirect is `http://127.0.0.1:<port>/auth-callback`.
- The contract fixes config.redirectUrl as a zod literal, and the app checks equality. Choose one:
  - bump the contract: add e.g. `loopbackRedirect` or allow a list, changing both sides plus fixtures; or
  - keep the check on config.redirectUrl and use the loopback URL only as redirect_to.
- Supabase: add the loopback URL to Authentication > URL Configuration > Redirect URLs.
  - A fixed port gives an exact entry.
  - For an ephemeral port, a glob like `http://127.0.0.1:*/auth-callback`. Supabase's glob wildcard rules for ports need checking before relying on this.
- The Google console needs no change: Google redirects to Supabase's own /auth/v1/callback.
- Upstream already runs tiny_http on 127.0.0.1:48666 (config.rs:229, server.rs:9-14). Its CSRF guard rejects `Sec-Fetch-Site: cross-site` (server.rs:97-127), and the Supabase→loopback hop is a cross-site top-level navigation. The auth route must bypass that guard.
- Pros: works when the scheme isn't registered; can serve a "you can close this tab" page; no scheme hijack at all.
- Recommendation: A as primary, with no website change. B only if the website adds the redirect.

======================================================================
3. THE WEBSITE ADDRESS AND app-config.json (commit 1d1fde8)
======================================================================

3.1 Address rule, CloudWebsite.validated (CloudHTTP.swift:122-142)

- Trim; empty → nil.
- Without "://", prefix "https://".
- Parse; the scheme is lowercased; the host is lowercased and must be non-empty; no user, password, query or fragment.
- The scheme must be https, or http with host in {localhost, 127.0.0.1, ::1, [::1]}.
- Strip trailing "/" from the path.
- The result string is `https://agentnotch.example.com` with NO trailing slash.
- Vectors (T/CloudContractTests.swift:368-382):
  - "https://agentnotch.example.com/" → "https://agentnotch.example.com";
  - bare host → https;
  - "HTTPS://Example.com/app/" → "https://example.com/app";
  - http://localhost:3000 and http://127.0.0.1:3000 are OK;
  - rejected: http://example.com, ftp://, ?next=x, user:pw@, blank, nil.
- RUST PITFALL: `url::Url` normalizes an empty path to "/" and drops a default port. Build and keep your own canonical string: scheme + "://" + lowercased host + (":" + port as typed) + path without the trailing slash.
- validatedLink (:146-157) is the same scheme and host rule with the path kept as given. It is used for supabaseUrl and dashboardUrl.

3.2 Where the website comes from

- app-config.json at the repo root is `{"websiteURL": "https://agentnotch.rivant.in"}`.
- The build (Scripts/spm-build-app.sh:112-161) validates it in Python:
  - a JSON object with a string websiteURL;
  - "" → a build with no website;
  - no whitespace;
  - scheme https, or http only to localhost/127.0.0.1/::1;
  - no user or password; no trailing colon without a port; no ? or #;
  - host labels match `[a-z0-9](?:[a-z0-9-]*[a-z0-9])?`;
  - path matches `(?:/[A-Za-z0-9._~!$&*+,;=:@%-]+)*`;
  - the URL must equal its canonical form.
  - A failure stops the build.
- The build writes it into Info.plist as `AgentNotchWebsiteURL` (:419-424).
- The host reads it with ClaudeControlConfiguration.websiteURL(infoDictionary:) (ClaudeControlConfiguration.swift:98-106; bridge ClaudeBridge.swift:203).
- Runtime override: AGENTNOTCH_WEB_URL, validated (DevFlags.swift:102).
- effectiveWebsite = override if valid, else the build's if valid, else nil (CloudSync.swift:860-864). A sealed run ignores both (Dependencies.live :705-706).
- The website is fixed for the run; the user can't change it. The UI shows it read-only with "Set by AGENTNOTCH_WEB_URL for this run." when overridden.
- retireTypedWebsite (:874-885) handles the old Mac setting `claudeControl.cloudWebsiteURL`: delete it, and if it named a different website, turn the switches off. Windows has no legacy, so skip it.
- The release workflow reads app-config.json for the website job (.github/workflows/release.yml:391-434) with jq.
- A Rust test mirrors CloudSyncTests.theRepositorysAppConfigNamesAWebsiteTheAppAccepts.

WINDOWS:
- In windows/<crate>/build.rs (or the CI step), read `../../app-config.json`, apply the same validation (port the Python rules), and emit `cargo:rustc-env=AGENTNOTCH_WEBSITE_URL=<url or empty>`, plus `cargo:rerun-if-changed=../../app-config.json`.
- Runtime: `option_env!`, then the AGENTNOTCH_WEB_URL override, then CloudWebsite::validated.

======================================================================
4. LOCAL STATE: FILES, PERMISSIONS, ATOMICITY
======================================================================

4.1 Mac (CloudFiles.swift)

- writeAtomically (:28-66):
  - create the folder at 0700 if it is missing;
  - temp file `.<name>.<UUID>.tmp` in the same folder, opened O_WRONLY|O_CREAT|O_EXCL at 0600, then fchmod 0600 (the umask would otherwise filter it);
  - full write with EINTR retry, fsync, close, rename over the target;
  - on failure, unlink the temp.
- createExclusively (:79-92): write a temp atomically, then link() it to the target. EEXIST → false. The temp is always unlinked.
- CloudInstallSecret (:102-140):
  - file `cloud-install-secret`, raw 32 bytes (not hex);
  - load: if the file exists with 32 bytes, use it;
  - if it exists with the wrong size, overwrite with new random bytes (the keys change);
  - otherwise createExclusively, and if another run won, read theirs;
  - a failure → nil, and CloudStores then uses a random in-memory secret for the run.
  - Made LAZILY, only when a sync pass needs a project key (signed in, sync on), never at launch (CloudSync.swift:266-279).
- CloudStateFile (:146-221): JSON via CloudJSON.
  - load: nil on a missing or bad file; a bad file is logged and treated as starting fresh.
  - save: background write, throttled to at most one per writeDelay, always the newest value.
  - saveNow: synchronous, used on quit.
  - persists=false (sealed, tests) never touches disk.
- The support folder is `~/Library/Application Support/Agent Notch/Claude` (0700, AppIdentity.swift:101-125), overridable with AGENTNOTCH_SUPPORT_DIR.

4.2 The files (all in <support>). Each has a version field, and a mismatch starts fresh.

- cloud-session.json: §2.5.
- cloud-install-secret: 32 raw bytes.
- cloud-ledger.json: SessionLedger.Contents v2 (SessionLedger.swift:226-241).
  - sessions: map from "sessionId|accountKey" to CloudLedgerEntry. Entry fields: sessionId, identityId, accountKey, projectName, projectPath, transcriptPath?, configDir?, source, startedAt, lastActivityAt, endedAt?, model?, costUsd?, title?, origin "live"|"backfill".
  - accounts: accountKey → {identityId, email?, organizationName?, plan?, label?}.
  - owners: sessionId → [{from?: date, accountKey}].
  - unattributed?: sessionId → date.
  - writeDelay 10 s; capacity 10,000 entries (the oldest by lastActivity are evicted).
- cloud-scan-state.json: SessionTokenScanner.State v3 (SessionTokenScanner.swift:128-181). files: map from real path to FileState {sessionId, size, mtime (f64 s), inode, offset, owners, parts{owner→{totals{input,output,cacheCreation,cacheRead}, responses, modelCounts, first?, last?}}, first?, last?, cwd?, entrypoint?, customTitle?, aiTitle?, summaryTitle?, recent[≤32 {key,model,usage,owner}], claimed[16-hex], isMain}.
- cloud-usage-outbox.json: v1 {pending[RecordedUsageReading], last{"accountKey|source"→reading}}. Capacity 5,000.
- cloud-summaries.json: v2 {summaries{entryKey→{text,model,generatedAt,messageCount,costUsd?}}, attempts{entryKey→{failures,nextAttemptAt,reason}}, runs[date], enabledAt?}.
- cloud-sync-state.json: CloudSyncMemory v2 (CloudSync.swift:129-151) {userId?, website?, sessions{entryKey→{base: sha256hex, summary?: sha256hex, ended, transcript?{bytes,modified}}}, lastSyncAt?, dashboardUrl?}.
- cloud-folder-logins.json: v1 {folders{normalizedConfigDir→{login: sha256hex, since}}}. Kept whether or not sync is on.
- session-summary/: an empty working folder (0700) for the summarizer.

WINDOWS MAPPING for storage:
- Location: `%LOCALAPPDATA%\Agent Notch\Claude\` (FOLDERID_LocalAppData), NOT %APPDATA% (Roaming). Reasons:
  - a roaming profile would copy the refresh token, the install secret and the device id (two PCs would share a device id) to the domain server and other machines;
  - the ledger and scanner hold machine-local paths.
  - Upstream's config lives at `dirs::config_dir()/codenotch` = %APPDATA% (windows/codenotch/src/config.rs:266-270). Keep the cloud switches and the device id in the LOCAL cloud folder, not in a roaming settings file.
  - Keep the AGENTNOTCH_SUPPORT_DIR override.
- 0700 folder / 0600 file equivalent: a protected DACL, so nothing is inherited from above.
  - Create the Claude folder with SDDL `D:P(A;OICI;FA;;;<current user SID>)(A;OICI;FA;;;SY)`. Get the SID with OpenProcessToken + GetTokenInformation(TokenUser). Apply via ConvertStringSecurityDescriptorToSecurityDescriptorW, then CreateDirectoryW(SECURITY_ATTRIBUTES), or SetNamedSecurityInfoW with PROTECTED_DACL_SECURITY_INFORMATION on an existing folder.
  - Files then inherit that ACL.
  - For defense in depth, create each temp file with CreateFileW(CREATE_NEW, SECURITY_ATTRIBUTES with the same DACL) (the O_EXCL equivalent). That leaves no window in which the file is readable by others.
  - Leave out Administrators to match 0600, or keep them: admins can take ownership anyway.
  - The default %LOCALAPPDATA% ACL already limits access to the user, SYSTEM and Administrators.
- Optional hardening: DPAPI (CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN) around cloud-session.json and the secret. This is not a credential store and not the Keychain analog; the rule "never in the Keychain" maps to "never in Credential Manager". If you use it, keep a plain-JSON path for tests.
- Atomic replace:
  - write temp, then FlushFileBuffers (File::sync_all);
  - then MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH). std::fs::rename uses MoveFileExW with REPLACE_EXISTING, or use ReplaceFileW.
  - Antivirus and indexers briefly hold handles: retry about 5 times over about 500 ms on ERROR_ACCESS_DENIED or ERROR_SHARING_VIOLATION.
  - createExclusively: `MoveFileExW(temp, target, 0)` (no replace) fails with ERROR_ALREADY_EXISTS atomically on the same volume. `std::fs::hard_link` (CreateHardLinkW) also works on NTFS.
- A tests check: after the write only the file remains in the folder; the permissions check becomes an ACL check (owner SID, protected, no Everyone or Users ACE).

======================================================================
5. THE SYNC SERVICE (CloudSync.swift)
======================================================================

5.1 Consent and state machine

Inputs:
- Settings (ClaudeControlSettings.swift:45-48, 283-299):
  - `claudeControl.cloudSyncEnabled` (default false);
  - `claudeControl.cloudSummariesEnabled` (default false);
  - `claudeControl.cloudDeviceId`.
- The flags:
  - canUpload (:1121-1125) = started && !sealed && syncEnabled && effectiveWebsite != nil && authState is signedIn.
  - isCapturing = canUpload. Sessions are captured and readings recorded ONLY while it holds.
  - canSummarize (:1128-1132) = canUpload && summariesEnabled && summariesAllowed() && !environment.isLaunchingClaude (a usage probe is running) && the summaries pause has ended.
  - summariesAllowedByDefault (:715-717) = bootstrapped && !sealed && !DevFlags.probesDisabled && !DevFlags.installsDisabled. This means never in a `--no-install` dev run.

The switches:
- setSyncEnabled(true) (:887-897): nextSyncAt = nil and failures = 0. notBefore is NOT reset.
- setSyncEnabled(false) → stopSyncing():
  - syncGeneration++, so a pass in flight sends nothing more;
  - stopSummarizing: summaryGeneration++ and cancel the task, which kills the child;
  - ledger.forgetRunState();
  - lastLiveIDs = []; liveObservedSinceResume = false;
  - recorder.clear(): the readings waiting are deleted;
  - pendingSessions = 0.
- setSummariesEnabled(true) → summaries.noteEnabled(at: now). Only sessions that END after this are summarised.
- setSummariesEnabled(false) → stopSummarizing() and forgetUnsentSummaries(). The latter deletes stored summaries whose hash isn't the one remembered as sent for that entry (:923-929). Summaries already sent stay on the website.
- turnSwitchesOff (:913-918) = both switches false, stopSyncing(), forgetUnsentSummaries(). It is called by:
  - every successful sign-in;
  - sign-out;
  - a refusal of the refresh token or otherWebsite during sync;
  - a website change (on the Mac, the retire logic).
- start() (:777-812):
  - sealed → state = sealedFixture and return;
  - otherwise create the stores;
  - if summaries are on without an enabledAt, noteEnabled(now);
  - CloudAuth; the live environment; dashboardURL from memory;
  - subscribe to usage observations;
  - publish; restoreSession(); a tick loop every 20 s.
- stop() (:814-824): cancel the tick and the summary, then saveNow() on every store.

5.2 Tick, every 20 s (:1088-1107)

1. Always, even with sync off: `folderLogins.observe(environment.folderLogins(), now)`. The environment returns nil until the registry has read its folders.
2. If capturing and liveObservedSinceResume: `ledger.settle(liveIDs: lastLiveIDs, now)`; any ended → syncSoon().
3. If canSummarize and no summary is running: spawn summarizeNext() beside the schedule.
4. If canUpload, !isSyncing, nextSyncAt ≤ now (nil counts as due) and notBefore ≤ now: syncNow().

5.3 Timing constants (:650-663)

- syncInterval 300 s; tick 20 s; soonDelay 30 s (after a session ends or a summary is written).
- initialBackoff 30 s, doubling (exponent capped at 16), max 1800 s: backoff(f) = min(30·2^min(f-1,16), 1800) (:1260-1263).
- backfillInterval 6 h (the first pass after start always backfills).
- summaryPause 30 min; noFolderWait 60 min; summaryUsageCeiling 80 %.
- CloudSyncPass (:299-306): backfillOpenWindow 30 min; backfillFilesPerPass 200; maxRequestsPerPass 5.
- syncSoon (:1113-1118): soon = max(now+30 s, notBefore). Keep the existing nextSyncAt if it is ≤ soon.

5.4 syncNow (:1143-1217)

1. Guard canUpload, !isSyncing and the website; isSyncing = true.
2. Capture generation = syncGeneration and passSignIn = authGeneration. isBound() means the same authGeneration and the same effective website string.
3. Backfill folders: if due (6 h), environment.backfillFolders(), each dated with folderLogins.since(folder, login); lastBackfillAt = now.
4. Build the input: accounts; includeSummaries = summariesEnabled; device {cloudDeviceId, deviceName, appVersion}; now; home; maxSessionsPerRequest.
5. CloudSyncPass.prepare off the main actor (§5.5).
6. If !isBound → return. pendingSessions = prepared.sessionCount.
7. For each batch:
   - stop unless generation, bound and canUpload still hold;
   - if summaries are now off, batch.withoutSummaries();
   - api.sync;
   - on success, if bound: memory.markSent(records, at: now), recorder.markSent(readings), pendingSessions −= records.count;
   - on error, if bound: handleSyncFailure and return; if not bound, drop the error.
8. After the loop, if still the same generation and bound: memory.noteSynced(now), failures = 0, notBefore = nil, lastError = nil, nextSyncAt = now + (hasMore ? 30 s : 300 s).
9. "Sync now" (the user) calls syncNow directly, so it ignores notBefore and nextSyncAt.

5.5 CloudSyncPass.prepare (:348-396)

- allowed = accounts by accountKey.
- If backfilling: scanner.pruneMissingFiles(), then backfill (§7.3).
- Entries: ledger entries whose accountKey is allowed, sorted by (startedAt, key) ascending, so an original claims its responses before a copy.
- Per entry:
  - Skip-fast 1: memory.sent(key) exists, sent.ended, entry.endedAt != nil, a stamp exists, stat(transcriptPath) == stamp, and no new summary. hasNewSummary = includeSummaries && a stored summary whose hash ≠ sent.summary.
  - Skip-fast 2: never sent, ended, a transcript exists, the cached scanner summary has part(accountKey).messageCount == 0, and the stamp is unchanged.
  - Otherwise payload(); then record(for:). If memory.needsSending → changed; else memory.noteTranscript(key, stamp).
- scanner.save().
- recorder.discard(accounts not allowed); readings = recorder.pending().
- CloudBatcher.batches, then take the first maxRequests. hasMore = total > max.

payload(for entry) (:421-484):
1. path = entry.transcriptPath, else TranscriptLocator.searchTranscript(sessionId, configDir), which lists `<configDir>/projects/*/<id>.jsonl`.
2. owners = ledger.owners(sessionId).
3. totals = scanner.scan(sessionId, path, owners), falling back to the cached summary. If found for a registry-only entry, refine its transcriptPath.
4. part = totals.part(accountKey). Return nil when part.messageCount == 0.
5. If part.lastTimestamp exists: refine lastActivity. A backfill-origin entry that is still open and has been quiet > 30 min ends at its last timestamp.
6. startedAt = part.firstTimestamp ?? entry.startedAt.
7. lastActivityAt = max(part.lastTimestamp ?? entry.lastActivityAt, startedAt).
8. endedAt = (the latest ledger endedAt) mapped to max(it, lastActivityAt).
9. models = part.models, else [entry.model].
10. source = entry.source, or from totals.entrypoint when it is "other".
11. isSplit = any other owner key, including "", has messageCount > 0 (:488-490). Then costUsd = nil. Tokens are always the part's.
12. project = {key: projectKey(accountKey, entry.projectPath, secret), name: entry.projectName}.
13. title = entry.title ?? totals.title.
14. summary = includeSummaries ? stored summary.contract (re-scrubbed) : nil.
15. Returns the session plus TranscriptStamp{totals.transcriptBytes, transcriptModified}.

record(for:) (:495-503):
- base = sha256hex(CloudJSON encoding of the session with summary = nil);
- summary = sha256hex(encoding of the summary);
- ended = endedAt != nil;
- transcript = the stamp.
- markSent keeps the previous summary hash when the new record has none (:217).
- NOTE: api.sync clamps the request, but the records are for the unclamped sessions. A session that the clamp dropped is still marked sent. Port this as it is.

CloudBatcher (:597-638):
- Sessions first, then readings.
- A new batch starts when the current one has maxSessions (200, or reduced after a 413) sessions, or 500 readings, or when a new account key would exceed 50 keys.
- accounts[] = the sorted keys used in the batch, mapped to their contract account.
- records are keyed "sessionId|accountKey".
- Empty batches are not emitted.

5.6 CloudSyncMemory (:129-236)

- adopt(userId, website, dashboardUrl): a different user or website → wipe to fresh contents.
- needsSending: no record, or base differs, or (the new record has a summary and it differs).
- A version mismatch on load → fresh.

5.7 Summary candidates (:578-591)

- Requires since = summaries.enabledAt.
- Entries whose account is allowed (the caller first removes accounts whose 5-hour utilization is ≥ 80), where endedAt ≥ since, with a transcriptPath and a cached scanner summary.
- messageCount = part(accountKey).messageCount.

5.8 handleSyncFailure (:1219-1252)

- CloudAuthError.signedOut, CloudAuthError.otherWebsite or CloudAPIError.notSignedIn:
  - authGeneration++; turnSwitchesOff(); signedOut; dashboardURL = nil;
  - lastError = "Signed out of the website. Sign in again."
- CloudAPIError.server(status, …):
  - 413 while maxSessionsPerRequest > 10: halve it (minimum 10) and set nextSyncAt = now+5 s. No failure is counted; the size is never raised again within the run.
  - Otherwise failures++.
    - A 401 (after a good refresh) sets lastError = "The website didn't accept this Mac's sign-in. Trying again later." The session is kept.
    - Any other status sets lastError = message ?? "The website answered <status>."
    - Back off until now + max(backoff(failures), Retry-After ?? 0).
- Anything else (transport, badResponse, CloudAuthError.http/transport from Supabase):
  - failures++; lastError = describe(error); back off by backoff(failures).
  - Supabase's own Retry-After is ignored.
- backOff sets notBefore = nextSyncAt = date.

5.9 Published state, ClaudeCloudState (ClaudeControlHub+Cloud.swift:25-120; publish CloudSync.swift:1341-1358)

- Fields:
  - websiteURL, websiteIsOverridden;
  - auth: signedOut | signingIn | signedIn(email?) | error(msg);
  - syncEnabled, summariesEnabled, summariesAvailable (= summariesAllowed()), isSyncing, lastSyncAt (memory), lastError;
  - pendingSessions (0 when sync is off), pendingUsage (recorder.pendingCount), summarizedSessions (store count), dashboardURL.
- poolsURL = dashboard + "/pools".
- settingsURL (:99-105): if the dashboard's last component is "dashboard", its sibling "settings"; otherwise the website + "/settings".
- sealedFixture (:109-119): website https://agentnotch.example.com; signedIn me@example.com; sync on; summaries off and unavailable; lastSyncAt now−180 s; dashboard …/dashboard.
- Hub actions (:131-168): cloudSignIn, cloudSignOut, setCloudSync, setSessionSummaries, syncCloudNow. All are no-ops when sealed.

WINDOWS: port the same as a Rust service struct driven by a tokio interval (or a thread and a channel).
- Expose it to the web UI as Tauri commands: cloud_sign_in, cloud_cancel_sign_in, cloud_sign_out, cloud_set_sync, cloud_set_summaries, cloud_sync_now, cloud_open(dashboard|pools|settings).
- Publish a "cloud-state" event carrying the ClaudeCloudState JSON whenever it changes.
- Device name: GetComputerNameExW(ComputerNamePhysicalDnsHostname), falling back to the COMPUTERNAME env var, clamped to 120. This is what Settings > System > About shows.
- appVersion: tauri package_info().version or CARGO_PKG_VERSION.
- Device id: an uppercase UUID v4 stored in the LOCAL cloud folder (see §4).

======================================================================
6. SESSION LEDGER AND OWNER STRETCHES (SessionLedger.swift)
======================================================================

6.1 Feeding from the hub

- Called on every hub projection (ClaudeControlHub.swift:376): feedCloud(attributed, unsure, waiting, liveIDs = all running session ids).
- cloudPlacement (ClaudeControlHub+Cloud.swift:207-214):
  - known identity → certain;
  - otherwise, if the state is ≥ placementGrace (30 s) old → unsure;
  - a folder not grouped yet (`.known(nil)`) → waiting;
  - desktop-hosted with no hostSessionId and no registryStatus → waiting;
  - else unsure.
- cloudObservation (:222-250) is a LiveSessionObservation (SessionLedger.swift:181-199):
  - sessionId, identityId;
  - accountKey = CloudKeys.accountKey(identity), skipped if nil;
  - cwd (skipped if empty), transcriptPath, configDir (state.accountId);
  - entrypoint;
  - startedAt = when the app first saw it, lastActivityAt, model, costUsd;
  - title = sessionTitle unless it was derived from the folder name, else conversation summary;
  - processStartedAt = the process's kernel start time.
- CloudSync.observeLive (CloudSync.swift:1052-1071):
  - only while capturing;
  - lastLiveIDs = liveIDs ∪ waiting;
  - re-keys each observation by environment.accounts() (identity → accountKey), dropping identities not allowed;
  - ledger.observe(…); anything ended → syncSoon().

6.2 observe (:360-426)

1. Filter live observations: a valid UUID, isKey(accountKey), a non-empty cwd, and not in `unsure`.
2. Resolve new cwds to projectPath outside the lock (cached per cwd).
3. Upsert contents.accounts.
4. A session live under two accounts at once is attributable only to its currentOwner. A new one isn't captured.
5. For each attributable observation:
   - if currentOwner exists and differs:
     - a hand-over from a real account is refused when the account-stretch count ≥ maxOwners (32);
     - a hand-over away from "" (nobody) is always allowed;
     - partStart = handOver(…);
   - seenThisRun[key] = now; missingSince is removed;
   - merged(…) (:545-587);
   - openIDs.insert.
6. Running but not counted: liveIDs ∪ unsure − counted − (waiting − unsure).
   - A known session → stopCounting, unless its owner is already "".
   - An unknown session in unsure → noteUnattributed. It is remembered as nobody's from now; capacity 2,000, the oldest evicted.
7. endMissing(liveIDs ∪ notPlaced ∪ live ids).
8. Persist if anything changed.

6.3 handOver (:517-539)

- boundary = min(processStartedAt ?? startedAt, now); max'd with the old part's lastActivityAt and with the previous owner's `from`.
- owners.append({from: boundary, account: new}) and normalize.
- The unattributed mark is removed.
- The old part gets endedAt = max(boundary, its lastActivity) if it was open.

6.4 stopCounting (:435-454)

- Only if the current owner is non-empty and the nobody-stretch count < 32.
- boundary = the current part's lastActivityAt + 0.001 s (uncountedAfter), or now; max'd with the previous `from`.
- owners.append({from: boundary, account: ""}).
- The part ends at its lastActivityAt.

6.5 merged (:545-587)

- New entry: start = partStart ?? min(startedAt, lastActivity); origin live; source from the entrypoint.
- Existing entry:
  - backfill → live, with projectName and projectPath refreshed;
  - identityId updated;
  - transcriptPath and configDir filled;
  - source filled if it was "other";
  - if not split, startedAt = min;
  - lastActivity = max;
  - endedAt = nil (it comes back to life);
  - model, cost and title filled.

6.6 SessionOwners (:111-168)

- owner(at:) walks the owners until one's `from` > date. A nil date means the first owner.
- stretches(of:) is nil when there is a single owner, else [(from, nextFrom)].
- contains: from ≤ d < to.
- normalized: drop zero-length stretches and repeats.
- agree (:127-134): whether two owner lists give the same owner at every change moment up to `end`. The scanner uses it to decide on a recount.

6.7 Other rules

- setOwners: remove the owners entry when it collapses to a single owner from the start whose entry is the session's only one.
- endMissing (:590-614): an open live entry absent from liveIDs gets missingSince = now. After ≥ 60 s (endGrace) it ends at:
  - max(since, lastActivity) if it was seen this run;
  - otherwise lastActivity.
- settle(liveIDs, now): the same, on the tick.
- forgetRunState: clear seenThisRun and missingSince. After a capture gap, a session found gone ends at its last activity.
- record(backfill:) (:621-638): add only unknown sessions (no entry, no owners, not unattributed).
- refine (:643-657) fills in lastActivity, title and transcriptPath, and ends a backfill entry.
- persist: capacity eviction, then a throttled save.
- currentOwner (:331-338): the last owner's key; else the entry with the largest lastActivity (ties broken by key); else "" if unattributed; else nil.

WINDOWS:
- The logic is platform-neutral. Port it with SessionLedgerTests' vectors.
- Dependencies: processStartedAt = GetProcessTimes creation time (OpenProcess with PROCESS_QUERY_LIMITED_INFORMATION). The hub pipeline, CLAUDE_CONFIG_DIR per process and Desktop-hosted attribution belong to other areas.
- Claude Desktop's records on Windows are probably under `%APPDATA%\Claude\claude-code-sessions\<accountUuid>\<org>\<hostSessionId>.json` (Electron userData). Verify this before relying on it.

======================================================================
7. TRANSCRIPT TOKEN SCANNER AND BACKFILL
======================================================================

7.1 What counts (SessionTokenScanner.swift:11-44)

- Files of a session: `<project>/<id>.jsonl` (main); `<project>/<id>/subagents/**/agent-*.jsonl`; and flat `<project>/agent-*.jsonl`, whose session is taken from their lines.
  - A flat file is scanned only if it is unclaimed or already this session's.
- Only a path is opened only when isSafeTranscriptPath holds (:220-226):
  - it ends ".jsonl";
  - it contains a "projects" component with at least 2 components below it;
  - no "sessions", "." or ".." among those below.
  - It is checked on both the given path and its realPath, so a link out of projects/ is never opened.
- Lines: JSON objects.
  - A line is copied when `sessionId` ≠ this session or `forkedFrom.sessionId` ≠ this session. In the MAIN file such lines are skipped entirely: no tokens, timestamps or titles.
  - timestamp is parsed as ISO with or without fractions.
  - owner = owner(at: stamp ?? file.last).
  - first and last are tracked per file and per owner part.
  - cwd and entrypoint are the first seen.
  - Titles: type "summary"→summary, "ai-title"→aiTitle, "custom-title"→customTitle. Title priority is custom > ai > summary.
  - type "assistant" with message.usage is a response:
    - model "<synthetic>" is skipped;
    - key = "<message.id>|<requestId or ''>", else "uuid:<uuid>";
    - claim = the first 8 bytes of SHA-256 as hex (16 characters) (:412-414);
    - tokens from input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens, each max(0, int). JSON booleans are not numbers; numeric strings are accepted (UsageParser.number).
    - First claim anywhere: count it and remember it in recent (≤ 32).
    - The same file claimed it before: replace that response's usage and model in the part it was counted in (a later line of the same response wins).
    - Another file claimed it: ignore.
- Incremental: per file a watermark {size, mtime, inode, offset}.
  - Unchanged, with the same owners → skip.
  - It shrank below the offset, the inode changed, or the owners no longer agree through file.last → release its claims and recount from 0.
  - TranscriptLineReader: complete lines only (a partial last line is re-read next time), 8 MiB chunks. If the size < offset at read time, reset and recount.
- summary() (:596-638): sums the parts across the session's files, sorted by path. Models are ordered by count descending, then name ascending. parts by owner. cwd, entrypoint and title come from the main file. transcriptBytes and transcriptModified come from the main file's size and mtime.
- firstTimestamp(ofTranscript:) (:289-306): the first own-line timestamp, reading up to 1 MiB. Cached by (realPath, inode) or taken from the scan state.
- birthTime: st_birthtimespec.
- pruneMissingFiles: drop files that are gone.

7.2 Backfill roots, CloudBackfill.roots (SessionLedger.swift:721-742)

- For each folder with `<configDir>/projects` as a directory: real = realPath(projects); reachedBy[real] += the folder.
- A folder is a candidate only when all hold:
  - real == projects, and realPath(configDir)+"/projects" == projects, so no link anywhere on the way;
  - it has an identityId and an accountKey;
  - signedInSince != nil.
- Keep a candidate only when reachedBy[real] has exactly one folder.
- backfillFolders (CloudLiveEnvironment.swift:95-116): the account is set only for run folders of allowed identities.
  - Corrected (mirrored) folders get none.
  - The default ~/.claude gets none while Claude Parallel Profiles mirrors into it.
  - Infrastructure folders (e.g. ~/.claude-shared) are added with no account, so they "reach" shared projects.

7.3 backfill (CloudSync.swift:510-573)

- Per root with an allowed account, for each `<root>/<slug>/<uuid>.jsonl`, sorted: skip if the ledger knows the session.
- Keep it if first own timestamp > signedInSince, recording its birth time.
- Sort by (first, birth, path).
- Budget: 200 never-read transcripts per pass.
- scan with owners [{nil, accountKey}]. Require messageCount > 0, a cwd, first > since, and a last timestamp.
- Skip desktop-hosted entrypoints: Claude Desktop runs them as its own account.
- Add an entry with origin backfill:
  - endedAt = last if quiet > 30 min;
  - model = the first model;
  - cost nil;
  - title from the transcript;
  - projectName and projectPath from the cwd.

7.4 CloudFolderLogins (SessionLedger.swift:769-821)

- observe(folder→login, now): set {login, since: now} when new or changed. A folder that is signed out keeps its record.
- since(folder, login) is the date only if the recorded login matches.
- Keys are AccountPaths.normalize(folder).

WINDOWS:
- realPath: `dunce::canonicalize`.
- The link checks in roots(): comparing a canonical path with a lexical one is case-sensitive and fails on Windows. Walk the path components instead and check each for a reparse point (`symlink_metadata().file_type().is_symlink()`, which is true for symlinks and junctions on Windows, or FILE_ATTRIBUTE_REPARSE_POINT with a name-surrogate tag). Compare canonical paths case-insensitively when grouping reachedBy.
- inode: the NTFS file index (GetFileInformationByHandle nFileIndexHigh/Low; `winapi-util` or the `same-file` crate). `std::os::windows::fs::MetadataExt::file_index` is still unstable.
- mtime: metadata.modified() as f64 seconds (100 ns resolution).
- birthTime: metadata.created().
- Reading while Claude Code appends: Rust's File::open on Windows already shares read, write and delete. Node (libuv) opens with share-all too.
- isSafeTranscriptPath: use Path::components() and compare "projects" and "sessions" case-insensitively on Windows.
- Config-folder dictionary keys (folder logins, ledger configDir comparisons): normalize to a canonical case-folded form on Windows. `C:\Users\Me\.claude` and `c:\users\me\.claude` from different sources must match.
- The default config dir is %USERPROFILE%\.claude, with `%USERPROFILE%\.claude.json` for the default folder.
- The transcript slug on Windows is, for example, `C--Users-me-code-app` (every non-alphanumeric character becomes "-"). This only matters for locating transcripts; searchTranscript lists all slugs anyway.

======================================================================
8. USAGE HISTORY OUTBOX (UsageHistoryRecorder.swift)
======================================================================

- UsageObservation {identityId, source, observedAt (when Claude Code or Desktop took it), windows}.
- From a full snapshot (:34-62):
  - session = fiveHour; weekly_all = sevenDay;
  - scoped models → scopedID, de-duplicated;
  - extra_usage when enabled, with utilization = its value, else used/limit·100 when limit > 0; resetsAt nil;
  - every id goes through contractWindowID; utilization is max(0, …) and must be finite.
- statusLine readings carry only session and weekly_all, and only when the line brought new values that the account now shows (UsageStore.swift:554-565).
- record (CloudSync.swift:1075-1082): only while capturing and only for allowed identities. RecordedUsageReading adds accountKey and identityId.
- Recorder.record (:111-133):
  - empty windows → drop;
  - stream = "accountKey|source";
  - the same dedupeKey (accountKey|source|epoch-ms rounded) as the stream's last reading → merge in the windows the pending one lacks;
  - older than the last → drop;
  - the same set of (id, utilization, resetsAt) within 10 min → drop;
  - otherwise last = reading and append;
  - capacity 5,000, dropping the oldest.
- markSent removes by dedupeKey. clear() (sync off or sign-out) wipes pending and last.

WINDOWS:
- Platform-neutral. Reuse UsageHistoryRecorderTests.
- Sources on Windows:
  - probe: claude -p get_usage;
  - statusLine: the status line wrapper;
  - claudeJson: `cachedUsageUtilization` in %USERPROFILE%\.claude.json or <dir>\.claude.json;
  - desktop: Claude Desktop's cache under %APPDATA%\Claude (another area).

======================================================================
9. SESSION SUMMARIES (SessionSummarizer.swift)
======================================================================

9.1 Scheduling, summarizeNext (CloudSync.swift:1276-1337)

1. One at a time.
2. accounts = allowed accounts whose 5-hour utilization is < 80 (unknown counts as 0).
3. candidate = store.nextCandidate(summaryCandidates(…, since: enabledAt), now).
4. folder = environment.summaryFolder(identity). If there is none → recordFailure("No folder is signed in as this account now", minimumWait 1 h).
   - The folder is chosen like the usage probe's: a run folder of the account, never a Parallel Profiles store, whose .claude.json is still signed in as the identity (CloudLiveEnvironment.swift:122-152).
5. stretches = SessionOwners.stretches(of: accountKey, in: owners). excerpt = SessionExcerpt.build(path, sessionId, stretches). If nil → recordFailure("No conversation to summarise", minimumWait 24 h).
6. Re-check the generation, canSummarize and cancellation. Then noteRun(passStart).
7. workingDirectory = <support>/session-summary.
8. Run.
9. After the run: stop if the generation changed, summaries are off, or the task was cancelled. Then summaryFolderStillRuns(folder, identity), or recordFailure("The folder changed accounts during the summary").
10. Outcome:
    - summary → store.record(key, summary, candidate.messageCount, now), then syncSoon();
    - rateLimited → pause all summaries for 30 min and recordFailure(minimumWait 30 min);
    - unavailable → recordFailure(minimumWait 6 h);
    - failed → recordFailure.

9.2 SessionSummaryStore (:916-1104)

- maxPerHour 20; maxPerDay 60 (by `runs` timestamps, pruned to 24 h); quietPeriod 10 min; minimumResponses 2; regrowthFactor 1.5; initialRetry 15 min, doubling (exponent capped at 10) up to 24 h. The wait is max(minimumWait, that).
- isDue: all of these must hold:
  - ended ≥ since;
  - ended ≥ 10 min ago;
  - count ≥ 2;
  - not waiting out a failure;
  - never summarised, or count > 1.5 × the count at the previous summary.
- nextCandidate: nil when the hour or day cap is used up. Among due candidates, the latest endedAt wins; ties go to the smaller key.
- Entry.contract re-scrubs the text on the way out.

9.3 The command (:67-77; the header at :8-10)

- Arguments, exactly:
  `claude -p --model haiku --max-turns 1 --max-budget-usd 0.10 --output-format json --no-session-persistence --strict-mcp-config --settings {"disableAllHooks":true} --tools ""`
  - The JSON is one argv element; the last one is an empty argv element.
- stdin = instructions (:89-96, verbatim) + "\n\n<session>\n" + excerpt + "\n</session>\n". Never in argv.
- Environment (:82-87 plus UsageProbe.swift:77-90):
  - start from the base environment and remove CLAUDECODE, CLAUDE_PID, CLAUDE_EFFORT, AI_AGENT, CLAUDE_CONFIG_DIR, CLAUDE_CODE_*, CLAUDE_AGENT_SDK_*, ANTHROPIC_API_KEY and ANTHROPIC_AUTH_TOKEN;
  - set CLAUDE_CONFIG_DIR = the folder's raw configDirEnv; leave it unset for the default ~/.claude.
  - Test vector: T/SessionSummarizerTests.swift:20-27.
- The working directory is <support>/session-summary, created at 0700.
- Timeout 90 s. stdout is capped at 4 MiB (more → failed "Unexpected output from Claude Code"). A stderr tail of 4 KiB is kept.
- On exit: wait 0.3 s for the pipes, then parse.
- finish: resume once, then stop the child if it still runs: terminate after a grace of 2 s (0 on cancel), then SIGKILL 3 s later.
- Cancelling the task (switch off, sign-out, quit) kills it at once, or keeps it from starting.
- The real runner refuses before bootstrap or when sealed (:431-441). Tests inject a runner; tests never run claude.

9.4 parse (:135-168)

- The last non-empty stdout line must be a JSON object with type "result" or a "result" key. Otherwise → failed(exitDescription).
- is_error or subtype ≠ "success": message = result or subtype, mapped through UsageProbe.outcome(forErrorMessage:) (UsageProbe.swift:141-150):
  - contains "rate limit" or "429" → rateLimited;
  - contains "not logged in", "please run /login" or "claude.ai" → unavailable("Not signed in to Claude");
  - else failed(first 200 characters).
- text = scrub(result); empty → failed.
- model = the modelUsage key with the most outputTokens (ties → the smaller key), else "haiku".
- costUsd = total_cost_usd if finite and ≥ 0.
- exitDescription (:411-424):
  - a last stderr line containing "unknown option" or "unknown argument" → "This Claude Code is too old for session summaries (…)";
  - else "Claude Code exited (<status>): <line ≤160>" or "…with status <n>".

9.5 The excerpt, SessionExcerpt (:708-907)

- Only lines inside the stretches when the session is split.
- Skipped: isSidechain, isMeta, and copied lines.
- User lines count only if isHumanPrompt (ConversationParser.swift:326-342): not meta, no toolUseResult, not a compact summary, origin.kind absent or "human", and the text doesn't start with <task-notification>, <command-name>, <command-message>, <local-command, <bash-input>, <bash-stdout> or "Caveat:" (HookEvent.swift:421-434).
- Text is a string content, or the joined text blocks.
- Assistant lines contribute their text blocks.
- Each segment is "User: …" or "Claude: …", redacted and clipped to 4,000 characters (head and tail with " […] ").
- Accumulator: 24,000 characters maximum. The head keeps up to 8,000; then a rolling tail; "\n\n[…]\n\n" between them when something was dropped.
- Lengths are Swift Character counts. In Rust use graphemes; exact parity doesn't matter here (this goes to the model, not the contract).

9.6 Redaction (:797-849)

The regexes, applied in order, verbatim at :806-829:
- a PEM key block;
- sk-…;
- sk/rk/pk_live|test_…;
- gh[pousr]_…;
- github_pat_…;
- AKIA…;
- xox?-…;
- AIza…;
- sb_secret_…;
- npm_…;
- ya29.…;
- JWT eyJ….….…;
- `Bearer <tok>` → "$1 [redacted]";
- scheme://user:pw@ → the password redacted;
- the value of settings named like password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credential (value ≥ 6 characters);
- the value of any NAME ending in KEY|TOKEN|SECRET|PASSWORD|PASS (UPPER, or after _ . -, or a camelCase ending).

Then random runs: `[A-Za-z0-9+/=_\-]{32,}` containing a digit and a letter with Shannon entropy ≥ 3 bits per character → "[redacted]".

RUST: these use lookbehind `(?<!…)`, `(?<=…)`, lookahead `(?!\[redacted\])` and inline `(?i:…)`. The `regex` crate can't do lookaround, so use `fancy-regex` (or pcre2) and port SessionSummarizerTests' vectors (:306 onward), including "scrubbing twice changes nothing".

9.7 scrub (:187-189)

- scrub = oneParagraph(shortenPaths(foldingWhitespace(redact(text)))).
- oneParagraph folds whitespace and clamps to 2000 UTF-16 units.
- shortenPaths (:224-409):
  - The path pattern is `(?<![\w.~-])(?:~|/(?:Users|home|private|Volumes|System|opt|var|Library|etc|tmp))/[^\s"'`<>()\[\]{}]+`.
  - Each match becomes its last component; trailing punctuation `.,;:!?)]}'"`` stays outside.
  - A path that ends at /Users/<name> or /home/<name> becomes "~"; one that ends at /Volumes/<name> becomes "…".
  - It continues across spaces for Capitalised words (up to 4, the last one followed by "/"), or a single lowercase name word after a user or volume name that isn't a joining word.
  - knownNames = this machine's /Volumes and /Users entries plus the home folder name (LocalNames, :662-702; only when bootstrapped and not sealed; re-listed at most once a minute). Names containing spaces are joined with U+E000 while shortening.
- Example vectors (T/SessionSummarizerTests.swift:41-107):
  - "Edited /Users/jane/app/main.swift." → "Edited main.swift.";
  - "Cleaned /home/jane/ and /Users/jane" → "Cleaned ~ and ~";
  - "Set STRIPE_KEY=abcdef123456 in staging." → "Set STRIPE_KEY=[redacted] in staging.";
  - "The key: a new index." is unchanged.

WINDOWS for summaries (the hard parts):
- The scrub must learn Windows paths. Otherwise summaries of Windows sessions leak `C:\Users\jane\…`. Add:
  - drive paths `[A-Za-z]:[\\/]…`;
  - UNC `\\server\share\…` and `\\?\`;
  - `%USERPROFILE%`, `%APPDATA%` and `%LOCALAPPDATA%` forms;
  - `~\…`;
  - MSYS and Git Bash `/c/Users/…`;
  - WSL `/mnt/c/Users/…` and `\\wsl$\…`, `\\wsl.localhost\…`.
- `X:\Users\<name>` (either separator) becomes "~". A path that is only a drive root or volume becomes "…".
- Components may contain spaces ("Program Files", "OneDrive - Company"). Extend the capitalised-continuation rule to "\".
- LocalNames on Windows:
  - the entries of `%SystemDrive%\Users`, excluding Public, Default, "Default User", "All Users" and desktop.ini;
  - the home folder name;
  - volume labels (GetVolumeInformationW) and mapped drive names.
- Keep the Mac and Linux rules: WSL and remote sessions produce `/home/...`.
- This is a new test surface: write Windows vectors alongside ported Mac ones.
- Locating claude: the native installer puts it at `%USERPROFILE%\.local\bin\claude.exe`; npm puts a shim at `%APPDATA%\npm\claude.cmd`.
  - Avoid .cmd shims: argument escaping through cmd.exe for the JSON and empty-string arguments is error-prone (the BatBadBut rules; Rust's std escapes or refuses some batch arguments).
  - Prefer claude.exe. If only the npm shim exists, run `node <…\node_modules\@anthropic-ai\claude-code\cli.js>` directly.
  - Test that the arguments round-trip.
- Spawning:
  - `CREATE_NO_WINDOW` (0x08000000) so no console flashes;
  - piped stdin, stdout and stderr; write stdin on its own thread, then close it.
  - Windows has no SIGTERM or SIGKILL. Put the child in a Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE (upstream's Cargo.toml already enables Win32_System_JobObjects). Terminate it with TerminateJobObject, which also kills node grandchildren. The "grace" becomes a wait-then-terminate.
  - Writing to a closed pipe returns an error (no SIGPIPE).
- Environment names are case-insensitive on Windows: filter case-insensitively (Claude_Config_Dir, etc.) and build with env_clear() + envs().
- The working dir is %LOCALAPPDATA%\Agent Notch\Claude\session-summary. Claude Code may record that cwd in the folder's .claude.json projects map; the Mac accepts the same.
- The 5-hour utilization and the folder checks come from the usage area's Windows port.

======================================================================
10. SETTINGS UI (P/UI/Settings/CloudSection.swift)
======================================================================

The section is "Cloud".

Website row, always shown (:45-66):
- the value is websiteDisplay (strips "https://", keeps "http://"), or "None";
- the caption "Set by AGENTNOTCH_WEB_URL for this run." when overridden.

Signed out (:70-88):
- The title is "Signing in…" or "Not signed in".
- The detail is signInDetail:
  - while signing in: "Finish signing in in your browser.";
  - with no website: "This build has no website, so it can't sign in or sync.";
  - otherwise "Opens Google's sign-in in your browser. " + fileNote.
- A critical caption shows signInProblem: the .error message, else lastError.
- A spinner shows while signing in.
- The "Sign in with Google" button is disabled when there is no website or a sign-in is in progress.

Signed in (:93-162):
- Row "Signed in as <email>" (or "Signed in"): detail fileNote "The website sign-in is kept in a file only your user can read.", and "Sign out…" with a "Sign out" confirmation.
- Toggle "Sync sessions and usage". syncDetail(readsDesktopUsage) (:215-219): "Sends project folder names, session titles, models, times, token counts, cost, and usage limits for each signed-in Claude account (Claude Desktop's readings too | Claude Desktop's too, once reading its cache is on). Never file paths, prompts or your Claude login."
- Toggle "Summarise finished sessions with Claude":
  - its value is syncEnabled && summariesEnabled, and it is disabled when sync is off;
  - summariesDetail (:222-227), verbatim;
  - summariesNote: "Needs sync." / "Not in this run: it can't start Claude Code." / "N sessions summarised on this Mac."
- Status row, status() (:245-261):
  - titles: "Sync is off" (detail "Nothing is sent while it is off.") / "Syncing…" / "Last sync failed" / "Last synced <age>" / "Not synced yet";
  - detail: lastError, or waiting() "X sessions and Y usage readings to send.";
  - button "Sync now", disabled when sync is off or a sync is running.
- Dashboard row:
  - the caption `dashboard` (:178);
  - the text "Remove summaries or delete synced data in the website's Settings." with "Settings" linked to settingsURL, routed through an action that a sealed run refuses;
  - buttons "Open dashboard" and "Share accounts…" (pools), disabled until dashboardURL is known.

WINDOWS:
- Port the copy verbatim; the fork's UI is English-only.
- Replace "this Mac" with "this PC" in:
  - summariesDetail;
  - summariesNote;
  - the 401 lastError at CloudSync.swift:1240.
- Adjust fileNote if the DACL isn't strictly owner-only.
- Add a "Cancel" button, and a timeout, while signing in (§2).

======================================================================
11. SEALED / SAFE MODE, DEV FLAGS
======================================================================

- Sealed (AGENTNOTCH_SAFE_MODE, which fails closed):
  - hub.cloud = sealedFixture;
  - every action is a no-op;
  - no stores, no files, no network, no claude;
  - the browser step refuses (CancellationError);
  - LocalNames is empty.
- Windows must gate the same things: the transport, the session store, the summarizer runner, the deep-link handler (ignore callbacks) and the opener.
- AGENTNOTCH_WEB_URL is ignored when sealed.
- --no-install / AGENTNOTCH_NO_INSTALL and probes-disabled → summariesAvailable = false.
- AGENTNOTCH_SUPPORT_DIR relocates all the files.

======================================================================
12. TESTS AND FIXTURES TO REUSE IN RUST
======================================================================

Fixture path from windows/<crate>/: `concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/contract/fixtures/")`. Do not copy the files: a fixture changes only together with both sides.

Port as Rust tests:

From T/CloudContractTests.swift:
- keysMatchTheFixture (:10-35), including the case-insensitivity, random-secret-differs and not-a-plain-hash checks;
- accountKeysDependOnlyOnTheAccountsOwnOrganization (:46-72);
- theInstallSecretIsMadeOnceAndKeptPrivately (:76-95), with an ACL check replacing 0600;
- projectPathsExpandTildeAndResolveLinks (:97-109), plus Windows junction and case variants;
- fixtureRequest() (:114-157), which rebuilds F/sync-request.json from values;
- encodedRequestHasTheFixturesShape (:159-166): compare semantics (:245-273): the same key sets at every level (an explicit null counts as present), numbers compared as f64, strings equal or dates within 1 ms;
- fixtureRequestDecodesAndReencodesToItself;
- datesOutsideTheContractsRangeAreLeftOut (:178-208);
- resetTimesTheWebsiteWouldRefuseAreSentAsNull (:214-241);
- everyResponseFixtureDecodes (:277-298), including the error.json mapping and Retry-After "12";
- datesAreUTCWithZ (:300-306);
- clampingLeavesOut… (:310-341);
- windowIds… (:343-353);
- sessionSources… (:355-364);
- onlyHttpsOrThisMac… (:368-382).

From T/CloudContractE2ETests.swift:
- the CloudContractRules validator (:314-527) and theContractRulesCatchWhatTheWebsiteRefuses (:283-321, 20 textual breakages of the fixture);
- the "morning" scenario (:56-197): two accounts, live sessions A/B/C with C resumed under the other account, a backfilled D, four usage sources, a 503 then success, a summary. Expected totals (:237-278):
  - A: tokens {2772, 1485, 35000, 266000}, 4 responses, cost 2.4617, endedAt base+1800;
  - B: cacheRead 3,211,052,654, which needs 64-bit;
  - C split into {400, 90, 1500, 9000} and {75, 430, 0, 23000}, neither with a cost;
  - D: sdk, "Monthly usage report";
  - 10 windows over 4 sources.
  - Leak checks: prompts, tool output, "/Users/me", the temp root, ".jsonl", the account UUIDs and org, the secret hex. On Windows add "C:\\Users".

From T/CloudAuthTests.swift:
- the PKCE vector;
- the authorize URL query set;
- the callback vectors;
- refresh single-flight, refusal and keep-on-5xx semantics;
- the 401-retry-once.

Also port the ledger, scanner, recorder, summarizer and sync regression suites. The test names are self-describing (116 tests).

Scripts/cloud-contract-e2e.sh currently runs three steps:
1. The Swift test theAppsSyncRequestKeepsTheContract writes AGENTNOTCH_CONTRACT_OUT.
2. web/tests/integration/contract-e2e.test.ts runs in Docker Postgres `agentnotch-web-test-e2e` on 127.0.0.1:55439. It needs ≥ 1 session with a summary, all dates near the real clock, and `makeSyncRequestSchema().parse(sent)` equal to sent. It writes AGENTNOTCH_CONTRACT_RESPONSE.
3. The Swift test theWebsitesSyncResponseIsRead.

The web step doesn't care which client made the request. For Rust:
- add an app-side selector (e.g. AGENTNOTCH_CONTRACT_APP=rust) that runs `cargo test --manifest-path windows/Cargo.toml -p agentnotch-cloud contract_e2e_request -- --exact` and then `contract_e2e_response`;
- check cargo's "1 passed", because a filter that matches nothing passes;
- keep the sourced helpers (web/tests/unit/contract-e2e-script.test.ts tests them);
- the crate must build on Linux for an ubuntu CI job with Docker.

======================================================================
13. WHAT THE WEBSITE / SUPABASE MUST ALLOW
======================================================================

- Custom scheme (recommended): nothing new. `agentnotch://auth-callback` is already required in Supabase > Authentication > URL Configuration > Redirect URLs (web/README.md:156-158), and the config route returns that literal.
- Loopback (optional): add `http://127.0.0.1:<port>/auth-callback` (or a glob; check how Supabase's globs treat ports) to Redirect URLs, and change the contract's config (a new field or a relaxed literal) in README, the fixtures, schema.ts and both apps together.
- Device naming and copy:
  - devices are per-user rows keyed by a lowercased device.id, so a Windows device id and name need no website change;
  - the website copy says "Mac" in places (e.g. web/src/app/_components/connect-mac.tsx, the README "Connect a Mac" section). Updating it is optional;
  - /download/windows already exists (web/README.md:66).
- Rate limits: the Mac and Windows share the per-user 12-burst bucket. 5 requests per pass per device stays within it, but two catching-up devices can hit 429s. Retry-After is honoured.

======================================================================
14. HARD POINTS, SUMMARY
======================================================================

1. Browser sign-in without ASWebAuthenticationSession:
   - deep-link and single-instance forwarding;
   - a pending-flow gate;
   - cancel and a timeout, because a closed tab can't be seen;
   - cold-start callbacks;
   - scheme squatting, which PKCE neutralizes.
2. 0600/0700 as a protected DACL, with no readable window at creation; %LOCALAPPDATA%, not Roaming; atomic replace and exclusive create on NTFS, with antivirus-lock retries.
3. Windows path canonicalization for project keys, realPath, link detection, safe-path checks and folder keys (case, junctions, `\\?\`, drive letters).
4. The inode equivalent (file index) and birth time, for the scanner watermarks and the backfill order.
5. Byte-level JSON: sorted keys, explicit nulls except summary, ms-rounded Z dates, UTF-16 grapheme-safe clamps, and URL canonicalization without the url crate's trailing "/".
6. The regexes need lookaround (`fancy-regex`), and the scrub must be extended for Windows paths and Windows LocalNames.
7. Running claude -p:
   - locating claude.exe versus .cmd shims;
   - argument quoting of the JSON and empty-string arguments;
   - Job Object kill-tree instead of SIGTERM/SIGKILL; CREATE_NO_WINDOW;
   - a case-insensitive environment scrub.
8. Upstream dependencies from other areas: CLAUDE_CONFIG_DIR per process (reading the PEB), process start time, and the Desktop-hosted session records under %APPDATA%\Claude. The cloud feed needs them to attribute sessions for certain; without them everything is unsure and counts for nobody.
