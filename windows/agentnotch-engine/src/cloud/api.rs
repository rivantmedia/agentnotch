//! What can go wrong talking to the website (CL§2; the Mac's
//! `CloudAPIError`, `CloudHTTP.swift`). The HTTP client and the calls built
//! on it join this file in later steps; the error type comes first because
//! the contract tests decode `error.json` through it.

use super::contract::{error_code, ErrorBody};
use std::fmt;

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
        }
    }
}

impl std::error::Error for ApiError {}

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
