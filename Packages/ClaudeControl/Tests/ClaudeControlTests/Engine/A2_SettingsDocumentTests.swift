import Foundation
import Testing
@testable import ClaudeControl

/// The order- and spelling-keeping JSON the installer edits settings.json with.
struct OrderedJSONTests {
    @Test func parsesAndWritesBackJavaScriptsFormat() throws {
        let text = """
        {
          "b": 1,
          "a": [
            0.1,
            1e-7,
            -0,
            true,
            null
          ],
          "s": "tab\\tquote\\"slash/é\\u0001",
          "o": {},
          "l": []
        }
        """
        let value = try OrderedJSON.parse(Data(text.utf8))
        #expect(value.members?.map(\.key) == ["b", "a", "s", "o", "l"])
        #expect(value.serialized() == text)
        #expect(value["s"]?.stringValue == "tab\tquote\"slash/é\u{01}")
    }

    @Test func oneLineFormat() throws {
        let value = try OrderedJSON.parse(Data(#"{"a": [1, {"b": null}], "c": "d"}"#.utf8))
        #expect(value.serialized(unit: "") == #"{"a":[1,{"b":null}],"c":"d"}"#)
    }

    @Test(arguments: ["", "{", "[1,]x", "{\"a\" 1}", "01", "1.", "\"\\x\"", "\"\\ud800\"", "{\"a\":1}}", "nul"])
    func rejectsWhatIsNotJSON(_ text: String) {
        #expect(throws: OrderedJSON.ParseError.self) { try OrderedJSON.parse(Data(text.utf8)) }
    }

    @Test func acceptsWhatFoundationAccepts() throws {
        // A BOM, trailing commas, surrogate pairs.
        let value = try OrderedJSON.parse(Data([0xEF, 0xBB, 0xBF] + Array(#"{"a": [1, 2,], "e": "\ud83d\ude00",}"#.utf8)))
        #expect(value["e"]?.stringValue == "😀")
        #expect(value["a"]?.items?.count == 2)
    }

    @Test func equivalenceIgnoresKeyOrderAndNumberSpelling() throws {
        let lhs = try OrderedJSON.parse(Data(#"{"a": 1.0, "b": [1e2, "x"]}"#.utf8))
        let rhs = try OrderedJSON.parse(Data(#"{"b": [100, "x"], "a": 1}"#.utf8))
        #expect(lhs.isEquivalent(to: rhs))
        #expect(!lhs.isEquivalent(to: try OrderedJSON.parse(Data(#"{"a": 1, "b": ["x", 100]}"#.utf8))))
        // A repeated key: the last one counts, as JavaScript reads it.
        let repeated = try OrderedJSON.parse(Data(#"{"a": 1, "a": 2}"#.utf8))
        #expect(OrderedJSON.equivalent(repeated["a"], .int(2)))
    }

    @Test func setKeepsPositionAndAppendsNewKeys() throws {
        var value = try OrderedJSON.parse(Data(#"{"a": 1, "b": 2, "a": 3}"#.utf8))
        value.set("a", .int(9))
        value.set("c", .bool(false))
        value.set("b", nil)
        #expect(value.serialized(unit: "") == #"{"a":9,"c":false}"#)
    }
}

/// Splicing: a change to one member rewrites only that member's bytes.
struct SettingsDocumentTests {
    @Test func untouchedDocumentsKeepTheirBytes() throws {
        let text = "{\n    \"z\" : 0.10,\n  \"a\":[ ]\n}\n"
        let document = try #require(SettingsDocument(data: Data(text.utf8)))
        #expect(document.data() == Data(text.utf8))
    }

    @Test func replacingAValueKeepsEverythingAroundIt() throws {
        let text = "{\n  \"model\": \"opus\",\n  \"hooks\": {\"x\": 1},\n  \"tiny\": 1e-7\n}\n"
        var document = try #require(SettingsDocument(data: Data(text.utf8)))
        document.set("hooks", .object(["Stop": .array([])]))
        #expect(String(decoding: document.data(), as: UTF8.self)
            == "{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"Stop\": []\n  },\n  \"tiny\": 1e-7\n}\n")
    }

    @Test func addingAndRemovingAMemberRoundTrips() throws {
        let text = "{\n  \"model\": \"opus\",\n  \"env\": {\n    \"B\": \"1\"\n  }\n}"
        var document = try #require(SettingsDocument(data: Data(text.utf8)))
        document.set("statusLine", .object(["type": .string("command")]))
        let added = document.data()
        #expect(String(decoding: added, as: UTF8.self).hasSuffix("  },\n  \"statusLine\": {\n    \"type\": \"command\"\n  }\n}"))

        var again = try #require(SettingsDocument(data: added))
        again.set("statusLine", nil)
        #expect(again.data() == Data(text.utf8))
    }

    @Test func removingTheFirstMemberAndEveryMember() throws {
        let text = #"{"a": 1, "b": 2}"#
        var document = try #require(SettingsDocument(data: Data(text.utf8)))
        document.set("a", nil)
        #expect(String(decoding: document.data(), as: UTF8.self) == #"{"b": 2}"#)
        document.set("b", nil)
        #expect(String(decoding: document.data(), as: UTF8.self) == "{}")
    }

    @Test func aMissingOrBlankFileIsEmpty() throws {
        for data in [nil, Data(), Data(" \n".utf8)] {
            let document = try #require(SettingsDocument(data: data))
            #expect(document.value.members?.isEmpty == true)
        }
        var fresh = try #require(SettingsDocument(data: nil))
        fresh.set("hooks", .object([]))
        #expect(String(decoding: fresh.data(), as: UTF8.self) == "{\n  \"hooks\": {}\n}\n")
    }

    @Test(arguments: ["[1]", "\"s\"", "{", "{} {}", "\u{FEFF}[]"])
    func refusesAnythingButAnObject(_ text: String) {
        #expect(SettingsDocument(data: Data(text.utf8)) == nil)
    }
}

/// Shell words, for reading back the commands in a settings.json.
struct ShellWordsTests {
    @Test func quotesAnything() {
        for value in ["plain", "with space", "it's", "$HOME `x` $(y) \"z\"", "new\nline", ""] {
            #expect(ShellWords.words("echo " + ShellWords.quote(value)) == ["echo", value])
        }
    }

    @Test func splitsLikeTheShell() {
        #expect(ShellWords.tokens(#"a 'b c' "d \"e\" \$f" g\ h; i && j || k | l &"#) == [
            .word("a"), .word("b c"), .word(#"d "e" $f"#), .word("g h"), .operator(";"), .word("i"), .operator("&&"),
            .word("j"), .operator("||"), .word("k"), .operator("|"), .word("l"), .operator("&"),
        ])
        #expect(ShellWords.words("python3 '/a b/c.py'").last == "/a b/c.py")
        #expect(ShellWords.words("unterminated 'quote") == ["unterminated", "quote"])
    }
}

/// The commands the installer writes, and recognising any notch app's.
struct HookCommandsTests {
    @Test func theCommandWeWrite() {
        #expect(HookCommands.command(runningScript: "/a b/h.py", python: "/usr/bin/python3")
            == #"[ -f '/a b/h.py' ] || exit 0; P='/usr/bin/python3'; [ -x "$P" ] || P=python3; exec "$P" -S '/a b/h.py'"#)
        // S8: a bare python3 never runs xcode-select's shim on a Mac without
        // the developer tools.
        #expect(HookCommands.command(runningScript: "/a/h.py", python: "python3")
            == #"[ -f '/a/h.py' ] || exit 0; P='python3'; case "$(command -v "$P")" in ''|/usr/bin/python3) /usr/bin/xcode-select -p >/dev/null 2>&1 || exit 0;; esac; exec "$P" -S '/a/h.py'"#)
        #expect(HookCommands.runs(HookCommands.command(runningScript: "/a/h.py", python: "python3"), script: "h.py"))
    }

    @Test func lastSimpleCommandAndInterpreter() {
        #expect(HookCommands.lastSimpleCommand("a; b c && d e") == ["d", "e"])
        #expect(HookCommands.lastSimpleCommand("a;") == ["a"])
        #expect(HookCommands.interpreterWord(in: ["exec", "X=1", "/usr/bin/env", "-i", "Y=2", "python3", "-S"]) == "python3")
        #expect(HookCommands.isInterpreter("/opt/homebrew/bin/python3.12"))
        #expect(HookCommands.isInterpreter("$P"))
        #expect(!HookCommands.isInterpreter("node"))
    }

    @Test func vibeIsland() {
        #expect(HookCommands.runsVibeIsland(#"/bin/sh -c '[ -x "$HOME/.vibe-island/bin/vibe-island-bridge" ] && exit 0'"#))
        #expect(!HookCommands.runsVibeIsland("python3 /x/vibe-island.py"))
    }
}
