//
//  JSONFieldScanner.swift
//  ClaudeControl
//
//  Picks a few top-level values out of a JSON object without parsing the
//  rest. `.claude.json` can be megabytes of project history, MCP servers and
//  feature flags; the app needs two keys of it (`oauthAccount`,
//  `cachedUsageUtilization`). The scanner walks the top-level object byte by
//  byte, skipping every other value (strings with their escapes, nested
//  objects and arrays) without building it, and hands back the raw bytes of
//  the wanted values, which are then parsed on their own. Nothing else in
//  the file is ever turned into a value. Pure.
//

import Foundation

nonisolated enum JSONFieldScanner {
    /// The raw bytes of the top-level values named in `keys`, keyed by name.
    /// Nil when `data` isn't a JSON object (or is cut off before its end,
    /// e.g. caught mid-write). A key that appears twice keeps its last value,
    /// as JSON parsers do.
    static func values(in data: Data, keys: Set<String>) -> [String: Data]? {
        data.withUnsafeBytes { raw -> [String: Data]? in
            let bytes = raw.bindMemory(to: UInt8.self)
            var scanner = Scanner(bytes: bytes)
            return scanner.topLevel(keys: keys, data: data)
        }
    }

    /// The wanted top-level values, each parsed on its own (scalars too).
    static func objects(in data: Data, keys: Set<String>) -> [String: Any]? {
        guard let raw = values(in: data, keys: keys) else { return nil }
        var result: [String: Any] = [:]
        for (key, bytes) in raw {
            guard let value = try? JSONSerialization.jsonObject(with: bytes, options: [.fragmentsAllowed]) else { continue }
            result[key] = value
        }
        return result
    }

    private struct Scanner {
        let bytes: UnsafeBufferPointer<UInt8>
        var index = 0

        init(bytes: UnsafeBufferPointer<UInt8>) {
            self.bytes = bytes
        }

        private static let quote = UInt8(ascii: "\"")
        private static let backslash = UInt8(ascii: "\\")

        mutating func topLevel(keys: Set<String>, data: Data) -> [String: Data]? {
            skipByteOrderMark()
            skipSpace()
            guard take(UInt8(ascii: "{")) else { return nil }
            var result: [String: Data] = [:]
            skipSpace()
            if take(UInt8(ascii: "}")) { return result }
            while index < bytes.count {
                skipSpace()
                guard index < bytes.count, bytes[index] == Self.quote else { return nil }
                let keyStart = index
                guard skipString() else { return nil }
                let key = decodeKey(from: keyStart, to: index)
                skipSpace()
                guard take(UInt8(ascii: ":")) else { return nil }
                skipSpace()
                let valueStart = index
                guard skipValue() else { return nil }
                if let key, keys.contains(key) {
                    result[key] = data.subdata(in: data.startIndex + valueStart ..< data.startIndex + index)
                }
                skipSpace()
                if take(UInt8(ascii: ",")) { continue }
                if take(UInt8(ascii: "}")) { return result }
                return nil
            }
            return nil
        }

        private mutating func skipByteOrderMark() {
            if bytes.count >= 3, bytes[0] == 0xEF, bytes[1] == 0xBB, bytes[2] == 0xBF { index = 3 }
        }

        private mutating func skipSpace() {
            while index < bytes.count {
                switch bytes[index] {
                case 0x20, 0x09, 0x0A, 0x0D: index += 1
                default: return
                }
            }
        }

        private mutating func take(_ byte: UInt8) -> Bool {
            guard index < bytes.count, bytes[index] == byte else { return false }
            index += 1
            return true
        }

        /// Past a string starting at `index` (on its opening quote).
        private mutating func skipString() -> Bool {
            guard take(Self.quote) else { return false }
            while index < bytes.count {
                let byte = bytes[index]
                if byte == Self.backslash {
                    index += 2
                    continue
                }
                index += 1
                if byte == Self.quote { return true }
            }
            return false
        }

        /// Past one value of any kind, without building it.
        private mutating func skipValue() -> Bool {
            guard index < bytes.count else { return false }
            switch bytes[index] {
            case Self.quote:
                return skipString()
            case UInt8(ascii: "{"), UInt8(ascii: "["):
                var depth = 0
                while index < bytes.count {
                    let byte = bytes[index]
                    switch byte {
                    case Self.quote:
                        guard skipString() else { return false }
                        continue
                    case UInt8(ascii: "{"), UInt8(ascii: "["):
                        depth += 1
                    case UInt8(ascii: "}"), UInt8(ascii: "]"):
                        depth -= 1
                        if depth == 0 {
                            index += 1
                            return true
                        }
                    default:
                        break
                    }
                    index += 1
                }
                return false
            default:
                // A number, true, false or null: up to the next delimiter.
                let start = index
                while index < bytes.count {
                    switch bytes[index] {
                    case UInt8(ascii: ","), UInt8(ascii: "}"), UInt8(ascii: "]"), 0x20, 0x09, 0x0A, 0x0D:
                        return index > start
                    default:
                        index += 1
                    }
                }
                return index > start
            }
        }

        /// A key's text. Keys with escapes go through the JSON parser.
        private func decodeKey(from start: Int, to end: Int) -> String? {
            let inner = UnsafeBufferPointer(rebasing: bytes[(start + 1)..<(end - 1)])
            if !inner.contains(Self.backslash) {
                return String(decoding: inner, as: UTF8.self)
            }
            let quoted = Data(UnsafeBufferPointer(rebasing: bytes[start..<end]))
            return (try? JSONSerialization.jsonObject(with: quoted, options: [.fragmentsAllowed])) as? String
        }
    }
}
