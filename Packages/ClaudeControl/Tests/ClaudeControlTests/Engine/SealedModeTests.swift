import Testing
@testable import ClaudeControl

@Suite struct SealedModeTests {
    @Test func offWithoutEitherVariable() {
        #expect(!SealedMode.isOn(environment: [:]))
        #expect(!SealedMode.isOn(environment: ["PATH": "/usr/bin"]))
    }

    @Test(arguments: ["SPCN_SAFE_MODE", "CODENOTCH_DEMO"])
    func onWhenSetToOne(key: String) {
        #expect(SealedMode.isOn(environment: [key: "1"]))
    }

    /// S3/CS-11: the safety switch fails closed. Only an empty value or a
    /// clear "off" leaves the run live.
    @Test(arguments: ["0", "", "  ", "false", "FALSE", "no", "off"])
    func onlyAClearOffLeavesTheRunLive(value: String) {
        #expect(!SealedMode.isOn(environment: ["SPCN_SAFE_MODE": value]))
        #expect(!SealedMode.isOn(environment: ["CODENOTCH_DEMO": value]))
    }

    @Test(arguments: ["1", "true", "TRUE", "yes", " Yes ", "on", "2", "sealed", "ture"])
    func anyOtherValueSeals(value: String) {
        #expect(SealedMode.isOn(environment: ["SPCN_SAFE_MODE": value]))
        #expect(SealedMode.isOn(environment: ["CODENOTCH_DEMO": value]))
    }

    @Test func anOddValueIsReportedAndAClearOneIsNot() {
        #expect(SealedMode.unrecognised(environment: ["SPCN_SAFE_MODE": "ture"]) == "SPCN_SAFE_MODE=ture")
        #expect(SealedMode.unrecognised(environment: ["SPCN_SAFE_MODE": "true"]) == nil)
        #expect(SealedMode.unrecognised(environment: ["SPCN_SAFE_MODE": "0"]) == nil)
        #expect(SealedMode.unrecognised(environment: [:]) == nil)
    }

    /// The badge (UI/) builds on the main actor.
    @Test func badgeBuilds() {
        _ = SealedModeBadge().body
    }
}
