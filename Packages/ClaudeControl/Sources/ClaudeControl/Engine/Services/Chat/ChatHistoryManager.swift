//
//  ChatHistoryManager.swift
//  ClaudeControl
//

import Combine
import Foundation

/// The chat histories of the sessions whose chat is open.
///
/// Only those sessions keep their whole history (the store keeps the newest
/// items of every other session), so at most `maxOpenHistories` are held:
/// opening another chat releases the one opened longest ago. A history is
/// filtered again only when its session's items changed (`chatRevision`),
/// not on every publish.
@MainActor
class ChatHistoryManager: ObservableObject {
    static let shared = ChatHistoryManager()

    /// Chats whose whole history is kept at once.
    static let maxOpenHistories = 2

    @Published private(set) var histories: [String: [ChatHistoryItem]] = [:]
    @Published private(set) var agentDescriptions: [String: [String: String]] = [:]

    /// Open chats, least recently opened first.
    private var openOrder: [String] = []
    /// Open chats whose transcript has been read in full.
    private var loadedSessions: Set<String> = []
    /// `chatRevision` each open history was built from.
    private var builtRevisions: [String: Int] = [:]
    private let monitor: ClaudeSessionMonitor
    private var cancellables = Set<AnyCancellable>()

    private convenience init() {
        self.init(monitor: .shared)
    }

    init(monitor: ClaudeSessionMonitor) {
        self.monitor = monitor
        monitor.$instances
            .sink { [weak self] sessions in
                self?.updateFromSessions(sessions)
            }
            .store(in: &cancellables)
    }

    // MARK: - Public API

    func history(for sessionId: String) -> [ChatHistoryItem] {
        histories[sessionId] ?? []
    }

    /// Whether the session's whole transcript has been read for its chat
    /// (and is still kept).
    func isLoaded(sessionId: String) -> Bool {
        loadedSessions.contains(sessionId)
    }

    /// Opens the session's chat: reads its whole transcript once and keeps
    /// the history current until the chat closes or is released.
    func loadFromFile(sessionId: String, cwd: String) async {
        guard !loadedSessions.contains(sessionId) else {
            touch(sessionId)
            return
        }
        touch(sessionId)
        await monitor.loadHistory(sessionId: sessionId, cwd: cwd)
        guard openOrder.contains(sessionId) else { return }
        loadedSessions.insert(sessionId)
        // Ahead of the store's next publish, so the chat shows it at once.
        if let session = await monitor.currentSession(sessionId) {
            rebuild(session)
        }
    }

    /// The session's chat closed: its history is released.
    func chatClosed(sessionId: String) {
        guard let index = openOrder.firstIndex(of: sessionId) else { return }
        openOrder.remove(at: index)
        release(sessionId)
    }

    // MARK: - State Updates

    /// Marks `sessionId` most recently opened; releases the oldest beyond the limit.
    private func touch(_ sessionId: String) {
        openOrder.removeAll { $0 == sessionId }
        openOrder.append(sessionId)
        // The open chat must find its entry in `histories` before any other
        // change is published (a missing entry means "the session is gone").
        if histories[sessionId] == nil, let session = monitor.instances.first(where: { $0.sessionId == sessionId }) {
            rebuild(session)
        }
        while openOrder.count > Self.maxOpenHistories {
            release(openOrder.removeFirst())
        }
    }

    private func release(_ sessionId: String) {
        loadedSessions.remove(sessionId)
        builtRevisions.removeValue(forKey: sessionId)
        histories.removeValue(forKey: sessionId)
        agentDescriptions.removeValue(forKey: sessionId)
        monitor.releaseHistory(sessionId: sessionId)
    }

    private func updateFromSessions(_ sessions: [SessionState]) {
        guard !openOrder.isEmpty else { return }
        for sessionId in openOrder {
            guard let session = sessions.first(where: { $0.sessionId == sessionId }) else {
                // Ended (or cleared away): an open chat of it goes back.
                if histories[sessionId] != nil {
                    histories.removeValue(forKey: sessionId)
                    agentDescriptions.removeValue(forKey: sessionId)
                    builtRevisions.removeValue(forKey: sessionId)
                }
                continue
            }
            if builtRevisions[sessionId] != session.chatRevision || histories[sessionId] == nil {
                rebuild(session)
            }
        }
    }

    private func rebuild(_ session: SessionState) {
        builtRevisions[session.sessionId] = session.chatRevision
        let filtered = Self.filterOutSubagentTools(session.chatItems)
        if histories[session.sessionId] != filtered {
            histories[session.sessionId] = filtered
        }
        if agentDescriptions[session.sessionId] != session.subagentState.agentDescriptions {
            agentDescriptions[session.sessionId] = session.subagentState.agentDescriptions
        }
    }

    /// Tool calls a subagent made show under their Agent item, not on their own.
    static func filterOutSubagentTools(_ items: [ChatHistoryItem]) -> [ChatHistoryItem] {
        var subagentToolIds = Set<String>()
        for item in items {
            if case .toolCall(let tool) = item.type, tool.isSubagentContainer {
                for subagentTool in tool.subagentTools {
                    subagentToolIds.insert(subagentTool.id)
                }
            }
        }
        guard !subagentToolIds.isEmpty else { return items }
        return items.filter { !subagentToolIds.contains($0.id) }
    }
}

// MARK: - Models

nonisolated struct ChatHistoryItem: Identifiable, Equatable, Sendable {
    let id: String
    let type: ChatHistoryItemType
    let timestamp: Date

    static func == (lhs: ChatHistoryItem, rhs: ChatHistoryItem) -> Bool {
        lhs.id == rhs.id && lhs.type == rhs.type
    }
}

nonisolated enum ChatHistoryItemType: Equatable, Sendable {
    case user(String)
    case assistant(String)
    case toolCall(ToolCallItem)
    case thinking(String)
    case image(ImageBlock)
    case interrupted
}

nonisolated struct ToolCallItem: Equatable, Sendable {
    let name: String
    let input: [String: String]
    var status: ToolStatus
    var result: String?
    var structuredResult: ToolResultData?

    /// For Task tools: nested subagent tool calls
    var subagentTools: [SubagentToolCall]

    /// Whether this tool is the subagent-container tool. "Task" is the
    /// legacy name; Claude Code now uses "Agent".
    var isSubagentContainer: Bool {
        Self.isSubagentContainerName(name)
    }

    /// Same check by raw tool-name string (used when we don't have a
    /// ToolCallItem — e.g. when matching against `HookEvent.tool`).
    static func isSubagentContainerName(_ name: String?) -> Bool {
        name == "Task" || name == "Agent"
    }

    /// Status display text for the tool
    var statusDisplay: ToolStatusDisplay {
        if status == .running {
            return ToolStatusDisplay.running(for: name, input: input)
        }
        if status == .waitingForApproval {
            return ToolStatusDisplay(text: "Waiting for approval...", isRunning: true)
        }
        if status == .interrupted {
            return ToolStatusDisplay(text: "Interrupted", isRunning: false)
        }
        return ToolStatusDisplay.completed(for: name, result: structuredResult)
    }

    // Custom Equatable implementation to handle structuredResult
    static func == (lhs: ToolCallItem, rhs: ToolCallItem) -> Bool {
        lhs.name == rhs.name &&
        lhs.input == rhs.input &&
        lhs.status == rhs.status &&
        lhs.result == rhs.result &&
        lhs.structuredResult == rhs.structuredResult &&
        lhs.subagentTools == rhs.subagentTools
    }
}

nonisolated enum ToolStatus: Sendable, CustomStringConvertible {
    case running
    case waitingForApproval
    case success
    case error
    case interrupted

    nonisolated var description: String {
        switch self {
        case .running: return "running"
        case .waitingForApproval: return "waitingForApproval"
        case .success: return "success"
        case .error: return "error"
        case .interrupted: return "interrupted"
        }
    }
}

// Explicit nonisolated Equatable conformance to avoid actor isolation issues
extension ToolStatus: Equatable {
    nonisolated static func == (lhs: ToolStatus, rhs: ToolStatus) -> Bool {
        switch (lhs, rhs) {
        case (.running, .running): return true
        case (.waitingForApproval, .waitingForApproval): return true
        case (.success, .success): return true
        case (.error, .error): return true
        case (.interrupted, .interrupted): return true
        default: return false
        }
    }
}

// MARK: - Subagent Tool Call

/// Represents a tool call made by a subagent (Task tool)
nonisolated struct SubagentToolCall: Equatable, Identifiable, Sendable {
    let id: String
    let name: String
    let input: [String: String]
    var status: ToolStatus
    let timestamp: Date

    /// Short description for display
    var displayText: String {
        switch name {
        case "Read":
            if let path = input["file_path"] {
                return URL(fileURLWithPath: path).lastPathComponent
            }
            return "Reading..."
        case "Grep":
            if let pattern = input["pattern"] {
                return "grep: \(pattern)"
            }
            return "Searching..."
        case "Glob":
            if let pattern = input["pattern"] {
                return "glob: \(pattern)"
            }
            return "Finding files..."
        case "Bash":
            if let desc = input["description"] {
                return desc
            }
            if let cmd = input["command"] {
                let firstLine = cmd.components(separatedBy: "\n").first ?? cmd
                return String(firstLine.prefix(40))
            }
            return "Running command..."
        case "Edit":
            if let path = input["file_path"] {
                return "Edit: \(URL(fileURLWithPath: path).lastPathComponent)"
            }
            return "Editing..."
        case "Write":
            if let path = input["file_path"] {
                return "Write: \(URL(fileURLWithPath: path).lastPathComponent)"
            }
            return "Writing..."
        case "WebFetch":
            if let url = input["url"] {
                return "Fetching: \(url.prefix(30))..."
            }
            return "Fetching..."
        case "WebSearch":
            if let query = input["query"] {
                return "Search: \(query.prefix(30))"
            }
            return "Searching web..."
        default:
            return name
        }
    }
}
