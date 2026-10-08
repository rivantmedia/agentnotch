//
//  ChatStatusLine.swift
//  ClaudeControl
//
//  The one line a chat shows under its last message when Claude is not
//  working: the turn failed, it is ready for review, or the session is idle.
//  It speaks with the session row's words (`SessionRowContent`), so the row
//  and the chat never disagree about the same session.
//

import Foundation

nonisolated struct ChatStatusLine: Equatable, Sendable {
    /// The row's mark for this state (the view maps it to a colour).
    let glyph: SessionGlyphKind
    let text: String
    /// A failed turn can be dismissed from the chat as it can from the row.
    let canDismiss: Bool

    /// Nil while the session is working (the chat's working indicator says
    /// so, and the text must not appear twice) and while it needs an answer:
    /// the chat's bottom bar already shows the request, question, plan or
    /// terminal-only dialog, with its own words and buttons.
    static func make(
        for session: SessionState,
        rateLimit: RateLimitReset?,
        now: Date,
        home: String = AccountPaths.homeDirectory
    ) -> ChatStatusLine? {
        let glyph = SessionRowContent.glyph(for: session)
        let elapsed = SessionRowContent.elapsed(for: session, now: now)
        switch session.attention {
        case .working:
            return nil
        case .needsInput(let reason):
            guard reason.isError else { return nil }
            // The row's detail for a failure: what happened and, for a rate
            // limit, when it lifts.
            let detail = SessionRowContent.detail(for: session, rateLimit: rateLimit, now: now, home: home)
            return ChatStatusLine(glyph: glyph, text: detail.plainText, canDismiss: true)
        case .readyForReview:
            return ChatStatusLine(glyph: glyph, text: join(glyph.spokenState, elapsed.map { "finished \($0)" }), canDismiss: false)
        case .idle:
            return ChatStatusLine(glyph: glyph, text: join(glyph.spokenState, elapsed.map { "last active \($0)" }), canDismiss: false)
        }
    }

    private static func join(_ lead: String, _ rest: String?) -> String {
        [lead, rest].compactMap { $0 }.joined(separator: " · ")
    }
}
