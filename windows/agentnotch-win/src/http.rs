//! HTTPS for cloud sync (DESIGN-WIN §3.2 `Http`, §4.11; WP8): ureq over native-tls (schannel), no
//! cookies, no cache, redirects never followed to another host, the contract's headers.
//!
//! The client is deliberately thin: the engine builds every header and body and judges every
//! status, so this file only moves bytes and sorts transport failures into `HttpError`'s kinds.

#![cfg(windows)]

use std::collections::HashMap;
use std::error::Error as _;
use std::io::{self, Read};
use std::sync::Arc;

use agentnotch_engine::platform::{Http, HttpError, HttpRequest, HttpResponse};

/// The most a response body may hold. The website's answers are a few KiB; anything near this is
/// not the contract, and reading it whole would only cost memory.
pub const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

pub struct UreqHttp {
    /// `Err` when schannel couldn't give a TLS connector: every request then reports it as a TLS
    /// failure, where ureq's own default would panic.
    agent: Result<ureq::Agent, String>,
}

impl std::fmt::Debug for UreqHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UreqHttp")
            .field("ready", &self.agent.is_ok())
            .finish()
    }
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqHttp {
    pub fn new() -> Self {
        let agent = native_tls::TlsConnector::new()
            .map(|tls| {
                ureq::AgentBuilder::new()
                    // The OS's TLS stack, named explicitly: the app crate enables ureq's rustls
                    // feature too, and with both ureq would pick rustls.
                    .tls_connector(Arc::new(tls))
                    // A 3xx comes back to the engine as a response, so no request (and no
                    // Authorization or apikey header) ever reaches a host the engine didn't name.
                    .redirects(0)
                    // An HTTP(S)_PROXY variable must not route the sign-in through another
                    // machine.
                    .try_proxy_from_env(false)
                    .build()
            })
            .map_err(|e| e.to_string());
        // No cookie store: ureq 2 keeps cookies only with its `cookies` feature, which nothing in
        // the workspace enables (win_http.rs checks that no Cookie header is ever sent back).
        UreqHttp { agent }
    }
}

impl Http for UreqHttp {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let agent = self.agent.as_ref().map_err(|e| HttpError::Tls(e.clone()))?;
        // Only the engine's headers: ureq adds Host, Content-Length and Accept-Encoding (gzip,
        // which it then decodes), and its own User-Agent/Accept only when the engine sent none.
        let mut request = agent.request(&req.method, &req.url).timeout(req.timeout);
        for (name, value) in &req.headers {
            request = request.set(name, value);
        }
        let result = match &req.body {
            Some(body) => request.send_bytes(body),
            None => request.call(),
        };
        match result {
            Ok(response) => read_response(response),
            // Every status is the engine's to judge (a 401 refreshes, a 429 backs off, …).
            Err(ureq::Error::Status(_, response)) => read_response(response),
            Err(ureq::Error::Transport(transport)) => Err(transport_error(&transport)),
        }
    }
}

fn read_response(response: ureq::Response) -> Result<HttpResponse, HttpError> {
    let status = response.status();
    let headers = headers_of(&response);
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|e| io_error(&e))?;
    if body.len() as u64 > MAX_BODY_BYTES {
        return Err(HttpError::Other(format!(
            "the response body is larger than {} MiB",
            MAX_BODY_BYTES / (1024 * 1024)
        )));
    }
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

/// Every header line in arrival order, names lower-cased, a repeated name kept as separate pairs.
fn headers_of(response: &ureq::Response) -> Vec<(String, String)> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut headers = Vec::new();
    for name in response.headers_names() {
        let index = seen.entry(name.clone()).or_insert(0);
        if let Some(value) = response.all(&name).get(*index) {
            headers.push((name.clone(), (*value).to_owned()));
        }
        *index += 1;
    }
    headers
}

fn is_timeout(error: &io::Error) -> bool {
    // ureq turns a socket read past its deadline into TimedOut on some paths and leaves Windows'
    // WouldBlock on others.
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

fn io_error(error: &io::Error) -> HttpError {
    if is_timeout(error) {
        HttpError::Timeout
    } else {
        HttpError::Other(error.to_string())
    }
}

fn transport_error(transport: &ureq::Transport) -> HttpError {
    let message = transport.to_string();
    // The cause decides first: a deadline is a timeout at whatever stage it hit, and ureq reports
    // a failed TLS handshake as a failed connection.
    let mut source = transport.source();
    while let Some(cause) = source {
        if cause.downcast_ref::<io::Error>().is_some_and(is_timeout) {
            return HttpError::Timeout;
        }
        if cause.downcast_ref::<native_tls::Error>().is_some() {
            return HttpError::Tls(message);
        }
        source = cause.source();
    }
    match transport.kind() {
        ureq::ErrorKind::Dns
        | ureq::ErrorKind::ConnectionFailed
        | ureq::ErrorKind::ProxyConnect => HttpError::Connect(message),
        _ => HttpError::Other(message),
    }
}
