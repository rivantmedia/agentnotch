//
//  SessionTaskList.swift
//  ClaudeIsland
//
//  Per-session progress (the task list Claude Code shows under the prompt),
//  rebuilt the way Claude Code rebuilds it itself:
//  - TaskCreate calls are pending until their result names the new task id
//    (keyed by tool_use_id, so duplicate subjects can't be confused),
//  - TaskUpdate changes status/subject/activeForm (`deleted` removes the task),
//  - TodoWrite replaces the legacy todo list, shown only when no Task* tasks exist,
//  - subagent calls (hooks with agent_id, sidechain transcript lines) are ignored,
//  - the list survives new prompts; /clear resets it, and so does a new batch:
//    Claude Code deletes a list once every task in it is completed, so a
//    TaskCreate arriving when all tasks are done starts a fresh list (task ids
//    keep counting up, so nothing collides).
//  Each task also remembers when it was created, started and completed (hook
//  arrival times, transcript timestamps for history), so the list knows its
//  own pace: `timing` turns that into how far along it is and how long the
//  rest should take (TaskTiming).
//

import Foundation

/// One task (or todo) in a session's progress list.
nonisolated struct SessionTaskItem: Equatable, Sendable, Identifiable {
    nonisolated enum Status: String, Sendable {
        case pending
        case inProgress = "in_progress"
        case completed
    }

    /// Claude Code's task id ("3"), or "todo-<index>" for TodoWrite items.
    let id: String
    var subject: String
    /// Present-continuous label shown while in progress, e.g. "Running tests".
    var activeForm: String?
    var description: String?
    var status: Status
    /// The subject is the "Task #<id>" stand-in for a task seen only in an update.
    var hasPlaceholderSubject: Bool = false
    /// When the task was created, when it last went in progress and when it
    /// was completed: hook arrival times, or transcript timestamps for history.
    /// Nil when not seen (a task first met mid-flight, or a list built without dates).
    var createdAt: Date?
    var startedAt: Date?
    var completedAt: Date?

    /// Label for the in-progress row: activeForm, else the subject.
    var activeLabel: String {
        if let activeForm, !activeForm.isEmpty { return activeForm }
        return subject
    }

    /// Changes the status and dates the change. Going back to pending forgets
    /// both times; (re)starting restarts the clock; a status that doesn't
    /// change keeps its time (a repeated "in_progress" isn't a new start).
    mutating func setStatus(_ newStatus: Status, at date: Date?) {
        guard newStatus != status else { return }
        status = newStatus
        switch newStatus {
        case .pending:
            startedAt = nil
            completedAt = nil
        case .inProgress:
            startedAt = date
            completedAt = nil
        case .completed:
            completedAt = date
        }
    }

    /// Wall-clock time from start to completion (completed tasks), or so far
    /// (the task in progress); nil when a time is unknown.
    func duration(now: Date) -> TimeInterval? {
        switch status {
        case .pending:
            return nil
        case .inProgress:
            return startedAt.map { max(0, now.timeIntervalSince($0)) }
        case .completed:
            guard let startedAt, let completedAt else { return nil }
            return max(0, completedAt.timeIntervalSince(startedAt))
        }
    }

    /// `self` (newer, from hooks) with the times `history` (the same task
    /// rebuilt from the transcript) knew earlier: a hook seen late misses the
    /// creation or the start the transcript dated. A time the status no
    /// longer has stays unset.
    func withTimes(mergedFrom history: SessionTaskItem) -> SessionTaskItem {
        func earliest(_ a: Date?, _ b: Date?) -> Date? {
            switch (a, b) {
            case let (a?, b?): return min(a, b)
            default: return a ?? b
            }
        }
        var merged = self
        merged.createdAt = earliest(createdAt, history.createdAt)
        merged.startedAt = status == .pending ? nil : earliest(startedAt, history.startedAt)
        merged.completedAt = status == .completed ? earliest(completedAt, history.completedAt) : nil
        return merged
    }
}

/// A session's task list plus the reducer that maintains it.
nonisolated struct SessionTaskList: Equatable, Sendable {
    /// A TaskCreate call whose task id isn't known yet.
    nonisolated struct PendingCreate: Equatable, Sendable {
        var subject: String
        var description: String?
        var activeForm: String?
        /// When the call was made: the task's creation time.
        var createdAt: Date?
    }

    /// Tasks from the Task* tools, in creation order.
    private(set) var tasks: [SessionTaskItem] = []
    /// The latest TodoWrite list.
    private(set) var todos: [SessionTaskItem] = []
    /// TaskCreate calls keyed by tool_use_id, waiting for their task id.
    private(set) var pendingCreates: [String: PendingCreate] = [:]
    /// TaskCreate calls already resolved or failed (tool_use_ids).
    private(set) var settledCreates: Set<String> = []
    /// Task ids deleted with TaskUpdate status "deleted".
    private(set) var deletedIds: Set<String> = []
    /// Whether a TodoWrite list was seen (an empty list is still a replacement).
    private(set) var hasTodoList = false

    init() {}

    // MARK: - Derived

    /// What to display: Task* tasks when any exist, else the TodoWrite list.
    var items: [SessionTaskItem] {
        tasks.isEmpty ? todos : tasks
    }

    var totalCount: Int { items.count }

    var completedCount: Int {
        items.reduce(0) { $0 + ($1.status == .completed ? 1 : 0) }
    }

    /// The first in-progress item.
    var activeItem: SessionTaskItem? {
        items.first { $0.status == .inProgress }
    }

    /// Completed fraction 0...1; 0 when there are no items.
    var fraction: Double {
        totalCount == 0 ? 0 : Double(completedCount) / Double(totalCount)
    }

    var isEmpty: Bool { items.isEmpty }

    /// Pace and progress measured from the list's own times (see TaskTiming).
    var timing: TaskTiming {
        TaskTiming(
            completed: completedCount,
            total: totalCount,
            secondsPerTask: TaskTiming.secondsPerTask(items),
            activeSince: activeItem?.startedAt
        )
    }

    // MARK: - Reducer operations

    /// Forget everything (/clear, SessionStart source "clear").
    mutating func reset() {
        self = SessionTaskList()
    }

    /// PreToolUse TaskCreate.
    mutating func taskCreateStarted(toolUseId: String, subject: String, description: String?, activeForm: String?, at date: Date? = nil) {
        startNewBatchIfDone()
        pendingCreates[toolUseId] = PendingCreate(subject: subject, description: description, activeForm: activeForm, createdAt: date)
    }

    /// Claude Code resets the task list once all of it is completed (in the
    /// terminal, 5 s later). A new task after that belongs to a new list;
    /// until then, the finished one stays (so a review row still reads 3/3).
    private mutating func startNewBatchIfDone() {
        guard !tasks.isEmpty, tasks.allSatisfy({ $0.status == .completed }) else { return }
        tasks.removeAll()
    }

    /// PostToolUse TaskCreate (or its transcript result) with the new task id.
    mutating func taskCreateFinished(toolUseId: String, taskId: String, subject: String?, at date: Date? = nil) {
        let pending = pendingCreates.removeValue(forKey: toolUseId)
        settledCreates.insert(toolUseId)
        if pending == nil && !tasks.contains(where: { $0.id == taskId }) {
            startNewBatchIfDone()
        }
        let resolvedSubject = nonEmpty(subject) ?? pending?.subject ?? "Task #\(taskId)"
        upsert(
            id: taskId,
            subject: resolvedSubject,
            description: pending?.description,
            activeForm: pending?.activeForm,
            status: nil,
            at: pending?.createdAt ?? date
        )
    }

    /// PostToolUseFailure TaskCreate: the task was never created.
    mutating func taskCreateFailed(toolUseId: String) {
        pendingCreates.removeValue(forKey: toolUseId)
        settledCreates.insert(toolUseId)
    }

    /// TaskCreated hook event.
    mutating func taskCreated(taskId: String, subject: String?, at date: Date? = nil) {
        if !tasks.contains(where: { $0.id == taskId }) {
            startNewBatchIfDone()
        }
        upsert(id: taskId, subject: nonEmpty(subject), description: nil, activeForm: nil, status: nil, at: date)
    }

    /// PreToolUse TaskUpdate. `status` is Claude Code's raw value; "deleted" removes the task.
    mutating func taskUpdated(taskId: String, status: String?, subject: String?, activeForm: String?, at date: Date? = nil) {
        if status == "deleted" {
            tasks.removeAll { $0.id == taskId }
            deletedIds.insert(taskId)
            return
        }
        upsert(
            id: taskId,
            subject: nonEmpty(subject),
            description: nil,
            activeForm: nonEmpty(activeForm),
            status: status.flatMap(SessionTaskItem.Status.init(rawValue:)),
            at: date
        )
    }

    /// TaskCompleted hook event.
    mutating func taskCompleted(taskId: String, subject: String?, at date: Date? = nil) {
        upsert(id: taskId, subject: nonEmpty(subject), description: nil, activeForm: nil, status: .completed, at: date)
    }

    /// PreToolUse TodoWrite: the full list is replaced. A todo keeps the times
    /// of the previous list's todo with the same text (each matched once), so
    /// rewriting the list doesn't restart its clocks.
    mutating func todosReplaced(_ newTodos: [(content: String, status: String, activeForm: String?)], at date: Date? = nil) {
        hasTodoList = true
        var previous = todos
        todos = newTodos.enumerated().map { index, todo in
            var item = SessionTaskItem(
                id: "todo-\(index)",
                subject: todo.content,
                activeForm: nonEmpty(todo.activeForm),
                description: nil,
                status: .pending,
                createdAt: date
            )
            if let match = previous.firstIndex(where: { $0.subject == todo.content }) {
                let earlier = previous.remove(at: match)
                item.status = earlier.status
                item.createdAt = earlier.createdAt
                item.startedAt = earlier.startedAt
                item.completedAt = earlier.completedAt
            }
            item.setStatus(SessionTaskItem.Status(rawValue: todo.status) ?? .pending, at: date)
            return item
        }
    }

    private mutating func upsert(
        id: String,
        subject: String?,
        description: String?,
        activeForm: String?,
        status: SessionTaskItem.Status?,
        at date: Date?
    ) {
        if let index = tasks.firstIndex(where: { $0.id == id }) {
            if let subject {
                tasks[index].subject = subject
                tasks[index].hasPlaceholderSubject = false
            }
            if let description { tasks[index].description = description }
            if let activeForm { tasks[index].activeForm = activeForm }
            if let status { tasks[index].setStatus(status, at: date) }
        } else {
            var item = SessionTaskItem(
                id: id,
                subject: subject ?? "Task #\(id)",
                activeForm: activeForm,
                description: description,
                status: .pending,
                hasPlaceholderSubject: subject == nil,
                createdAt: date
            )
            if let status { item.setStatus(status, at: date) }
            tasks.append(item)
        }
    }

    /// Combines a list rebuilt from the transcript (history before the app saw
    /// the session) with `self`, built from hooks since then. Hook data is
    /// newer and wins, except that a placeholder subject never replaces a real one.
    func merged(intoReconstructed reconstructed: SessionTaskList) -> SessionTaskList {
        var result = reconstructed
        for id in deletedIds {
            result.tasks.removeAll { $0.id == id }
        }
        result.deletedIds.formUnion(deletedIds)
        for task in tasks {
            guard let index = result.tasks.firstIndex(where: { $0.id == task.id }) else {
                result.tasks.append(task)
                continue
            }
            var mergedTask = task
            let base = result.tasks[index]
            if task.hasPlaceholderSubject && !base.hasPlaceholderSubject {
                mergedTask.subject = base.subject
                mergedTask.hasPlaceholderSubject = false
            }
            mergedTask.activeForm = task.activeForm ?? base.activeForm
            mergedTask.description = task.description ?? base.description
            result.tasks[index] = mergedTask.withTimes(mergedFrom: base)
        }
        result.settledCreates.formUnion(settledCreates)
        result.pendingCreates = result.pendingCreates
            .filter { !settledCreates.contains($0.key) }
            .merging(pendingCreates) { _, hook in hook }
        if hasTodoList {
            var history = result.todos
            result.todos = todos.map { todo in
                guard let match = history.firstIndex(where: { $0.subject == todo.subject }) else { return todo }
                return todo.withTimes(mergedFrom: history.remove(at: match))
            }
            result.hasTodoList = true
        }
        return result
    }

    private func nonEmpty(_ value: String?) -> String? {
        guard let value, !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return value
    }
}

// MARK: - Hook events

nonisolated extension SessionTaskList {
    /// Applies a hook event. Returns true if the event was a task event for the
    /// main session (whether or not it changed anything). Subagent events are ignored.
    @discardableResult
    mutating func apply(_ event: HookEvent) -> Bool {
        guard !event.isSubagentEvent else { return false }
        let date = event.receivedAt

        switch event.event {
        case "PreToolUse":
            let input = event.toolInput ?? [:]
            switch event.tool {
            case "TaskCreate":
                guard let toolUseId = event.toolUseId else { return false }
                taskCreateStarted(
                    toolUseId: toolUseId,
                    subject: JSONValue.string(input["subject"]?.value) ?? "Untitled task",
                    description: JSONValue.string(input["description"]?.value),
                    activeForm: JSONValue.string(input["activeForm"]?.value),
                    at: date
                )
                return true
            case "TaskUpdate":
                guard let taskId = JSONValue.string(input["taskId"]?.value) else { return false }
                taskUpdated(
                    taskId: taskId,
                    status: JSONValue.string(input["status"]?.value),
                    subject: JSONValue.string(input["subject"]?.value),
                    activeForm: JSONValue.string(input["activeForm"]?.value),
                    at: date
                )
                return true
            case "TodoWrite":
                todosReplaced(Self.todos(from: input["todos"]?.value), at: date)
                return true
            default:
                return false
            }

        case "PostToolUse" where event.tool == "TaskCreate":
            guard let toolUseId = event.toolUseId else { return false }
            if let taskId = event.taskId {
                taskCreateFinished(toolUseId: toolUseId, taskId: taskId, subject: event.taskSubject, at: date)
            }
            return true

        case "PostToolUseFailure" where event.tool == "TaskCreate":
            if let toolUseId = event.toolUseId {
                taskCreateFailed(toolUseId: toolUseId)
            }
            return true

        case "TaskCreated":
            guard let taskId = event.taskId else { return false }
            taskCreated(taskId: taskId, subject: event.taskSubject, at: date)
            return true

        case "TaskCompleted":
            guard let taskId = event.taskId else { return false }
            taskCompleted(taskId: taskId, subject: event.taskSubject, at: date)
            return true

        case "SessionStart" where event.source == "clear":
            reset()
            return true

        default:
            return false
        }
    }

    /// TodoWrite `todos` array → (content, status, activeForm) tuples.
    static func todos(from raw: Any?) -> [(content: String, status: String, activeForm: String?)] {
        guard let array = raw as? [Any] else { return [] }
        return array.compactMap { element in
            guard let todo = element as? [String: Any],
                  let content = JSONValue.string(todo["content"]) else { return nil }
            return (content, JSONValue.string(todo["status"]) ?? "pending", JSONValue.string(todo["activeForm"]))
        }
    }
}

// MARK: - Transcript reconstruction

nonisolated extension SessionTaskList {
    /// Rebuilds the list from a session transcript, for sessions first seen
    /// mid-flight (no hooks observed yet). Mirrors the hook path: main-thread
    /// (non-sidechain) assistant tool_use blocks for TaskCreate/TaskUpdate/TodoWrite,
    /// resolved by the `Task #<id> created successfully` tool results.
    static func reconstruct(fromTranscriptLines lines: some Sequence<Substring>) -> SessionTaskList {
        var list = SessionTaskList()
        for line in lines where !line.isEmpty {
            // Cheap pre-filter: only lines that can matter get decoded.
            guard line.contains("Task") || line.contains("TodoWrite") || line.contains("/clear") else { continue }
            guard let data = line.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
            list.applyTranscriptEntry(json)
        }
        return list
    }

    /// Reads and reconstructs from a transcript file; nil if it can't be read.
    /// Lines are split and pre-filtered as bytes (no String of the file).
    static func reconstruct(fromTranscriptAt path: String) -> SessionTaskList? {
        var list = SessionTaskList()
        var offset: UInt64 = 0
        let outcome = TranscriptLineReader.forEachLine(path: path, from: &offset) { line in
            guard relevantMarkers.contains(where: { line.range(of: $0) != nil }),
                  let json = try? JSONSerialization.jsonObject(with: line) as? [String: Any] else { return }
            list.applyTranscriptEntry(json)
        }
        return outcome == nil ? nil : list
    }

    /// Bytes a line must contain to matter for the task list.
    private static let relevantMarkers = ["Task", "TodoWrite", "/clear"].map { Data($0.utf8) }

    private static let createdPattern = try? NSRegularExpression(pattern: #"^Task #(\S+) created successfully"#)

    mutating func applyTranscriptEntry(_ json: [String: Any]) {
        if json["isSidechain"] as? Bool == true { return }
        guard let type = json["type"] as? String,
              let message = json["message"] as? [String: Any] else { return }
        let date = (json["timestamp"] as? String).flatMap(ConversationParser.parseDate)

        if type == "user", let text = message["content"] as? String,
           text.contains("<command-name>/clear</command-name>") {
            reset()
            return
        }
        guard let blocks = message["content"] as? [[String: Any]] else { return }

        if type == "assistant" {
            for block in blocks where block["type"] as? String == "tool_use" {
                guard let name = block["name"] as? String,
                      let toolUseId = block["id"] as? String else { continue }
                let input = block["input"] as? [String: Any] ?? [:]
                switch name {
                case "TaskCreate":
                    taskCreateStarted(
                        toolUseId: toolUseId,
                        subject: JSONValue.string(input["subject"]) ?? "Untitled task",
                        description: JSONValue.string(input["description"]),
                        activeForm: JSONValue.string(input["activeForm"]),
                        at: date
                    )
                case "TaskUpdate":
                    if let taskId = JSONValue.string(input["taskId"]) {
                        taskUpdated(
                            taskId: taskId,
                            status: JSONValue.string(input["status"]),
                            subject: JSONValue.string(input["subject"]),
                            activeForm: JSONValue.string(input["activeForm"]),
                            at: date
                        )
                    }
                case "TodoWrite":
                    todosReplaced(Self.todos(from: input["todos"]), at: date)
                default:
                    break
                }
            }
        } else if type == "user" {
            for block in blocks where block["type"] as? String == "tool_result" {
                guard let toolUseId = block["tool_use_id"] as? String,
                      pendingCreates[toolUseId] != nil else { continue }
                if block["is_error"] as? Bool == true {
                    taskCreateFailed(toolUseId: toolUseId)
                    continue
                }
                // Prefer the structured result, then the text.
                let structured = (json["toolUseResult"] as? [String: Any])?["task"] as? [String: Any]
                if let taskId = JSONValue.string(structured?["id"]) {
                    taskCreateFinished(toolUseId: toolUseId, taskId: taskId, subject: JSONValue.string(structured?["subject"]), at: date)
                } else if let text = Self.resultText(block["content"]), let taskId = Self.createdTaskId(in: text) {
                    taskCreateFinished(toolUseId: toolUseId, taskId: taskId, subject: nil, at: date)
                }
            }
        }
    }

    /// Task id from a `Task #<id> created successfully: <subject>` result text.
    static func createdTaskId(in text: String) -> String? {
        guard let regex = createdPattern else { return nil }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let range = NSRange(trimmed.startIndex..., in: trimmed)
        guard let match = regex.firstMatch(in: trimmed, range: range),
              let idRange = Range(match.range(at: 1), in: trimmed) else { return nil }
        var id = String(trimmed[idRange])
        if id.hasSuffix(":") { id.removeLast() }
        return id
    }

    /// tool_result `content`: a string or an array of text blocks.
    private static func resultText(_ content: Any?) -> String? {
        if let text = content as? String { return text }
        if let blocks = content as? [[String: Any]] {
            return blocks.compactMap { $0["text"] as? String }.first
        }
        return nil
    }
}
