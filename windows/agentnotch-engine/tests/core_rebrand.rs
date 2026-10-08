//! `core::rebrand` against the Mac's `RebrandTests` and the vectors shared
//! with `ui/agentnotch/rebrand.js`.

use agentnotch_engine::core::rebrand::{
    names_upstream, names_upstream_product, rebranded, rebranded_with, UPSTREAM_PRODUCT_PHRASES,
};
use serde_json::Value;

/// A catalog string as `L10n.t` hands it over: `text` is also the template
/// (no arguments), `key` the English catalog key.
fn lookup(text: &str, language: &str, key: &str) -> String {
    rebranded_with(text, language, key, || text.to_owned())
}

#[test]
fn the_shared_vectors() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/ui-contract/rebrand-vectors.json");
    let vectors: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let vectors = vectors.as_array().unwrap();
    assert!(vectors.len() >= 10);
    for v in vectors {
        let (input, expected) = (
            v["input"].as_str().unwrap(),
            v["expected"].as_str().unwrap(),
        );
        assert_eq!(rebranded(input), expected, "{input}");
    }
}

#[test]
fn lookups_name_this_app() {
    assert_eq!(rebranded("Quit Codenotch"), "Quit Agent Notch");
    assert_eq!(
        rebranded("Signs out of MiniMax — the session belongs to Codenotch."),
        "Signs out of MiniMax — the session belongs to Agent Notch."
    );
    assert_eq!(rebranded("Refresh all"), "Refresh all");
    assert_eq!(rebranded(""), "");
}

/// A folder, device or account the user named is their data, not upstream's
/// copy: the template (before its arguments) doesn't name Codenotch.
#[test]
fn arguments_keep_their_name() {
    for folder in ["Codenotch", "Codenotch-main", "MyCodenotchFork"] {
        let text = format!("Working in {folder}");
        assert_eq!(
            rebranded_with(&text, "en", "Working in %@", || "Working in %@".to_owned()),
            text
        );
    }
}

#[test]
fn english_takes_an_before_the_name() {
    let key = "Most readings are borrowed from a tool that already holds the account. DeepSeek and MiniMax are the exceptions: clicking Sign in opens a Codenotch window for that account, and signing out here clears only that session and its saved reading.";
    assert!(lookup(key, "en", key).contains("opens an Agent Notch window"));
    // A language without a translation of the key shows the English, fixed the same way.
    assert!(lookup(key, "fr", key).contains("opens an Agent Notch window"));
    assert_eq!(
        lookup("A Codenotch ring", "en", "A Codenotch ring"),
        "An Agent Notch ring"
    );
    assert_eq!(
        lookup("a Codenotch ring", "en-US", "a Codenotch ring"),
        "an Agent Notch ring"
    );
}

#[test]
fn the_phone_app_and_upstreams_windows_build_keep_their_name() {
    for text in [
        "Scan this code with the Codenotch app on your phone.",
        "1. Open Codenotch on your phone",
        "Codenotch for Windows, installable",
    ] {
        assert_eq!(lookup(text, "en", text), text);
    }
    // A translation follows its English source.
    assert_eq!(
        lookup(
            "1. Buka Codenotch di ponsel",
            "id",
            "1. Open Codenotch on your phone"
        ),
        "1. Buka Codenotch di ponsel"
    );
}

#[test]
fn translations_read_naturally() {
    let cases = [
        (
            "Réglages de Codenotch",
            "fr",
            "Codenotch Settings",
            "Réglages d'Agent Notch",
        ),
        (
            "Ce que Codenotch vous dit, et quand.",
            "fr",
            "What Codenotch tells you, and when.",
            "Ce qu'Agent Notch vous dit, et quand.",
        ),
        (
            "Quitter Codenotch",
            "fr",
            "Quit Codenotch",
            "Quitter Agent Notch",
        ),
        (
            "Codenotch-Einstellungen",
            "de",
            "Codenotch Settings",
            "Agent-Notch-Einstellungen",
        ),
        (
            "Codenotch'tan Çık",
            "tr",
            "Quit Codenotch",
            "Agent Notch'tan Çık",
        ),
        (
            "lorsque Codenotch démarre",
            "fr",
            "when Codenotch starts",
            "lorsqu'Agent Notch démarre",
        ),
        (
            "chaque Codenotch",
            "fr",
            "each Codenotch",
            "chaque Agent Notch",
        ),
        (
            "presque Codenotch",
            "fr",
            "almost Codenotch",
            "presque Agent Notch",
        ),
        // Only French elides.
        (
            "Configurações de Codenotch",
            "pt-BR",
            "Codenotch Settings",
            "Configurações de Agent Notch",
        ),
    ];
    for (text, language, key, expected) in cases {
        assert_eq!(lookup(text, language, key), expected, "{language}: {text}");
    }
}

#[test]
fn the_template_is_only_asked_for_when_the_text_names_upstream() {
    let mut asked = false;
    assert_eq!(
        rebranded_with("Refresh all", "en", "Refresh all", || {
            asked = true;
            String::new()
        }),
        "Refresh all"
    );
    assert!(!asked);
    // Nor for a product that keeps its name.
    assert_eq!(
        rebranded_with(
            "Codenotch for Windows",
            "en",
            "Codenotch for Windows",
            || {
                asked = true;
                String::new()
            }
        ),
        "Codenotch for Windows"
    );
    assert!(!asked);
}

#[test]
fn what_names_upstream_and_its_products() {
    assert!(names_upstream("x Codenotch y"));
    assert!(!names_upstream("codenotch"));
    for phrase in UPSTREAM_PRODUCT_PHRASES {
        assert!(
            names_upstream_product(&format!("see {phrase}.")),
            "{phrase}"
        );
    }
    assert!(!names_upstream_product("Quit Codenotch"));
}

/// Every string of upstream's catalog, in every language: none names
/// Codenotch afterwards unless its English names the phone app or upstream's
/// Windows build, and those are left exactly as they were.
#[test]
fn every_catalog_string_names_this_app() {
    let catalog = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../Sources/Localizable.xcstrings");
    let catalog: Value = serde_json::from_slice(
        &std::fs::read(&catalog).unwrap_or_else(|e| panic!("{}: {e}", catalog.display())),
    )
    .unwrap();
    let strings = catalog["strings"].as_object().unwrap();
    let article = regex::Regex::new(r"\b[aA] Agent Notch").unwrap();
    let (mut checked, mut kept) = (0, 0);
    for (key, entry) in strings {
        let localizations = entry["localizations"].as_object();
        let value = |language: &str| -> Option<String> {
            localizations?.get(language)?["stringUnit"]["value"]
                .as_str()
                .map(str::to_owned)
        };
        let source = value("en").unwrap_or_else(|| key.clone());
        let mut texts = vec![("en".to_owned(), source.clone())];
        for language in localizations.into_iter().flat_map(|l| l.keys()) {
            if language != "en" {
                if let Some(text) = value(language) {
                    texts.push((language.clone(), text));
                }
            }
        }
        for (language, text) in texts.iter().filter(|(_, t)| t.contains("Codenotch")) {
            let result = lookup(text, language, &source);
            checked += 1;
            if names_upstream_product(&source) {
                kept += 1;
                assert_eq!(&result, text, "{language}: {key}");
            } else {
                assert!(!result.contains("Codenotch"), "{language}: {result}");
                assert!(
                    result.contains("Agent Notch") || result.contains("Agent-Notch"),
                    "{language}: {result}"
                );
                if language == "en" {
                    assert!(!article.is_match(&result), "{result}");
                }
                if language == "fr" {
                    assert!(
                        !result.contains("de Agent Notch") && !result.contains("que Agent Notch"),
                        "{result}"
                    );
                }
            }
        }
    }
    assert!(checked > 500, "{checked}");
    assert!(kept > 0);
}
