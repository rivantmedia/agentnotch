//
//  OrderedJSON.swift
//  ClaudeControl
//
//  A JSON value that remembers what JSONSerialization forgets: the order of an
//  object's keys and how each number was spelled. The hook installer edits the
//  user's settings.json through it, so a write changes only the values it
//  means to change (`hooks`, `statusLine`) and leaves every other byte alone:
//  no sorted keys, no `0.1` turning into `0.10000000000000001`, no `\/`
//  losing its escape. See `SettingsDocument`.
//

import Foundation

nonisolated enum OrderedJSON: Sendable {
    struct Member: Sendable {
        var key: String
        var value: OrderedJSON
    }

    case object([Member])
    case array([OrderedJSON])
    case string(String)
    /// The number exactly as written in the source (or by us).
    case number(String)
    case bool(Bool)
    case null

    // MARK: - Building

    static func int(_ value: Int) -> OrderedJSON { .number(String(value)) }

    /// An object from `(key, value)` pairs, in that order.
    static func object(_ pairs: KeyValuePairs<String, OrderedJSON>) -> OrderedJSON {
        .object(pairs.map { Member(key: $0.key, value: $0.value) })
    }

    // MARK: - Reading

    var members: [Member]? {
        if case .object(let members) = self { return members }
        return nil
    }

    var items: [OrderedJSON]? {
        if case .array(let items) = self { return items }
        return nil
    }

    var stringValue: String? {
        if case .string(let string) = self { return string }
        return nil
    }

    var isObject: Bool { members != nil }

    /// The value for `key` in an object (the last one, as JavaScript reads a
    /// repeated key), else nil.
    subscript(key: String) -> OrderedJSON? {
        members?.last { $0.key == key }?.value
    }

    /// Set `key` in an object: in place where it already is (repeats of it
    /// dropped), appended where it isn't, removed for nil. No-op for non-objects.
    mutating func set(_ key: String, _ value: OrderedJSON?) {
        guard var members else { return }
        if let value {
            if let index = members.lastIndex(where: { $0.key == key }) {
                members[index].value = value
                members = members.enumerated().filter { $0.offset == index || $0.element.key != key }.map(\.element)
            } else {
                members.append(Member(key: key, value: value))
            }
        } else {
            members.removeAll { $0.key == key }
        }
        self = .object(members)
    }

    // MARK: - Comparing

    /// Equal as JSON: objects regardless of key order (last repeat wins),
    /// numbers by value.
    func isEquivalent(to other: OrderedJSON) -> Bool {
        switch (self, other) {
        case (.object(let lhs), .object(let rhs)):
            let left = Dictionary(lhs.map { ($0.key, $0.value) }, uniquingKeysWith: { _, last in last })
            let right = Dictionary(rhs.map { ($0.key, $0.value) }, uniquingKeysWith: { _, last in last })
            guard left.count == right.count else { return false }
            return left.allSatisfy { key, value in right[key].map(value.isEquivalent) ?? false }
        case (.array(let lhs), .array(let rhs)):
            return lhs.count == rhs.count && zip(lhs, rhs).allSatisfy { $0.isEquivalent(to: $1) }
        case (.string(let lhs), .string(let rhs)):
            return lhs == rhs
        case (.number(let lhs), .number(let rhs)):
            if lhs == rhs { return true }
            guard let left = Double(lhs), let right = Double(rhs) else { return false }
            return left == right
        case (.bool(let lhs), .bool(let rhs)):
            return lhs == rhs
        case (.null, .null):
            return true
        default:
            return false
        }
    }

    static func equivalent(_ lhs: OrderedJSON?, _ rhs: OrderedJSON?) -> Bool {
        switch (lhs, rhs) {
        case (nil, nil): return true
        case (let lhs?, let rhs?): return lhs.isEquivalent(to: rhs)
        default: return false
        }
    }

    // MARK: - Writing

    /// JSON text the way JavaScript's `JSON.stringify(value, null, unit)` writes
    /// it (Claude Code's own format), each line after the first starting with
    /// `indent`. An empty `unit` writes it on one line with no spaces.
    func serialized(indent: String = "", unit: String = "  ") -> String {
        var out = ""
        write(to: &out, indent: indent, unit: unit)
        return out
    }

    private func write(to out: inout String, indent: String, unit: String) {
        switch self {
        case .object(let members):
            guard !members.isEmpty else { out += "{}"; return }
            let inner = indent + unit
            out += "{"
            for (index, member) in members.enumerated() {
                if index > 0 { out += "," }
                if !unit.isEmpty { out += "\n" + inner }
                out += Self.quoted(member.key) + (unit.isEmpty ? ":" : ": ")
                member.value.write(to: &out, indent: inner, unit: unit)
            }
            if !unit.isEmpty { out += "\n" + indent }
            out += "}"
        case .array(let items):
            guard !items.isEmpty else { out += "[]"; return }
            let inner = indent + unit
            out += "["
            for (index, item) in items.enumerated() {
                if index > 0 { out += "," }
                if !unit.isEmpty { out += "\n" + inner }
                item.write(to: &out, indent: inner, unit: unit)
            }
            if !unit.isEmpty { out += "\n" + indent }
            out += "]"
        case .string(let string):
            out += Self.quoted(string)
        case .number(let text):
            out += text
        case .bool(let value):
            out += value ? "true" : "false"
        case .null:
            out += "null"
        }
    }

    /// A JSON string literal, escaped as `JSON.stringify` escapes it.
    static func quoted(_ string: String) -> String {
        var out = "\""
        for scalar in string.unicodeScalars {
            switch scalar {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\u{08}": out += "\\b"
            case "\u{0C}": out += "\\f"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            default:
                if scalar.value < 0x20 {
                    out += String(format: "\\u%04x", scalar.value)
                } else {
                    out.unicodeScalars.append(scalar)
                }
            }
        }
        return out + "\""
    }
}

// MARK: - Parsing

nonisolated extension OrderedJSON {
    struct ParseError: Error, Equatable {
        let offset: Int
    }

    /// Where one member of the top-level object sits in the source bytes.
    struct MemberSpan: Sendable {
        let key: String
        /// The key's opening quote.
        let keyStart: Int
        /// First byte of the value.
        let valueStart: Int
        /// One past the value's last byte.
        let valueEnd: Int
    }

    /// The top-level object of a document, with the byte range of each member.
    struct TopLevel: Sendable {
        var value: OrderedJSON
        /// The `{` and one past the `}`.
        let objectStart: Int
        let objectEnd: Int
        let spans: [MemberSpan]
    }

    /// Parse a whole JSON document (UTF-8, optional BOM). Tolerates trailing
    /// commas, as JSONSerialization does; rejects everything else that isn't JSON.
    static func parse(_ data: Data) throws -> OrderedJSON {
        var parser = Parser(bytes: Array(data))
        return try parser.document().value
    }

    /// Parse a document whose top level must be an object, keeping where its
    /// members are. Nil when the top level is something else.
    static func parseTopLevelObject(_ bytes: [UInt8]) throws -> TopLevel? {
        var parser = Parser(bytes: bytes)
        let (value, spans, start, end) = try parser.document()
        guard value.isObject, let start, let end else { return nil }
        return TopLevel(value: value, objectStart: start, objectEnd: end, spans: spans)
    }

    /// Deepest nesting the parser follows: anything deeper (no settings.json
    /// comes close) is refused rather than recursed into on a small stack.
    static let maxDepth = 256

    private struct Parser {
        let bytes: [UInt8]
        var index = 0
        var depth = 0

        init(bytes: [UInt8]) {
            self.bytes = bytes
            if bytes.starts(with: [0xEF, 0xBB, 0xBF]) { index = 3 }
        }

        mutating func document() throws -> (value: OrderedJSON, spans: [MemberSpan], start: Int?, end: Int?) {
            skipWhitespace()
            let start = index
            var spans: [MemberSpan] = []
            let value: OrderedJSON
            if peek() == UInt8(ascii: "{") {
                value = try object(spans: &spans)
            } else {
                value = try self.value()
            }
            let end = index
            skipWhitespace()
            guard index == bytes.count else { throw ParseError(offset: index) }
            return value.isObject ? (value, spans, start, end) : (value, [], nil, nil)
        }

        func peek() -> UInt8? { index < bytes.count ? bytes[index] : nil }

        mutating func skipWhitespace() {
            while let byte = peek(), byte == 0x20 || byte == 0x09 || byte == 0x0A || byte == 0x0D { index += 1 }
        }

        mutating func expect(_ byte: UInt8) throws {
            guard peek() == byte else { throw ParseError(offset: index) }
            index += 1
        }

        mutating func value() throws -> OrderedJSON {
            skipWhitespace()
            guard let byte = peek() else { throw ParseError(offset: index) }
            switch byte {
            case UInt8(ascii: "{"):
                var ignored: [MemberSpan] = []
                return try object(spans: &ignored)
            case UInt8(ascii: "["):
                return try array()
            case UInt8(ascii: "\""):
                return .string(try string())
            case UInt8(ascii: "t"):
                try literal("true")
                return .bool(true)
            case UInt8(ascii: "f"):
                try literal("false")
                return .bool(false)
            case UInt8(ascii: "n"):
                try literal("null")
                return .null
            default:
                return .number(try number())
            }
        }

        mutating func literal(_ word: String) throws {
            for byte in word.utf8 { try expect(byte) }
        }

        mutating func object(spans: inout [MemberSpan]) throws -> OrderedJSON {
            try expect(UInt8(ascii: "{"))
            depth += 1
            defer { depth -= 1 }
            guard depth <= OrderedJSON.maxDepth else { throw ParseError(offset: index) }
            var members: [Member] = []
            skipWhitespace()
            if peek() == UInt8(ascii: "}") {
                index += 1
                return .object(members)
            }
            while true {
                skipWhitespace()
                if peek() == UInt8(ascii: "}"), !members.isEmpty {  // trailing comma
                    index += 1
                    return .object(members)
                }
                let keyStart = index
                let key = try string()
                skipWhitespace()
                try expect(UInt8(ascii: ":"))
                skipWhitespace()
                let valueStart = index
                let value = try self.value()
                spans.append(MemberSpan(key: key, keyStart: keyStart, valueStart: valueStart, valueEnd: index))
                members.append(Member(key: key, value: value))
                skipWhitespace()
                guard let byte = peek() else { throw ParseError(offset: index) }
                index += 1
                if byte == UInt8(ascii: "}") { return .object(members) }
                guard byte == UInt8(ascii: ",") else { throw ParseError(offset: index - 1) }
            }
        }

        mutating func array() throws -> OrderedJSON {
            try expect(UInt8(ascii: "["))
            depth += 1
            defer { depth -= 1 }
            guard depth <= OrderedJSON.maxDepth else { throw ParseError(offset: index) }
            var items: [OrderedJSON] = []
            skipWhitespace()
            if peek() == UInt8(ascii: "]") {
                index += 1
                return .array(items)
            }
            while true {
                skipWhitespace()
                if peek() == UInt8(ascii: "]"), !items.isEmpty {  // trailing comma
                    index += 1
                    return .array(items)
                }
                items.append(try value())
                skipWhitespace()
                guard let byte = peek() else { throw ParseError(offset: index) }
                index += 1
                if byte == UInt8(ascii: "]") { return .array(items) }
                guard byte == UInt8(ascii: ",") else { throw ParseError(offset: index - 1) }
            }
        }

        mutating func number() throws -> String {
            let start = index
            if peek() == UInt8(ascii: "-") { index += 1 }
            guard let first = peek(), isDigit(first) else { throw ParseError(offset: index) }
            if first == UInt8(ascii: "0") {
                index += 1
            } else {
                while let byte = peek(), isDigit(byte) { index += 1 }
            }
            if peek() == UInt8(ascii: ".") {
                index += 1
                guard let byte = peek(), isDigit(byte) else { throw ParseError(offset: index) }
                while let byte = peek(), isDigit(byte) { index += 1 }
            }
            if let byte = peek(), byte == UInt8(ascii: "e") || byte == UInt8(ascii: "E") {
                index += 1
                if let sign = peek(), sign == UInt8(ascii: "+") || sign == UInt8(ascii: "-") { index += 1 }
                guard let byte = peek(), isDigit(byte) else { throw ParseError(offset: index) }
                while let byte = peek(), isDigit(byte) { index += 1 }
            }
            return String(decoding: bytes[start..<index], as: UTF8.self)
        }

        func isDigit(_ byte: UInt8) -> Bool { byte >= 0x30 && byte <= 0x39 }

        mutating func string() throws -> String {
            try expect(UInt8(ascii: "\""))
            var scalars = String.UnicodeScalarView()
            var runStart = index
            func flushRun(_ end: Int) throws {
                guard end > runStart else { return }
                guard let text = String(validating: bytes[runStart..<end], as: UTF8.self) else {
                    throw ParseError(offset: runStart)
                }
                scalars.append(contentsOf: text.unicodeScalars)
            }
            while true {
                guard let byte = peek() else { throw ParseError(offset: index) }
                if byte == UInt8(ascii: "\"") {
                    try flushRun(index)
                    index += 1
                    return String(scalars)
                }
                if byte < 0x20 { throw ParseError(offset: index) }
                if byte == UInt8(ascii: "\\") {
                    try flushRun(index)
                    index += 1
                    guard let escape = peek() else { throw ParseError(offset: index) }
                    index += 1
                    switch escape {
                    case UInt8(ascii: "\""): scalars.append("\"")
                    case UInt8(ascii: "\\"): scalars.append("\\")
                    case UInt8(ascii: "/"): scalars.append("/")
                    case UInt8(ascii: "b"): scalars.append("\u{08}")
                    case UInt8(ascii: "f"): scalars.append("\u{0C}")
                    case UInt8(ascii: "n"): scalars.append("\n")
                    case UInt8(ascii: "r"): scalars.append("\r")
                    case UInt8(ascii: "t"): scalars.append("\t")
                    case UInt8(ascii: "u"):
                        var code = try hex4()
                        if (0xD800...0xDBFF).contains(code),
                           peek() == UInt8(ascii: "\\"), index + 1 < bytes.count, bytes[index + 1] == UInt8(ascii: "u") {
                            let save = index
                            index += 2
                            let low = try hex4()
                            if (0xDC00...0xDFFF).contains(low) {
                                code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00)
                            } else {
                                index = save
                            }
                        }
                        // A lone surrogate can't be held in a Swift string;
                        // JavaScript would keep it, so refuse rather than alter it.
                        guard let scalar = Unicode.Scalar(code) else { throw ParseError(offset: index) }
                        scalars.append(scalar)
                    default:
                        throw ParseError(offset: index - 1)
                    }
                    runStart = index
                    continue
                }
                index += 1
            }
        }

        mutating func hex4() throws -> UInt32 {
            guard index + 4 <= bytes.count else { throw ParseError(offset: index) }
            var value: UInt32 = 0
            for _ in 0..<4 {
                let byte = bytes[index]
                let digit: UInt32
                switch byte {
                case 0x30...0x39: digit = UInt32(byte - 0x30)
                case 0x41...0x46: digit = UInt32(byte - 0x41 + 10)
                case 0x61...0x66: digit = UInt32(byte - 0x61 + 10)
                default: throw ParseError(offset: index)
                }
                value = value * 16 + digit
                index += 1
            }
            return value
        }
    }
}
