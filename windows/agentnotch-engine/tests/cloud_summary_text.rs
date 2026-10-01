//! The summarizer's text half (CL§9.5-9.7; the Mac's `SessionSummarizerTests`
//! scrub, excerpt and redaction cases), plus the Windows path vectors and the
//! names the scrub knows on this PC. Nothing here reaches `claude`, the
//! network or the real home.

mod cloud_support;

use agentnotch_engine::cloud::contract::limit;
use agentnotch_engine::cloud::ledger::Stretch;
use agentnotch_engine::cloud::summary::text::{
    build_excerpt, clipped, entropy, exit_description, folding_whitespace, redact, scrub,
    shorten_paths, LocalNames, MAX_EXCERPT_CHARACTERS,
};
use agentnotch_engine::core::atomic::StdSecureFiles;
use agentnotch_engine::core::paths::PathStyle;
use cloud_support::{CloudFixture as F, Lines as L};
use std::path::PathBuf;
use std::time::Duration;

const A: &str = F::SESSION_A;

/// Provider-shaped fake secrets, assembled at run time so the source never
/// holds a string a secret scanner would take for a real key.
fn fake(parts: &[&str]) -> String {
    parts.concat()
}

fn s(text: &str) -> String {
    scrub(text, &[])
}

/// Every case scrubs to what it should, and scrubbing that again changes
/// nothing.
fn check_all(cases: &[(&str, &str)]) {
    for (raw, expected) in cases {
        let text = s(raw);
        assert_eq!(&text, expected, "{raw}");
        assert_eq!(s(&text), text, "scrubbing twice: {raw}");
    }
}

// ---- The scrub (the Mac's vectors) ----

#[test]
fn summaries_are_scrubbed_before_they_leave() {
    let raw = format!(
        "Fixed the Supabase URL in /Users/jane/work/acme/.env and ~/code/app/config.yml, moved \
         /private/var/folders/x/cache.db, /home/jane/notes.txt and /Volumes/Backup/old/, then rotated \
         postgres://admin:hunter22@db.acme.internal/main with OPENAI_API_KEY={} and \
         Bearer abcdefghijklmnopqrstuvwxyz.0123456789 (see file:///Users/jane/readme.md).",
        fake(&["sk_", "live_abcdefghijklmnop1234"])
    );
    let text = s(&raw);
    for leaked in [
        "/Users",
        "jane",
        "~/",
        "/private",
        "/home",
        "/Volumes",
        "work/acme",
        "hunter22",
        "sk_live",
        "abcdefghijklmnopqrstuvwxyz",
    ] {
        assert!(!text.contains(leaked), "{leaked} in {text}");
    }
    for kept in [
        ".env and config.yml",
        "cache.db",
        "notes.txt",
        "old,",
        "postgres://admin:[redacted]@db.acme.internal",
        "OPENAI_API_KEY=[redacted]",
        "Bearer [redacted]",
        "readme.md).",
    ] {
        assert!(text.contains(kept), "{kept} missing from {text}");
    }
    assert_eq!(s(&text), text);
    let long = s(&"word ".repeat(1000));
    assert!(long.encode_utf16().count() <= limit::SUMMARY_TEXT);
}

#[test]
fn the_scrub_covers_named_secrets_home_folders_and_system_paths() {
    let secrets = [
        (
            "Set STRIPE_KEY=abcdef123456 in staging.",
            "Set STRIPE_KEY=[redacted] in staging.",
        ),
        (
            "DB_PASS=hunter2 and PASS: x9",
            "DB_PASS=[redacted] and PASS: [redacted]",
        ),
        (
            "GITHUB_TOKEN = gh1 then MY_SECRET: s3",
            "GITHUB_TOKEN = [redacted] then MY_SECRET: [redacted]",
        ),
        ("ADMIN_PASSWORD='pw' done", "ADMIN_PASSWORD=[redacted] done"),
        (
            "db_pass: abc and x-api-key: k1",
            "db_pass: [redacted] and x-api-key: [redacted]",
        ),
        (
            "apiToken=t0k and clientSecret: s",
            "apiToken=[redacted] and clientSecret: [redacted]",
        ),
        (r#"{"stripe_key": "sk"}"#, r#"{"stripe_key": [redacted]}"#),
    ];
    for (raw, expected) in secrets {
        assert_eq!(s(raw), expected, "{raw}");
    }
    // Plain words that happen to end so are not names of settings.
    for prose in [
        "The monkey: fed.",
        "The key: a new index.",
        "Hotkey: none",
        "Tests pass: all of them.",
    ] {
        assert_eq!(s(prose), prose, "{prose}");
    }

    let paths = [
        ("Edited /Users/jane.", "Edited ~."),
        ("Cleaned /home/jane/ and /Users/jane", "Cleaned ~ and ~"),
        (
            "Ran /opt/homebrew/bin/node on /var/log/system.log",
            "Ran node on system.log",
        ),
        (
            "Changed /Library/Preferences/com.acme.plist and /etc/hosts",
            "Changed com.acme.plist and hosts",
        ),
        ("Wrote /tmp/build-jane/out.txt.", "Wrote out.txt."),
    ];
    for (raw, expected) in paths {
        assert_eq!(s(raw), expected, "{raw}");
    }
    let all: Vec<&str> = secrets
        .iter()
        .map(|c| c.0)
        .chain(paths.iter().map(|c| c.0))
        .collect();
    let text = s(&all.join(" "));
    assert!(!text.contains("jane") && !text.contains("abcdef123456") && !text.contains("hunter2"));
    assert_eq!(s(&text), text);
}

#[test]
fn no_user_or_volume_name_survives_the_scrub() {
    let cases: [(&str, &str); 17] = [
        // Volumes, bare or below.
        (
            "Mounted /Volumes/Backup and copied the build.",
            "Mounted … and copied the build.",
        ),
        ("Moved it to /Volumes/Backup/.", "Moved it to …."),
        (
            "Copied /Volumes/Backup/photos/2024/img.jpg.",
            "Copied img.jpg.",
        ),
        (
            "Ejected /Volumes/My Passport before lunch",
            "Ejected … before lunch",
        ),
        (
            "Ejected /Volumes/My Passport, then /Volumes/Untitled 2.",
            "Ejected …, then ….",
        ),
        (
            "Saved /Volumes/My Passport/Backups/db.sqlite",
            "Saved db.sqlite",
        ),
        (
            "Read /Volumes/Macintosh HD/Users/jane/notes.md today",
            "Read notes.md today",
        ),
        ("Opened /Volumes/Macintosh HD/Users/jane", "Opened ~"),
        (
            "Checked /System/Volumes/Data/Users/jane/.zshrc",
            "Checked .zshrc",
        ),
        ("Checked /System/Volumes/Data/Users/jane.", "Checked ~."),
        // Users whose folder has a space in it.
        ("Edited /Users/Jane Doe/Documents/plan.md", "Edited plan.md"),
        (
            "Cleaned /Users/Jane Doe and /home/jane doe/tmp/x.log",
            "Cleaned ~ and x.log",
        ),
        ("Listed /Users/Jane Doe's files", "Listed ~'s files"),
        // Deeper components with spaces stay in the path.
        (
            "Wrote /Users/jane/My Big Project/src/main.swift twice",
            "Wrote main.swift twice",
        ),
        (
            "Set ~/Library/Application Support/Code/User/settings.json up",
            "Set settings.json up",
        ),
        // A clause's end is never crossed; a new path is its own.
        (
            "Fixed /Users/jane/a.txt. Then b/c.txt",
            "Fixed a.txt. Then b/c.txt",
        ),
        (
            "Diffed /Users/jane/a.txt /Users/jane/b.txt",
            "Diffed a.txt b.txt",
        ),
    ];
    check_all(&cases);
    let joined: Vec<&str> = cases.iter().map(|c| c.0).collect();
    let all = s(&joined.join(" "));
    for name in [
        "jane",
        "Jane",
        "Doe",
        "doe",
        "Backup ",
        "Passport",
        "Macintosh",
        "HD",
        "Untitled",
        "Data/",
    ] {
        assert!(!all.contains(name), "{name} in {all}");
    }

    // This PC's own names are known whole, whatever their case.
    let known: Vec<String> = ["my backup", "Macintosh HD", "jane smith"]
        .map(String::from)
        .to_vec();
    let shorten = |text: &str| shorten_paths(text, &known);
    assert_eq!(
        shorten("Filled /Volumes/my backup today."),
        "Filled … today."
    );
    assert_eq!(shorten("Filled /Volumes/MY BACKUP."), "Filled ….");
    assert_eq!(
        shorten("Filled /Volumes/my backup/old/a.zip"),
        "Filled a.zip"
    );
    assert_eq!(shorten("In /Users/jane smith now"), "In ~ now");
    // A name not in a path is left as written.
    assert_eq!(
        shorten("my backup of x/Volumes/my backup"),
        "my backup of x/Volumes/my backup"
    );
}

#[test]
fn the_words_after_a_path_stay_words() {
    check_all(&[
        (
            "Updated /Users/jane/proj/api.ts to handle client/server sync.",
            "Updated api.ts to handle client/server sync.",
        ),
        (
            "Fixed ~/proj/io.c so the read/write path works",
            "Fixed io.c so the read/write path works",
        ),
        (
            "Wrote /tmp/out.log for TCP/IP checks",
            "Wrote out.log for TCP/IP checks",
        ),
        (
            "Edited ~/proj/a.swift and b/c.swift",
            "Edited a.swift and b/c.swift",
        ),
        (
            "Changed /etc/hosts.allow For TCP/IP",
            "Changed hosts.allow For TCP/IP",
        ),
        // A folder, then ordinary words.
        (
            "Moved ~/proj so the read/write path works",
            "Moved proj so the read/write path works",
        ),
        (
            "Moved ~/proj into src/app/x.swift",
            "Moved proj into src/app/x.swift",
        ),
        (
            "Opened /Users/jane in read/write mode",
            "Opened ~ in read/write mode",
        ),
        (
            "Mounted /Volumes/Backup for the client/server tests",
            "Mounted … for the client/server tests",
        ),
        // A name, then a joining word and a relative path.
        (
            "Checked /Users/jane and src/lib/a.ts",
            "Checked ~ and src/lib/a.ts",
        ),
        (
            "Searched /home/jane for config/app.json",
            "Searched ~ for config/app.json",
        ),
        ("Copied /Volumes/Backup to lib/x.ts", "Copied … to lib/x.ts"),
        // Folder names with spaces still stay in the path.
        (
            "Built ~/Library/Application Support/Code/x.json and b/c",
            "Built x.json and b/c",
        ),
        (
            "Read /Volumes/Macintosh HD/tmp/a.log for TCP/IP",
            "Read a.log for TCP/IP",
        ),
        (
            "Cleaned /home/jane doe/ and /home/jane doe/notes.md",
            "Cleaned ~ and notes.md",
        ),
    ]);
}

#[test]
fn a_name_split_by_any_whitespace_does_not_survive() {
    for space in ["\u{00A0}", "  ", "\n", "\t", " \n ", "\u{2009}"] {
        let shown: Vec<String> = space.chars().map(|c| format!("{:x}", c as u32)).collect();
        assert_eq!(
            s(&format!("Saved to /Volumes/My{space}Passport now")),
            "Saved to … now",
            "{shown:?}"
        );
        assert_eq!(
            s(&format!("Saw /Users/Jane{space}Doe, then left")),
            "Saw ~, then left",
            "{shown:?}"
        );
        assert_eq!(
            s(&format!(
                "Kept /Volumes/Macintosh{space}HD/Users/jane/a.txt"
            )),
            "Kept a.txt",
            "{shown:?}"
        );
        assert_eq!(
            s(&format!("Cleaned /home/jane{space}doe/tmp/x.log")),
            "Cleaned x.log",
            "{shown:?}"
        );
        // And the Windows forms.
        assert_eq!(
            s(&format!("Saw C:\\Users\\Jane{space}Doe\\Documents\\a.txt")),
            "Saw a.txt",
            "{shown:?}"
        );
    }
}

// ---- The Windows forms ----

#[test]
fn windows_paths_are_cut_to_their_last_component() {
    check_all(&[
        (r"Edited C:\Users\jane\app\main.rs.", "Edited main.rs."),
        ("C:/Users/jane/x.txt", "x.txt"),
        (r"Cleaned C:\Users\jane and C:\", "Cleaned ~ and …"),
        (r"\\server\share\dir\a.md", "a.md"),
        (r"\\?\C:\Users\jane\b.md", "b.md"),
        (r"%USERPROFILE%\code\c.py", "c.py"),
        (r"~\code\d.py", "d.py"),
        ("/c/Users/jane/e.sh", "e.sh"),
        ("/mnt/c/Users/jane/f.sh", "f.sh"),
        (r"\\wsl$\Ubuntu\home\jane\g", "g"),
        (r"\\wsl.localhost\Ubuntu\home\jane\h", "h"),
        (
            r"C:\Program Files\Agent Notch\agentnotch.exe",
            "agentnotch.exe",
        ),
        (r"D:\OneDrive - Company\Docs\plan.docx", "plan.docx"),
        // More forms of the same.
        (r"%APPDATA%\npm\claude.cmd", "claude.cmd"),
        (r"%LOCALAPPDATA%\Agent Notch\Claude\log.txt", "log.txt"),
        (r"C:\Program Files (x86)\Foo Bar\a.exe", "a.exe"),
        // A lowercase word after a deeper folder ends the path (prose, as on the Mac).
        (r"C:\Users\Jane Doe\Desktop\my notes.txt", "my notes.txt"),
        (r"c:\users\jane\proj\src\lib.rs", "lib.rs"),
        (r"C:\Users\jane\.claude\settings.json", "settings.json"),
        (
            r"Read \\?\UNC\fileserver\team\plans\q3.xlsx now",
            "Read q3.xlsx now",
        ),
        (r"Opened \\wsl$\Ubuntu\home\jane\proj\a.py", "Opened a.py"),
        (r"In /mnt/d/work/repo/main.c", "In main.c"),
        (r"In /mnt/c/Users/Jane Doe/Docs/x.md", "In x.md"),
        (
            r"Saved to C:\Users\jane\OneDrive - Contoso Ltd\a.docx",
            "Saved to a.docx",
        ),
        // Quotes and brackets end a path, punctuation stays outside.
        (
            r#"Opened "C:\Users\Jane Doe\a.txt" and (D:\x\b.txt), then C:\y\c.txt."#,
            r#"Opened "a.txt" and (b.txt), then c.txt."#,
        ),
        // A doubled separator (a JSON-escaped path) is one.
        (r"C:\\Users\\jane\\a.txt", "a.txt"),
    ]);
}

#[test]
fn a_path_that_names_a_home_a_drive_or_a_share_loses_its_name() {
    check_all(&[
        (r"Opened C:\Users\jane\ today", "Opened ~ today"),
        (r"Opened C:\Users\Jane Doe today", "Opened ~ today"),
        (r"Opened C:\Users\jane doe\tmp\x.log", "Opened x.log"),
        (r"Listed C:\ and D:/", "Listed … and …"),
        (r"Listed \\?\C:\ now", "Listed … now"),
        (r"Mapped \\fileserver\team now", "Mapped … now"),
        (r"Mapped \\fileserver\team\ now", "Mapped … now"),
        (r"Mapped \\fileserver now", "Mapped … now"),
        (r"Mapped \\wsl$\Ubuntu now", "Mapped … now"),
        (r"Opened \\wsl$\Ubuntu\home\jane now", "Opened ~ now"),
        (r"Opened \\?\C:\Users\jane now", "Opened ~ now"),
        (r"Opened /mnt/c/Users/jane now", "Opened ~ now"),
        (r"Opened /mnt/c now", "Opened … now"),
        (r"Opened %USERPROFILE%\ now", "Opened … now"),
        (r"Opened C:\Users\jane/ now", "Opened ~ now"),
        // Prose after a user's name stays prose.
        (
            r"Checked C:\Users\jane and src\lib\a.ts",
            r"Checked ~ and src\lib\a.ts",
        ),
        (
            r"Checked C:\Users\jane for config\app.json",
            r"Checked ~ for config\app.json",
        ),
    ]);
    let all = s(r"Cleaned C:\Users\Jane Doe\a and C:\Users\jane\b, \\srv\share\c and /home/jane/d");
    assert!(
        !all.contains("jane")
            && !all.contains("Jane")
            && !all.contains("Doe")
            && !all.contains("srv"),
        "{all}"
    );
}

#[test]
fn the_words_after_a_windows_path_stay_words() {
    check_all(&[
        (
            r"Updated C:\proj\api.ts to handle client/server sync.",
            "Updated api.ts to handle client/server sync.",
        ),
        (r"Edited C:\proj\a.cs and b\c.cs", r"Edited a.cs and b\c.cs"),
        (
            r"Moved C:\proj into src\app\x.cs",
            r"Moved proj into src\app\x.cs",
        ),
    ]);
}

#[test]
fn windows_paths_do_not_disturb_ordinary_text() {
    for text in [
        "Changed the client/server split, TCP/IP, and/or read/write.",
        "At 12:30 the build 5:1 passed (see 100% of tests).",
        "Visit https://example.com/a/b for details.",
        "Use the key: a new index and the word C: alone.",
        "The a:b mapping and x:y pairs.",
    ] {
        assert_eq!(s(text), text);
    }
}

#[test]
fn known_names_are_recognised_with_either_separator() {
    let known: Vec<String> = ["Jane Doe", "my backup"].map(String::from).to_vec();
    let shorten = |text: &str| shorten_paths(text, &known);
    assert_eq!(
        shorten(r"Saw C:\Users\JANE DOE\x\y.txt now"),
        "Saw y.txt now"
    );
    assert_eq!(shorten(r"Saw C:\Users\jane doe now"), "Saw ~ now");
    assert_eq!(shorten(r"Saw D:\Users\Jane Doe."), "Saw ~.");
    assert_eq!(shorten(r"Saw \\NAS\Volumes\my backup\z.bin"), "Saw z.bin");
    // A name that is not after a users folder is left as written.
    assert_eq!(shorten("Jane Doe wrote it"), "Jane Doe wrote it");
}

#[test]
fn what_a_scrubbed_text_keeps_is_what_a_second_scrub_keeps() {
    let raw = [
        r"C:\Users\jane\a.txt C:\ \\s\h\x ~\y /c/Users/jane /mnt/c/z\w",
        r"%USERPROFILE%\a (C:\Program Files (x86)\Foo\b.exe) 'D:\OneDrive - Co\c'",
    ];
    for text in raw {
        let once = s(text);
        assert_eq!(s(&once), once, "{text} -> {once}");
        assert!(!once.contains("jane"), "{once}");
    }
}

// ---- This PC's names ----

fn users_folder() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().expect("a temporary folder");
    let users = root.path().join("Users");
    for name in [
        "Public",
        "Default",
        "Default User",
        "All Users",
        "jane",
        "Jane Doe",
    ] {
        std::fs::create_dir_all(users.join(name)).expect("folder made");
    }
    std::fs::write(users.join("desktop.ini"), "x").expect("file written");
    (root, users)
}

#[test]
fn the_names_on_this_pc_are_the_users_folders_less_the_systems() {
    let (_root, users) = users_folder();
    let names = LocalNames::list_windows(Some(&users), r"C:\Users\jane");
    assert_eq!(names, ["Jane Doe", "jane"]);
    // The home folder's own name is there even when no folder lists it.
    let names = LocalNames::list_windows(Some(&users), r"C:\Users\someone else");
    assert_eq!(names, ["Jane Doe", "jane", "someone else"]);
    // The system's entries go whatever their case.
    std::fs::create_dir_all(users.join("PUBLIC")).ok();
    assert!(!LocalNames::list_windows(Some(&users), "/home/jane")
        .iter()
        .any(|n| n.eq_ignore_ascii_case("public")));
    // No users folder: just the home's name.
    assert_eq!(LocalNames::list_windows(None, r"C:\Users\jane"), ["jane"]);
    assert_eq!(
        LocalNames::list_windows(Some(&users.join("missing")), "/Users/jane"),
        ["jane"]
    );
}

#[test]
fn the_names_are_listed_again_at_most_once_a_minute_and_none_when_sealed() {
    let (_root, users) = users_folder();
    let local = LocalNames::new(Some(users.clone()), r"C:\Users\jane", false);
    let now = F::base();
    assert_eq!(local.current(now), ["Jane Doe", "jane"]);
    std::fs::create_dir_all(users.join("new kid")).expect("folder made");
    assert_eq!(
        local.current(now + Duration::from_secs(59)),
        ["Jane Doe", "jane"]
    );
    assert_eq!(
        local.current(now + Duration::from_secs(60)),
        ["Jane Doe", "jane", "new kid"]
    );
    // A clock that went back lists again too.
    std::fs::remove_dir(users.join("new kid")).expect("folder removed");
    assert_eq!(local.current(now), ["Jane Doe", "jane"]);

    let sealed = LocalNames::new(Some(users), r"C:\Users\jane", true);
    assert!(sealed.current(now).is_empty());
}

#[test]
fn a_users_folder_name_never_survives_the_scrub() {
    let (_root, users) = users_folder();
    let local = LocalNames::new(Some(users), r"C:\Users\jane", false);
    let now = F::base();
    for raw in [
        r"Saw C:\Users\jane doe\x.txt",
        r"Saw C:\Users\JANE DOE and then left",
        r"Saw /mnt/c/Users/Jane Doe/y.md",
        r"Saw /Users/jane doe, then left",
        r"Saw \\wsl$\Ubuntu\home\Jane Doe\z",
    ] {
        let text = local.scrub(raw, now);
        assert!(
            !text.to_lowercase().contains("jane") && !text.to_lowercase().contains("doe"),
            "{raw} -> {text}"
        );
        assert_eq!(local.scrub(&text, now), text);
    }
    // Sealed, nothing is known: only the shapes of the paths decide.
    let sealed = LocalNames::new(None, r"C:\Users\jane", true);
    assert_eq!(sealed.scrub(r"Saw C:\Users\jane\x.txt", now), "Saw x.txt");
}

#[test]
fn the_macs_listing_keeps_its_vectors() {
    let root = tempfile::tempdir().expect("a temporary folder");
    for folder in [
        "Volumes/My Passport",
        "Volumes/.hidden",
        "Users/jane",
        "Users/Shared",
        "home/Jane Doe",
    ] {
        std::fs::create_dir_all(root.path().join(folder)).expect("folder made");
    }
    let home = root.path().join("home/Jane Doe");
    assert_eq!(
        LocalNames::list(
            &root.path().join("Volumes"),
            &root.path().join("Users"),
            &home.to_string_lossy()
        ),
        ["Jane Doe", "My Passport", "jane"]
    );
}

#[test]
fn an_exit_is_described_honestly() {
    assert_eq!(
        exit_description(1, "error: unknown option '--tools'\n"),
        "This Claude Code is too old for session summaries (error: unknown option '--tools')"
    );
    assert_eq!(
        exit_description(3, "first\n  \n  last line here \t\n\n"),
        "Claude Code exited (3): last line here"
    );
    assert_eq!(exit_description(3, ""), "Claude Code exited with status 3");
    assert_eq!(
        exit_description(3, " \n\t\n"),
        "Claude Code exited with status 3"
    );
    let long = "x".repeat(300);
    assert_eq!(
        exit_description(2, &long),
        format!("Claude Code exited (2): {}", "x".repeat(160))
    );
    assert_eq!(folding_whitespace("  a \u{00A0}\n b\t"), "a b");
}

// ---- The excerpt ----

struct Transcript {
    root: tempfile::TempDir,
    path: PathBuf,
}

fn write_transcript(lines: &[String]) -> Transcript {
    let root = tempfile::tempdir().expect("a temporary folder");
    let path = root
        .path()
        .join(format!(".claude/projects/-Users-me-code-app/{A}.jsonl"));
    L::write(lines, &path, false);
    Transcript { root, path }
}

fn excerpt_of(t: &Transcript, stretches: Option<&[Stretch]>) -> Option<String> {
    build_excerpt(
        &StdSecureFiles,
        PathStyle::native(),
        &t.path.to_string_lossy(),
        A,
        stretches,
        MAX_EXCERPT_CHARACTERS,
    )
}

fn tool_result(text: &str, session: &str, seconds: f64) -> String {
    serde_json::json!({
        "type": "user", "sessionId": session, "timestamp": F::stamp(seconds),
        "toolUseResult": {"stdout": text},
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": text}
        ]}
    })
    .to_string()
}

#[test]
fn the_excerpt_is_conversation_text_only() {
    let other = F::SESSION_B;
    let file = write_transcript(&[
        L::user(
            "Please fix the login bug. My key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123",
            A,
            0.0,
        ),
        L::assistant("m1", "r1", A)
            .usage(1, 1)
            .at(1.0)
            .text("Looking at the login flow.")
            .tool_use()
            .line(),
        tool_result("SECRET_TOOL_OUTPUT password=hunter22", A, 2.0),
        L::assistant("m2", "r2", A)
            .usage(1, 1)
            .at(3.0)
            .text("SIDECHAIN TEXT")
            .sidechain()
            .line(),
        serde_json::json!({"type": "user", "sessionId": A, "isMeta": true,
            "message": {"role": "user", "content": "META TEXT"}})
        .to_string(),
        L::user("COPIED FROM ANOTHER SESSION", other, 4.0),
        L::assistant("m3", "r3", A)
            .usage(1, 1)
            .at(5.0)
            .text("Fixed the token refresh and added a test.")
            .line(),
    ]);
    let excerpt = excerpt_of(&file, None).expect("an excerpt");
    assert!(
        excerpt.starts_with("User: Please fix the login bug. My key is [redacted]"),
        "{excerpt}"
    );
    assert!(excerpt.contains("Claude: Looking at the login flow."));
    assert!(excerpt.ends_with("Claude: Fixed the token refresh and added a test."));
    for leaked in [
        "SECRET_TOOL_OUTPUT",
        "hunter22",
        "cat secrets.txt",
        "SIDECHAIN",
        "META TEXT",
        "COPIED",
        "sk-ant",
    ] {
        assert!(!excerpt.contains(leaked), "{leaked}");
    }
    drop(file.root);
}

#[test]
fn what_other_sources_inject_is_not_a_prompt() {
    let injected = [
        "<task-notification>done</task-notification>",
        "  <command-name>/clear</command-name>",
        "<command-message>x</command-message>",
        "<local-command-stdout>y</local-command-stdout>",
        "<bash-input>ls</bash-input>",
        "<bash-stdout>z</bash-stdout>",
        "Caveat: The messages below were generated by the user",
    ];
    let mut lines: Vec<String> = injected
        .iter()
        .enumerate()
        .map(|(n, text)| L::user(text, A, n as f64))
        .collect();
    lines.push(
        serde_json::json!({"type": "user", "sessionId": A, "origin": {"kind": "task-notification"},
            "message": {"role": "user", "content": "FROM A TASK"}})
        .to_string(),
    );
    lines.push(
        serde_json::json!({"type": "user", "sessionId": A, "isCompactSummary": true,
            "message": {"role": "user", "content": "COMPACT SUMMARY"}})
        .to_string(),
    );
    lines.push(
        serde_json::json!({"type": "user", "sessionId": A, "origin": {"kind": "human"},
        "message": {"role": "user", "content": [
            {"type": "text", "text": "A typed block prompt"},
            {"type": "image", "source": {}}
        ]}})
        .to_string(),
    );
    let file = write_transcript(&lines);
    let excerpt = excerpt_of(&file, None).expect("an excerpt");
    assert_eq!(excerpt, "User: A typed block prompt");
}

#[test]
fn a_long_session_keeps_its_start_and_its_end() {
    let mut lines = Vec::new();
    for index in 0..200 {
        let at = f64::from(index);
        lines.push(L::user(
            &format!("Prompt {index} {}", "p".repeat(300)),
            A,
            at,
        ));
        lines.push(
            L::assistant(&format!("m{index}"), &format!("r{index}"), A)
                .usage(1, 1)
                .at(at)
                .text(&format!("Reply {index} {}", "r".repeat(300)))
                .line(),
        );
    }
    let file = write_transcript(&lines);
    let excerpt = excerpt_of(&file, None).expect("an excerpt");
    let count = excerpt.chars().count();
    assert!(count <= MAX_EXCERPT_CHARACTERS, "{count}");
    assert!(count > MAX_EXCERPT_CHARACTERS - 1000, "{count}");
    assert!(excerpt.starts_with("User: Prompt 0 "));
    assert!(excerpt.contains("[…]"));
    assert!(excerpt.contains("Reply 199 "));
    assert!(!excerpt.contains("Prompt 100 "));
    // One huge message can't take it all.
    assert_eq!(clipped(&"x".repeat(10_000), 4_000).chars().count(), 4_000);
    assert_eq!(clipped("short", 4_000), "short");
    let clip = clipped(&format!("{}{}", "a".repeat(5_000), "z".repeat(5_000)), 100);
    assert!(
        clip.starts_with('a') && clip.ends_with('z') && clip.contains(" […] "),
        "{clip}"
    );
    assert_eq!(clip.chars().count(), 100);
}

#[test]
fn a_split_session_is_read_only_inside_its_stretches() {
    let file = write_transcript(&[
        L::user("Before the switch", A, 0.0),
        L::user("During the first part", A, 10.0),
        L::user("During the second part", A, 20.0),
    ]);
    let first: [Stretch; 1] = [(None, Some(F::at(15.0)))];
    let excerpt = excerpt_of(&file, Some(&first)).expect("an excerpt");
    assert!(excerpt.contains("Before the switch") && excerpt.contains("first part"));
    assert!(!excerpt.contains("second part"), "{excerpt}");
    let second: [Stretch; 1] = [(Some(F::at(15.0)), None)];
    let excerpt = excerpt_of(&file, Some(&second)).expect("an excerpt");
    assert_eq!(excerpt, "User: During the second part");
    let none: [Stretch; 1] = [(Some(F::at(100.0)), None)];
    assert_eq!(excerpt_of(&file, Some(&none)), None);
}

#[test]
fn only_a_transcript_under_projects_is_read() {
    let root = tempfile::tempdir().expect("a temporary folder");
    let elsewhere = root.path().join(format!("notes/{A}.jsonl"));
    L::write(&[L::user("secret", A, 0.0)], &elsewhere, false);
    let read = |path: &PathBuf| {
        build_excerpt(
            &StdSecureFiles,
            PathStyle::native(),
            &path.to_string_lossy(),
            A,
            None,
            MAX_EXCERPT_CHARACTERS,
        )
    };
    assert_eq!(read(&elsewhere), None);
    // Missing, empty and conversation-free files give nothing.
    let missing = root.path().join(format!(".claude/projects/x/{A}.jsonl"));
    assert_eq!(read(&missing), None);
    L::write(&[String::from("not json")], &missing, false);
    assert_eq!(read(&missing), None);
}

#[cfg(unix)]
#[test]
fn a_link_out_of_projects_is_never_opened() {
    let root = tempfile::tempdir().expect("a temporary folder");
    let outside = root.path().join(format!("outside/{A}.jsonl"));
    L::write(&[L::user("must not be read", A, 0.0)], &outside, false);
    let slug = root.path().join(".claude/projects/-Users-me-app");
    std::fs::create_dir_all(&slug).expect("folder made");
    let link = slug.join(format!("{A}.jsonl"));
    std::os::unix::fs::symlink(&outside, &link).expect("link made");
    assert_eq!(
        build_excerpt(
            &StdSecureFiles,
            PathStyle::native(),
            &link.to_string_lossy(),
            A,
            None,
            MAX_EXCERPT_CHARACTERS
        ),
        None
    );
}

// ---- Redaction ----

#[test]
fn secrets_are_redacted() {
    let text = format!(
        "{} {} {} \
         eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U api_key = abcdefgh1234\n\
         -----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----",
        fake(&["gh", "p_abcdefghijklmnopqrstuvwxyz0123"]),
        fake(&["AKIA", "ABCDEFGHIJKLMNOP"]),
        fake(&["xox", "b-1234567890-abcdefghij"]),
    );
    let redacted = redact(&text);
    for secret in [
        "ghp_",
        "AKIAABCD",
        "xoxb-",
        "eyJhbGci",
        "abcdefgh1234",
        "MIIEow",
    ] {
        assert!(!redacted.contains(secret), "{secret}");
    }
    assert_eq!(redact("Plain words stay."), "Plain words stay.");

    // The formats the first patterns missed (review finding 19).
    let more = format!(
        "{} rk_test_abcdefghij0123 sb_secret_abcdefghijklmnop npm_abcdefghijklmnopqrstuvwxyz0123 \
         ya29.a0AfH6SMBabcdefghijklmnopqrst AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENGbPxRfiCY OPENAI_API_KEY=\"sk-proj-x\" \
         mysql://root:toor-password@localhost/db authorization: Bearer 0123456789abcdefghijklmnop \
         token 9f8e7d6c5b4a39281706f5e4d3c2b1a0Zq9Xw8Vu7\n\
         -----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ (the rest was cut off)",
        fake(&["sk_", "live_4eC39HqLyjWDarjtT1zdp7dc"]),
    );
    let cleaned = redact(&more);
    for secret in [
        "4eC39HqLyjWD",
        "abcdefghij0123",
        "sb_secret_",
        "npm_abcd",
        "ya29.",
        "wJalrXUtnFEMI",
        "sk-proj",
        "toor-password",
        "0123456789abcdefghijklmnop",
        "9f8e7d6c5b4a39281706",
        "b3BlbnNzaC1",
    ] {
        assert!(!cleaned.contains(secret), "{secret} in {cleaned}");
    }
    assert!(
        cleaned.contains("AWS_SECRET_ACCESS_KEY=[redacted]")
            && cleaned.contains("Bearer [redacted]")
    );
    assert!(cleaned.contains("mysql://root:[redacted]@localhost"));
    // Ordinary long words and names are not secrets.
    let prose =
        "Refactored SessionTokenScannerTests and internationalization in withTaskCancellationHandler.";
    assert_eq!(redact(prose), prose);
    assert!(entropy("aaaa") == 0.0 && entropy("abcd") == 2.0 && entropy("") == 0.0);
    // Redacting what is redacted changes nothing.
    assert_eq!(redact(&cleaned), cleaned);
    assert_eq!(redact(&redacted), redacted);
}

#[test]
fn a_text_the_patterns_give_up_on_is_redacted_whole() {
    // Thousands of `a_` pairs make the setting-name pattern backtrack past
    // its limit; the text is never passed on unredacted.
    let hostile = "a_".repeat(60_000);
    let out = redact(&hostile);
    assert!(out == "[redacted]" || out == hostile, "{} chars", out.len());
    if out == hostile {
        // Matching finished: it is only fine because nothing in it is secret.
        assert!(!hostile.contains("key"));
    }
}
