//! The real ureq/schannel client (`UreqHttp`) against a local HTTP/1.1 test server (DESIGN-WIN
//! §7, `win_http.rs`): the contract's headers arrive exactly, no cookie is ever sent back, gzip is
//! decoded, a deadline is a `Timeout`, a refused port is `Connect`, a redirect is never followed,
//! and every status comes back as a response for the engine to judge.
//!
//! Plain http to 127.0.0.1 is enough: the contract allows it for local websites, and the cloud
//! never needs TLS to a local server.

#![cfg(windows)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use agentnotch_engine::platform::{Http, HttpError, HttpRequest, HttpResponse};
use agentnotch_win::http::{UreqHttp, MAX_BODY_BYTES};

/// `{"ok":true,"note":"decoded from gzip"}`, made once with
/// `python3 -c 'import gzip; print(list(gzip.compress(b"…", mtime=0)))'`.
const GZIP_BODY: [u8; 58] = [
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0xab, 0x56, 0xca, 0xcf, 0x56, 0xb2,
    0x2a, 0x29, 0x2a, 0x4d, 0xd5, 0x51, 0xca, 0xcb, 0x2f, 0x49, 0x55, 0xb2, 0x52, 0x4a, 0x49, 0x4d,
    0xce, 0x4f, 0x49, 0x4d, 0x51, 0x48, 0x2b, 0xca, 0xcf, 0x55, 0x48, 0xaf, 0xca, 0x2c, 0x50, 0xaa,
    0x05, 0x00, 0x4e, 0xb2, 0x9c, 0x99, 0x26, 0x00, 0x00, 0x00,
];
const GZIP_TEXT: &str = r#"{"ok":true,"note":"decoded from gzip"}"#;

/// How long a test server waits for the connections it expects before it gives up, so a failing
/// test ends instead of hanging.
const SERVER_PATIENCE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
struct Captured {
    method: String,
    path: String,
    /// Names lower-cased, in arrival order.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Captured {
    fn all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

/// A scripted response: the status line and headers (Content-Length and Connection: close are
/// added), then the body.
fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn read_request(stream: &mut TcpStream) -> Option<Captured> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let path = request_line.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(n, v)| (n.trim().to_lowercase(), v.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Captured {
        method,
        path,
        headers,
        body,
    })
}

/// Accepts one connection per scripted response, answers each request with the next response and
/// closes the connection; returns what it received. Gives up after `SERVER_PATIENCE`.
fn serve(responses: Vec<Vec<u8>>) -> (SocketAddr, JoinHandle<Vec<Captured>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    listener.set_nonblocking(true).expect("non-blocking");
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + SERVER_PATIENCE;
        let mut captured = Vec::new();
        for reply in responses {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                    Err(_) => break None,
                }
            };
            let Some(stream) = stream.as_mut() else { break };
            stream.set_nonblocking(false).expect("blocking");
            if let Some(request) = read_request(stream) {
                captured.push(request);
            }
            // The client may stop reading early (the body-cap test): a failed write is expected.
            let _ = stream.write_all(&reply);
            let _ = stream.flush();
        }
        captured
    });
    (addr, handle)
}

/// Counts the connections made to `listener` for `window`.
fn count_connections(listener: TcpListener, window: Duration) -> JoinHandle<usize> {
    listener.set_nonblocking(true).expect("non-blocking");
    thread::spawn(move || {
        let deadline = Instant::now() + window;
        let mut count = 0;
        while Instant::now() < deadline {
            if listener.accept().is_ok() {
                count += 1;
            }
            thread::sleep(Duration::from_millis(5));
        }
        count
    })
}

fn request(
    method: &str,
    url: String,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> HttpRequest {
    HttpRequest {
        method: method.to_owned(),
        url,
        headers: headers
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect(),
        body: body.map(<[u8]>::to_vec),
        timeout: Duration::from_secs(10),
    }
}

fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

const CONTRACT_HEADERS: [(&str, &str); 5] = [
    ("Accept", "application/json"),
    ("User-Agent", "AgentNotch/9.9.9"),
    ("Content-Type", "application/json"),
    ("Authorization", "Bearer t"),
    ("apikey", "anon-key"),
];

#[test]
fn the_contract_headers_and_body_arrive_exactly() {
    let (addr, server) = serve(vec![response(
        "200 OK",
        &[("Content-Type", "application/json")],
        br#"{"ok":true}"#,
    )]);
    let body = br#"{"schemaVersion":1,"sessions":[]}"#;
    let answer = UreqHttp::new()
        .send(request(
            "POST",
            format!("http://{addr}/api/app/v1/sync"),
            &CONTRACT_HEADERS,
            Some(body),
        ))
        .expect("answer");
    assert_eq!(answer.status, 200);
    assert_eq!(answer.body, br#"{"ok":true}"#);
    assert_eq!(header(&answer, "content-type"), Some("application/json"));

    let seen = server.join().expect("server");
    assert_eq!(seen.len(), 1);
    let seen = &seen[0];
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.path, "/api/app/v1/sync");
    assert_eq!(seen.body, body);
    for (name, value) in CONTRACT_HEADERS {
        assert_eq!(seen.all(&name.to_lowercase()), vec![value], "{name}");
    }
    // Beyond the engine's headers, only what HTTP/1.1 itself needs (and the gzip offer whose
    // answer the client decodes).
    let allowed = [
        "accept",
        "user-agent",
        "content-type",
        "authorization",
        "apikey",
        "host",
        "content-length",
        "accept-encoding",
    ];
    for (name, _) in &seen.headers {
        assert!(allowed.contains(&name.as_str()), "unexpected header {name}");
    }
    assert_eq!(seen.all("host"), vec![addr.to_string().as_str()]);
}

#[test]
fn a_set_cookie_is_never_sent_back() {
    let (addr, server) = serve(vec![
        response(
            "200 OK",
            &[("Set-Cookie", "sb-session=secret; Path=/; HttpOnly")],
            b"{}",
        ),
        response("200 OK", &[], b"{}"),
    ]);
    let http = UreqHttp::new();
    let first = http
        .send(request(
            "GET",
            format!("http://{addr}/api/app/v1/config"),
            &CONTRACT_HEADERS,
            None,
        ))
        .expect("first");
    assert_eq!(
        header(&first, "set-cookie"),
        Some("sb-session=secret; Path=/; HttpOnly")
    );
    http.send(request(
        "GET",
        format!("http://{addr}/api/app/v1/me"),
        &CONTRACT_HEADERS,
        None,
    ))
    .expect("second");

    let seen = server.join().expect("server");
    assert_eq!(seen.len(), 2);
    for request in &seen {
        assert!(request.all("cookie").is_empty(), "{:?}", request.headers);
    }
}

#[test]
fn a_gzip_body_is_decoded() {
    let (addr, server) = serve(vec![response(
        "200 OK",
        &[
            ("Content-Type", "application/json"),
            ("Content-Encoding", "gzip"),
        ],
        &GZIP_BODY,
    )]);
    let answer = UreqHttp::new()
        .send(request(
            "GET",
            format!("http://{addr}/api/app/v1/config"),
            &CONTRACT_HEADERS,
            None,
        ))
        .expect("answer");
    assert_eq!(answer.status, 200);
    assert_eq!(String::from_utf8_lossy(&answer.body), GZIP_TEXT);
    server.join().expect("server");
}

#[test]
fn a_server_that_never_answers_is_a_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("address");
    // Accepts, then holds the connection open in silence until the client gives up.
    let silent = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut sink = [0u8; 4096];
            while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
        }
    });
    let mut req = request(
        "GET",
        format!("http://{addr}/api/app/v1/me"),
        &CONTRACT_HEADERS,
        None,
    );
    req.timeout = Duration::from_millis(300);
    let started = Instant::now();
    let result = UreqHttp::new().send(req);
    let elapsed = started.elapsed();
    assert_eq!(result, Err(HttpError::Timeout));
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
    silent.join().expect("server");
}

#[test]
fn a_refused_port_is_a_connect_error() {
    let addr = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("address")
    };
    // Windows retries a refused loopback connection for about two seconds before reporting it,
    // so the deadline is long enough for the refusal to arrive as itself.
    let result = UreqHttp::new().send(request(
        "GET",
        format!("http://{addr}/api/app/v1/config"),
        &CONTRACT_HEADERS,
        None,
    ));
    assert!(matches!(result, Err(HttpError::Connect(_))), "{result:?}");
}

#[test]
fn a_redirect_is_returned_not_followed() {
    let elsewhere = TcpListener::bind("127.0.0.1:0").expect("bind");
    let elsewhere_addr = elsewhere.local_addr().expect("address");
    let location = format!("http://{elsewhere_addr}/steal");
    let (addr, server) = serve(vec![response(
        "302 Found",
        &[("Location", location.as_str())],
        b"",
    )]);
    let visits = count_connections(elsewhere, Duration::from_secs(1));
    let answer = UreqHttp::new()
        .send(request(
            "POST",
            format!("http://{addr}/api/app/v1/sync"),
            &CONTRACT_HEADERS,
            Some(b"{}"),
        ))
        .expect("answer");
    assert_eq!(answer.status, 302);
    assert_eq!(header(&answer, "location"), Some(location.as_str()));
    assert_eq!(server.join().expect("server").len(), 1);
    assert_eq!(visits.join().expect("counter"), 0);
}

#[test]
fn error_statuses_come_back_as_responses() {
    let unauthorized = br#"{"error":{"code":"UNAUTHORIZED","message":"Sign in again."}}"#;
    let unavailable = br#"{"error":{"code":"INTERNAL","message":"Try again later."}}"#;
    let (addr, server) = serve(vec![
        response(
            "401 Unauthorized",
            &[("Content-Type", "application/json")],
            unauthorized,
        ),
        response(
            "503 Service Unavailable",
            &[("Content-Type", "application/json"), ("Retry-After", "120")],
            unavailable,
        ),
    ]);
    let http = UreqHttp::new();
    let first = http
        .send(request(
            "GET",
            format!("http://{addr}/api/app/v1/me"),
            &CONTRACT_HEADERS,
            None,
        ))
        .expect("401 is a response");
    assert_eq!(first.status, 401);
    assert_eq!(first.body, unauthorized);
    assert_eq!(header(&first, "content-type"), Some("application/json"));

    let second = http
        .send(request(
            "POST",
            format!("http://{addr}/api/app/v1/sync"),
            &CONTRACT_HEADERS,
            Some(b"{}"),
        ))
        .expect("503 is a response");
    assert_eq!(second.status, 503);
    assert_eq!(second.body, unavailable);
    assert_eq!(header(&second, "retry-after"), Some("120"));
    assert_eq!(server.join().expect("server").len(), 2);
}

#[test]
fn an_oversized_body_is_refused() {
    let body = vec![b'x'; MAX_BODY_BYTES as usize + 1];
    let (addr, server) = serve(vec![response("200 OK", &[], &body)]);
    let result = UreqHttp::new().send(request(
        "GET",
        format!("http://{addr}/api/app/v1/config"),
        &CONTRACT_HEADERS,
        None,
    ));
    assert!(
        matches!(result, Err(HttpError::Other(_))),
        "{:?}",
        result.map(|r| r.status)
    );
    server.join().expect("server");
}
