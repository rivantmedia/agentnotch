//! Signing in to the website (CL§2; the Mac's `CloudAuth.swift`): Supabase
//! Auth with Google as a PKCE flow (RFC 7636). This is the app's own login to
//! its own website; it has nothing to do with Claude's login, which the app
//! never reads.
//!
//! 1. authorize: `{supabase}/auth/v1/authorize?provider=google&redirect_to=…
//!    &code_challenge=<S256(verifier)>&code_challenge_method=s256`, opened in
//!    the default browser ([`begin`], [`open_browser`]);
//! 2. the browser comes back to `agentnotch://auth-callback?code=…` (or
//!    `error`/`error_description`, in the query or the fragment) as a deep
//!    link, handed to [`Auth::finish`];
//! 3. exchange: `POST {supabase}/auth/v1/token?grant_type=pkce`
//!    `{auth_code, code_verifier}` with the publishable key as `apikey`;
//! 4. refresh: `POST …/token?grant_type=refresh_token {refresh_token}`, one
//!    at a time across threads. Supabase rotates the refresh token, so the
//!    new session is saved before its access token is used. The session is
//!    forgotten only when Supabase refuses the refresh token itself (400/401
//!    naming an invalid grant, a used, revoked or unknown refresh token, or an
//!    ended session); anything else (a 5xx, a 429, the network) is tried again
//!    later with the session kept;
//! 5. sign out: `POST …/logout?scope=local` (this PC's session only), after
//!    the saved session is removed, whatever the answer.
//!
//! The Mac's browser step is one awaited sheet; on Windows the browser is
//! another program and the callback arrives later through the URL scheme, so
//! the sign-in is split in two: [`begin`] makes the verifier and the
//! authorize URL (kept in memory only, in a [`PendingSignIn`]), and
//! [`Auth::finish`] / [`Auth::exchange`] trade the code for a session. A
//! dropped `PendingSignIn` is a cancelled sign-in: nothing was saved.
//!
//! A session belongs to the website it was made through: an access token is
//! only ever handed out for that website ([`Auth::access_grant`]), and a
//! request is bound to the sign-in it started with: the token its 401 retry
//! gets is of that same sign-in and website, never of one made since
//! ([`Auth::refresh_after_unauthorized`]).
//!
//! The session lives in `<support>\cloud-session.json` (private, written
//! atomically), never when sealed or before the service starts. Tests use
//! [`MemorySessionStore`]. Nothing here ever writes a token into a message.

use super::contract::{self, ConfigResponse, CALLBACK_HOST, CALLBACK_SCHEME, REDIRECT_URL};
use super::files::{lock, random_bytes, write_private};
use super::website;
use crate::core::claude_json::lenient_number;
use crate::platform::{Browser, Clock, Http, HttpError, HttpRequest, SecureFiles};
use crate::runtime_types::CloudConfig;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use unicode_segmentation::UnicodeSegmentation;

/// Refresh when the access token has this little left (seconds).
pub const REFRESH_MARGIN_S: f64 = 60.0;

/// How long a request to Supabase may take (the Mac's URLSession setting).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// ---- Session ----

/// A Supabase session for the website, and where it came from: the
/// `cloud-session.json` format (the Mac's `CloudAuthSession`, keys as the Mac
/// writes them, `userId`/`email` left out when unknown).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSession {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    #[serde(rename = "expiresAt", with = "contract::date")]
    pub expires_at: SystemTime,
    #[serde(rename = "userId", default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(rename = "email", default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The Supabase project and its publishable (public) key, so a refresh
    /// or a sign-out needs no config request.
    #[serde(rename = "supabaseUrl")]
    pub supabase_url: String,
    #[serde(rename = "publishableKey")]
    pub publishable_key: String,
    /// The website it was made through; another website means signing in
    /// again.
    #[serde(rename = "websiteURL")]
    pub website_url: String,
}

/// Without the tokens: a session may end up in a panic message or a log.
impl fmt::Debug for AuthSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthSession")
            .field("expires_at", &contract::date::to_string(self.expires_at))
            .field("user_id", &self.user_id)
            .field("email", &self.email)
            .field("supabase_url", &self.supabase_url)
            .field("website_url", &self.website_url)
            .finish_non_exhaustive()
    }
}

impl AuthSession {
    /// The file's bytes: compact JSON with sorted keys, as the Mac writes it.
    pub fn encode(&self) -> Vec<u8> {
        contract::to_json(self)
    }

    /// `None` when the bytes aren't a session (a damaged file is no session).
    pub fn decode(bytes: &[u8]) -> Option<AuthSession> {
        serde_json::from_slice(bytes).ok()
    }

    /// Seconds the access token has left at `now` (negative once expired).
    fn seconds_left(&self, now: SystemTime) -> f64 {
        match self.expires_at.duration_since(now) {
            Ok(left) => left.as_secs_f64(),
            Err(past) => -past.duration().as_secs_f64(),
        }
    }

    fn is_fresh(&self, now: SystemTime) -> bool {
        self.seconds_left(now) > REFRESH_MARGIN_S
    }
}

/// An access token, the website it may be sent to, and the sign-in it
/// belongs to ([`Auth`]'s count of sessions adopted or dropped): what a
/// request needs to ask for its 401 retry's token.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessGrant {
    pub token: String,
    pub website: String,
    pub sign_in: u64,
}

impl fmt::Debug for AccessGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessGrant")
            .field("website", &self.website)
            .field("sign_in", &self.sign_in)
            .finish_non_exhaustive()
    }
}

// ---- Where the session is kept ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// Sealed, or the service hasn't started: nothing is written.
    NotAllowed,
    Io(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::NotAllowed => f.write_str("not allowed in this run"),
            StoreError::Io(message) => f.write_str(message),
        }
    }
}

pub trait SessionStore: Send + Sync {
    fn load(&self) -> Option<AuthSession>;
    fn save(&self, session: &AuthSession) -> Result<(), StoreError>;
    fn clear(&self);
}

/// `<support>\cloud-session.json`, through the platform's [`SecureFiles`]:
/// private, replaced atomically.
pub struct FileSessionStore {
    path: PathBuf,
    files: Arc<dyn SecureFiles>,
    allowed: bool,
    guard: Mutex<()>,
}

impl FileSessionStore {
    pub const FILE_NAME: &'static str = "cloud-session.json";

    /// `allowed` false (a sealed run, or anything before the cloud service
    /// starts): nothing is read or written, and a file found there is left
    /// alone.
    pub fn new(support: &Path, files: Arc<dyn SecureFiles>, allowed: bool) -> FileSessionStore {
        FileSessionStore {
            path: support.join(Self::FILE_NAME),
            files,
            allowed,
            guard: Mutex::new(()),
        }
    }

    /// The service's store: allowed unless sealed.
    pub fn for_config(cfg: &CloudConfig, files: Arc<dyn SecureFiles>) -> FileSessionStore {
        Self::new(&cfg.support, files, !cfg.sealed)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SessionStore for FileSessionStore {
    fn load(&self) -> Option<AuthSession> {
        if !self.allowed {
            return None;
        }
        let _guard = lock(&self.guard);
        let bytes = std::fs::read(&self.path).ok()?;
        AuthSession::decode(&bytes)
    }

    fn save(&self, session: &AuthSession) -> Result<(), StoreError> {
        if !self.allowed {
            return Err(StoreError::NotAllowed);
        }
        let bytes = session.encode();
        let _guard = lock(&self.guard);
        write_private(self.files.as_ref(), &self.path, &bytes)
            .map_err(|e| StoreError::Io(e.to_string()))
    }

    fn clear(&self) {
        if !self.allowed {
            return;
        }
        let _guard = lock(&self.guard);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// In memory only (tests; a sealed run never signs in).
#[derive(Default)]
pub struct MemorySessionStore {
    state: Mutex<(Option<AuthSession>, usize)>,
}

impl MemorySessionStore {
    pub fn new(session: Option<AuthSession>) -> MemorySessionStore {
        MemorySessionStore {
            state: Mutex::new((session, 0)),
        }
    }

    /// How many times a session was saved.
    pub fn save_count(&self) -> usize {
        lock(&self.state).1
    }
}

impl SessionStore for MemorySessionStore {
    fn load(&self) -> Option<AuthSession> {
        lock(&self.state).0.clone()
    }

    fn save(&self, session: &AuthSession) -> Result<(), StoreError> {
        let mut state = lock(&self.state);
        state.0 = Some(session.clone());
        state.1 += 1;
        Ok(())
    }

    fn clear(&self) {
        lock(&self.state).0 = None;
    }
}

// ---- PKCE ----

pub mod pkce {
    use super::*;

    /// A verifier from 32 random bytes: 43 base64url characters (RFC 7636
    /// allows 43–128). `None` when the system gives no randomness: never a
    /// predictable verifier.
    pub fn make_verifier() -> Option<String> {
        random_bytes::<32>().map(|bytes| verifier_from_bytes(&bytes))
    }

    pub fn verifier_from_bytes(bytes: &[u8]) -> String {
        base64_url(bytes)
    }

    /// `S256`: base64url(SHA-256(ASCII(verifier))), no padding.
    pub fn challenge(verifier: &str) -> String {
        base64_url(&Sha256::digest(verifier.as_bytes()))
    }

    pub fn base64_url(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }
}

// ---- Errors ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// The website's config names no usable Supabase project.
    BadConfig(String),
    /// The browser came back somewhere else, or without a code.
    InvalidCallback,
    /// Supabase or Google said no (or the browser couldn't open).
    Provider(String),
    /// The user gave up on the sign-in.
    Cancelled,
    /// No session (never signed in, signed out, or the refresh token was
    /// refused).
    SignedOut,
    /// The session was made through another website than the one asked for.
    OtherWebsite,
    /// Supabase answered with an error (`code`: its `error_code` or `error`).
    Http {
        status: u16,
        code: Option<String>,
        message: Option<String>,
    },
    Transport(String),
    BadResponse(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::BadConfig(detail) => {
                write!(f, "The website's sign-in settings aren't usable: {detail}")
            }
            AuthError::InvalidCallback => f.write_str("Sign-in didn't come back with a code."),
            AuthError::Provider(message) => f.write_str(message),
            AuthError::Cancelled => f.write_str("Sign-in was cancelled."),
            AuthError::SignedOut => f.write_str("Signed out. Sign in again."),
            AuthError::OtherWebsite => f.write_str("Signed in to another website. Sign in again."),
            AuthError::Http {
                status, message, ..
            } => match message {
                Some(message) => f.write_str(message),
                None => write!(f, "Sign-in failed ({status})."),
            },
            AuthError::Transport(message) => {
                write!(f, "Couldn't reach the sign-in service: {message}")
            }
            AuthError::BadResponse(message) => {
                write!(f, "Unexpected answer from the sign-in service: {message}")
            }
        }
    }
}

impl std::error::Error for AuthError {}

/// What a failed send says (the Mac shows the transport's own description).
pub(crate) fn describe_http_error(error: &HttpError) -> String {
    match error {
        HttpError::Timeout => "The request timed out.".into(),
        HttpError::Connect(message) | HttpError::Tls(message) | HttpError::Other(message) => {
            message.clone()
        }
    }
}

// ---- Pure pieces ----

/// `{supabase}/auth/v1/<leaf>?<query>`; `None` when `supabase` isn't a URL
/// a path can be added to.
fn supabase_endpoint(supabase: &str, leaf: &str, query: &[(&str, &str)]) -> Option<String> {
    let mut url = url::Url::parse(supabase).ok()?;
    url.path_segments_mut()
        .ok()?
        .pop_if_empty()
        .extend(["auth", "v1", leaf]);
    url.set_query(None);
    url.set_fragment(None);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Some(url.into())
}

/// Where the browser goes first.
pub fn authorize_url(supabase: &str, challenge: &str) -> Option<String> {
    authorize_url_with(supabase, challenge, REDIRECT_URL)
}

pub fn authorize_url_with(supabase: &str, challenge: &str, redirect: &str) -> Option<String> {
    supabase_endpoint(
        supabase,
        "authorize",
        &[
            ("provider", "google"),
            ("redirect_to", redirect),
            ("code_challenge", challenge),
            ("code_challenge_method", "s256"),
        ],
    )
}

/// The code in the callback, or the error it carries (query or fragment).
/// `agentnotch://auth-callback/?code=…` (a trailing slash some browsers add)
/// is the same callback.
pub fn authorization_code(callback: &str) -> Result<String, AuthError> {
    let url = url::Url::parse(callback).map_err(|_| AuthError::InvalidCallback)?;
    let host = url.host_str().map(str::to_lowercase);
    if url.scheme().to_lowercase() != CALLBACK_SCHEME || host.as_deref() != Some(CALLBACK_HOST) {
        return Err(AuthError::InvalidCallback);
    }
    // Pairs parsed by hand: the callback is someone else's text, and a
    // malformed escape in it must be kept, never trusted or fatal.
    let mut pairs = form_pairs(url.query());
    pairs.extend(form_pairs(url.fragment()));
    let value = |name: &str| {
        pairs
            .iter()
            .find(|(n, v)| n == name && !v.is_empty())
            .map(|(_, v)| v.clone())
    };
    if let Some(error) = value("error").or_else(|| value("error_code")) {
        let message = value("error_description").unwrap_or(error);
        return Err(AuthError::Provider(
            message.graphemes(true).take(300).collect(),
        ));
    }
    value("code").ok_or(AuthError::InvalidCallback)
}

/// `a=1&b=two+words` as (name, value) pairs, `+` as a space and percent
/// escapes decoded (a part with a malformed escape is kept as written).
pub fn form_pairs(encoded: Option<&str>) -> Vec<(String, String)> {
    let Some(encoded) = encoded.filter(|e| !e.is_empty()) else {
        return Vec::new();
    };
    let decode = |part: &str| {
        let spaced = part.replace('+', " ");
        percent_decoded(&spaced).unwrap_or(spaced)
    };
    encoded
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (decode(name), decode(value)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

/// Percent escapes decoded; `None` for a malformed escape or a result that
/// isn't UTF-8 (Foundation's `removingPercentEncoding`).
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Whether Supabase's answer to a refresh says the refresh token itself is
/// no good (used, revoked, expired, unknown, or its session ended), rather
/// than that something failed on the way.
pub fn refuses_refresh_token(status: u16, code: Option<&str>, message: Option<&str>) -> bool {
    if status != 400 && status != 401 {
        return false;
    }
    const REFUSALS: [&str; 7] = [
        "invalid_grant",
        "refresh_token_not_found",
        "refresh_token_already_used",
        "session_not_found",
        "session_expired",
        "user_not_found",
        "user_banned",
    ];
    if code.is_some_and(|code| REFUSALS.contains(&code.to_lowercase().as_str())) {
        return true;
    }
    message.is_some_and(|m| m.to_lowercase().contains("invalid refresh token"))
}

/// A Supabase token answer as a session.
pub fn session_from_answer(
    answer: &Map<String, Value>,
    supabase_url: &str,
    publishable_key: &str,
    website: &str,
    now: SystemTime,
) -> Result<AuthSession, AuthError> {
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    let (Some(access), Some(refresh)) = (
        text(answer.get("access_token")).filter(|t| !t.is_empty()),
        text(answer.get("refresh_token")).filter(|t| !t.is_empty()),
    ) else {
        return Err(AuthError::BadResponse("no tokens".into()));
    };
    let positive = |key: &str| {
        answer
            .get(key)
            .and_then(lenient_number)
            .filter(|n| *n > 0.0)
            .and_then(|n| Duration::try_from_secs_f64(n).ok())
    };
    let expires_at = positive("expires_at")
        .and_then(|since_epoch| UNIX_EPOCH.checked_add(since_epoch))
        .or_else(|| positive("expires_in").and_then(|left| now.checked_add(left)))
        .unwrap_or(now + Duration::from_secs(3600));
    let user = answer.get("user").and_then(Value::as_object);
    Ok(AuthSession {
        access_token: access,
        refresh_token: refresh,
        expires_at,
        user_id: text(user.and_then(|u| u.get("id"))),
        email: text(user.and_then(|u| u.get("email"))),
        supabase_url: supabase_url.to_owned(),
        publishable_key: publishable_key.to_owned(),
        website_url: website.to_owned(),
    })
}

/// The session's access token, if it was made through `website` and is
/// still the sign-in `epoch` (`now` is the current one).
fn check(session: &AuthSession, website: &str, epoch: u64, now: u64) -> Result<String, AuthError> {
    if session.website_url != website {
        return Err(AuthError::OtherWebsite);
    }
    if epoch != now {
        return Err(AuthError::SignedOut);
    }
    Ok(session.access_token.clone())
}

// ---- The browser half of a sign-in ----

/// A sign-in waiting for its callback: kept in memory only, never on disk.
/// Dropping it is cancelling the sign-in (nothing was saved).
pub struct PendingSignIn {
    verifier: String,
    /// What the browser opens.
    pub authorize_url: String,
    /// The validated Supabase address and its publishable key.
    pub supabase_url: String,
    pub publishable_key: String,
    /// The website the sign-in is for; the session is bound to it.
    pub website: String,
}

impl PendingSignIn {
    pub fn verifier(&self) -> &str {
        &self.verifier
    }
}

impl fmt::Debug for PendingSignIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingSignIn")
            .field("supabase_url", &self.supabase_url)
            .field("website", &self.website)
            .finish_non_exhaustive()
    }
}

/// The first half of a sign-in: checks the website's config and makes the
/// PKCE pair and the authorize URL.
pub fn begin(config: &ConfigResponse, website: &str) -> Result<PendingSignIn, AuthError> {
    if config.redirect_url != REDIRECT_URL {
        return Err(AuthError::BadConfig(format!(
            "redirectUrl is {}",
            config.redirect_url
        )));
    }
    let Some(supabase) = website::validated_link(Some(&config.supabase_url)) else {
        return Err(AuthError::BadConfig("supabaseUrl".into()));
    };
    if config.supabase_publishable_key.is_empty() {
        return Err(AuthError::BadConfig("supabasePublishableKey".into()));
    }
    let Some(verifier) = pkce::make_verifier() else {
        return Err(AuthError::Provider(
            "This PC couldn't make a sign-in key. Try again.".into(),
        ));
    };
    let Some(authorize_url) = authorize_url(&supabase, &pkce::challenge(&verifier)) else {
        return Err(AuthError::BadConfig("supabaseUrl".into()));
    };
    Ok(PendingSignIn {
        verifier,
        authorize_url,
        supabase_url: supabase,
        publishable_key: config.supabase_publishable_key.clone(),
        website: website.to_owned(),
    })
}

/// Opens the sign-in page; a browser that can't is a failed sign-in with
/// its reason, not a cancellation.
pub fn open_browser(browser: &dyn Browser, pending: &PendingSignIn) -> Result<(), AuthError> {
    browser
        .open(&pending.authorize_url)
        .map_err(AuthError::Provider)
}

// ---- Auth ----

struct Inner {
    session: Option<AuthSession>,
    loaded: bool,
    /// Which sign-in `session` is: bumped whenever it is replaced or dropped
    /// (adopted, forgotten, set aside), never by a refresh.
    epoch: u64,
    refresh: RefreshSlot,
}

/// The one refresh in flight, and what the last one came to: callers that
/// arrive while it runs wait for its result instead of sending their own
/// (the refresh token is single-use).
#[derive(Default)]
struct RefreshSlot {
    running: bool,
    round: u64,
    result: Option<Result<AuthSession, AuthError>>,
}

pub struct Auth {
    http: Arc<dyn Http>,
    store: Arc<dyn SessionStore>,
    clock: Arc<dyn Clock>,
    inner: Mutex<Inner>,
    refreshed: Condvar,
}

/// Ends the refresh round however it ends (even by a panic), so the
/// callers waiting on it never hang.
struct RoundGuard<'a> {
    auth: &'a Auth,
    result: Option<Result<AuthSession, AuthError>>,
}

impl Drop for RoundGuard<'_> {
    fn drop(&mut self) {
        let mut inner = lock(&self.auth.inner);
        inner.refresh.running = false;
        inner.refresh.result = Some(
            self.result
                .take()
                .unwrap_or_else(|| Err(AuthError::Transport("the refresh stopped".into()))),
        );
        self.auth.refreshed.notify_all();
    }
}

impl Auth {
    pub fn new(http: Arc<dyn Http>, store: Arc<dyn SessionStore>, clock: Arc<dyn Clock>) -> Auth {
        Auth {
            http,
            store,
            clock,
            inner: Mutex::new(Inner {
                session: None,
                loaded: false,
                epoch: 0,
                refresh: RefreshSlot::default(),
            }),
            refreshed: Condvar::new(),
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        let mut inner = lock(&self.inner);
        if !inner.loaded {
            inner.loaded = true;
            inner.session = self.store.load();
        }
        inner
    }

    /// The saved session, if any.
    pub fn current_session(&self) -> Option<AuthSession> {
        self.inner().session.clone()
    }

    /// The current sign-in's number (see [`AccessGrant::sign_in`]).
    pub fn sign_in_epoch(&self) -> u64 {
        self.inner().epoch
    }

    // ---- Sign in ----

    /// The second half of a sign-in: the callback's code for a session.
    pub fn finish(
        &self,
        callback: &str,
        pending: &PendingSignIn,
    ) -> Result<AuthSession, AuthError> {
        let code = authorization_code(callback)?;
        self.exchange(&code, pending)
    }

    /// Trades `code` for a session without using it: the caller
    /// [`adopt`](Self::adopt)s it (saved, and used from then on) once it
    /// knows it is still wanted, or [`revoke`](Self::revoke)s it.
    pub fn exchange(&self, code: &str, pending: &PendingSignIn) -> Result<AuthSession, AuthError> {
        let body = contract::to_json(&json!({
            "auth_code": code,
            "code_verifier": pending.verifier,
        }));
        let answer = self.token_request(
            &pending.supabase_url,
            "pkce",
            body,
            &pending.publishable_key,
        )?;
        session_from_answer(
            &answer,
            &pending.supabase_url,
            &pending.publishable_key,
            &pending.website,
            self.clock.now(),
        )
    }

    /// Use `new` from now on, and save it. A save that fails leaves it
    /// signed in for this run only (the next start asks again); the engine
    /// has no log to say so in.
    pub fn adopt(&self, new: AuthSession) {
        let mut inner = lock(&self.inner);
        inner.loaded = true;
        inner.epoch += 1;
        let _ = self.store.save(&new);
        inner.session = Some(new);
    }

    /// Ends `unwanted` on Supabase (best effort), a session that was never
    /// adopted: the saved one, if any, is left alone.
    pub fn revoke(&self, unwanted: &AuthSession) {
        self.logout(unwanted);
    }

    // ---- Tokens ----

    /// An access token with more than a minute left, refreshing first when
    /// needed.
    pub fn valid_access_token(&self) -> Result<String, AuthError> {
        let current = self.inner().session.clone().ok_or(AuthError::SignedOut)?;
        if current.is_fresh(self.clock.now()) {
            return Ok(current.access_token);
        }
        self.refresh().map(|fresh| fresh.access_token)
    }

    /// [`access_grant`](Self::access_grant)'s token.
    pub fn valid_access_token_for(&self, website: &str) -> Result<String, AuthError> {
        self.access_grant(website).map(|grant| grant.token)
    }

    /// An access token for `website` with more than a minute left (refreshed
    /// first when needed), and the sign-in it belongs to. Only a session made
    /// through `website` gives one: another website never sees this one's
    /// token, even if the session was replaced during the refresh.
    pub fn access_grant(&self, website: &str) -> Result<AccessGrant, AuthError> {
        let (current, epoch) = {
            let inner = self.inner();
            let current = inner.session.clone().ok_or(AuthError::SignedOut)?;
            (current, inner.epoch)
        };
        if current.website_url != website {
            return Err(AuthError::OtherWebsite);
        }
        let grant = |token| AccessGrant {
            token,
            website: website.to_owned(),
            sign_in: epoch,
        };
        if current.is_fresh(self.clock.now()) {
            return Ok(grant(current.access_token));
        }
        let fresh = self.refresh()?;
        let now = lock(&self.inner).epoch;
        check(&fresh, website, epoch, now).map(grant)
    }

    /// After `website` answered 401 to `failed_token`, which sign-in
    /// `sign_in` gave ([`AccessGrant`]): a fresh access token of that same
    /// sign-in, for that same website. When another call already refreshed
    /// past it, that one is used. The user may have signed out and in again
    /// while the request was out: a session made through another website is
    /// never handed out (`OtherWebsite`), and neither is another sign-in's on
    /// the same website (`SignedOut`: the request's sign-in is over).
    pub fn refresh_after_unauthorized(
        &self,
        failed_token: &str,
        website: &str,
        sign_in: u64,
    ) -> Result<String, AuthError> {
        {
            let inner = self.inner();
            let current = inner.session.as_ref().ok_or(AuthError::SignedOut)?;
            let token = check(current, website, sign_in, inner.epoch)?;
            if token != failed_token && current.is_fresh(self.clock.now()) {
                return Ok(token);
            }
        }
        let fresh = self.refresh()?;
        let now = lock(&self.inner).epoch;
        check(&fresh, website, sign_in, now)
    }

    /// Refresh now. Callers on other threads share one request: the refresh
    /// token is single-use.
    pub fn refresh(&self) -> Result<AuthSession, AuthError> {
        let mut inner = self.inner();
        if inner.refresh.running {
            let round = inner.refresh.round;
            while inner.refresh.running && inner.refresh.round == round {
                inner = self
                    .refreshed
                    .wait(inner)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            return inner
                .refresh
                .result
                .clone()
                .unwrap_or(Err(AuthError::SignedOut));
        }
        let current = inner.session.clone().ok_or(AuthError::SignedOut)?;
        inner.refresh.running = true;
        inner.refresh.round += 1;
        drop(inner);
        let mut round = RoundGuard {
            auth: self,
            result: None,
        };
        let outcome = self.perform_refresh(current);
        round.result = Some(outcome.clone());
        drop(round);
        outcome
    }

    fn perform_refresh(&self, current: AuthSession) -> Result<AuthSession, AuthError> {
        let Some(supabase) = website::validated_link(Some(&current.supabase_url)) else {
            self.forget();
            return Err(AuthError::SignedOut);
        };
        let body = contract::to_json(&json!({ "refresh_token": current.refresh_token }));
        let answer =
            match self.token_request(&supabase, "refresh_token", body, &current.publishable_key) {
                Ok(answer) => answer,
                Err(AuthError::Http {
                    status,
                    code,
                    message,
                }) if refuses_refresh_token(status, code.as_deref(), message.as_deref()) => {
                    // The refresh token was refused (used, revoked, expired): the
                    // session is over, unless another one replaced it meanwhile.
                    let mut inner = lock(&self.inner);
                    if inner.session.as_ref().map(|s| &s.refresh_token)
                        == Some(&current.refresh_token)
                    {
                        self.forget_locked(&mut inner);
                    }
                    return Err(AuthError::SignedOut);
                }
                Err(other) => return Err(other),
            };
        let mut inner = lock(&self.inner);
        // Signed out (or signed in again) while the request was out: this
        // answer belongs to a session that is over.
        if inner.session.as_ref().map(|s| &s.refresh_token) != Some(&current.refresh_token) {
            return Err(AuthError::SignedOut);
        }
        let mut new = session_from_answer(
            &answer,
            &current.supabase_url,
            &current.publishable_key,
            &current.website_url,
            self.clock.now(),
        )?;
        new.user_id = new.user_id.or(current.user_id);
        new.email = new.email.or(current.email);
        // Saved before use (and before anyone else can see it): the old
        // refresh token no longer works. A failed save leaves it for this
        // run only.
        let _ = self.store.save(&new);
        inner.session = Some(new.clone());
        Ok(new)
    }

    // ---- Signing out ----

    /// Forget the session, then sign it out on Supabase (best effort; this
    /// PC's session only, not the user's other ones).
    pub fn sign_out(&self) {
        let ended = {
            let mut inner = self.inner();
            let ended = inner.session.take();
            self.forget_locked(&mut inner);
            ended
        };
        if let Some(ended) = ended {
            self.logout(&ended);
        }
    }

    /// Stop using the saved session without removing it: it was made through
    /// another website than the one set now (a dev run pointed elsewhere
    /// shares the support folder; the file stays the app's).
    pub fn set_aside(&self) {
        let mut inner = lock(&self.inner);
        inner.loaded = true;
        inner.session = None;
        inner.epoch += 1;
    }

    /// Forget the session here (no request).
    pub fn forget(&self) {
        let mut inner = lock(&self.inner);
        self.forget_locked(&mut inner);
    }

    fn forget_locked(&self, inner: &mut Inner) {
        inner.loaded = true;
        inner.session = None;
        inner.epoch += 1;
        self.store.clear();
    }

    // ---- Requests ----

    /// `POST …/logout?scope=local` with the session's own token; the answer
    /// is ignored.
    fn logout(&self, ended: &AuthSession) {
        let Some(supabase) = website::validated_link(Some(&ended.supabase_url)) else {
            return;
        };
        let Some(url) = supabase_endpoint(&supabase, "logout", &[("scope", "local")]) else {
            return;
        };
        let _ = self.http.send(HttpRequest {
            method: "POST".into(),
            url,
            headers: vec![
                ("apikey".into(), ended.publishable_key.clone()),
                (
                    "Authorization".into(),
                    format!("Bearer {}", ended.access_token),
                ),
            ],
            body: None,
            timeout: REQUEST_TIMEOUT,
        });
    }

    fn token_request(
        &self,
        supabase: &str,
        grant: &str,
        body: Vec<u8>,
        publishable_key: &str,
    ) -> Result<Map<String, Value>, AuthError> {
        let url = supabase_endpoint(supabase, "token", &[("grant_type", grant)])
            .ok_or_else(|| AuthError::BadConfig("supabaseUrl".into()))?;
        let response = self
            .http
            .send(HttpRequest {
                method: "POST".into(),
                url,
                headers: vec![
                    ("Content-Type".into(), "application/json".into()),
                    ("apikey".into(), publishable_key.to_owned()),
                ],
                body: Some(body),
                timeout: REQUEST_TIMEOUT,
            })
            .map_err(|e| AuthError::Transport(describe_http_error(&e)))?;
        let object = match serde_json::from_slice::<Value>(&response.body) {
            Ok(Value::Object(object)) => Some(object),
            _ => None,
        };
        if !(200..300).contains(&response.status) {
            let field = |key: &str| {
                object
                    .as_ref()
                    .and_then(|o| o.get(key))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            let message = field("error_description")
                .or_else(|| field("msg"))
                .or_else(|| field("message"))
                .or_else(|| field("error"));
            let code = field("error_code").or_else(|| field("error"));
            return Err(AuthError::Http {
                status: response.status,
                code,
                message,
            });
        }
        object.ok_or_else(|| AuthError::BadResponse("not JSON".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_keep_the_projects_path() {
        assert_eq!(
            supabase_endpoint("https://x.supabase.co", "token", &[("grant_type", "pkce")])
                .as_deref(),
            Some("https://x.supabase.co/auth/v1/token?grant_type=pkce")
        );
        assert_eq!(
            supabase_endpoint("https://x.example.com/sb/", "logout", &[("scope", "local")])
                .as_deref(),
            Some("https://x.example.com/sb/auth/v1/logout?scope=local")
        );
    }

    #[test]
    fn a_session_prints_without_its_tokens() {
        let session = AuthSession {
            access_token: "secret-access".into(),
            refresh_token: "secret-refresh".into(),
            expires_at: UNIX_EPOCH,
            user_id: None,
            email: None,
            supabase_url: "https://x.supabase.co".into(),
            publishable_key: "pk".into(),
            website_url: "https://w.example.com".into(),
        };
        let printed = format!("{session:?}");
        assert!(!printed.contains("secret"), "{printed}");
    }

    #[test]
    fn percent_decoding_refuses_what_foundation_refuses() {
        assert_eq!(percent_decoded("a%20b").as_deref(), Some("a b"));
        assert_eq!(percent_decoded("%zz"), None);
        assert_eq!(percent_decoded("%+f"), None);
        assert_eq!(percent_decoded("%f"), None);
        assert_eq!(percent_decoded("%ff"), None);
        assert_eq!(percent_decoded("%C3%A9").as_deref(), Some("é"));
    }
}
