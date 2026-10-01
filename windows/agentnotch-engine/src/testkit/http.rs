//! Canned HTTP, a browser that only records, and a fixed device: tests never
//! reach a real website or Supabase project.
//!
//! Owner after WP0: WP8.

use super::lock;
use crate::platform::{Browser, Device, Http, HttpError, HttpRequest, HttpResponse};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Answers one request. `Fn`, not `FnMut`: it may be called again while it
/// runs (from another thread, or from inside itself when it calls into the
/// code under test while "the request is out"); keep its state in a
/// `Mutex` of its own.
pub type HttpHandler = Arc<dyn Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + Sync>;

/// Canned answers: a handler that answers by request when one is set,
/// else a queue taken in order (empty: a connect error).
#[derive(Default)]
pub struct FixtureHttp {
    responses: Mutex<VecDeque<Result<HttpResponse, HttpError>>>,
    requests: Mutex<Vec<HttpRequest>>,
    handler: Mutex<Option<HttpHandler>>,
}

/// A JSON answer.
pub fn json_response(status: u16, body: impl AsRef<[u8]>) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
        body: body.as_ref().to_vec(),
    }
}

/// A request header's value (names compared without case).
pub fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// A request URL's path (`/auth/v1/token`), or "" when it isn't a URL.
pub fn path_of(request: &HttpRequest) -> String {
    url::Url::parse(&request.url)
        .map(|url| url.path().to_owned())
        .unwrap_or_default()
}

/// A request URL's query, as sent.
pub fn query_of(request: &HttpRequest) -> Option<String> {
    url::Url::parse(&request.url)
        .ok()
        .and_then(|url| url.query().map(str::to_owned))
}

/// A request body as JSON (`Null` when there is none or it isn't JSON).
pub fn body_json(request: &HttpRequest) -> serde_json::Value {
    request
        .body
        .as_deref()
        .and_then(|body| serde_json::from_slice(body).ok())
        .unwrap_or(serde_json::Value::Null)
}

impl FixtureHttp {
    /// Every later request is answered by `handler` (the queue is skipped).
    pub fn set_handler(
        &self,
        handler: impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + Sync + 'static,
    ) {
        *lock(&self.handler) = Some(Arc::new(handler));
    }

    /// Back to the queue.
    pub fn clear_handler(&self) {
        *lock(&self.handler) = None;
    }

    /// The requests sent to this URL path, in order.
    pub fn requests_to(&self, path: &str) -> Vec<HttpRequest> {
        self.requests()
            .into_iter()
            .filter(|request| path_of(request) == path)
            .collect()
    }

    /// The next request gets this.
    pub fn push(&self, response: Result<HttpResponse, HttpError>) {
        lock(&self.responses).push_back(response);
    }

    pub fn push_json(&self, status: u16, body: &str) {
        self.push(Ok(json_response(status, body)));
    }

    /// Every request sent, in order.
    pub fn requests(&self) -> Vec<HttpRequest> {
        lock(&self.requests).clone()
    }
}

impl Http for FixtureHttp {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        lock(&self.requests).push(req.clone());
        // Called with no lock held: the handler may send again.
        let handler = lock(&self.handler).clone();
        if let Some(handler) = handler {
            return handler(&req);
        }
        lock(&self.responses)
            .pop_front()
            .unwrap_or_else(|| Err(HttpError::Connect("no fixture response".into())))
    }
}

#[derive(Default)]
pub struct RecordingBrowser {
    opened: Mutex<Vec<String>>,
}

impl RecordingBrowser {
    pub fn opened(&self) -> Vec<String> {
        lock(&self.opened).clone()
    }
}

impl Browser for RecordingBrowser {
    fn open(&self, url: &str) -> Result<(), String> {
        lock(&self.opened).push(url.to_owned());
        Ok(())
    }
}

/// `TEST-PC`, a made-up SID, not elevated.
#[derive(Debug, Default)]
pub struct FakeDevice;

impl Device for FakeDevice {
    fn computer_name(&self) -> String {
        "TEST-PC".into()
    }

    fn user_sid(&self) -> Option<String> {
        Some("S-1-5-21-1000000001-1000000002-1000000003-1001".into())
    }

    fn elevated(&self) -> bool {
        false
    }

    fn smart_app_control(&self) -> Option<String> {
        None
    }
}
