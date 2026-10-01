//! The website's API (`web/contract/README.md`; the Mac's `CloudAPI` and
//! `CloudAPIError`, `CloudHTTP.swift`): `config` (no sign-in), `me` and
//! `sync` (a Supabase access token as Bearer, only ever the token of a
//! session made through this same website). A 401 is retried once after a
//! refresh, with a token of the sign-in the request started with (one made
//! since, or through another website, is never sent); a second one is the
//! caller's to back off from (the session is kept: only Supabase refusing
//! the refresh token ends it).
//!
//! Calls block: the cloud thread makes them, and the platform's [`Http`]
//! follows no redirect, keeps no cookie and never caches.

use super::auth::{describe_http_error, Auth, AuthError};
use super::contract::{
    self, error_code, ConfigResponse, ErrorBody, MeResponse, SyncRequest, SyncResponse,
};
use super::website;
use crate::platform::{Clock, Http, HttpRequest, HttpResponse, SystemClock};
use serde::de::DeserializeOwned;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// How long a call to the website may take (the Mac's request timeout).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A failed call to the website or to Supabase's auth server.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// Sealed, or the engine isn't bootstrapped.
    NetworkNotAllowed,
    /// This build has no website (or none the app accepts).
    NoWebsite,
    /// Not signed in (or the session ended).
    NotSignedIn,
    /// The request didn't get an answer.
    Transport(String),
    /// The website answered with an error (the contract's error shape when
    /// it sent one). `retry_after` is in seconds.
    Server {
        status: u16,
        code: Option<String>,
        message: Option<String>,
        retry_after: Option<f64>,
    },
    /// An answer that doesn't follow the contract.
    BadResponse(String),
    /// The sign-in gave no token for this website (signed out, another
    /// website's session) or its refresh failed.
    Auth(AuthError),
}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> ApiError {
        ApiError::Auth(error)
    }
}

impl ApiError {
    /// The contract's error shape from a failed response (the status alone
    /// when the body isn't one). `retry_after_header` is the `Retry-After`
    /// header as sent: seconds. A value that isn't a finite, non-negative
    /// number (an HTTP date, `nan`) counts as none, so a caller can always
    /// wait for it.
    pub fn from_response(status: u16, body: &[u8], retry_after_header: Option<&str>) -> ApiError {
        let parsed: Option<ErrorBody> = serde_json::from_slice(body).ok();
        let retry_after = retry_after_header
            .map(super::contract::trim_spaces)
            .and_then(|text| text.parse::<f64>().ok())
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0);
        ApiError::Server {
            status,
            code: parsed.as_ref().map(|b| b.error.code.clone()),
            message: parsed.map(|b| b.error.message),
            retry_after,
        }
    }

    /// A 401, an `UNAUTHORIZED` code, or no sign-in at all.
    pub fn is_unauthorized(&self) -> bool {
        match self {
            ApiError::Server { status, code, .. } => {
                *status == 401 || code.as_deref() == Some(error_code::UNAUTHORIZED)
            }
            other => *other == ApiError::NotSignedIn,
        }
    }

    /// The call found the sign-in over: no session, or one made through
    /// another website (the service signs out then, CL§5.8).
    pub fn ends_sign_in(&self) -> bool {
        matches!(
            self,
            ApiError::NotSignedIn
                | ApiError::Auth(AuthError::SignedOut)
                | ApiError::Auth(AuthError::OtherWebsite)
        )
    }

    pub fn is_rate_limited(&self) -> bool {
        match self {
            ApiError::Server { status, code, .. } => {
                *status == 429 || code.as_deref() == Some(error_code::RATE_LIMITED)
            }
            _ => false,
        }
    }

    /// Seconds the website asked to wait, when it said.
    pub fn retry_after(&self) -> Option<f64> {
        match self {
            ApiError::Server { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::NetworkNotAllowed => {
                f.write_str("The website can't be reached from this run.")
            }
            ApiError::NoWebsite => f.write_str("This build has no website to sign in to."),
            ApiError::NotSignedIn => f.write_str("Sign in to the website first."),
            ApiError::Transport(message) => write!(f, "Couldn't reach the website: {message}"),
            ApiError::Server {
                status, message, ..
            } => match message {
                Some(message) if !message.is_empty() => f.write_str(message),
                _ => write!(f, "The website answered {status}."),
            },
            ApiError::BadResponse(message) => {
                write!(f, "Unexpected answer from the website: {message}")
            }
            ApiError::Auth(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ApiError {}

/// A response header's value (names compared without case).
fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// The website's three endpoints.
pub struct CloudApi {
    /// The website, as [`website::validated`] keeps it.
    pub website: String,
    http: Arc<dyn Http>,
    auth: Option<Arc<Auth>>,
    user_agent: String,
    clock: Arc<dyn Clock>,
}

impl CloudApi {
    /// `auth` `None`: only [`config`](Self::config) works.
    pub fn new(
        website: impl Into<String>,
        http: Arc<dyn Http>,
        auth: Option<Arc<Auth>>,
        app_version: &str,
    ) -> CloudApi {
        CloudApi {
            website: website.into(),
            http,
            auth,
            user_agent: format!("AgentNotch/{app_version}"),
            clock: Arc::new(SystemClock),
        }
    }

    /// The clock a sync request is clamped against (its dates must lie
    /// between 2023 and a day from now).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> CloudApi {
        self.clock = clock;
        self
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// `GET /api/app/v1/config`.
    pub fn config(&self) -> Result<ConfigResponse, ApiError> {
        let response = self.send(self.request(contract::path::CONFIG, "GET", None, None))?;
        Self::answer(response)
    }

    /// `GET /api/app/v1/me`.
    pub fn me(&self) -> Result<MeResponse, ApiError> {
        self.authorized(contract::path::ME, "GET", None)
    }

    /// `POST /api/app/v1/sync` with the request clamped to the contract's
    /// limits.
    pub fn sync(&self, request: &SyncRequest) -> Result<SyncResponse, ApiError> {
        let body = contract::to_json(&request.clamped(self.clock.now()));
        self.authorized(contract::path::SYNC, "POST", Some(body))
    }

    /// An authenticated call: the current access token (refreshed when it
    /// has a minute or less left) of a session made through this website,
    /// and on a 401 one retry after a refresh, with a token of the same
    /// sign-in for the same website (the user may have signed in elsewhere
    /// since).
    fn authorized<T: DeserializeOwned>(
        &self,
        path: &str,
        method: &str,
        body: Option<Vec<u8>>,
    ) -> Result<T, ApiError> {
        let Some(auth) = &self.auth else {
            return Err(ApiError::NotSignedIn);
        };
        let grant = auth.access_grant(&self.website)?;
        let mut response =
            self.send(self.request(path, method, body.clone(), Some(&grant.token)))?;
        if response.status == 401 {
            let fresh =
                auth.refresh_after_unauthorized(&grant.token, &grant.website, grant.sign_in)?;
            response = self.send(self.request(path, method, body, Some(&fresh)))?;
        }
        Self::answer(response)
    }

    fn request(
        &self,
        path: &str,
        method: &str,
        body: Option<Vec<u8>>,
        token: Option<&str>,
    ) -> HttpRequest {
        let mut headers = vec![
            ("Accept".to_owned(), "application/json".to_owned()),
            ("User-Agent".to_owned(), self.user_agent.clone()),
        ];
        if body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        if let Some(token) = token {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
        HttpRequest {
            method: method.to_owned(),
            url: website::endpoint(&self.website, path),
            headers,
            body,
            timeout: REQUEST_TIMEOUT,
        }
    }

    fn send(&self, request: HttpRequest) -> Result<HttpResponse, ApiError> {
        self.http
            .send(request)
            .map_err(|e| ApiError::Transport(describe_http_error(&e)))
    }

    /// A 2xx's body as `T`; anything else as the contract's error.
    fn answer<T: DeserializeOwned>(response: HttpResponse) -> Result<T, ApiError> {
        if !(200..300).contains(&response.status) {
            return Err(ApiError::from_response(
                response.status,
                &response.body,
                header(&response, "Retry-After"),
            ));
        }
        serde_json::from_slice(&response.body).map_err(|e| {
            let name = std::any::type_name::<T>().rsplit("::").next().unwrap_or("");
            ApiError::BadResponse(format!("{name}: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_that_is_not_the_error_shape_leaves_the_status() {
        let error = ApiError::from_response(502, b"<html>bad gateway</html>", None);
        assert_eq!(
            error,
            ApiError::Server {
                status: 502,
                code: None,
                message: None,
                retry_after: None
            }
        );
        assert_eq!(error.to_string(), "The website answered 502.");
        assert!(!error.is_unauthorized() && !error.is_rate_limited());
    }

    #[test]
    fn retry_after_takes_seconds_only() {
        let after = |header: &str| ApiError::from_response(429, b"", Some(header)).retry_after();
        assert_eq!(after("12"), Some(12.0));
        assert_eq!(after(" 1.5 "), Some(1.5));
        assert_eq!(after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(after("nan"), None);
        assert_eq!(after("-3"), None);
        assert!(ApiError::from_response(429, b"", None).is_rate_limited());
    }

    #[test]
    fn unauthorized_by_status_or_code() {
        let by_code = br#"{"error":{"code":"UNAUTHORIZED","message":"x"}}"#;
        assert!(ApiError::from_response(400, by_code, None).is_unauthorized());
        assert!(ApiError::from_response(401, b"", None).is_unauthorized());
        assert!(ApiError::NotSignedIn.is_unauthorized());
        assert!(!ApiError::NoWebsite.is_unauthorized());
    }
}
