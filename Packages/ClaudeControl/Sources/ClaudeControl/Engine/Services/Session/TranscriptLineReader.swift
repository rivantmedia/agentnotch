//
//  TranscriptLineReader.swift
//  ClaudeControl
//
//  Reads the complete lines appended to a JSONL transcript since an offset,
//  as raw bytes. Transcripts reach hundreds of MB, so the file is read in
//  fixed-size chunks and split on 0x0A without ever becoming one String:
//  Character-level `split` and bridged `contains` cost seconds per 40 MB,
//  byte splitting a fraction of that.
//

import Foundation

nonisolated enum TranscriptLineReader {
    /// Bytes read per chunk.
    static let chunkSize = 8 * 1024 * 1024

    struct Outcome: Equatable, Sendable {
        /// The file shrank (it was rewritten): reading restarted at 0.
        var didReset = false
        /// Complete lines handed to the body.
        var lineCount = 0
    }

    /// Calls `body` with every complete, non-empty line after `offset`, in
    /// order, and moves `offset` past the last newline, so a half-written
    /// final line is read again next time. Returns nil when the file can't
    /// be opened or read.
    static func forEachLine(
        path: String,
        from offset: inout UInt64,
        chunkSize: Int = chunkSize,
        _ body: (Data) -> Void
    ) -> Outcome? {
        guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
        defer { try? handle.close() }
        guard let size = try? handle.seekToEnd() else { return nil }

        var outcome = Outcome()
        if size < offset {
            offset = 0
            outcome.didReset = true
        }
        guard size > offset, (try? handle.seek(toOffset: offset)) != nil else { return outcome }

        var carry = Data()
        var position = offset
        while position < size {
            let wanted = Int(min(UInt64(chunkSize), size - position))
            guard let chunk = try? handle.read(upToCount: wanted), !chunk.isEmpty else { break }
            position += UInt64(chunk.count)

            var buffer = carry.isEmpty ? chunk : carry + chunk
            guard let lastNewline = buffer.lastIndex(of: newline) else {
                carry = buffer
                continue
            }
            let complete = buffer[buffer.startIndex...lastNewline]
            outcome.lineCount += forEachLine(in: complete, body)
            offset += UInt64(complete.count)
            buffer = buffer[buffer.index(after: lastNewline)...]
            carry = Data(buffer)
        }
        return outcome
    }

    /// Splits `data` (which ends with a newline) into non-empty lines.
    @discardableResult
    static func forEachLine(in data: Data, _ body: (Data) -> Void) -> Int {
        var count = 0
        var start = data.startIndex
        while start < data.endIndex {
            let end = data[start...].firstIndex(of: newline) ?? data.endIndex
            if end > start {
                body(data[start..<end])
                count += 1
            }
            start = end < data.endIndex ? data.index(after: end) : end
        }
        return count
    }

    /// Size of the file in bytes (a stat, no read); nil when it can't be read.
    static func size(of path: String) -> UInt64? {
        var info = stat()
        guard stat(path, &info) == 0 else { return nil }
        return UInt64(info.st_size)
    }

    /// Whether `line` contains `needle` (raw bytes, no decoding).
    static func contains(_ line: Data, _ needle: Data) -> Bool {
        line.range(of: needle) != nil
    }

    private static let newline = UInt8(ascii: "\n")
}
