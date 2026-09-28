//! `core::paths` under both styles. The POSIX vectors are the Mac's
//! AccountPathsTests (and the Mac's slug rule, TranscriptLocator); the
//! Windows ones are their Windows counterparts plus AU§1.2's rules.

use agentnotch_engine::core::paths::{project_slug, PathStyle, Paths};

fn mac() -> Paths {
    Paths::new(PathStyle::Posix, "/Users/me")
}

fn win() -> Paths {
    Paths::new(PathStyle::Windows, r"C:\Users\me")
}

// ---- AccountPathsTests, POSIX ----

#[test]
fn config_dir_from_main_transcript() {
    let p = mac();
    assert_eq!(
        p.config_dir_from_transcript("/Users/me/.claude-work/projects/-Users-me-proj/0b7c.jsonl")
            .as_deref(),
        Some("/Users/me/.claude-work")
    );
}

#[test]
fn config_dir_from_subagent_transcript() {
    let p = mac();
    assert_eq!(
        p.config_dir_from_transcript(
            "/Users/me/.claude/projects/-Users-me-proj/abc/subagents/agent-1.jsonl"
        )
        .as_deref(),
        Some("/Users/me/.claude")
    );
}

#[test]
fn config_dir_rejects_other_shapes() {
    let p = mac();
    assert_eq!(p.config_dir_from_transcript("/tmp/file.jsonl"), None);
    // As on the Mac, `projects` right under the root names the root itself.
    assert_eq!(
        p.config_dir_from_transcript("/projects/x/y.jsonl")
            .as_deref(),
        Some("/")
    );
    assert_eq!(p.config_dir_from_transcript("relative.jsonl"), None);
}

#[test]
fn normalize_drops_trailing_slash_and_expands_tilde() {
    let p = mac();
    assert_eq!(p.normalize("~/.claude-work/"), "/Users/me/.claude-work");
    assert_eq!(p.normalize("~"), "/Users/me");
    assert_eq!(
        p.normalize("/Users/me/./x/../.claude//"),
        "/Users/me/.claude"
    );
    assert_eq!(p.normalize("/"), "/");
    assert_eq!(p.normalize("/.."), "/");
    assert_eq!(p.normalize("a/../../b"), "../b");
    assert_eq!(p.normalize(""), "");
    // Only a leading `~` alone or before a separator expands.
    assert_eq!(p.normalize("~other/x"), "~other/x");
    assert_eq!(p.normalize("/a/~/b"), "/a/~/b");
}

#[test]
fn global_config_file_follows_claude_code() {
    let p = mac();
    assert_eq!(
        p.global_config_file("~/.claude", None),
        "/Users/me/.claude.json"
    );
    assert_eq!(
        p.global_config_file("~/.claude-work", Some("/Users/me/.claude-work")),
        "/Users/me/.claude-work/.claude.json"
    );
    // Set, even to the default folder, it reads the folder's own file.
    assert_eq!(
        p.global_config_file("~/.claude", Some("/Users/me/.claude")),
        "/Users/me/.claude/.claude.json"
    );
    // Empty counts as unset.
    assert_eq!(
        p.global_config_file("/Users/me/.claude", Some("")),
        "/Users/me/.claude.json"
    );
    assert_eq!(
        p.global_config_file("/Users/me/lab", None),
        "/Users/me/lab/.claude.json"
    );
}

#[test]
fn posix_is_case_sensitive() {
    let p = mac();
    assert!(!p.same("/Users/me/.Claude", "/Users/me/.claude"));
    assert_eq!(p.key("/Users/Me"), "/Users/Me");
}

// ---- Windows ----

#[test]
fn windows_normalize() {
    let p = win();
    for (input, expected) in [
        (r"C:\Users\me\.claude", r"C:\Users\me\.claude"),
        ("C:/Users/me/.claude-work/", r"C:\Users\me\.claude-work"),
        (r"c:\users\me\.CLAUDE\\", r"C:\users\me\.CLAUDE"),
        (r"C:\Users\me\.\x\..\.claude", r"C:\Users\me\.claude"),
        (r"C:\..\..", r"C:\"),
        ("C:/", r"C:\"),
        ("c:", "C:"),
        (r"\\?\C:\Users\me\.claude", r"C:\Users\me\.claude"),
        (
            r"\\?\UNC\server\share\me\.claude",
            r"\\server\share\me\.claude",
        ),
        (r"\\server\share\me\..\.claude\", r"\\server\share\.claude"),
        (r"\\server\share", r"\\server\share\"),
        ("//server/share/x", r"\\server\share\x"),
        (r"~\.claude-work", r"C:\Users\me\.claude-work"),
        ("~/.claude-work", r"C:\Users\me\.claude-work"),
        ("~", r"C:\Users\me"),
        (r"\temp\x", r"\temp\x"),
        (r"a\..\..\b", r"..\b"),
        ("", ""),
    ] {
        assert_eq!(p.normalize(input), expected, "{input:?}");
    }
}

#[test]
fn windows_keys_fold_case_and_display_keeps_it() {
    let p = win();
    assert_eq!(
        p.key(r"C:\Users\Me\.Claude-Work\"),
        r"c:\users\me\.claude-work"
    );
    assert_eq!(
        p.key("c:/users/me/.claude-work"),
        r"c:\users\me\.claude-work"
    );
    assert!(p.same(r"C:\Users\ME\.claude", "c:/users/me/.CLAUDE/"));
    assert_eq!(
        p.normalize(r"C:\Users\Me\.Claude-Work"),
        r"C:\Users\Me\.Claude-Work"
    );
}

#[test]
fn windows_abbreviation_and_ancestry() {
    let p = win();
    assert_eq!(p.abbreviate(r"C:\Users\me\.claude-work"), r"~\.claude-work");
    assert_eq!(p.abbreviate(r"c:\USERS\ME\.claude"), r"~\.claude");
    assert_eq!(p.abbreviate(r"C:\Users\me"), "~");
    assert_eq!(p.abbreviate(r"D:\work\.claude"), r"D:\work\.claude");
    assert_eq!(
        p.abbreviate(r"C:\Users\meadow\.claude"),
        r"C:\Users\meadow\.claude"
    );
    assert!(p.is_within(r"C:\Users\me", r"c:\users\me\.claude\projects"));
    assert!(p.is_within(r"C:\Users\me", r"C:\Users\me"));
    assert!(!p.is_within(r"C:\Users\me", r"C:\Users\meadow"));
    assert!(p.is_root(r"C:\"));
    assert!(p.is_root(r"\\server\share"));
    assert!(!p.is_root(r"C:\Users"));
    assert_eq!(
        p.strip_prefix(r"C:\Users\me", r"C:\Users\me\.claude\hooks")
            .as_deref(),
        Some(r".claude\hooks")
    );
    let m = mac();
    assert_eq!(m.abbreviate("/Users/me/.claude-work"), "~/.claude-work");
}

#[test]
fn windows_components_parent_and_names() {
    let p = win();
    assert_eq!(
        p.components(r"C:\Users\me\.claude"),
        [r"C:\", "Users", "me", ".claude"]
    );
    assert_eq!(p.components(r"\\server\share\x"), [r"\\server\share\", "x"]);
    assert_eq!(
        p.file_name(r"C:\Users\me\.claude-work\").as_deref(),
        Some(".claude-work")
    );
    assert_eq!(p.file_name(r"C:\"), None);
    assert_eq!(
        p.parent(r"C:\Users\me\.claude").as_deref(),
        Some(r"C:\Users\me")
    );
    assert_eq!(p.parent(r"C:\Users").as_deref(), Some(r"C:\"));
    assert_eq!(p.parent(r"C:\"), None);
    assert_eq!(
        p.join(r"C:\Users\me\.claude", "hooks/agentnotch-hook.exe"),
        r"C:\Users\me\.claude\hooks\agentnotch-hook.exe"
    );
}

#[test]
fn windows_transcripts() {
    let p = win();
    let main = r"C:\Users\me\.claude-work\projects\C--Users-me-code-app\5d1e0a7b.jsonl";
    assert_eq!(
        p.config_dir_from_transcript(main).as_deref(),
        Some(r"C:\Users\me\.claude-work")
    );
    // Either separator, any case of `projects`.
    let odd = "c:/Users/me/.claude/PROJECTS/C--x/abc/subagents/agent-1.jsonl";
    assert_eq!(
        p.config_dir_from_transcript(odd).as_deref(),
        Some(r"C:\Users\me\.claude")
    );
    // A folder that itself contains `projects`: the last one counts.
    let nested = r"D:\projects\.claude\projects\D--projects-app\s.jsonl";
    assert_eq!(
        p.config_dir_from_transcript(nested).as_deref(),
        Some(r"D:\projects\.claude")
    );
    assert_eq!(p.config_dir_from_transcript(r"C:\temp\file.jsonl"), None);
    let unc = r"\\server\share\me\.claude\projects\x\s.jsonl";
    assert_eq!(
        p.config_dir_from_transcript(unc).as_deref(),
        Some(r"\\server\share\me\.claude")
    );
}

#[test]
fn windows_global_config_file() {
    let p = win();
    assert_eq!(
        p.global_config_file(r"C:\Users\me\.claude", None),
        r"C:\Users\me\.claude.json"
    );
    assert_eq!(
        p.global_config_file(r"c:\users\ME\.claude\", None),
        r"C:\Users\me\.claude.json"
    );
    // CLAUDE_CONFIG_DIR set to the default folder: its own file (the Mac's rule).
    assert_eq!(
        p.global_config_file(r"C:\Users\me\.claude", Some(r"C:\Users\me\.claude")),
        r"C:\Users\me\.claude\.claude.json"
    );
    assert_eq!(
        p.global_config_file(
            r"C:\Users\me\.claude-work",
            Some("C:/Users/me/.claude-work")
        ),
        r"C:\Users\me\.claude-work\.claude.json"
    );
    assert!(p.is_default_config_dir(r"c:\users\me\.CLAUDE"));
    assert_eq!(p.default_identity_file(), r"C:\Users\me\.claude.json");
}

#[test]
fn session_config_dir_precedence() {
    let p = win();
    let transcript = r"C:\Users\me\.claude-work\projects\C--x\s.jsonl";
    let none = |_: &str| false;
    assert_eq!(
        p.session_config_dir(Some(transcript), Some(r"C:\other"), none),
        r"C:\Users\me\.claude-work"
    );
    assert_eq!(
        p.session_config_dir(None, Some(r"C:\Users\me\.claude-x\"), none),
        r"C:\Users\me\.claude-x"
    );
    assert_eq!(
        p.session_config_dir(None, Some(""), none),
        r"C:\Users\me\.claude"
    );
    assert_eq!(
        p.session_config_dir(Some(r"C:\temp\x.jsonl"), None, none),
        r"C:\Users\me\.claude"
    );
    // A transcript through shared history names no account: the environment decides.
    let shared = r"C:\Users\me\.claude-shared\projects\C--x\s.jsonl";
    let infra = |dir: &str| dir.to_lowercase().ends_with(".claude-shared");
    assert_eq!(
        p.session_config_dir(Some(shared), Some(r"C:\Users\me\.claude-b"), infra),
        r"C:\Users\me\.claude-b"
    );
}

#[test]
fn extra_config_dirs_split() {
    assert_eq!(
        win().split_list(r" ~\.claude-a ; D:/lab/.claude-b;;C:\x\ "),
        [r"C:\Users\me\.claude-a", r"D:\lab\.claude-b", r"C:\x"]
    );
    assert_eq!(
        mac().split_list("~/.claude-a:/opt/b::"),
        ["/Users/me/.claude-a", "/opt/b"]
    );
}

#[test]
fn transcript_slug() {
    assert_eq!(project_slug(r"C:\Users\me\proj"), "C--Users-me-proj");
    assert_eq!(project_slug("/Users/me/my_app@2.0"), "-Users-me-my-app-2-0");
    // Per UTF-16 unit, as JavaScript's replace counts.
    assert_eq!(project_slug(r"C:\Users\zoë\😀"), "C--Users-zo----");
    assert_eq!(
        win().expected_transcript_path(r"C:\Users\me\.claude", r"C:\Users\me\proj", "abc"),
        r"C:\Users\me\.claude\projects\C--Users-me-proj\abc.jsonl"
    );
    assert_eq!(
        mac().expected_transcript_path("~/.claude", "/Users/me/proj", "abc"),
        "/Users/me/.claude/projects/-Users-me-proj/abc.jsonl"
    );
}

#[test]
fn native_style_matches_the_build() {
    let native = PathStyle::native();
    assert_eq!(native == PathStyle::Windows, cfg!(windows));
}
