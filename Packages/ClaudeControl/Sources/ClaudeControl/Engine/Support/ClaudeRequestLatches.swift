//
//  ClaudeRequestLatches.swift
//  ClaudeControl
//
//  Two small Combine helpers for requests that may be sent before anyone
//  listens (a settings pane chosen while its window is still being built).
//  Generic, so kept apart from the panel policy they started in (CS-13).
//
//  Owned by WP-D.
//

import Combine
import Foundation

/// A request that has to reach a listener which may not exist yet: sent now
/// when someone listens, otherwise held for the first listener that arrives
/// within `lifetime`. Opening Codenotch's settings window builds its view
/// after the request to select a pane has already been sent, so a plain
/// subject lost the first request.
public nonisolated struct ClaudeRequestLatch<Value> {
    public var lifetime: TimeInterval
    public private(set) var pending: (value: Value, at: Date)?

    public init(lifetime: TimeInterval = 5) {
        self.lifetime = lifetime
    }

    /// A new request: returned to deliver now when there are listeners,
    /// otherwise held (replacing any older one) and nil.
    public mutating func offer(_ value: Value, at now: Date, hasListeners: Bool) -> Value? {
        if hasListeners {
            pending = nil
            return value
        }
        pending = (value, now)
        return nil
    }

    /// A listener arrived: the held request if it is still fresh. Taken once.
    public mutating func take(at now: Date) -> Value? {
        defer { pending = nil }
        guard let pending, now.timeIntervalSince(pending.at) <= lifetime else { return nil }
        return pending.value
    }
}

extension ClaudeRequestLatch: Sendable where Value: Sendable {}

/// `ClaudeRequestLatch` as a publisher: `send` delivers to the subscribers
/// there are, or, with none, holds the request for the first one to
/// subscribe (if it arrives within the latch's lifetime). SettingsView
/// subscribes to the settings-pane requests this way, so the request that
/// opened its window is not lost while the window is being built.
@MainActor
public final class ClaudeHeldRequests<Value> {
    private let subject = PassthroughSubject<Value, Never>()
    private var latch: ClaudeRequestLatch<Value>
    private let clock: () -> Date
    /// Subscribers right now.
    public private(set) var listeners = 0

    public init(lifetime: TimeInterval = 5, clock: @escaping () -> Date = Date.init) {
        latch = ClaudeRequestLatch(lifetime: lifetime)
        self.clock = clock
    }

    /// Delivers on the main thread, synchronously with `send` and with the
    /// subscription; a held request comes first, once.
    public lazy var publisher: AnyPublisher<Value, Never> = Deferred { [weak self] () -> AnyPublisher<Value, Never> in
        MainActor.assumeIsolated {
            guard let self else { return Empty().eraseToAnyPublisher() }
            let live = self.subject
                .handleEvents(receiveSubscription: { [weak self] _ in
                                  MainActor.assumeIsolated { self?.listeners += 1 }
                              },
                              receiveCancel: { [weak self] in
                                  MainActor.assumeIsolated { self?.listeners -= 1 }
                              })
                .eraseToAnyPublisher()
            guard let held = self.latch.take(at: self.clock()) else { return live }
            return Just(held).append(live).eraseToAnyPublisher()
        }
    }
    .eraseToAnyPublisher()

    public func send(_ value: Value) {
        if let now = latch.offer(value, at: clock(), hasListeners: listeners > 0) {
            subject.send(now)
        }
    }

    // Swift 6.3's optimizer crashes on this class's implicit deinit (signal
    // 11 in EarlyPerfInliner, release builds only; 6.4 is fine), which breaks
    // the Release workflow's Xcode 26 build. Nothing to clean up here.
    @_optimize(none)
    deinit {}
}
