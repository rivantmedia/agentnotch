//
//  ToolResultViews.swift
//  ClaudeControl
//
//  What each tool returned, as the chat shows it under the tool's line:
//  file excerpts with line numbers, diffs, command output, matches, todos,
//  subagent results, fetched pages and search hits, answers, MCP results.
//  Code sits in the theme's soft code box; added and removed lines take the
//  review and critical colours.
//

import SwiftUI

// MARK: - Dispatcher

struct ToolResultContent: View {
    let tool: ToolCallItem

    var body: some View {
        if let structured = tool.structuredResult {
            switch structured {
            case .read(let result):
                if !result.content.isEmpty {
                    FileCodeView(filename: result.filename, content: result.content,
                                 startLine: result.startLine, maxLines: 10)
                }
            case .edit(let result):
                EditResultContent(result: result, toolInput: tool.input)
            case .write(let result):
                WriteResultContent(result: result)
            case .bash(let result):
                BashResultContent(result: result)
            case .grep(let result):
                GrepResultContent(result: result)
            case .glob(let result):
                GlobResultContent(result: result)
            case .todoWrite(let result):
                TodoWriteResultContent(result: result)
            case .task(let result):
                TaskResultContent(result: result)
            case .webFetch(let result):
                WebFetchResultContent(result: result)
            case .webSearch(let result):
                WebSearchResultContent(result: result)
            case .askUserQuestion(let result):
                AskUserQuestionResultContent(result: result)
            case .bashOutput(let result):
                BashOutputResultContent(result: result)
            case .killShell(let result):
                ResultNote(text: result.message.isEmpty ? "Shell \(result.shellId) stopped" : result.message)
            case .exitPlanMode(let result):
                ExitPlanModeResultContent(result: result)
            case .mcp(let result):
                MCPResultContent(result: result)
            case .generic(let result):
                if let content = result.rawContent, !content.isEmpty {
                    CodePreview(content: content, maxLines: 15)
                } else {
                    ResultNote(text: "Completed")
                }
            }
        } else if tool.name == "Edit" {
            EditInputDiffView(input: tool.input)
        } else if let result = tool.result {
            CodePreview(content: result, maxLines: 15)
        }
    }
}

// MARK: - Edits

/// An Edit's diff from its input, before (or without) a structured result.
struct EditInputDiffView: View {
    let input: [String: String]

    var body: some View {
        let old = input["old_string"] ?? "", new = input["new_string"] ?? ""
        if !old.isEmpty || !new.isEmpty {
            SimpleDiffView(oldString: old, newString: new,
                           filename: input["file_path"].map { URL(fileURLWithPath: $0).lastPathComponent } ?? "file")
        }
    }
}

private struct EditResultContent: View {
    let result: EditResult
    let toolInput: [String: String]

    var body: some View {
        let old = result.oldString.isEmpty ? toolInput["old_string"] ?? "" : result.oldString
        let new = result.newString.isEmpty ? toolInput["new_string"] ?? "" : result.newString
        VStack(alignment: .leading, spacing: 4) {
            if !old.isEmpty || !new.isEmpty {
                SimpleDiffView(oldString: old, newString: new, filename: result.filename)
            }
            if result.userModified {
                ResultNote(text: "You changed it before it was applied")
            }
        }
    }
}

private struct WriteResultContent: View {
    let result: WriteResult

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ResultNote(text: "\(result.type == .create ? "Created" : "Wrote") \(result.filename)")
            if result.type == .create && !result.content.isEmpty {
                CodePreview(content: result.content, maxLines: 8)
            } else if let patches = result.structuredPatch, !patches.isEmpty {
                DiffView(patches: patches)
            }
        }
    }
}

// MARK: - Commands

private struct BashResultContent: View {
    let result: BashResult

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let id = result.backgroundTaskId {
                ResultNote(text: "Running in the background (\(id))")
            }
            if let interpretation = result.returnCodeInterpretation {
                ResultNote(text: interpretation)
            }
            if !result.stdout.isEmpty {
                CodePreview(content: result.stdout, maxLines: 15)
            }
            if !result.stderr.isEmpty {
                CodePreview(content: result.stderr, maxLines: 10, ink: .critical)
            }
            if !result.hasOutput && result.backgroundTaskId == nil && result.returnCodeInterpretation == nil {
                ResultNote(text: "No output")
            }
        }
    }
}

private struct BashOutputResultContent: View {
    let result: BashOutputResult

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                ResultNote(text: result.status.capitalized)
                if let exitCode = result.exitCode {
                    Text("exit \(exitCode)")
                        .claudeFont(.monoCaption)
                        .foregroundStyle(.ink(exitCode == 0 ? .secondary : .critical))
                }
            }
            if !result.stdout.isEmpty {
                CodePreview(content: result.stdout, maxLines: 10)
            }
            if !result.stderr.isEmpty {
                CodePreview(content: result.stderr, maxLines: 5, ink: .critical)
            }
        }
    }
}

// MARK: - Searches

private struct GrepResultContent: View {
    let result: GrepResult

    var body: some View {
        switch result.mode {
        case .filesWithMatches:
            if result.filenames.isEmpty {
                ResultNote(text: "No matches")
            } else {
                FileListView(files: result.filenames, limit: 10)
            }
        case .content:
            if let content = result.content, !content.isEmpty {
                CodePreview(content: content, maxLines: 15)
            } else {
                ResultNote(text: "No matches")
            }
        case .count:
            ResultNote(text: "\(result.numFiles) file\(result.numFiles == 1 ? "" : "s") with matches")
        }
    }
}

private struct GlobResultContent: View {
    let result: GlobResult

    var body: some View {
        if result.filenames.isEmpty {
            ResultNote(text: "No files")
        } else {
            VStack(alignment: .leading, spacing: 3) {
                FileListView(files: result.filenames, limit: 10)
                if result.truncated {
                    ResultNote(text: "More were found than are listed")
                }
            }
        }
    }
}

private struct WebFetchResultContent: View {
    let result: WebFetchResult

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Text("\(result.code)")
                    .claudeFont(.monoCaption)
                    .foregroundStyle(.ink(result.code < 400 ? .secondary : .critical))
                Text(result.url)
                    .claudeFont(.monoCaption)
                    .foregroundStyle(.ink(.secondary))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            if !result.result.isEmpty {
                ResultText(text: result.result, lineLimit: 8)
            }
        }
    }
}

private struct WebSearchResultContent: View {
    let result: WebSearchResult

    var body: some View {
        if result.results.isEmpty {
            ResultNote(text: "No results")
        } else {
            VStack(alignment: .leading, spacing: 5) {
                ForEach(Array(result.results.prefix(5).enumerated()), id: \.offset) { _, item in
                    VStack(alignment: .leading, spacing: 1) {
                        Text(item.title)
                            .claudeFont(.body, weight: .medium)
                            .foregroundStyle(.ink(.primary))
                            .lineLimit(1)
                        if !item.snippet.isEmpty {
                            ResultText(text: item.snippet, lineLimit: 2)
                        }
                    }
                }
                if result.results.count > 5 {
                    ResultNote(text: "and \(result.results.count - 5) more")
                }
            }
        }
    }
}

// MARK: - Tasks and agents

private struct TodoWriteResultContent: View {
    let result: TodoWriteResult

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(Array(result.newTodos.enumerated()), id: \.offset) { _, todo in
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(todo.status == "completed" ? "✓" : todo.status == "in_progress" ? "›" : "·")
                        .claudeFont(.caption, weight: .bold)
                        .foregroundStyle(.ink(todo.status == "pending" ? .tertiary : .primary))
                        .frame(width: 9)
                    Text(todo.content)
                        .claudeFont(.body)
                        .foregroundStyle(.ink(todo.status == "pending" ? .secondary : .primary))
                        .strikethrough(todo.status == "completed")
                        .opacity(todo.status == "completed" ? 0.55 : 1)
                        .lineLimit(2)
                }
            }
        }
    }
}

private struct TaskResultContent: View {
    let result: TaskResult

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text(result.status.capitalized)
                    .claudeFont(.caption, weight: .semibold)
                    .foregroundStyle(.ink(["failed", "error"].contains(result.status) ? .critical : .primary))
                if let duration = result.totalDurationMs {
                    ResultNote(text: Self.duration(duration))
                }
                if let tools = result.totalToolUseCount {
                    ResultNote(text: "\(tools) tool\(tools == 1 ? "" : "s")")
                }
            }
            if !result.content.isEmpty {
                ResultText(text: result.content, lineLimit: 5)
            }
        }
    }

    static func duration(_ milliseconds: Int) -> String {
        if milliseconds >= 60_000 { return "\(milliseconds / 60_000)m \((milliseconds % 60_000) / 1000)s" }
        if milliseconds >= 1000 { return "\(milliseconds / 1000)s" }
        return "\(milliseconds)ms"
    }
}

private struct AskUserQuestionResultContent: View {
    let result: AskUserQuestionResult

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(Array(result.questions.enumerated()), id: \.offset) { index, question in
                VStack(alignment: .leading, spacing: 1) {
                    ResultText(text: question.question, lineLimit: 3)
                    if let answer = result.answers["\(index)"] {
                        Text("↳ \(answer)")
                            .claudeFont(.body, weight: .medium)
                            .foregroundStyle(.ink(.primary))
                    }
                }
            }
        }
    }
}

private struct ExitPlanModeResultContent: View {
    let result: ExitPlanModeResult

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let path = result.filePath {
                ResultNote(text: URL(fileURLWithPath: path).lastPathComponent)
            }
            if let plan = result.plan, !plan.isEmpty {
                ResultText(text: plan, lineLimit: 6)
            }
        }
    }
}

private struct MCPResultContent: View {
    let result: MCPResult

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ResultNote(text: "\(MCPToolFormatter.toTitleCase(result.serverName)) · \(MCPToolFormatter.toTitleCase(result.toolName))")
            ForEach(Array(result.rawResult.prefix(5)), id: \.key) { key, value in
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text("\(key)")
                        .claudeFont(.monoCaption)
                        .foregroundStyle(.ink(.secondary))
                    Text(String(String(describing: value).prefix(100)))
                        .claudeFont(.monoCaption)
                        .foregroundStyle(.ink(.primary))
                        .lineLimit(2)
                }
            }
        }
    }
}

// MARK: - Building blocks

/// A quiet one-line fact about a result.
private struct ResultNote: View {
    let text: String

    var body: some View {
        Text(text)
            .claudeFont(.caption)
            .foregroundStyle(.ink(.secondary))
            .lineLimit(2)
    }
}

/// Prose from a result, cut to a few lines.
private struct ResultText: View {
    let text: String
    let lineLimit: Int

    var body: some View {
        Text(text)
            .claudeFont(.body)
            .foregroundStyle(.ink(.secondary))
            .lineLimit(lineLimit)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// The soft box code sits in.
private struct CodeBox<Content: View>: View {
    var header: String? = nil
    @ViewBuilder let content: Content

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let header {
                Text(header)
                    .claudeFont(.monoCaption)
                    .foregroundStyle(.ink(.primary))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .padding(.horizontal, 8)
                    .padding(.top, 6)
                    .padding(.bottom, 4)
                Rectangle().fill(.ink(.separator)).frame(height: theme.hairline)
            }
            content
                .padding(.vertical, 5)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(theme.controlFill))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

/// Lines of a file with their numbers.
struct FileCodeView: View {
    let filename: String
    let content: String
    let startLine: Int
    let maxLines: Int

    var body: some View {
        let lines = content.components(separatedBy: "\n")
        CodeBox(header: filename) {
            VStack(alignment: .leading, spacing: 1) {
                if startLine > 1 { overflow("…") }
                ForEach(Array(lines.prefix(maxLines).enumerated()), id: \.offset) { index, line in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("\(startLine + index)")
                            .foregroundStyle(.ink(.tertiary))
                            .frame(width: 26, alignment: .trailing)
                        Text(line.isEmpty ? " " : line)
                            .foregroundStyle(.ink(.primary))
                            .lineLimit(1)
                    }
                    .claudeFont(.monoCaption)
                }
                if lines.count > maxLines { overflow("\(lines.count - maxLines) more lines") }
            }
            .padding(.trailing, 6)
        }
    }

    private func overflow(_ text: String) -> some View {
        Text(text)
            .claudeFont(.monoCaption)
            .foregroundStyle(.ink(.secondary))
            .padding(.leading, 34)
    }
}

/// Plain output, cut to `maxLines`.
struct CodePreview: View {
    let content: String
    let maxLines: Int
    var ink: ClaudeInk.Token = .primary

    var body: some View {
        let lines = content.components(separatedBy: "\n")
        CodeBox {
            VStack(alignment: .leading, spacing: 1) {
                ForEach(Array(lines.prefix(maxLines).enumerated()), id: \.offset) { _, line in
                    Text(line.isEmpty ? " " : line)
                        .claudeFont(.monoCaption)
                        .foregroundStyle(.ink(ink))
                        .lineLimit(1)
                }
                if lines.count > maxLines {
                    Text("\(lines.count - maxLines) more lines")
                        .claudeFont(.monoCaption)
                        .foregroundStyle(.ink(.secondary))
                }
            }
            .padding(.horizontal, 8)
        }
    }
}

/// File names, one per line.
struct FileListView: View {
    let files: [String]
    let limit: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(Array(files.prefix(limit).enumerated()), id: \.offset) { _, file in
                Text(URL(fileURLWithPath: file).lastPathComponent)
                    .claudeFont(.monoCaption)
                    .foregroundStyle(.ink(.primary))
                    .lineLimit(1)
                    .help(file)
            }
            if files.count > limit {
                ResultNote(text: "and \(files.count - limit) more")
            }
        }
    }
}

/// Unified-diff hunks from a Write.
struct DiffView: View {
    let patches: [PatchHunk]

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(Array(patches.prefix(3).enumerated()), id: \.offset) { _, patch in
                CodeBox(header: "Lines \(patch.newStart)–\(patch.newStart + max(patch.newLines - 1, 0))") {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(patch.lines.prefix(10).enumerated()), id: \.offset) { _, line in
                            DiffLineRow(text: String(line.dropFirst()), kind: DiffLineKind(prefixOf: line))
                        }
                        if patch.lines.count > 10 {
                            Text("\(patch.lines.count - 10) more lines")
                                .claudeFont(.monoCaption)
                                .foregroundStyle(.ink(.secondary))
                                .padding(.leading, 20)
                        }
                    }
                }
            }
            if patches.count > 3 {
                ResultNote(text: "and \(patches.count - 3) more changes")
            }
        }
    }
}

/// What an Edit replaced with what, line by line.
struct SimpleDiffView: View {
    let oldString: String
    let newString: String
    var filename: String? = nil

    /// Changed lines shown before "…".
    static let maxLines = 12

    var body: some View {
        let diff = LineDiff.changes(old: oldString, new: newString, limit: Self.maxLines)
        CodeBox(header: filename) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(diff.lines.enumerated()), id: \.offset) { _, line in
                    DiffLineRow(text: line.text, kind: line.kind, number: line.number)
                }
                if diff.isTruncated {
                    Text("…")
                        .claudeFont(.monoCaption)
                        .foregroundStyle(.ink(.secondary))
                        .padding(.leading, 20)
                }
            }
        }
    }
}

nonisolated enum DiffLineKind: Equatable, Sendable {
    case added, removed, context

    init(prefixOf line: String) {
        if line.hasPrefix("+") { self = .added } else if line.hasPrefix("-") { self = .removed } else { self = .context }
    }
}

private struct DiffLineRow: View {
    let text: String
    let kind: DiffLineKind
    var number: Int? = nil

    @Environment(\.claudeControlTheme) private var theme

    private var ink: ClaudeInk.Token {
        switch kind {
        case .added: return .review
        case .removed: return .critical
        case .context: return .secondary
        }
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            if let number {
                Text("\(number)")
                    .foregroundStyle(.ink(.tertiary))
                    .frame(width: 22, alignment: .trailing)
            }
            Text(kind == .added ? "+" : kind == .removed ? "−" : " ")
                .foregroundStyle(.ink(ink))
                .frame(width: 10)
            Text(text.isEmpty ? " " : text)
                .foregroundStyle(.ink(kind == .context ? .secondary : .primary))
                .lineLimit(1)
        }
        .claudeFont(.monoCaption)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 6)
        .padding(.vertical, 1)
        .background(kind == .context ? Color.clear : theme.color(ink).opacity(0.12))
    }
}

/// The changed lines between two texts (a longest-common-subsequence diff).
///
/// Drawn in a view body, so its cost is bounded: the lines both texts start
/// and end with are set aside first (an edit usually touches a few lines of
/// a long block), and when what remains is still too big for the table, its
/// old lines are listed as removed and the new ones as added.
nonisolated enum LineDiff {
    struct Line: Equatable, Sendable {
        let text: String
        let kind: DiffLineKind
        /// In the old text for removals, the new text for additions.
        let number: Int
    }

    struct Result: Equatable, Sendable {
        let lines: [Line]
        let isTruncated: Bool
    }

    /// The most cells the LCS table may have (old lines × new lines, after
    /// the shared start and end are set aside): about 2 MB.
    static let maxTableCells = 250_000

    static func changes(old: String, new: String, limit: Int) -> Result {
        let a = old.components(separatedBy: "\n"), b = new.components(separatedBy: "\n")
        var start = 0
        while start < a.count && start < b.count && a[start] == b[start] { start += 1 }
        var end = 0
        while end < a.count - start && end < b.count - start && a[a.count - 1 - end] == b[b.count - 1 - end] { end += 1 }
        let oldMiddle = Array(a[start..<(a.count - end)]), newMiddle = Array(b[start..<(b.count - end)])
        let common = oldMiddle.count * newMiddle.count <= maxTableCells ? lcs(oldMiddle, newMiddle) : []

        var lines: [Line] = []
        var i = 0, j = 0, k = 0
        var truncated = false
        while i < oldMiddle.count || j < newMiddle.count {
            let shared = k < common.count ? common[k] : nil
            if i < oldMiddle.count && (shared == nil || oldMiddle[i] != shared) {
                lines.append(Line(text: oldMiddle[i], kind: .removed, number: start + i + 1))
                i += 1
            } else if j < newMiddle.count && (shared == nil || newMiddle[j] != shared) {
                lines.append(Line(text: newMiddle[j], kind: .added, number: start + j + 1))
                j += 1
            } else {
                i += 1; j += 1; k += 1
            }
            if lines.count > limit {
                truncated = true
                lines.removeLast()
                break
            }
        }
        return Result(lines: lines, isTruncated: truncated)
    }

    private static func lcs(_ a: [String], _ b: [String]) -> [String] {
        guard !a.isEmpty, !b.isEmpty else { return [] }
        var table = Array(repeating: Array(repeating: 0, count: b.count + 1), count: a.count + 1)
        for i in 1...a.count {
            for j in 1...b.count {
                table[i][j] = a[i - 1] == b[j - 1] ? table[i - 1][j - 1] + 1 : max(table[i - 1][j], table[i][j - 1])
            }
        }
        var result: [String] = []
        var i = a.count, j = b.count
        while i > 0 && j > 0 {
            if a[i - 1] == b[j - 1] {
                result.append(a[i - 1]); i -= 1; j -= 1
            } else if table[i - 1][j] > table[i][j - 1] {
                i -= 1
            } else {
                j -= 1
            }
        }
        return result.reversed()
    }
}
