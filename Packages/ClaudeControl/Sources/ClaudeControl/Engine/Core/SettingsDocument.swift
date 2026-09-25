//
//  SettingsDocument.swift
//  ClaudeControl
//
//  A settings.json as text plus its parsed top-level object, edited by
//  splicing: changing `hooks` rewrites only the bytes of that value (in the
//  file's own indentation), so every other key keeps its position, spacing,
//  number spelling and escapes. A file Claude Code wrote (`JSON.stringify`
//  with two spaces) comes back byte for byte after an install and uninstall.
//
//  Refuses (nil) anything that isn't a JSON object at the top level. A blank
//  or missing file is the empty object.
//

import Foundation

nonisolated struct SettingsDocument: Sendable {
    /// The top-level object as parsed (or as edited).
    private(set) var value: OrderedJSON
    /// The source bytes; nil for a missing or blank file.
    private let source: [UInt8]?
    private let topLevel: OrderedJSON.TopLevel?
    /// Keys changed with `set`, and their new values (nil = removed).
    private var changes: [(key: String, value: OrderedJSON?)] = []

    /// Parse `data` (nil = no file). Nil when it can't be read back as a JSON
    /// object, which the installer must never overwrite.
    init?(data: Data?) {
        guard let data, !Self.isBlank(data) else {
            value = .object([])
            source = nil
            topLevel = nil
            return
        }
        // Two independent parsers must agree it is an object: Foundation's
        // (what the installer always trusted) and ours (which keeps order).
        guard (try? JSONSerialization.jsonObject(with: data)) is [String: Any] else { return nil }
        let bytes = Array(data)
        guard let parsed = try? OrderedJSON.parseTopLevelObject(bytes) else { return nil }
        value = parsed.value
        source = bytes
        topLevel = parsed
    }

    /// True for an empty or whitespace-only file, which is safe to treat as `{}`.
    /// Bytes that aren't valid UTF-8 are not blank: they fall through to the
    /// parse, and the parse refuses.
    static func isBlank(_ data: Data) -> Bool {
        guard let text = String(data: data, encoding: .utf8) else { return false }
        return text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    subscript(key: String) -> OrderedJSON? { value[key] }

    /// Replace (or with nil, remove) one top-level key.
    mutating func set(_ key: String, _ newValue: OrderedJSON?) {
        value.set(key, newValue)
        changes.removeAll { $0.key == key }
        changes.append((key, newValue))
    }

    /// The document's bytes with every `set` applied.
    func data() -> Data {
        guard let source, let topLevel else {
            return Data((value.serialized(indent: "", unit: "  ") + "\n").utf8)
        }
        guard !changes.isEmpty else { return Data(source) }
        let style = Style(source: source, topLevel: topLevel)
        let spans = topLevel.spans

        // What becomes of each original member: kept as is, a new value, or gone.
        // A repeated key keeps only its last occurrence (the one JSON readers use).
        var replacements: [Int: [UInt8]] = [:]
        var removed = Set<Int>()
        var inserted: [OrderedJSON.Member] = []
        for change in changes {
            let indices = spans.indices.filter { spans[$0].key == change.key }
            if let newValue = change.value {
                if let last = indices.last {
                    replacements[last] = Array(newValue.serialized(indent: style.memberIndent, unit: style.unit).utf8)
                    removed.formUnion(indices.dropLast())
                } else {
                    inserted.append(OrderedJSON.Member(key: change.key, value: newValue))
                }
            } else {
                removed.formUnion(indices)
            }
        }

        let kept = spans.indices.filter { !removed.contains($0) }
        guard !kept.isEmpty else {
            // Every original member went: write the object afresh.
            var text = source
            let object = value.serialized(indent: "", unit: style.unit)
            text.replaceSubrange(topLevel.objectStart..<topLevel.objectEnd, with: Array(object.utf8))
            return Data(text)
        }

        // `{` + what preceded the first member, then each kept member with the
        // separator that preceded it in the file, then additions, then what
        // followed the last member, and `}`.
        var body: [UInt8] = Array(source[(topLevel.objectStart + 1)..<spans[0].keyStart])
        for (position, index) in kept.enumerated() {
            if position > 0 {
                body += source[spans[index - 1].valueEnd..<spans[index].keyStart]
            }
            body += source[spans[index].keyStart..<spans[index].valueStart]
            body += replacements[index] ?? Array(source[spans[index].valueStart..<spans[index].valueEnd])
        }
        for member in inserted {
            var addition = style.unit.isEmpty ? "," : ",\n" + style.memberIndent
            addition += OrderedJSON.quoted(member.key) + (style.unit.isEmpty ? ":" : ": ")
            addition += member.value.serialized(indent: style.memberIndent, unit: style.unit)
            body += Array(addition.utf8)
        }
        body += source[spans[spans.count - 1].valueEnd..<(topLevel.objectEnd - 1)]

        var text = Array(source[0...topLevel.objectStart])
        text += body
        text += source[(topLevel.objectEnd - 1)...]
        return Data(text)
    }

    /// How the file lays out its top-level object.
    private struct Style {
        /// Whitespace before each top-level key; empty when they share a line.
        let memberIndent: String
        /// One level of indentation; empty for a one-line file.
        let unit: String

        init(source: [UInt8], topLevel: OrderedJSON.TopLevel) {
            guard let first = topLevel.spans.first else {
                memberIndent = "  "
                unit = "  "
                return
            }
            // The whitespace between the line start and the first key.
            var start = first.keyStart
            while start > 0, source[start - 1] == 0x20 || source[start - 1] == 0x09 { start -= 1 }
            if start > 0, source[start - 1] == 0x0A {
                let indent = String(decoding: source[start..<first.keyStart], as: UTF8.self)
                memberIndent = indent
                unit = indent.isEmpty ? "  " : indent
            } else {
                memberIndent = ""
                unit = ""
            }
        }
    }
}
