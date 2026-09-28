//! Canned HTTP, a browser that only records, and a fixed device: tests never
//! reach a real website or Supabase project.
//!
//! Owner after WP0: WP8.

use super::lock;
use crate::platform::{Browser, Device, Http, HttpError, HttpRequest, HttpResponse};
use std::collections::VecDeque;
use std::sync::Mutex;

#[derive(Default)]
pub struct FixtureHttp {
    responses: Mutex<VecDeque<Result<HttpResponse, HttpError>>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl FixtureHttp {
    /// The next request gets this.
    pub fn push(&self, response: Result<HttpResponse, HttpError>) {
        lock(&self.responses).push_back(response);
    }

    pub fn push_json(&self, status: u16, body: &str) {
        self.push(Ok(HttpResponse {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.as_bytes().to_vec(),
        }));
    }

    /// Every request sent, in order.
    pub fn requests(&self) -> Vec<HttpRequest> {
        lock(&self.requests).clone()
    }
}

impl Http for FixtureHttp {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        lock(&self.requests).push(req);
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
