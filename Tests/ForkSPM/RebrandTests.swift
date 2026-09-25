import Foundation
import Testing
@testable import Codenotch

/// Upstream's copy calls the app "Codenotch"; everything `L10n.t` returns
/// names this app instead (seam R1), except copy about products that really
/// are called Codenotch.
@Suite struct RebrandTests {
    private let english = Locale(identifier: "en")

    /// A catalog string as `L10n.t` hands it over: `text` is also the
    /// template (no arguments), `key` the English catalog key.
    private func rebranded(_ text: String, _ language: String, english key: String) -> String {
        Fork.rebranded(text, locale: Locale(identifier: language), key: key, template: { text })
    }

    @Test func lookupsNameThisApp() {
        #expect(L10n.t("Quit Codenotch", locale: english) == "Quit Agent Notch")
        let name = "MiniMax"
        #expect(L10n.t("Signs out of \(name) — the session belongs to Codenotch.", locale: english)
            == "Signs out of MiniMax — the session belongs to Agent Notch.")
        #expect(L10n.t("Refresh all", locale: english) == "Refresh all")
    }

    /// A folder, device or account the user named is their data, not
    /// upstream's copy.
    @Test func argumentsKeepTheirName() {
        for folder in ["Codenotch", "Codenotch-main", "MyCodenotchFork"] {
            #expect(L10n.t("Working in \(folder)", locale: english) == "Working in \(folder)")
        }
    }

    @Test func englishTakesAnBeforeTheName() {
        let key = "Most readings are borrowed from a tool that already holds the account. DeepSeek and MiniMax are the exceptions: clicking Sign in opens a Codenotch window for that account, and signing out here clears only that session and its saved reading."
        #expect(L10n.t("Most readings are borrowed from a tool that already holds the account. DeepSeek and MiniMax are the exceptions: clicking Sign in opens a Codenotch window for that account, and signing out here clears only that session and its saved reading.", locale: english)
            .contains("opens an Agent Notch window"))
        // A language without a translation of the key shows the English, fixed the same way.
        #expect(rebranded(key, "fr", english: key).contains("opens an Agent Notch window"))
        #expect(rebranded("A Codenotch ring", "en", english: "A Codenotch ring") == "An Agent Notch ring")
    }

    @Test func thePhoneAppAndUpstreamsWindowsBuildKeepTheirName() {
        #expect(L10n.t("Scan this code with the Codenotch app on your phone.", locale: english)
            == "Scan this code with the Codenotch app on your phone.")
        #expect(L10n.t("1. Open Codenotch on your phone", locale: english) == "1. Open Codenotch on your phone")
        #expect(L10n.t("Codenotch for Windows, installable", locale: english) == "Codenotch for Windows, installable")
        // A translation follows its English source.
        #expect(rebranded("1. Buka Codenotch di ponsel", "id", english: "1. Open Codenotch on your phone")
            == "1. Buka Codenotch di ponsel")
    }

    @Test func translationsReadNaturally() {
        #expect(rebranded("Réglages de Codenotch", "fr", english: "Codenotch Settings") == "Réglages d'Agent Notch")
        #expect(rebranded("Ce que Codenotch vous dit, et quand.", "fr", english: "What Codenotch tells you, and when.")
            == "Ce qu'Agent Notch vous dit, et quand.")
        #expect(rebranded("Quitter Codenotch", "fr", english: "Quit Codenotch") == "Quitter Agent Notch")
        #expect(rebranded("Codenotch-Einstellungen", "de", english: "Codenotch Settings") == "Agent-Notch-Einstellungen")
        #expect(rebranded("Codenotch'tan Çık", "tr", english: "Quit Codenotch") == "Agent Notch'tan Çık")
        #expect(rebranded("lorsque Codenotch démarre", "fr", english: "when Codenotch starts") == "lorsqu'Agent Notch démarre")
        #expect(rebranded("chaque Codenotch", "fr", english: "each Codenotch") == "chaque Agent Notch")
        #expect(rebranded("presque Codenotch", "fr", english: "almost Codenotch") == "presque Agent Notch")
        // Only French elides.
        #expect(rebranded("Configurações de Codenotch", "pt-BR", english: "Codenotch Settings")
            == "Configurações de Agent Notch")
    }

    @Test func theTemplateIsOnlyAskedForWhenTheTextNamesUpstream() {
        var asked = false
        #expect(Fork.rebranded("Refresh all", locale: english, key: "Refresh all", template: { asked = true; return "" })
            == "Refresh all")
        #expect(!asked)
    }

    /// Every string of upstream's catalog, in every language: none names
    /// Codenotch afterwards unless its English names the phone app or
    /// upstream's Windows build, and those are left exactly as they were.
    @Test func everyCatalogStringNamesThisApp() throws {
        let catalogURL = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/Localizable.xcstrings")
        let catalog = try #require(try JSONSerialization.jsonObject(with: Data(contentsOf: catalogURL)) as? [String: Any])
        let strings = try #require(catalog["strings"] as? [String: [String: Any]])
        var checked = 0
        var kept = 0
        for (key, entry) in strings {
            let localizations = entry["localizations"] as? [String: [String: Any]] ?? [:]
            func value(_ language: String) -> String? {
                (localizations[language]?["stringUnit"] as? [String: Any])?["value"] as? String
            }
            let source = value("en") ?? key
            var texts = [("en", source)]
            for language in localizations.keys where language != "en" {
                if let text = value(language) { texts.append((language, text)) }
            }
            for (language, text) in texts where text.contains("Codenotch") {
                let result = rebranded(text, language, english: source)
                checked += 1
                if Fork.namesUpstreamProduct(source) {
                    kept += 1
                    #expect(result == text, "\(language): \(key)")
                } else {
                    #expect(!result.contains("Codenotch"), "\(language): \(result)")
                    #expect(result.contains("Agent Notch") || result.contains("Agent-Notch"), "\(language): \(result)")
                    if language == "en" {
                        #expect(result.range(of: #"\b[aA] Agent Notch"#, options: .regularExpression) == nil, "\(result)")
                    }
                    if language == "fr" {
                        #expect(!result.contains("de Agent Notch") && !result.contains("que Agent Notch"), "\(result)")
                    }
                }
            }
        }
        #expect(checked > 500)
        #expect(kept > 0)
    }
}
