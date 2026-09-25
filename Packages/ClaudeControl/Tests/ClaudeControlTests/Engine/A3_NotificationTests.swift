import Foundation
import Testing
import UserNotifications
@testable import ClaudeControl

/// The notification delegate passes foreign notifications through
/// (Codenotch's thresholds and limits) and routes only its own.
struct A3_NotificationPassThroughTests {
    @Test func foreignNotificationsAreNotPresentedOrActedOn() {
        for identifier in ["codenotch.threshold.claude.80", "limit-reached-openai", "", "agentnotch", "needs-input.sess-1", "review.sess-1"] {
            #expect(!NotificationRouting.isOurs(identifier: identifier), "\(identifier)")
            #expect(NotificationRouting.presentationOptions(forIdentifier: identifier) == [], "\(identifier)")
            #expect(NotificationRouting.response(identifier: identifier, actionIdentifier: UNNotificationDefaultActionIdentifier) == nil)
            #expect(NotificationRouting.response(identifier: identifier, actionIdentifier: NotificationService.markReviewedAction) == nil)
        }
    }

    @Test func oursAreSilentBanners() {
        let options = NotificationRouting.presentationOptions(forIdentifier: "agentnotch.needsInput.abc")
        #expect(options.contains(.banner) && options.contains(.list))
        #expect(!options.contains(.sound) && !options.contains(.badge))
    }

    @Test func clicksOnOursRoute() {
        typealias R = NotificationRouting
        #expect(R.response(identifier: "agentnotch.needsInput.s-1", actionIdentifier: UNNotificationDefaultActionIdentifier) == .openSession("s-1"))
        #expect(R.response(identifier: "agentnotch.review.s-1", actionIdentifier: NotificationService.openAction) == .openSession("s-1"))
        #expect(R.response(identifier: "agentnotch.review.s-1", actionIdentifier: NotificationService.markReviewedAction) == .markReviewed("s-1"))
        #expect(R.response(identifier: "agentnotch.review.s-1", actionIdentifier: UNNotificationDismissActionIdentifier) == R.Response.none)
        #expect(R.response(identifier: "agentnotch.limit.claude-work", actionIdentifier: UNNotificationDefaultActionIdentifier) == .openRing("claude-work"))
        #expect(R.response(identifier: "agentnotch.unknown.x", actionIdentifier: UNNotificationDefaultActionIdentifier) == R.Response.none)
    }

    @Test func identifiersFollowTheDesign() throws {
        #expect(SessionNotificationContent.identifier(kind: .needsInput, sessionId: "a1") == "agentnotch.needsInput.a1")
        #expect(SessionNotificationContent.identifier(kind: .readyForReview, sessionId: "a1") == "agentnotch.review.a1")
        let parsed = try #require(SessionNotificationContent.parse(identifier: "agentnotch.review.a1.b2"))
        #expect(parsed.kind == .readyForReview && parsed.sessionId == "a1.b2")
        #expect(SessionNotificationContent.parse(identifier: "agentnotch.review.") == nil)
        #expect(SessionNotificationContent.parse(identifier: "agentnotch.limit.claude") == nil)
        #expect(LimitNotificationContent.parse(identifier: "agentnotch.limit.claude-work") == "claude-work")
        #expect(LimitNotificationContent.parse(identifier: "agentnotch.review.x") == nil)
        #expect(Set([NotificationService.needsInputCategory, NotificationService.reviewCategory, NotificationService.limitCategory]).count == 3)
    }
}

struct A3_NotificationContentTests {
    @Test func oneLimitBannerPerAccount() {
        let many = LimitNotificationContent.make(ringID: "claude-work", accountLabel: "Work",
                                                 sessionTitles: ["A", "B", "C"], limitReset: "resets 14:05")
        #expect(many.identifier == "agentnotch.limit.claude-work")
        #expect(many.title == "Work: 3 sessions hit the limit")
        #expect(many.body == "Rate limited · resets 14:05")
        let one = LimitNotificationContent.make(ringID: "claude", accountLabel: nil,
                                                sessionTitles: ["Refactor the parser"], limitReset: nil)
        #expect(one.title == "Refactor the parser hit the limit")
        #expect(one.body.hasPrefix("Rate limited"))
    }

    @Test func rateLimitedBodyNamesTheReset() {
        var state = SessionState(sessionId: "s", cwd: "/Users/me/app", phase: .waitingForInput)
        state.applyTitle("Refactor", source: .hook)
        let content = SessionNotificationContent.needsInput(session: state, reason: .error("Rate limited"),
                                                            accountLabel: nil, limitReset: "resets Thu 09:00")
        #expect(content.body == "Rate limited · resets Thu 09:00")
        // Other errors never carry a reset.
        let other = SessionNotificationContent.needsInput(session: state, reason: .error("Overloaded"),
                                                          accountLabel: nil, limitReset: "resets Thu 09:00")
        #expect(other.body == "Overloaded")
    }

    @Test func collidingAccountLabelsAreToldApart() {
        let personal = ClaudeAccount(configDir: "/Users/me/.claude", email: "me@x.dev", subscriptionType: "max")
        let team = ClaudeAccount(configDir: "/Users/me/.claude-team", configDirEnv: "/Users/me/.claude-team",
                                 email: "me@x.dev", organizationName: "Acme", subscriptionType: "team")
        let labels = [personal.id: "me@x.dev", team.id: "me@x.dev"]
        #expect(NotificationService.subtitle(for: team, label: "me@x.dev", allLabels: labels) == "me@x.dev · Acme")
        #expect(NotificationService.subtitle(for: personal, label: "me@x.dev", allLabels: labels) == "me@x.dev · Max")
        // Distinct names: the name alone; one account: nothing.
        let distinct = [personal.id: "Personal", team.id: "Work"]
        #expect(NotificationService.subtitle(for: team, label: "Work", allLabels: distinct) == "Work")
        #expect(NotificationService.subtitle(for: team, label: "Work", allLabels: [team.id: "Work"]) == nil)
    }
}
