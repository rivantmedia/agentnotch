//! The testkit builds a whole platform over a temporary root, and its fakes
//! record what the engine did.

use agentnotch_engine::platform::*;
use agentnotch_engine::testkit::{self, Script, TEST_START_MS};
use std::time::Duration;

#[test]
fn a_platform_of_fakes() {
    let root = tempfile::tempdir().unwrap();
    let (platform, handles) = testkit::platform(root.path());
    assert!(handles.roots.home.starts_with(root.path()) && handles.roots.home.is_dir());
    assert!(
        !handles.roots.support.exists(),
        "the engine creates <support> itself, privately"
    );
    assert_eq!(
        agentnotch_engine::core::time::to_ms(platform.clock.now()),
        TEST_START_MS
    );
    handles.clock.advance(Duration::from_secs(90));
    assert_eq!(
        agentnotch_engine::core::time::to_ms(platform.clock.now()),
        TEST_START_MS + 90_000
    );

    // Processes.
    handles
        .processes
        .add(4242, 1, "claude.exe", platform.clock.now());
    assert_eq!(platform.processes.liveness(4242), Liveness::Alive);
    assert_eq!(platform.processes.liveness(7), Liveness::Gone);
    assert_eq!(platform.processes.config_dir_env(4242), EnvRead::Unreadable);

    // Transport: frames in, answers out.
    let (tx, rx) = crossbeam_channel::unbounded();
    platform
        .transport
        .start(r"\\.\pipe\agentnotch-test", tx)
        .unwrap();
    assert!(matches!(rx.recv().unwrap(), TransportEvent::Listening(_)));
    let conn = handles
        .transport
        .inject(b"{}".to_vec(), platform.clock.now());
    assert!(matches!(rx.recv().unwrap(), TransportEvent::Frame(f) if f.conn == conn));
    assert!(platform
        .transport
        .respond(conn, b"{\"decision\":\"allow\"}".to_vec()));
    assert_eq!(handles.transport.responses().len(), 1);

    // Runner: scripts, never a real process.
    let spec = CommandSpec {
        program: "claude.exe".into(),
        args: vec!["-p".into()],
        env: vec![],
        cwd: root.path().into(),
    };
    assert!(
        platform.runner.spawn(spec.clone()).is_err(),
        "no script, no run"
    );
    handles
        .runner
        .push(Script::ok(b"{\"type\":\"result\"}\n".to_vec()));
    let mut child = platform.runner.spawn(spec).unwrap();
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.take_stdout().unwrap(), &mut out).unwrap();
    assert!(out.contains("result"));
    assert_eq!(
        child.wait_timeout(Duration::from_secs(1)).unwrap(),
        Some(Exit::Code(0))
    );
    assert_eq!(handles.runner.spawned().len(), 2);

    // Toasts and HTTP are recorded, never sent.
    let toast = Toast {
        tag: "needs".into(),
        group: "s1".into(),
        kind: ToastKind::NeedsInput,
        title: "t".into(),
        subtitle: None,
        body: "b".into(),
        launch_url: "agentnotch://open?session=s1".into(),
        actions: vec![],
    };
    platform.notifier.post(&toast);
    assert_eq!(handles.notifier.posted(), [toast]);
    let request = HttpRequest {
        method: "GET".into(),
        url: "https://example.test/".into(),
        headers: vec![],
        body: None,
        timeout: Duration::from_secs(1),
    };
    assert!(matches!(
        platform.http.send(request),
        Err(HttpError::Connect(_))
    ));
    handles.http.push_json(200, "{}");
    let request = HttpRequest {
        method: "GET".into(),
        url: "https://example.test/".into(),
        headers: vec![],
        body: None,
        timeout: Duration::from_secs(1),
    };
    assert_eq!(platform.http.send(request).unwrap().status, 200);
    assert_eq!(handles.http.requests().len(), 2);
    assert!(platform.device.user_sid().unwrap().starts_with("S-1-5-21-"));
}
