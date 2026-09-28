//! HTTPS for cloud sync (DESIGN-WIN §3.2 `Http`, §4.11; WP8): ureq over native-tls (schannel), no
//! cookies, no cache, redirects never followed to another host, the contract's headers.
//!
//! Not implemented in this build: every request fails, so sync reports its error and retries
//! later; nothing leaves the PC.

use agentnotch_engine::platform::{Http, HttpError, HttpRequest, HttpResponse};

use crate::NOT_IMPLEMENTED;

#[derive(Debug, Default)]
pub struct UreqHttp;

impl UreqHttp {
    pub fn new() -> Self {
        UreqHttp
    }
}

impl Http for UreqHttp {
    fn send(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::Other(format!("HTTPS is {NOT_IMPLEMENTED}")))
    }
}
