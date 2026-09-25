//
//  HostAppCache.swift
//  ClaudeControl
//
//  What the focus button, the hover card and the "is the user looking at
//  it" checks need to know about apps, answered from memory:
//
//  - `RunningApps`: the regular apps running now, rebuilt off the main actor
//    only when an app launches or quits, and the frontmost app's pid, from
//    the activation notification. Nothing here reads LaunchServices on a
//    render or per hook event (a list row used to cost one XPC round trip
//    per running app per VS Code session).
//  - `SessionHostCache`: the app each Claude process runs under, resolved in
//    batches off the main actor (one process-table snapshot serves every
//    pid waiting) and kept until the process or its app goes away.
//

import AppKit
import Combine
import Foundation

@MainActor
final class RunningApps {
    static let shared = RunningApps()

    /// Regular apps, indexed. Empty until the first build finishes.
    private(set) var index: RunningAppIndex = .empty
    /// The frontmost app's pid.
    private(set) var frontmostPid: Int32?
    /// Bumped whenever `index` changes.
    @Published private(set) var revision = 0

    private var started = false
    private var observers: [NSObjectProtocol] = []
    private var rebuildTask: Task<Void, Never>?

    private init() {}

    /// Build the index and follow launches, quits and activations. Idempotent.
    func start() {
        guard !started else { return }
        started = true
        frontmostPid = NSWorkspace.shared.frontmostApplication?.processIdentifier
        let center = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didTerminateApplicationNotification] {
            observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.scheduleRebuild() }
            })
        }
        observers.append(center.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] note in
            let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
            let pid = app?.processIdentifier
            MainActor.assumeIsolated { self?.frontmostPid = pid }
        })
        scheduleRebuild(after: 0)
    }

    func stop() {
        let center = NSWorkspace.shared.notificationCenter
        observers.forEach(center.removeObserver)
        observers.removeAll()
        rebuildTask?.cancel()
        rebuildTask = nil
        started = false
    }

    /// The index now, building it first if nothing has yet (the first
    /// focus click can come before the launch build finished).
    func currentIndex() async -> RunningAppIndex {
        if revision == 0 {
            await rebuild()
        }
        return index
    }

    /// A running VS Code-family editor, the frontmost one first: where a
    /// `claude-vscode` session with no live process can still be opened.
    var runningEditorURL: URL? {
        let editors = index.apps.filter { $0.bundleIdentifier.map(TerminalAppRegistry.isEditorBundle) ?? false }
        let preferred = editors.first { $0.pid == frontmostPid }
            ?? editors.min { $0.pid < $1.pid }
        return preferred?.bundleURL
    }

    /// Launches often come in bursts (login items, a restored session).
    private func scheduleRebuild(after delay: TimeInterval = 0.5) {
        rebuildTask?.cancel()
        rebuildTask = Task { [weak self] in
            if delay > 0 {
                try? await Task.sleep(for: .seconds(delay))
                guard !Task.isCancelled else { return }
            }
            await self?.rebuild()
        }
    }

    private func rebuild() async {
        let index = await Task.detached(priority: .utility) { RunningAppIndex.current() }.value
        self.index = index
        revision += 1
    }
}

@MainActor
final class SessionHostCache {
    static let shared = SessionHostCache()

    private struct Entry {
        let host: HostApp?
        let resolvedAt: Date
    }

    /// A pid whose app wasn't found is looked up again after this long (its
    /// terminal may still have been launching).
    static let retryUnresolvedAfter: TimeInterval = 30

    /// Emits after a resolution pass changed what is known.
    let changes = PassthroughSubject<Void, Never>()

    private var entries: [Int: Entry] = [:]
    private var pending: Set<Int> = []
    private var resolving: Set<Int> = []
    private var passTask: Task<Void, Never>?
    private var appsRevision: AnyCancellable?

    private init() {}

    /// Forget hosts whose app quit whenever the set of apps changes.
    func start() {
        guard appsRevision == nil else { return }
        appsRevision = RunningApps.shared.$revision
            .dropFirst()
            .sink { [weak self] _ in
                Task { @MainActor in self?.dropHostsThatQuit() }
            }
    }

    func stop() {
        appsRevision = nil
        passTask?.cancel()
        passTask = nil
        pending.removeAll()
        resolving.removeAll()
        entries.removeAll()
    }

    /// What is known about `pid`: `.some(host)` once resolved (the host may
    /// be nil: no app found), nil while unknown, in which case a background
    /// lookup is queued. Never blocks.
    func host(forPid pid: Int, now: Date = Date()) -> HostApp?? {
        if let entry = entries[pid] {
            if entry.host == nil, now.timeIntervalSince(entry.resolvedAt) > Self.retryUnresolvedAfter {
                enqueue(pid)
            }
            return .some(entry.host)
        }
        enqueue(pid)
        return nil
    }

    /// The host of `pid`, resolving it now if it isn't known (a focus click).
    func resolve(pid: Int) async -> HostApp? {
        await resolve(pids: [pid])[pid] ?? nil
    }

    /// The hosts of `pids`: known ones from memory, the rest in one lookup
    /// (one process-table snapshot however many are missing).
    func resolve(pids: Set<Int>) async -> [Int: HostApp?] {
        var result: [Int: HostApp?] = [:]
        var missing: Set<Int> = []
        for pid in pids {
            if let entry = entries[pid], entry.host != nil {
                result[pid] = entry.host
            } else {
                missing.insert(pid)
            }
        }
        guard !missing.isEmpty else { return result }
        let apps = await RunningApps.shared.currentIndex()
        let found = await Self.lookUp(pids: missing, apps: apps)
        store(found)
        result.merge(found) { _, looked in looked }
        return result
    }

    /// Drop pids that aren't tracked any more.
    func retain(pids: Set<Int>) {
        let gone = Set(entries.keys).subtracting(pids)
        guard !gone.isEmpty else { return }
        for pid in gone { entries.removeValue(forKey: pid) }
    }

    private func enqueue(_ pid: Int) {
        guard pid > 0, !resolving.contains(pid) else { return }
        pending.insert(pid)
        guard passTask == nil else { return }
        passTask = Task { [weak self] in
            await self?.runPasses()
        }
    }

    /// One process-table snapshot per pass resolves every pending pid; pids
    /// asked for while a pass runs go into the next one.
    private func runPasses() async {
        while !pending.isEmpty, !Task.isCancelled {
            resolving = pending
            pending.removeAll()
            let apps = await RunningApps.shared.currentIndex()
            let found = await Self.lookUp(pids: resolving, apps: apps)
            resolving.removeAll()
            store(found)
        }
        passTask = nil
    }

    private func store(_ found: [Int: HostApp?]) {
        let now = Date()
        var changed = false
        for (pid, host) in found {
            if entries[pid]?.host != host || entries[pid] == nil { changed = true }
            entries[pid] = Entry(host: host, resolvedAt: now)
        }
        if changed { changes.send() }
    }

    private func dropHostsThatQuit() {
        let running = Set(RunningApps.shared.index.apps.map(\.pid))
        let before = entries.count
        entries = entries.filter { _, entry in entry.host.map { running.contains($0.pid) } ?? true }
        if entries.count != before { changes.send() }
    }

    @concurrent
    nonisolated private static func lookUp(pids: Set<Int>, apps: RunningAppIndex) async -> [Int: HostApp?] {
        let tree = ProcessTreeBuilder.shared.buildTree()
        var result: [Int: HostApp?] = [:]
        for pid in pids {
            result[pid] = SessionHostResolver.hostApp(forPid: pid, tree: tree, apps: apps)
        }
        return result
    }
}
