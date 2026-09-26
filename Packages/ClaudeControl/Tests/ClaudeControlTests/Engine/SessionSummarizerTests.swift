import Foundation
import Testing
@testable import ClaudeControl

/// Session summaries: the command line, what goes in, what comes out, and
/// when one is due. A stand-in `claude` only.
struct SessionSummarizerTests {
    typealias L = CloudTranscriptLines
    let a = CloudFixture.sessionA

    @Test func theCommandLineIsFixedAndSideEffectFree() {
        // Each run capped at $0.10 by Claude Code itself (review finding 4).
        #expect(SessionSummarizer.arguments == [
            "-p", "--model", "haiku", "--max-turns", "1", "--max-budget-usd", "0.10", "--output-format", "json",
            "--no-session-persistence", "--strict-mcp-config", "--settings", #"{"disableAllHooks":true}"#,
            "--tools", "",
        ])
    }

    @Test func theEnvironmentIsScrubbedAndUsesTheFoldersLogin() {
        let base = ["PATH": "/usr/bin", "HOME": "/Users/me", "CLAUDECODE": "1", "CLAUDE_CODE_ENTRYPOINT": "cli",
                    "CLAUDE_CONFIG_DIR": "/Users/me/.claude-other", "CLAUDE_PID": "12", "AI_AGENT": "x",
                    "ANTHROPIC_API_KEY": "from-a-terminal", "ANTHROPIC_AUTH_TOKEN": "also", "CLAUDE_AGENT_SDK_VERSION": "1"]
        let env = SessionSummarizer.environment(base: base, configDirEnv: "/Users/me/.claude-work")
        #expect(env == ["PATH": "/usr/bin", "HOME": "/Users/me", "CLAUDE_CONFIG_DIR": "/Users/me/.claude-work"])
        #expect(SessionSummarizer.environment(base: base, configDirEnv: nil)["CLAUDE_CONFIG_DIR"] == nil)
    }

    @Test func theInputIsTheInstructionsThenTheExcerpt() {
        let input = SessionSummarizer.input(excerpt: "User: hi")
        #expect(input.hasPrefix(SessionSummarizer.instructions))
        #expect(input.contains("<session>\nUser: hi\n</session>"))
        #expect(SessionSummarizer.instructions.contains("one or two plain sentences"))
        #expect(SessionSummarizer.instructions.contains("Don't include file paths, user names, host names, URLs, "
                                                        + "keys, tokens, passwords or other credentials"))
    }

    /// Regression (review finding 19): what the model writes is scrubbed
    /// before it is kept or sent: paths cut to their last part, likely
    /// secrets redacted, at most the contract's 2,000 characters.
    @Test func summariesAreScrubbedBeforeTheyLeave() {
        let raw = """
        Fixed the Supabase URL in /Users/jane/work/acme/.env and ~/code/app/config.yml, moved \
        /private/var/folders/x/cache.db, /home/jane/notes.txt and /Volumes/Backup/old/, then rotated \
        postgres://admin:hunter22@db.acme.internal/main with OPENAI_API_KEY=\(Self.fake("sk_", "live_abcdefghijklmnop1234")) and \
        Bearer abcdefghijklmnopqrstuvwxyz.0123456789 (see file:///Users/jane/readme.md).
        """
        let text = SessionSummarizer.scrub(raw)
        for leaked in ["/Users", "jane", "~/", "/private", "/home", "/Volumes", "work/acme", "hunter22", "sk_live",
                       "abcdefghijklmnopqrstuvwxyz"] {
            #expect(!text.contains(leaked), "\(leaked) in \(text)")
        }
        for kept in [".env and config.yml", "cache.db", "notes.txt", "old,", "postgres://admin:[redacted]@db.acme.internal",
                     "OPENAI_API_KEY=[redacted]", "Bearer [redacted]", "readme.md)."] {
            #expect(text.contains(kept), "\(kept) missing from \(text)")
        }
        #expect(SessionSummarizer.scrub(text) == text)
        #expect(SessionSummarizer.scrub(String(repeating: "word ", count: 1000)).count <= CloudContract.Limit.summaryText)
        // The answer is scrubbed on the way in, and again on the way out.
        let stdout = Data(#"{"type":"result","subtype":"success","is_error":false,"result":"Edited /Users/jane/app/main.swift."}"#.utf8)
        #expect(SessionSummarizer.parse(stdout: stdout, exitStatus: 0, stderr: "")
                == .summary(.init(text: "Edited main.swift.", model: "haiku", costUsd: nil)))
        let stored = SessionSummaryStore.Entry(text: "Read ~/secret/plan.txt", model: "m", generatedAt: CloudFixture.base,
                                               messageCount: 2, costUsd: nil)
        #expect(stored.contract.text == "Read plan.txt")
    }

    /// Regression (fix check): any setting whose name ends in KEY, TOKEN,
    /// SECRET, PASSWORD or PASS loses its value, a home folder loses its
    /// user's name, and paths under /opt, /var, /Library, /etc and /tmp are
    /// shortened too.
    @Test func theScrubCoversNamedSecretsHomeFoldersAndSystemPaths() {
        let secrets = [
            ("Set STRIPE_KEY=abcdef123456 in staging.", "Set STRIPE_KEY=[redacted] in staging."),
            ("DB_PASS=hunter2 and PASS: x9", "DB_PASS=[redacted] and PASS: [redacted]"),
            ("GITHUB_TOKEN = gh1 then MY_SECRET: s3", "GITHUB_TOKEN = [redacted] then MY_SECRET: [redacted]"),
            ("ADMIN_PASSWORD='pw' done", "ADMIN_PASSWORD=[redacted] done"),
            ("db_pass: abc and x-api-key: k1", "db_pass: [redacted] and x-api-key: [redacted]"),
            ("apiToken=t0k and clientSecret: s", "apiToken=[redacted] and clientSecret: [redacted]"),
            (#"{"stripe_key": "sk"}"#, #"{"stripe_key": [redacted]}"#),
        ]
        for (raw, expected) in secrets {
            #expect(SessionSummarizer.scrub(raw) == expected, "\(raw)")
        }
        // Plain words that happen to end so are not names of settings.
        for prose in ["The monkey: fed.", "The key: a new index.", "Hotkey: none", "Tests pass: all of them."] {
            #expect(SessionSummarizer.scrub(prose) == prose, "\(prose)")
        }

        let paths = [
            ("Edited /Users/jane.", "Edited ~."),
            ("Cleaned /home/jane/ and /Users/jane", "Cleaned ~ and ~"),
            ("Ran /opt/homebrew/bin/node on /var/log/system.log", "Ran node on system.log"),
            ("Changed /Library/Preferences/com.acme.plist and /etc/hosts", "Changed com.acme.plist and hosts"),
            ("Wrote /tmp/build-jane/out.txt.", "Wrote out.txt."),
        ]
        for (raw, expected) in paths {
            #expect(SessionSummarizer.scrub(raw) == expected, "\(raw)")
        }
        let text = SessionSummarizer.scrub(secrets.map(\.0).joined(separator: " ") + " " + paths.map(\.0).joined(separator: " "))
        #expect(!text.contains("jane") && !text.contains("abcdef123456") && !text.contains("hunter2"))
        #expect(SessionSummarizer.scrub(text) == text)
    }

    /// Regression (M1): no user's or volume's name survives the scrub: a
    /// path that ends at a volume loses its name like a home folder does,
    /// and components with spaces (volume names often have them) stay in
    /// the path instead of leaking what follows the space.
    @Test func noUserOrVolumeNameSurvivesTheScrub() {
        let cases: [(String, String)] = [
            // Volumes, bare or below.
            ("Mounted /Volumes/Backup and copied the build.", "Mounted … and copied the build."),
            ("Moved it to /Volumes/Backup/.", "Moved it to …."),
            ("Copied /Volumes/Backup/photos/2024/img.jpg.", "Copied img.jpg."),
            ("Ejected /Volumes/My Passport before lunch", "Ejected … before lunch"),
            ("Ejected /Volumes/My Passport, then /Volumes/Untitled 2.", "Ejected …, then …."),
            ("Saved /Volumes/My Passport/Backups/db.sqlite", "Saved db.sqlite"),
            ("Read /Volumes/Macintosh HD/Users/jane/notes.md today", "Read notes.md today"),
            ("Opened /Volumes/Macintosh HD/Users/jane", "Opened ~"),
            ("Checked /System/Volumes/Data/Users/jane/.zshrc", "Checked .zshrc"),
            ("Checked /System/Volumes/Data/Users/jane.", "Checked ~."),
            // Users whose folder has a space in it.
            ("Edited /Users/Jane Doe/Documents/plan.md", "Edited plan.md"),
            ("Cleaned /Users/Jane Doe and /home/jane doe/tmp/x.log", "Cleaned ~ and x.log"),
            ("Listed /Users/Jane Doe's files", "Listed ~'s files"),
            // Deeper components with spaces stay in the path.
            ("Wrote /Users/jane/My Big Project/src/main.swift twice", "Wrote main.swift twice"),
            ("Set ~/Library/Application Support/Code/User/settings.json up", "Set settings.json up"),
            // A clause's end is never crossed; a new path is its own.
            ("Fixed /Users/jane/a.txt. Then b/c.txt", "Fixed a.txt. Then b/c.txt"),
            ("Diffed /Users/jane/a.txt /Users/jane/b.txt", "Diffed a.txt b.txt"),
        ]
        for (raw, expected) in cases {
            let text = SessionSummarizer.scrub(raw)
            #expect(text == expected, "\(raw)")
            #expect(SessionSummarizer.scrub(text) == text, "\(raw)")
        }
        let all = SessionSummarizer.scrub(cases.map(\.0).joined(separator: " "))
        for name in ["jane", "Jane", "Doe", "doe", "Backup ", "Passport", "Macintosh", "HD", "Untitled", "Data/"] {
            #expect(!all.contains(name), "\(name) in \(all)")
        }

        // This Mac's own names are known whole, whatever their case.
        let known = ["my backup", "Macintosh HD", "jane smith"]
        #expect(SessionSummarizer.shortenPaths("Filled /Volumes/my backup today.", knownNames: known) == "Filled … today.")
        #expect(SessionSummarizer.shortenPaths("Filled /Volumes/MY BACKUP.", knownNames: known) == "Filled ….")
        #expect(SessionSummarizer.shortenPaths("Filled /Volumes/my backup/old/a.zip", knownNames: known) == "Filled a.zip")
        #expect(SessionSummarizer.shortenPaths("In /Users/jane smith now", knownNames: known) == "In ~ now")
        // A name not in a path is left as written.
        #expect(SessionSummarizer.shortenPaths("my backup of x/Volumes/my backup", knownNames: known)
                == "my backup of x/Volumes/my backup")
    }

    /// Regression (review, confirmed): the words after a path stay words. A
    /// path stops at a file's name, and prose with a slash in it
    /// (client/server, read/write, TCP/IP) is never taken for more of the
    /// path, while folder names with spaces still are.
    @Test func theWordsAfterAPathStayWords() {
        let cases: [(String, String)] = [
            ("Updated /Users/jane/proj/api.ts to handle client/server sync.", "Updated api.ts to handle client/server sync."),
            ("Fixed ~/proj/io.c so the read/write path works", "Fixed io.c so the read/write path works"),
            ("Wrote /tmp/out.log for TCP/IP checks", "Wrote out.log for TCP/IP checks"),
            ("Edited ~/proj/a.swift and b/c.swift", "Edited a.swift and b/c.swift"),
            ("Changed /etc/hosts.allow For TCP/IP", "Changed hosts.allow For TCP/IP"),
            // A folder, then ordinary words.
            ("Moved ~/proj so the read/write path works", "Moved proj so the read/write path works"),
            ("Moved ~/proj into src/app/x.swift", "Moved proj into src/app/x.swift"),
            ("Opened /Users/jane in read/write mode", "Opened ~ in read/write mode"),
            ("Mounted /Volumes/Backup for the client/server tests", "Mounted … for the client/server tests"),
            // A name, then a joining word and a relative path.
            ("Checked /Users/jane and src/lib/a.ts", "Checked ~ and src/lib/a.ts"),
            ("Searched /home/jane for config/app.json", "Searched ~ for config/app.json"),
            ("Copied /Volumes/Backup to lib/x.ts", "Copied … to lib/x.ts"),
            // Folder names with spaces still stay in the path.
            ("Built ~/Library/Application Support/Code/x.json and b/c", "Built x.json and b/c"),
            ("Read /Volumes/Macintosh HD/tmp/a.log for TCP/IP", "Read a.log for TCP/IP"),
            ("Cleaned /home/jane doe/ and /home/jane doe/notes.md", "Cleaned ~ and notes.md"),
        ]
        for (raw, expected) in cases {
            let text = SessionSummarizer.scrub(raw)
            #expect(text == expected, "\(raw)")
            #expect(SessionSummarizer.scrub(text) == text, "\(raw)")
        }
    }

    /// Regression (review): whitespace is folded before paths are
    /// shortened, so a user's or volume's name split by a newline, a tab, a
    /// double or a no-break space doesn't survive.
    @Test func aNameSplitByAnyWhitespaceDoesNotSurvive() {
        for space in ["\u{00A0}", "  ", "\n", "\t", " \n ", "\u{2009}"] {
            let shown = space.unicodeScalars.map { String($0.value, radix: 16) }
            #expect(SessionSummarizer.scrub("Saved to /Volumes/My\(space)Passport now") == "Saved to … now", "\(shown)")
            #expect(SessionSummarizer.scrub("Saw /Users/Jane\(space)Doe, then left") == "Saw ~, then left", "\(shown)")
            #expect(SessionSummarizer.scrub("Kept /Volumes/Macintosh\(space)HD/Users/jane/a.txt") == "Kept a.txt", "\(shown)")
            #expect(SessionSummarizer.scrub("Cleaned /home/jane\(space)doe/tmp/x.log") == "Cleaned x.log", "\(shown)")
        }
    }

    /// The names the scrub knows on this Mac: its volumes and users' home
    /// folders, from folder listings; none before bootstrap (tests).
    @Test func theScrubKnowsThisMacsVolumesAndUsers() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("local-names-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        for folder in ["Volumes/My Passport", "Volumes/.hidden", "Users/jane", "Users/Shared", "home/Jane Doe"] {
            try FileManager.default.createDirectory(at: root.appendingPathComponent(folder), withIntermediateDirectories: true)
        }
        #expect(LocalNames.list(volumes: root.appendingPathComponent("Volumes").path,
                                users: root.appendingPathComponent("Users").path,
                                home: root.appendingPathComponent("home/Jane Doe").path)
                == ["Jane Doe", "My Passport", "jane"])
        #expect(LocalNames.current().isEmpty)
    }

    // MARK: - The answer

    @Test func aResultBecomesASummary() {
        let stdout = Data(#"""
        {"type":"result","subtype":"success","is_error":false,"result":"  Fixed the parser.\n\nAdded tests.  ","total_cost_usd":0.0021,"modelUsage":{"claude-haiku-4-5-20251001":{"outputTokens":40},"claude-sonnet-4-5":{"outputTokens":2}}}
        """#.utf8)
        #expect(SessionSummarizer.parse(stdout: stdout, exitStatus: 0, stderr: "")
                == .summary(.init(text: "Fixed the parser. Added tests.", model: "claude-haiku-4-5-20251001", costUsd: 0.0021)))
        let noModel = Data(#"{"type":"result","subtype":"success","is_error":false,"result":"Did it."}"#.utf8)
        #expect(SessionSummarizer.parse(stdout: noModel, exitStatus: 0, stderr: "")
                == .summary(.init(text: "Did it.", model: "haiku", costUsd: nil)))
        let long = Data(("{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"" + String(repeating: "a", count: 3000) + "\"}").utf8)
        guard case .summary(let clipped) = SessionSummarizer.parse(stdout: long, exitStatus: 0, stderr: "") else {
            Issue.record("expected a summary")
            return
        }
        #expect(clipped.text.count == 2000)
    }

    @Test func errorsAreHonest() {
        func parse(_ json: String, status: Int32 = 1, stderr: String = "") -> SessionSummarizer.Outcome {
            SessionSummarizer.parse(stdout: Data(json.utf8), exitStatus: status, stderr: stderr)
        }
        #expect(parse(#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"}"#)
                == .unavailable("Not signed in to Claude"))
        #expect(parse(#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: 429 rate limit"}"#)
                == .rateLimited)
        #expect(parse(#"{"type":"result","subtype":"error_max_turns","is_error":false}"#) == .failed("error_max_turns"))
        #expect(parse(#"{"type":"result","subtype":"success","is_error":false,"result":"   "}"#)
                == .failed("Claude Code answered with no text"))
        #expect(parse("", stderr: "error: unknown option '--tools'\n")
                == .failed("This Claude Code is too old for session summaries (error: unknown option '--tools')"))
        #expect(parse("not json", status: 3) == .failed("Claude Code exited with status 3"))
    }

    // MARK: - The excerpt

    func writeTranscript(_ lines: [String]) throws -> (path: String, root: String) {
        let root = TestPaths.temporaryRoot("summary-excerpt")
        let path = (root as NSString).appendingPathComponent(".claude/projects/-Users-me-code-app/\(a).jsonl")
        try L.write(lines, to: path)
        return (path, root)
    }

    @Test func theExcerptIsConversationTextOnly() throws {
        let other = CloudFixture.sessionB
        let file = try writeTranscript([
            L.user("Please fix the login bug. My key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123", session: a, at: CloudFixture.stamp(0)),
            L.assistant(id: "m1", request: "r1", session: a, input: 1, output: 1, at: CloudFixture.stamp(1),
                        text: "Looking at the login flow.", toolUse: true),
            L.toolResult("SECRET_TOOL_OUTPUT password=hunter22", session: a, at: CloudFixture.stamp(2)),
            L.assistant(id: "m2", request: "r2", session: a, input: 1, output: 1, at: CloudFixture.stamp(3),
                        text: "SIDECHAIN TEXT", sidechain: true),
            L.line(["type": "user", "sessionId": a, "isMeta": true, "message": ["role": "user", "content": "META TEXT"]]),
            L.user("COPIED FROM ANOTHER SESSION", session: other, at: CloudFixture.stamp(4)),
            L.assistant(id: "m3", request: "r3", session: a, input: 1, output: 1, at: CloudFixture.stamp(5),
                        text: "Fixed the token refresh and added a test.", toolUse: false),
        ])
        defer { try? FileManager.default.removeItem(atPath: file.root) }
        let excerpt = try #require(SessionExcerpt.build(transcriptPath: file.path, sessionId: a))
        #expect(excerpt.hasPrefix("User: Please fix the login bug. My key is [redacted]"))
        #expect(excerpt.contains("Claude: Looking at the login flow."))
        #expect(excerpt.hasSuffix("Claude: Fixed the token refresh and added a test."))
        for leaked in ["SECRET_TOOL_OUTPUT", "hunter22", "cat secrets.txt", "SIDECHAIN", "META TEXT", "COPIED", "sk-ant"] {
            #expect(!excerpt.contains(leaked), "\(leaked)")
        }
    }

    @Test func aLongSessionKeepsItsStartAndItsEnd() throws {
        var lines: [String] = []
        for index in 0..<200 {
            lines.append(L.user("Prompt \(index) " + String(repeating: "p", count: 300), session: a, at: CloudFixture.stamp(Double(index))))
            lines.append(L.assistant(id: "m\(index)", request: "r\(index)", session: a, input: 1, output: 1,
                                     at: CloudFixture.stamp(Double(index)), text: "Reply \(index) " + String(repeating: "r", count: 300)))
        }
        let file = try writeTranscript(lines)
        defer { try? FileManager.default.removeItem(atPath: file.root) }
        let excerpt = try #require(SessionExcerpt.build(transcriptPath: file.path, sessionId: a))
        #expect(excerpt.count <= SessionSummarizer.maxExcerptCharacters)
        #expect(excerpt.count > SessionSummarizer.maxExcerptCharacters - 1000)
        #expect(excerpt.hasPrefix("User: Prompt 0 "))
        #expect(excerpt.contains("[…]"))
        #expect(excerpt.contains("Reply 199 "))
        #expect(!excerpt.contains("Prompt 100 "))
        // One huge message can't take it all.
        #expect(SessionExcerpt.clipped(String(repeating: "x", count: 10_000), to: 4_000).count == 4_000)
    }

    /// Provider-shaped fake secrets, assembled at run time so the source
    /// never holds a string a secret scanner would take for a real key.
    private static func fake(_ parts: String...) -> String { parts.joined() }

    @Test func secretsAreRedacted() {
        let text = """
        \(Self.fake("gh", "p_abcdefghijklmnopqrstuvwxyz0123")) \(Self.fake("AKIA", "ABCDEFGHIJKLMNOP")) \(Self.fake("xox", "b-1234567890-abcdefghij")) \
        eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U api_key = abcdefgh1234
        -----BEGIN RSA PRIVATE KEY-----
        MIIEowIBAAKCAQEA
        -----END RSA PRIVATE KEY-----
        """
        let redacted = SessionExcerpt.redact(text)
        for secret in ["ghp_", "AKIAABCD", "xoxb-", "eyJhbGci", "abcdefgh1234", "MIIEow"] {
            #expect(!redacted.contains(secret), "\(secret)")
        }
        #expect(SessionExcerpt.redact("Plain words stay.") == "Plain words stay.")
        // The formats the first patterns missed (review finding 19).
        let more = """
        \(Self.fake("sk_", "live_4eC39HqLyjWDarjtT1zdp7dc")) rk_test_abcdefghij0123 sb_secret_abcdefghijklmnop npm_abcdefghijklmnopqrstuvwxyz0123 \
        ya29.a0AfH6SMBabcdefghijklmnopqrst AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENGbPxRfiCY OPENAI_API_KEY="sk-proj-x" \
        mysql://root:toor-password@localhost/db authorization: Bearer 0123456789abcdefghijklmnop \
        token 9f8e7d6c5b4a39281706f5e4d3c2b1a0Zq9Xw8Vu7
        -----BEGIN OPENSSH PRIVATE KEY-----
        b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ (the rest was cut off)
        """
        let cleaned = SessionExcerpt.redact(more)
        for secret in ["4eC39HqLyjWD", "abcdefghij0123", "sb_secret_", "npm_abcd", "ya29.", "wJalrXUtnFEMI", "sk-proj",
                       "toor-password", "0123456789abcdefghijklmnop", "9f8e7d6c5b4a39281706", "b3BlbnNzaC1"] {
            #expect(!cleaned.contains(secret), "\(secret) in \(cleaned)")
        }
        #expect(cleaned.contains("AWS_SECRET_ACCESS_KEY=[redacted]") && cleaned.contains("Bearer [redacted]"))
        #expect(cleaned.contains("mysql://root:[redacted]@localhost"))
        // Ordinary long words and names are not secrets.
        let prose = "Refactored SessionTokenScannerTests and internationalization in withTaskCancellationHandler."
        #expect(SessionExcerpt.redact(prose) == prose)
        #expect(SessionExcerpt.entropy(of: "aaaa") == 0 && SessionExcerpt.entropy(of: "abcd") == 2)
    }

    // MARK: - Running (a stand-in claude)

    nonisolated struct Stub {
        let binary: String
        let dir: URL

        init(_ behaviour: String) throws {
            dir = FileManager.default.temporaryDirectory.appendingPathComponent("agentnotch-summary-\(UUID().uuidString)")
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let script = dir.appendingPathComponent("claude")
            let source = """
            #!/usr/bin/env python3
            import json, os, sys, time
            dir = os.path.dirname(os.path.abspath(__file__))
            with open(os.path.join(dir, "pid"), "w") as f:
                f.write(str(os.getpid()))
            with open(os.path.join(dir, "argv.json"), "w") as f:
                json.dump(sys.argv[1:], f)
            with open(os.path.join(dir, "env.json"), "w") as f:
                json.dump({k: v for k, v in os.environ.items() if k.startswith("CLAUDE") or k.startswith("ANTHROPIC") or k == "AI_AGENT"}, f)
            with open(os.path.join(dir, "cwd.txt"), "w") as f:
                f.write(os.getcwd())
            behaviour = \(behaviour.debugDescription)
            if behaviour == "hang":
                time.sleep(3600); sys.exit(0)
            data = sys.stdin.buffer.read()
            with open(os.path.join(dir, "stdin.txt"), "wb") as f:
                f.write(data)
            if behaviour == "login":
                sys.stdout.write(json.dumps({"type": "result", "subtype": "success", "is_error": True, "result": "Invalid API key · Please run /login"}) + "\\n")
                sys.exit(1)
            sys.stdout.write(json.dumps({"type": "result", "subtype": "success", "is_error": False, "num_turns": 1,
                "result": "Fixed the token refresh and added a test.", "total_cost_usd": 0.0012,
                "modelUsage": {"claude-haiku-4-5-20251001": {"outputTokens": 30}}}) + "\\n")
            """
            try source.write(to: script, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
            binary = script.path
        }

        func json(_ name: String) throws -> Any {
            try JSONSerialization.jsonObject(with: Data(contentsOf: dir.appendingPathComponent(name)))
        }
    }

    @Test func aRunFeedsStdinAndReadsTheResult() async throws {
        let stub = try Stub("ok")
        defer { try? FileManager.default.removeItem(at: stub.dir) }
        // Larger than a pipe holds: written on its own queue.
        let input = SessionSummarizer.input(excerpt: "User: " + String(repeating: "é", count: 60_000))
        let outcome = await SessionSummarizer.run(claudePath: stub.binary, configDirEnv: "/tmp/agentnotch-summary-tests/.claude-work",
                                                  input: input, workingDirectory: stub.dir.appendingPathComponent("cwd"),
                                                  timeout: 20)
        #expect(outcome == .summary(.init(text: "Fixed the token refresh and added a test.",
                                          model: "claude-haiku-4-5-20251001", costUsd: 0.0012)))
        #expect(try stub.json("argv.json") as? [String] == SessionSummarizer.arguments)
        #expect(try stub.json("env.json") as? [String: String] == ["CLAUDE_CONFIG_DIR": "/tmp/agentnotch-summary-tests/.claude-work"])
        #expect(try String(contentsOf: stub.dir.appendingPathComponent("stdin.txt"), encoding: .utf8) == input)
        let cwd = try String(contentsOf: stub.dir.appendingPathComponent("cwd.txt"), encoding: .utf8)
        #expect(TranscriptLocator.realPath(cwd) == TranscriptLocator.realPath(stub.dir.appendingPathComponent("cwd").path))
    }

    @Test func aSignedOutFolderIsUnavailable() async throws {
        let stub = try Stub("login")
        defer { try? FileManager.default.removeItem(at: stub.dir) }
        let outcome = await SessionSummarizer.run(claudePath: stub.binary, configDirEnv: nil, input: "x",
                                                  workingDirectory: stub.dir.appendingPathComponent("cwd"), timeout: 20)
        #expect(outcome == .unavailable("Not signed in to Claude"))
    }

    @Test(.timeLimit(.minutes(1)))
    func aHangingRunIsStopped() async throws {
        let stub = try Stub("hang")
        defer { try? FileManager.default.removeItem(at: stub.dir) }
        let outcome = await SessionSummarizer.run(claudePath: stub.binary, configDirEnv: nil, input: "x",
                                                  workingDirectory: stub.dir.appendingPathComponent("cwd"), timeout: 2)
        #expect(outcome == .failed("Claude Code didn't answer within 2s"))
        guard let text = try? String(contentsOf: stub.dir.appendingPathComponent("pid"), encoding: .utf8),
              let pid = pid_t(text) else { return }
        let deadline = Date().addingTimeInterval(30)
        while kill(pid, 0) == 0 || errno != ESRCH, Date() < deadline {
            try await Task.sleep(for: .milliseconds(250))
        }
        #expect(kill(pid, 0) != 0 && errno == ESRCH)
    }

    /// Regression (review finding 21): cancelling the task that runs a
    /// summary (summaries switched off, sign-out, quit) stops the child at
    /// once, and a task cancelled before the launch starts none.
    @Test(.timeLimit(.minutes(1)))
    func aCancelledRunStopsItsChild() async throws {
        let stub = try Stub("hang")
        defer { try? FileManager.default.removeItem(at: stub.dir) }
        let started = Date()
        let task = Task {
            await SessionSummarizer.run(claudePath: stub.binary, configDirEnv: nil, input: "x",
                                        workingDirectory: stub.dir.appendingPathComponent("cwd"), timeout: 60)
        }
        let pidFile = stub.dir.appendingPathComponent("pid")
        let deadline = Date().addingTimeInterval(20)
        while !FileManager.default.fileExists(atPath: pidFile.path), Date() < deadline {
            try await Task.sleep(for: .milliseconds(50))
        }
        try await Task.sleep(for: .milliseconds(200))
        task.cancel()
        #expect(await task.value == .failed(SessionSummarizer.cancelledReason))
        #expect(Date().timeIntervalSince(started) < 30)
        let pid = try #require(pid_t(try String(contentsOf: pidFile, encoding: .utf8)))
        let gone = Date().addingTimeInterval(10)
        while kill(pid, 0) == 0, Date() < gone {
            try await Task.sleep(for: .milliseconds(100))
        }
        #expect(kill(pid, 0) != 0 && errno == ESRCH)

        // Cancelled before it starts: no child at all.
        let early = try Stub("ok")
        defer { try? FileManager.default.removeItem(at: early.dir) }
        let never = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return await SessionSummarizer.run(claudePath: early.binary, configDirEnv: nil, input: "x",
                                               workingDirectory: early.dir.appendingPathComponent("cwd"), timeout: 20)
        }
        #expect(await never.value == .failed(SessionSummarizer.cancelledReason))
        try await Task.sleep(for: .milliseconds(500))
        #expect(!FileManager.default.fileExists(atPath: early.dir.appendingPathComponent("pid").path))
    }

    @Test func theRealRunnerNeverRunsBeforeBootstrap() async {
        #expect(!AppIdentity.isFrozen)
        let request = SessionSummarizer.Request(sessionId: a, identityId: CloudFixture.identityId, input: "x",
                                                configDirEnv: nil, configDirs: [],
                                                workingDirectory: FileManager.default.temporaryDirectory)
        #expect(await SessionSummarizer.runClaudeCode(request) == .failed("Session summaries need a bootstrapped engine"))
        // Nor anything outside the temporary directory, even called directly.
        #expect(await SessionSummarizer.run(claudePath: "/usr/local/bin/claude", configDirEnv: nil, input: "x",
                                            workingDirectory: FileManager.default.temporaryDirectory)
                == .failed("Session summaries need a bootstrapped engine"))
    }

    // MARK: - When

    @Test func aSessionIsDueTenMinutesAfterItEnded() {
        typealias S = SessionSummaryStore
        let now = CloudFixture.base
        let on = now.addingTimeInterval(-3600)
        let ended = now.addingTimeInterval(-11 * 60)
        #expect(S.isDue(endedAt: ended, messageCount: 2, existing: nil, attempt: nil, since: on, now: now))
        #expect(!S.isDue(endedAt: nil, messageCount: 20, existing: nil, attempt: nil, since: on, now: now))
        #expect(!S.isDue(endedAt: now.addingTimeInterval(-9 * 60), messageCount: 20, existing: nil, attempt: nil, since: on, now: now))
        #expect(!S.isDue(endedAt: ended, messageCount: 1, existing: nil, attempt: nil, since: on, now: now))
        let done = S.Entry(text: "t", model: "m", generatedAt: now, messageCount: 10, costUsd: nil)
        #expect(!S.isDue(endedAt: ended, messageCount: 15, existing: done, attempt: nil, since: on, now: now))
        #expect(S.isDue(endedAt: ended, messageCount: 16, existing: done, attempt: nil, since: on, now: now))
        let waiting = S.Attempt(failures: 1, nextAttemptAt: now.addingTimeInterval(60), reason: "x")
        #expect(!S.isDue(endedAt: ended, messageCount: 5, existing: nil, attempt: waiting, since: on, now: now))
        // Ended before summaries were turned on, or never turned on: never
        // (review findings 4 and 25).
        #expect(!S.isDue(endedAt: ended, messageCount: 5, existing: nil, attempt: nil, since: ended.addingTimeInterval(1), now: now))
        #expect(!S.isDue(endedAt: ended, messageCount: 5, existing: nil, attempt: nil, since: nil, now: now))
    }

    @Test func theNewestDueSessionGoesFirstAndAnHourHoldsTwenty() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("summary-store"))
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent(SessionSummaryStore.fileName)
        let store = SessionSummaryStore(fileURL: file, persists: true)
        let now = CloudFixture.base
        func candidate(_ id: String, endedMinutesAgo: Double, messages: Int = 5) -> SessionSummaryStore.Candidate {
            .init(key: id, sessionId: id, identityId: CloudFixture.identityId, accountKey: CloudFixture.accountKey,
                  transcriptPath: "/x/projects/-y/\(id).jsonl", messageCount: messages,
                  endedAt: now.addingTimeInterval(-endedMinutesAgo * 60))
        }
        let list = [candidate("s1", endedMinutesAgo: 30), candidate("s2", endedMinutesAgo: 12),
                    candidate("s3", endedMinutesAgo: 5), candidate("s4", endedMinutesAgo: 60, messages: 1)]
        // Never turned on: nothing is due.
        #expect(store.nextCandidate(from: list, now: now) == nil)
        store.noteEnabled(at: now.addingTimeInterval(-2 * 3600))
        #expect(store.nextCandidate(from: list, now: now)?.sessionId == "s2")

        store.record(key: "s2", summary: .init(text: "t", model: "m", costUsd: 0.001), messageCount: 5, at: now)
        #expect(store.nextCandidate(from: list, now: now)?.sessionId == "s1")
        store.recordFailure(key: "s1", reason: "x", at: now)
        #expect(store.attempt(for: "s1")?.nextAttemptAt == now.addingTimeInterval(SessionSummaryStore.initialRetry))
        #expect(store.nextCandidate(from: list, now: now) == nil)
        store.recordFailure(key: "s1", reason: "x", at: now)
        #expect(store.attempt(for: "s1")?.nextAttemptAt == now.addingTimeInterval(2 * SessionSummaryStore.initialRetry))

        for minute in 0..<SessionSummaryStore.maxPerHour {
            store.noteRun(at: now.addingTimeInterval(Double(minute - 59) * 60))
        }
        #expect(store.runsInLastHour(now: now) == 20)
        let later = now.addingTimeInterval(24 * 3600)
        #expect(store.nextCandidate(from: list, now: now.addingTimeInterval(1)) == nil)
        // A day on: every due one is, and the one that ended last goes first.
        #expect(store.nextCandidate(from: list, now: later)?.sessionId == "s3")
        #expect(store.nextCandidate(from: list.filter { $0.sessionId != "s3" }, now: later)?.sessionId == "s1")

        store.saveNow()
        #expect(CloudFiles.permissions(of: file) == 0o600)
        let reloaded = SessionSummaryStore(fileURL: file, persists: true)
        #expect(reloaded.summary(for: "s2")?.text == "t")
        #expect(reloaded.enabledAt == now.addingTimeInterval(-2 * 3600))
    }

    /// Regression (review finding 4): at most 60 runs a day, whatever the hours allow.
    @Test func aDayHoldsSixty() {
        let store = SessionSummaryStore(fileURL: nil, persists: false)
        let now = CloudFixture.base
        store.noteEnabled(at: now.addingTimeInterval(-48 * 3600))
        let due = SessionSummaryStore.Candidate(key: "k", sessionId: "s", identityId: CloudFixture.identityId,
                                                accountKey: CloudFixture.accountKey, transcriptPath: "/x/projects/-y/s.jsonl",
                                                messageCount: 5, endedAt: now.addingTimeInterval(-3600))
        // Three runs every hour for the last 20 hours: never more than 20 an hour.
        for hour in 0..<20 {
            for minute in [0, 20, 40] {
                store.noteRun(at: now.addingTimeInterval(-Double(hour) * 3600 - Double(minute + 1) * 60))
            }
        }
        #expect(store.runsInLastHour(now: now) == 3)
        #expect(store.runsInLastDay(now: now) == 60)
        #expect(store.nextCandidate(from: [due], now: now) == nil)
        // Four hours later the oldest have aged out of the day.
        #expect(store.nextCandidate(from: [due], now: now.addingTimeInterval(4 * 3600 + 3600))?.key == "k")
    }
}
