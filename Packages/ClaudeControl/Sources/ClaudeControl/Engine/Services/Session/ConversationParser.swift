//
//  ConversationParser.swift
//  ClaudeControl
//
//  Reads Claude Code's JSONL transcripts. One pass per appended chunk feeds
//  everything the app derives from a transcript: the conversation summary
//  (title, last message, usage, how the last turn ended), the task list, the
//  chat messages and tool results, and the subagent tool lists of Agent
//  calls. Lines are split as bytes (TranscriptLineReader) and decoded once.
//
//  Memory: per session the parser keeps only offsets, the summary, the task
//  list, the tool calls still in flight and the agents still running. Chat
//  messages are handed to the store and not kept here; for a session whose
//  chat isn't open only the newest few are handed over at all. The full
//  history is read again, once, when a chat opens (`fullHistory`).
//

import Foundation
import os.log

/// Token usage of a session, summed over its main-thread API responses.
///
/// Each API response is counted once: Claude Code writes one transcript line
/// per content block, all repeating the same `message.usage`, so lines are
/// de-duplicated by `message.id` (+ `requestId`). Synthetic messages (local
/// errors, model "<synthetic>") and sidechain lines are skipped.
nonisolated struct UsageInfo: Equatable, Sendable {
    var inputTokens: Int = 0
    var outputTokens: Int = 0
    var cacheReadTokens: Int = 0
    var cacheCreationTokens: Int = 0

    /// Uncached input plus output tokens (cache reads/writes excluded).
    var totalTokens: Int {
        inputTokens + outputTokens
    }

    /// Prompt-cache reads plus cache writes.
    var cacheTokens: Int {
        cacheReadTokens + cacheCreationTokens
    }

    /// Everything processed, including cache reads and writes.
    var totalTokensIncludingCache: Int {
        totalTokens + cacheTokens
    }

    /// `totalTokens` (input + output, no cache) formatted for display, e.g. "12.5K".
    var formattedTotal: String {
        Self.format(totalTokens)
    }

    static func format(_ total: Int) -> String {
        if total >= 1_000_000 {
            return String(format: "%.1fM", Double(total) / 1_000_000)
        } else if total >= 1_000 {
            return String(format: "%.1fK", Double(total) / 1_000)
        }
        return "\(total)"
    }
}

/// How the transcript's latest turn ended, from main-chain entries only.
nonisolated struct TranscriptTurn: Equatable, Sendable {
    /// Text of Claude's last reply that had text, and when it was written.
    var replyText: String?
    var replyAt: Date?
    /// When a person last typed a prompt (not a task notification, compact
    /// summary, tool result, command echo or meta entry).
    var humanPromptAt: Date?
    /// When the user last interrupted Claude ("[Request interrupted by user").
    var interruptedAt: Date?
    /// Nothing came after the last reply: no tool call, tool result, prompt,
    /// wake-up or interrupt. A turn that ended this way finished.
    var endsWithReply = false

    /// Whether the last reply is a finished turn later than `date` (nil
    /// meaning no bound) and than anything the user did after it.
    func finishedTurn(after date: Date?) -> (text: String?, at: Date)? {
        guard endsWithReply, let replyAt else { return nil }
        let bounds = [date, humanPromptAt, interruptedAt].compactMap { $0 }
        guard bounds.allSatisfy({ replyAt > $0 }) else { return nil }
        return (replyText, replyAt)
    }
}

nonisolated struct ConversationInfo: Equatable, Sendable {
    let summary: String?
    let lastMessage: String?
    let lastMessageRole: String?  // "user", "assistant", or "tool"
    let lastToolName: String?  // Tool name if lastMessageRole is "tool"
    let firstUserMessage: String?  // Fallback title when no summary
    let lastUserMessageDate: Date?  // When a person last typed a prompt
    var usage: UsageInfo = UsageInfo()  // Token usage stats
    /// `custom-title` (set with /rename) else `ai-title` from the transcript.
    var title: String? = nil
    /// Context size of the latest main-thread response: input + cache creation + cache read tokens.
    var lastContextTokens: Int? = nil
    /// Model id of the latest main-thread response, e.g. "claude-opus-4-5".
    var lastModel: String? = nil
    /// How the latest turn ended.
    var lastTurn = TranscriptTurn()
}

/// Forward accumulator over transcript lines that produces `ConversationInfo`.
/// Pure and incremental: feeding lines in order gives the same result whether
/// they arrive in one batch or many.
nonisolated struct TranscriptSummary: Sendable {
    private(set) var summary: String?
    private(set) var aiTitle: String?
    private(set) var customTitle: String?
    private(set) var lastMessage: String?
    private(set) var lastMessageRole: String?
    private(set) var lastToolName: String?
    private(set) var firstUserMessage: String?
    private(set) var lastUserMessageDate: Date?
    private(set) var lastContextTokens: Int?
    private(set) var lastModel: String?
    private(set) var turn = TranscriptTurn()
    /// Usage per API response key, so a repeated response replaces rather than adds.
    /// Claude Code repeats a response's usage on the lines of its content
    /// blocks, which are written together, so only the latest responses are
    /// remembered (a long session doesn't keep one entry per response).
    private var usageByResponse: [String: UsageInfo] = [:]
    private var responseOrder: [String] = []
    static let rememberedResponses = 256
    private(set) var usage = UsageInfo()

    init() {}

    var info: ConversationInfo {
        ConversationInfo(
            summary: summary,
            lastMessage: Self.truncate(lastMessage, maxLength: 80),
            lastMessageRole: lastMessageRole,
            lastToolName: lastToolName,
            firstUserMessage: firstUserMessage,
            lastUserMessageDate: lastUserMessageDate,
            usage: usage,
            title: customTitle ?? aiTitle,
            lastContextTokens: lastContextTokens,
            lastModel: lastModel,
            lastTurn: turn
        )
    }

    mutating func consume(_ json: [String: Any], dateParser: (String) -> Date?) {
        let type = json["type"] as? String
        let isSidechain = json["isSidechain"] as? Bool ?? false
        let isMeta = json["isMeta"] as? Bool ?? false

        switch type {
        case "summary":
            if let text = json["summary"] as? String, !text.isEmpty { summary = text }
            return
        case "ai-title":
            if let text = json["aiTitle"] as? String, !text.isEmpty { aiTitle = text }
            return
        case "custom-title":
            if let text = json["customTitle"] as? String, !text.isEmpty { customTitle = text }
            return
        case "user", "assistant":
            break
        default:
            return
        }
        guard !isSidechain, let message = json["message"] as? [String: Any] else { return }

        if type == "assistant" {
            consumeUsage(json: json, message: message)
        }
        guard !isMeta else { return }
        let date = (json["timestamp"] as? String).flatMap(dateParser)

        if type == "user" {
            consumeUser(json: json, message: message, date: date)
        } else {
            consumeAssistant(message: message, date: date)
        }
    }

    private mutating func consumeUser(json: [String: Any], message: [String: Any], date: Date?) {
        let isHuman = Self.isHumanPrompt(json)
        if let text = message["content"] as? String {
            if Self.isInterruptMarker(text) {
                markInterrupt(date)
                return
            }
            guard isHuman else {
                // A wake-up (task notification), compact summary or other
                // injected entry: Claude works on, and it isn't anyone's words.
                turn.endsWithReply = false
                return
            }
            guard !Self.isCommandText(text) else { return }
            consumeHumanText(text, date: date)
        } else if let blocks = message["content"] as? [[String: Any]] {
            var sawText = false
            for block in blocks {
                switch block["type"] as? String {
                case "tool_result":
                    // Claude asked for a tool; the turn goes on.
                    turn.endsWithReply = false
                case "text":
                    guard let text = block["text"] as? String else { continue }
                    if Self.isInterruptMarker(text) {
                        markInterrupt(date)
                    } else if isHuman, !sawText, !Self.isCommandText(text) {
                        sawText = true
                        consumeHumanText(text, date: date)
                    } else if !isHuman {
                        turn.endsWithReply = false
                    }
                default:
                    continue
                }
            }
        }
    }

    private mutating func consumeHumanText(_ text: String, date: Date?) {
        lastMessage = text
        lastMessageRole = "user"
        lastToolName = nil
        if firstUserMessage == nil {
            firstUserMessage = Self.truncate(text, maxLength: 50)
        }
        if let date {
            lastUserMessageDate = date
            turn.humanPromptAt = date
        }
        turn.endsWithReply = false
    }

    private mutating func markInterrupt(_ date: Date?) {
        turn.interruptedAt = date ?? turn.interruptedAt
        turn.endsWithReply = false
    }

    private mutating func consumeAssistant(message: [String: Any], date: Date?) {
        if let text = message["content"] as? String {
            guard !Self.isCommandText(text) else { return }
            lastMessage = text
            lastMessageRole = "assistant"
            lastToolName = nil
            recordReply(text, date: date)
            return
        }
        guard let blocks = message["content"] as? [[String: Any]] else { return }
        for block in blocks.reversed() {
            let blockType = block["type"] as? String
            if blockType == "tool_use" {
                let toolName = block["name"] as? String ?? "Tool"
                lastMessage = ConversationParser.formatToolInput(block["input"] as? [String: Any], toolName: toolName)
                lastMessageRole = "tool"
                lastToolName = toolName
                turn.endsWithReply = false
                return
            } else if blockType == "text", let text = block["text"] as? String {
                if Self.isInterruptMarker(text) {
                    markInterrupt(date)
                    return
                }
                lastMessage = text
                lastMessageRole = "assistant"
                lastToolName = nil
                recordReply(text, date: date)
                return
            }
        }
    }

    private mutating func recordReply(_ text: String, date: Date?) {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        turn.replyText = text
        turn.replyAt = date ?? turn.replyAt
        turn.endsWithReply = true
    }

    private mutating func consumeUsage(json: [String: Any], message: [String: Any]) {
        guard let usageDict = message["usage"] as? [String: Any] else { return }
        let model = message["model"] as? String
        if model == "<synthetic>" { return }

        let entry = UsageInfo(
            inputTokens: JSONValue.int(usageDict["input_tokens"]) ?? 0,
            outputTokens: JSONValue.int(usageDict["output_tokens"]) ?? 0,
            cacheReadTokens: JSONValue.int(usageDict["cache_read_input_tokens"]) ?? 0,
            cacheCreationTokens: JSONValue.int(usageDict["cache_creation_input_tokens"]) ?? 0
        )
        let messageId = message["id"] as? String
        let requestId = json["requestId"] as? String
        let key = messageId.map { "\($0)|\(requestId ?? "")" } ?? (json["uuid"] as? String ?? UUID().uuidString)

        if let previous = usageByResponse[key] {
            usage.inputTokens -= previous.inputTokens
            usage.outputTokens -= previous.outputTokens
            usage.cacheReadTokens -= previous.cacheReadTokens
            usage.cacheCreationTokens -= previous.cacheCreationTokens
        } else {
            responseOrder.append(key)
            if responseOrder.count > Self.rememberedResponses * 2 {
                // Amortized: forget the oldest in batches.
                let forgotten = responseOrder.prefix(responseOrder.count - Self.rememberedResponses)
                for old in forgotten {
                    usageByResponse.removeValue(forKey: old)
                }
                responseOrder.removeFirst(forgotten.count)
            }
        }
        usageByResponse[key] = entry
        usage.inputTokens += entry.inputTokens
        usage.outputTokens += entry.outputTokens
        usage.cacheReadTokens += entry.cacheReadTokens
        usage.cacheCreationTokens += entry.cacheCreationTokens

        lastContextTokens = entry.inputTokens + entry.cacheCreationTokens + entry.cacheReadTokens
        if let model, !model.isEmpty { lastModel = model }
    }

    /// Claude Code's own test for words a person typed (`W6e`): a user entry
    /// that isn't meta, carries no tool result, isn't a compact summary, and
    /// has no origin or a human one. Task notifications (`origin.kind ==
    /// "task-notification"`) and auto-continuations are not. Transcripts
    /// from before `origin` existed are checked by their text instead.
    static func isHumanPrompt(_ json: [String: Any]) -> Bool {
        guard json["type"] as? String == "user",
              json["isMeta"] as? Bool != true,
              json["toolUseResult"] == nil,
              json["isCompactSummary"] as? Bool != true else { return false }
        if let origin = json["origin"] as? [String: Any], origin["kind"] as? String != "human" {
            return false
        }
        let message = json["message"] as? [String: Any]
        let text = (message?["content"] as? String)
            ?? (message?["content"] as? [[String: Any]])?.first { $0["type"] as? String == "text" }?["text"] as? String
        return !ToolInput.isInjectedPrompt(text)
    }

    /// Slash-command echoes and caveats aren't conversation text.
    static func isCommandText(_ text: String) -> Bool {
        text.hasPrefix("<command-name>") || text.hasPrefix("<local-command") || text.hasPrefix("Caveat:")
    }

    static func isInterruptMarker(_ text: String) -> Bool {
        text.hasPrefix("[Request interrupted by user")
    }

    static func truncate(_ message: String?, maxLength: Int) -> String? {
        guard let msg = message else { return nil }
        let cleaned = msg.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "\n", with: " ")
        if cleaned.count > maxLength {
            return String(cleaned.prefix(maxLength - 3)) + "..."
        }
        return cleaned
    }
}

/// Context window usage estimate for sessions without status line data.
nonisolated enum ContextUsageEstimator {
    static let standardWindow = 200_000
    static let extendedWindow = 1_000_000

    /// Window size: the status line's when known, else 1M for "[1m]" models or
    /// when the context already exceeds 200k, else 200k.
    static func windowSize(contextTokens: Int, statusLineWindowSize: Int?, modelIds: [String?]) -> Int {
        if let statusLineWindowSize, statusLineWindowSize > 0 { return statusLineWindowSize }
        let isExtended = modelIds.contains { $0?.lowercased().contains("[1m]") == true }
        if isExtended || contextTokens > standardWindow { return extendedWindow }
        return standardWindow
    }

    /// Percent 0...100 of the window used by `contextTokens`.
    static func percent(contextTokens: Int, statusLineWindowSize: Int?, modelIds: [String?]) -> Double {
        let window = windowSize(contextTokens: contextTokens, statusLineWindowSize: statusLineWindowSize, modelIds: modelIds)
        return min(max(Double(contextTokens) / Double(window) * 100, 0), 100)
    }
}

actor ConversationParser {
    static let shared = ConversationParser()

    /// Logger for conversation parser (nonisolated static for cross-context access)
    nonisolated static var logger: Logger { EngineLog.logger("Parser") }

    /// Chat messages handed over per sync for a session whose chat isn't open.
    static let retainedMessageCount = 30
    /// Agents followed per session (the newest; older ones are settled).
    static let maxTrackedAgents = 64
    /// A running agent whose transcript hasn't changed for this long is settled.
    static let agentIdleSettleInterval: TimeInterval = 10 * 60

    /// Parsed tool result data
    struct ToolResult: Sendable {
        let content: String?
        let stdout: String?
        let stderr: String?
        let isError: Bool
        let isInterrupted: Bool

        init(content: String?, stdout: String?, stderr: String?, isError: Bool) {
            self.content = content
            self.stdout = stdout
            self.stderr = stderr
            self.isError = isError
            // Detect if this was an interrupt or rejection (various formats)
            self.isInterrupted = isError && (
                content?.contains("Interrupted by user") == true ||
                content?.contains("interrupted by user") == true ||
                content?.contains("user doesn't want to proceed") == true
            )
        }
    }

    /// What the store asks for when it syncs a session.
    struct SyncRequest: Sendable {
        let sessionId: String
        let transcriptPath: String
        /// The chat is open: hand over every new message, not only the newest.
        var keepsFullHistory = false
        /// Tools the store shows as running or waiting: their results are
        /// wanted even when their tool_use line is long gone.
        var wantedToolIds: Set<String> = []
    }

    /// Everything one sync learned. Only what changed since the last sync,
    /// apart from `conversationInfo` and `transcriptTasks` (whole transcript).
    struct SyncResult: Sendable {
        /// New bytes were read (or the file was rewritten and read again).
        var advanced = false
        var didReset = false
        /// A /clear line appeared after the first read.
        var clearDetected = false
        var newMessages: [ChatMessage] = []
        var completedToolIds: Set<String> = []
        var toolResults: [String: ToolResult] = [:]
        var structuredResults: [String: ToolResultData] = [:]
        var conversationInfo = TranscriptSummary().info
        var transcriptTasks = SessionTaskList()
        /// Subagent tool lists that changed, keyed by the Agent call's tool_use_id.
        var subagentTools: [String: [SubagentToolInfo]] = [:]

        /// Nothing for the store to apply.
        var isEmpty: Bool {
            !advanced && !didReset && !clearDetected && subagentTools.isEmpty
        }
    }

    /// A whole transcript, for a chat being opened.
    struct HistoryResult: Sendable {
        var messages: [ChatMessage] = []
        var completedToolIds: Set<String> = []
        var toolResults: [String: ToolResult] = [:]
        var structuredResults: [String: ToolResultData] = [:]
        var conversationInfo = TranscriptSummary().info
        var subagentTools: [String: [SubagentToolInfo]] = [:]
    }

    /// Work counters, for tests and the debug dump.
    struct Statistics: Equatable, Sendable {
        var syncs = 0
        var linesDecoded = 0
        var subagentFileReads = 0
    }

    private(set) var statistics = Statistics()

    /// Per-session transcript state, keyed by session id.
    private var sessionStates: [String: SessionParseState] = [:]


    // MARK: - Sync

    /// Reads what was appended to the session's transcript since the last
    /// sync and brings the session's running agents up to date. Nil when the
    /// transcript can't be read.
    func sync(_ request: SyncRequest) -> SyncResult? {
        statistics.syncs += 1
        var state = sessionState(for: request.sessionId, filePath: request.transcriptPath)
        var didReset = false
        if let size = TranscriptLineReader.size(of: request.transcriptPath), size < state.offset {
            // The file was rewritten: read it again from the top.
            Self.logger.debug("Transcript of \(request.sessionId.prefix(8), privacy: .public) shrank; reading it again")
            state = SessionParseState(filePath: request.transcriptPath)
            didReset = true
        }
        let wasRead = state.offset > 0
        var chunk = ChunkParser(
            keepsFullHistory: request.keepsFullHistory,
            retainedMessageCount: Self.retainedMessageCount,
            wantedToolIds: request.wantedToolIds
        )
        var offset = state.offset
        let outcome = TranscriptLineReader.forEachLine(path: request.transcriptPath, from: &offset) { line in
            chunk.consume(line, state: &state)
        }
        guard let outcome else {
            sessionStates[request.sessionId] = nil
            return nil
        }
        statistics.linesDecoded += chunk.linesDecoded

        var result = SyncResult()
        result.advanced = offset != state.offset || outcome.lineCount > 0
        result.didReset = didReset || outcome.didReset
        result.clearDetected = chunk.sawClear && wasRead && !result.didReset
        chunk.finish()
        result.newMessages = chunk.messages
        result.completedToolIds = chunk.completedToolIds
        result.toolResults = chunk.toolResults
        result.structuredResults = chunk.structuredResults
        state.offset = offset

        // Agents: start following the ones whose results just arrived, then
        // read what every followed agent wrote since last time.
        for (toolUseId, structured) in chunk.structuredResults {
            guard case .task(let task) = structured, !task.agentId.isEmpty,
                  state.agents[toolUseId] == nil, !state.settledAgents.contains(toolUseId) else { continue }
            state.agents[toolUseId] = AgentTrack(agentId: task.agentId, isFinished: task.status == "completed", addedAt: Date())
        }
        result.subagentTools = refreshAgents(in: &state, transcriptPath: request.transcriptPath)

        result.conversationInfo = state.summary.info
        result.transcriptTasks = state.tasks
        sessionStates[request.sessionId] = state
        return result
    }

    /// The whole transcript, for a chat being opened: every message, every
    /// tool result and every agent's tool list. Agents still running are
    /// followed by later syncs.
    func fullHistory(sessionId: String, transcriptPath: String) -> HistoryResult? {
        var scratch = SessionParseState(filePath: transcriptPath)
        var chunk = ChunkParser(keepsFullHistory: true, retainedMessageCount: Self.retainedMessageCount, wantedToolIds: [])
        var offset: UInt64 = 0
        guard TranscriptLineReader.forEachLine(path: transcriptPath, from: &offset, { line in
            chunk.consume(line, state: &scratch)
        }) != nil else { return nil }
        statistics.linesDecoded += chunk.linesDecoded
        chunk.finish()

        var result = HistoryResult()
        result.messages = chunk.messages
        result.completedToolIds = chunk.completedToolIds
        result.toolResults = chunk.toolResults
        result.structuredResults = chunk.structuredResults
        result.conversationInfo = scratch.summary.info

        var state = sessionState(for: sessionId, filePath: transcriptPath)
        for (toolUseId, structured) in chunk.structuredResults {
            guard case .task(let task) = structured, !task.agentId.isEmpty else { continue }
            var transcript = SubagentTranscript(agentFile: TranscriptLocator.subagentTranscriptPath(transcriptPath: transcriptPath, agentId: task.agentId))
            statistics.subagentFileReads += 1
            transcript.readNewLines()
            result.subagentTools[toolUseId] = transcript.tools
            if task.status == "completed" {
                state.settle(toolUseId)
            } else if state.agents[toolUseId] == nil, !state.settledAgents.contains(toolUseId) {
                // Still running as far as the transcript says, and not given
                // up on by an earlier sync: follow it from here.
                var track = AgentTrack(agentId: task.agentId, isFinished: false, addedAt: Date())
                track.transcript = transcript
                track.lastChangeAt = Date()
                state.agents[toolUseId] = track
            }
        }
        trimAgents(in: &state)
        sessionStates[sessionId] = state
        return result
    }

    /// Drop all cached state for a session (it ended or was removed).
    func forget(sessionId: String, transcriptPath: String?) {
        sessionStates.removeValue(forKey: sessionId)
    }

    /// Sessions with parser state (tests, leak checks).
    var trackedSessionIds: Set<String> {
        Set(sessionStates.keys)
    }

    /// Followed (not yet settled) agents of a session (tests).
    func followedAgentToolIds(sessionId: String) -> Set<String> {
        Set(sessionStates[sessionId].map { Array($0.agents.keys) } ?? [])
    }

    private func sessionState(for sessionId: String, filePath: String) -> SessionParseState {
        if let existing = sessionStates[sessionId],
           existing.filePath == filePath || TranscriptLocator.isSameFile(existing.filePath, filePath) {
            return existing
        }
        return SessionParseState(filePath: filePath)
    }

    // MARK: - Agents

    /// Reads new lines of every followed agent's transcript; returns the tool
    /// lists that changed. Finished agents are settled once read.
    private func refreshAgents(in state: inout SessionParseState, transcriptPath: String) -> [String: [SubagentToolInfo]] {
        var changed: [String: [SubagentToolInfo]] = [:]
        let now = Date()
        for toolUseId in Array(state.agents.keys) {
            guard var track = state.agents[toolUseId] else { continue }
            if track.transcript == nil {
                let path = TranscriptLocator.subagentTranscriptPath(transcriptPath: transcriptPath, agentId: track.agentId)
                if FileManager.default.fileExists(atPath: path) {
                    track.transcript = SubagentTranscript(agentFile: path)
                } else {
                    track.missingChecks += 1
                }
            }
            if var transcript = track.transcript, transcript.hasGrown {
                statistics.subagentFileReads += 1
                if transcript.readNewLines() {
                    changed[toolUseId] = transcript.tools
                    track.lastChangeAt = now
                }
                track.transcript = transcript
            }

            let settled: Bool
            if track.isFinished {
                // Its result is in: the agent wrote everything it will write.
                settled = track.transcript != nil || track.missingChecks >= 3
            } else {
                let lastChange = track.lastChangeAt ?? track.addedAt
                settled = now.timeIntervalSince(lastChange) > Self.agentIdleSettleInterval
            }
            if settled {
                state.settle(toolUseId)
            } else {
                state.agents[toolUseId] = track
            }
        }
        trimAgents(in: &state)
        return changed
    }

    private func trimAgents(in state: inout SessionParseState) {
        guard state.agents.count > Self.maxTrackedAgents else { return }
        let oldest = state.agents.sorted { $0.value.addedAt < $1.value.addedAt }
            .prefix(state.agents.count - Self.maxTrackedAgents)
        for (toolUseId, _) in oldest {
            state.settle(toolUseId)
        }
    }

    // MARK: - Decoding helpers

    nonisolated static func decode(_ line: Data) -> [String: Any]? {
        try? JSONSerialization.jsonObject(with: line) as? [String: Any]
    }

    /// Shared ISO 8601 formatters (expensive to create; thread-safe to use).
    nonisolated(unsafe) private static let isoFormatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    nonisolated(unsafe) private static let isoFormatterWholeSeconds: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter
    }()

    /// A transcript timestamp ("2026-09-24T10:00:00.000Z", fractional seconds optional).
    nonisolated static func parseDate(_ string: String) -> Date? {
        isoFormatter.date(from: string) ?? isoFormatterWholeSeconds.date(from: string)
    }

    /// Format tool input for display in instance list
    nonisolated static func formatToolInput(_ input: [String: Any]?, toolName: String) -> String {
        guard let input else { return "" }
        return ToolInput.preview(toolName: toolName, input: ToolInput.flatten(input)) ?? ""
    }
}

// MARK: - Per-session state

/// What the parser remembers about one session between syncs.
nonisolated struct SessionParseState: Sendable {
    /// Transcript this state was built from; a different path starts over.
    var filePath: String
    var offset: UInt64 = 0
    var summary = TranscriptSummary()
    /// The task list as the whole transcript defines it.
    var tasks = SessionTaskList()
    /// tool_use_id → tool name for calls whose result hasn't arrived.
    var inFlightTools: [String: String] = [:]
    static let maxInFlightTools = 1024
    /// Agent calls whose subagent transcript is still followed.
    var agents: [String: AgentTrack] = [:]
    /// Agent calls read to the end for good (the newest `maxSettledAgents`:
    /// an Agent result is read once, so only a whole-transcript read, such
    /// as a chat opening, meets an old one again).
    private(set) var settledAgents: Set<String> = []
    private var settledOrder: [String] = []
    static let maxSettledAgents = 512

    init(filePath: String) {
        self.filePath = filePath
    }

    /// Stops following an agent for good.
    mutating func settle(_ toolUseId: String) {
        agents[toolUseId] = nil
        guard settledAgents.insert(toolUseId).inserted else { return }
        settledOrder.append(toolUseId)
        if settledOrder.count > Self.maxSettledAgents * 2 {
            // Amortized: forget the oldest in batches.
            let forgotten = settledOrder.prefix(settledOrder.count - Self.maxSettledAgents)
            settledAgents.subtract(forgotten)
            settledOrder.removeFirst(forgotten.count)
        }
    }
}

/// One Agent (Task) call whose subagent transcript is followed.
nonisolated struct AgentTrack: Sendable {
    let agentId: String
    /// Its result says it completed (not launched in the background).
    let isFinished: Bool
    let addedAt: Date
    var transcript: SubagentTranscript?
    var lastChangeAt: Date?
    /// Syncs that found no transcript file yet.
    var missingChecks = 0

    init(agentId: String, isFinished: Bool, addedAt: Date) {
        self.agentId = agentId
        self.isFinished = isFinished
        self.addedAt = addedAt
    }
}

// MARK: - Chunk parsing

/// Parses one run of transcript lines into the session state (summary,
/// tasks, calls in flight) and the store's delta (messages, results).
nonisolated struct ChunkParser {
    let keepsFullHistory: Bool
    let retainedMessageCount: Int
    let wantedToolIds: Set<String>

    private(set) var messages: [ChatMessage] = []
    private(set) var completedToolIds: Set<String> = []
    private(set) var toolResults: [String: ConversationParser.ToolResult] = [:]
    private(set) var structuredResults: [String: ToolResultData] = [:]
    private(set) var sawClear = false
    private(set) var linesDecoded = 0

    /// tool_use ids of `messages`, so results are kept only for those.
    private var messageToolIds: Set<String> = []
    /// tool_use ids seen in this chunk, to drop streaming duplicates.
    private var seenToolIds: Set<String> = []

    /// Pre-filter only: JSON may escape the slashes ("\/clear").
    private static let clearMarker = Data("clear</command-name>".utf8)
    private static let escapedClearMarker = Data("clear<\\/command-name>".utf8)

    /// A user line echoing the /clear command.
    static func isClearCommand(_ json: [String: Any]) -> Bool {
        guard json["type"] as? String == "user", let message = json["message"] as? [String: Any] else { return false }
        let text = (message["content"] as? String)
            ?? (message["content"] as? [[String: Any]])?.first { $0["type"] as? String == "text" }?["text"] as? String
        return text?.contains("<command-name>/clear</command-name>") == true
    }
    private static let toolResultMarker = Data("\"tool_result\"".utf8)

    init(keepsFullHistory: Bool, retainedMessageCount: Int, wantedToolIds: Set<String>) {
        self.keepsFullHistory = keepsFullHistory
        self.retainedMessageCount = retainedMessageCount
        self.wantedToolIds = wantedToolIds
    }

    mutating func consume(_ line: Data, state: inout SessionParseState) {
        guard let json = ConversationParser.decode(line) else { return }
        linesDecoded += 1
        state.summary.consume(json, dateParser: ConversationParser.parseDate)
        state.tasks.applyTranscriptEntry(json)

        if line.range(of: Self.clearMarker) != nil || line.range(of: Self.escapedClearMarker) != nil,
           Self.isClearCommand(json) {
            // Everything before the /clear is gone from the conversation.
            sawClear = true
            messages.removeAll()
            messageToolIds.removeAll()
            completedToolIds.removeAll()
            toolResults.removeAll()
            structuredResults.removeAll()
            state.inFlightTools.removeAll()
            state.agents.removeAll()
            return
        }

        if line.range(of: Self.toolResultMarker) != nil {
            consumeToolResults(json, state: &state)
        }

        let type = json["type"] as? String
        if type == "user" || type == "assistant", let message = parseMessage(json, state: &state) {
            append(message)
        }
    }

    /// Drops what the store won't get, keeping the newest messages (all of
    /// them when the chat is open).
    mutating func finish() {
        trim(to: retainedMessageCount)
    }

    private mutating func append(_ message: ChatMessage) {
        messages.append(message)
        for case .toolUse(let tool) in message.content {
            messageToolIds.insert(tool.id)
        }
        // Amortized: trim in batches.
        if messages.count > retainedMessageCount * 2 {
            trim(to: retainedMessageCount)
        }
    }

    private mutating func trim(to count: Int) {
        guard !keepsFullHistory, messages.count > count else { return }
        let dropped = messages.prefix(messages.count - count)
        messages.removeFirst(dropped.count)
        for message in dropped {
            for case .toolUse(let tool) in message.content where !wantedToolIds.contains(tool.id) {
                messageToolIds.remove(tool.id)
                completedToolIds.remove(tool.id)
                toolResults.removeValue(forKey: tool.id)
                structuredResults.removeValue(forKey: tool.id)
            }
        }
    }

    private func keepsResult(for toolUseId: String) -> Bool {
        keepsFullHistory || messageToolIds.contains(toolUseId) || wantedToolIds.contains(toolUseId)
    }

    private mutating func consumeToolResults(_ json: [String: Any], state: inout SessionParseState) {
        guard json["isSidechain"] as? Bool != true,
              let message = json["message"] as? [String: Any],
              let blocks = message["content"] as? [[String: Any]] else { return }
        let toolUseResult = json["toolUseResult"] as? [String: Any]
        let topLevelToolName = json["toolName"] as? String

        for block in blocks where block["type"] as? String == "tool_result" {
            guard let toolUseId = block["tool_use_id"] as? String else { continue }
            let inFlightName = state.inFlightTools.removeValue(forKey: toolUseId)
            let toolName = topLevelToolName ?? inFlightName
            guard keepsResult(for: toolUseId) else { continue }

            completedToolIds.insert(toolUseId)
            let isError = block["is_error"] as? Bool ?? false
            toolResults[toolUseId] = ConversationParser.ToolResult(
                content: Self.resultText(block["content"]),
                stdout: toolUseResult?["stdout"] as? String,
                stderr: toolUseResult?["stderr"] as? String,
                isError: isError
            )
            if let toolUseResult, let toolName {
                structuredResults[toolUseId] = ToolResultParser.parse(
                    toolName: toolName,
                    toolUseResult: toolUseResult,
                    isError: isError
                )
            }
        }
    }

    /// tool_result `content`: a string, or the first text block of an array.
    private static func resultText(_ content: Any?) -> String? {
        if let text = content as? String { return text }
        if let blocks = content as? [[String: Any]] {
            return blocks.lazy.compactMap { $0["type"] as? String == "text" ? $0["text"] as? String : nil }.first
        }
        return nil
    }

    private mutating func parseMessage(_ json: [String: Any], state: inout SessionParseState) -> ChatMessage? {
        guard let type = json["type"] as? String,
              let uuid = json["uuid"] as? String,
              json["isMeta"] as? Bool != true,
              json["isSidechain"] as? Bool != true,
              let messageDict = json["message"] as? [String: Any] else {
            return nil
        }

        let timestamp = (json["timestamp"] as? String).flatMap(ConversationParser.parseDate) ?? Date()
        var blocks: [MessageBlock] = []

        if let content = messageDict["content"] as? String {
            if TranscriptSummary.isCommandText(content) {
                return nil
            }
            if TranscriptSummary.isInterruptMarker(content) {
                blocks.append(.interrupted)
            } else {
                blocks.append(.text(content))
            }
        } else if let contentArray = messageDict["content"] as? [[String: Any]] {
            for block in contentArray {
                switch block["type"] as? String {
                case "text":
                    if let text = block["text"] as? String {
                        blocks.append(TranscriptSummary.isInterruptMarker(text) ? .interrupted : .text(text))
                    }
                case "tool_use":
                    guard let toolId = block["id"] as? String, let name = block["name"] as? String else { continue }
                    guard seenToolIds.insert(toolId).inserted else { continue }
                    if state.inFlightTools.count >= SessionParseState.maxInFlightTools {
                        // Calls whose result never came (a crash mid-turn);
                        // their names only label late results.
                        state.inFlightTools.removeAll()
                    }
                    state.inFlightTools[toolId] = name
                    let input = ToolInput.flatten(block["input"] as? [String: Any] ?? [:])
                    blocks.append(.toolUse(ToolUseBlock(id: toolId, name: name, input: input)))
                case "thinking":
                    if let thinking = block["thinking"] as? String {
                        blocks.append(.thinking(thinking))
                    }
                case "image":
                    // Claude Code stores inline images as base64 with media_type.
                    if let source = block["source"] as? [String: Any],
                       let mediaType = source["media_type"] as? String,
                       let data = source["data"] as? String {
                        blocks.append(.image(ImageBlock(mediaType: mediaType, base64Data: data)))
                    }
                default:
                    break
                }
            }
        }

        guard !blocks.isEmpty else { return nil }
        return ChatMessage(
            id: uuid,
            role: type == "user" ? .user : .assistant,
            timestamp: timestamp,
            content: blocks
        )
    }
}

// MARK: - Subagent transcripts

/// Info about a subagent tool call parsed from JSONL
nonisolated struct SubagentToolInfo: Equatable, Sendable {
    let id: String
    let name: String
    let input: [String: String]
    var isCompleted: Bool
    let timestamp: Date?
}

/// A subagent's transcript, read incrementally: its tool calls and which
/// of them finished.
nonisolated struct SubagentTranscript: Sendable {
    let agentFile: String
    private(set) var offset: UInt64 = 0
    private(set) var tools: [SubagentToolInfo] = []
    private var indexById: [String: Int] = [:]
    /// Results seen before their call (defensive; not expected).
    private var earlyResults: Set<String> = []

    private static let toolUseMarker = Data("\"tool_use\"".utf8)
    private static let toolResultMarker = Data("\"tool_result\"".utf8)

    init(agentFile: String) {
        self.agentFile = agentFile
    }

    /// The file is longer than what was read (a stat, no read).
    var hasGrown: Bool {
        guard let size = (try? FileManager.default.attributesOfItem(atPath: agentFile))?[.size] as? NSNumber else {
            return false
        }
        return size.uint64Value != offset
    }

    /// Reads appended lines. Returns whether the tool list changed.
    @discardableResult
    mutating func readNewLines() -> Bool {
        var changed = false
        if let size = TranscriptLineReader.size(of: agentFile), size < offset {
            // Rewritten: start over.
            self = SubagentTranscript(agentFile: agentFile)
            changed = true
        }
        var position = offset
        let outcome = TranscriptLineReader.forEachLine(path: agentFile, from: &position) { line in
            let hasUse = line.range(of: Self.toolUseMarker) != nil
            let hasResult = line.range(of: Self.toolResultMarker) != nil
            guard hasUse || hasResult, let json = ConversationParser.decode(line),
                  let message = json["message"] as? [String: Any],
                  let blocks = message["content"] as? [[String: Any]] else { return }
            let timestamp = (json["timestamp"] as? String).flatMap(ConversationParser.parseDate)
            for block in blocks {
                switch block["type"] as? String {
                case "tool_use":
                    guard let id = block["id"] as? String, let name = block["name"] as? String,
                          indexById[id] == nil else { continue }
                    indexById[id] = tools.count
                    tools.append(SubagentToolInfo(
                        id: id,
                        name: name,
                        input: ToolInput.flatten(block["input"] as? [String: Any] ?? [:]),
                        isCompleted: earlyResults.remove(id) != nil,
                        timestamp: timestamp
                    ))
                    changed = true
                case "tool_result":
                    guard let id = block["tool_use_id"] as? String else { continue }
                    if let index = indexById[id] {
                        if !tools[index].isCompleted {
                            tools[index].isCompleted = true
                            changed = true
                        }
                    } else {
                        earlyResults.insert(id)
                    }
                default:
                    continue
                }
            }
        }
        guard outcome != nil else { return changed }
        offset = position
        return changed
    }
}
