//! Default names and badge letters for accounts, so two accounts never read
//! the same before anyone renames them (AccountNaming.swift, AU§6.1). Pure.
//!
//! Names follow upstream's rule for Claude rings: `Claude Gmail` after the
//! signed-in address's domain, `Claude (work)` after the folder when nobody
//! is signed in, and the whole address for the accounts whose short names
//! collide. Beyond upstream: the same address in two organizations adds the
//! organization (or plan), and anything still equal adds the folder.
//!
//! Badge letters come from the domain (`GM`, `AC`), which is what tells a
//! personal login from a work one; when two collide they fall back to the
//! address's own letters, the organization's, the folder's, then a digit.

use super::folder::Folder;
use crate::core::paths::Paths;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A name and its badge letters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    pub label: String,
    pub monogram: String,
}

/// Steps of lengthening a clashing name.
const MAX_LEVEL: u8 = 3;

/// Upstream's one word for an address: its domain's first label,
/// capitalised (`someone@acme.co.uk` → `Acme`). `None` when there is none
/// worth showing.
pub fn domain_label(address: Option<&str>) -> Option<String> {
    let address = address?;
    let at = address.rfind('@')?;
    let domain = &address[at + 1..];
    let first = domain.split('.').next()?;
    if first.is_empty() || !first.chars().any(char::is_alphabetic) {
        return None;
    }
    let mut chars = first.chars();
    let head = chars.next()?;
    Some(head.to_uppercase().chain(chars).collect())
}

/// The folder's own word: `work` for `~\.claude-work` or `~\.claude_work`,
/// the folder name without its leading dot otherwise; `None` for
/// `~\.claude`.
pub fn folder_word(folder: &Folder, paths: &Paths) -> Option<String> {
    let name = paths.file_name(folder.dir())?;
    if paths.names_equal(&name, ".claude") || paths.is_default_config_dir(folder.dir()) {
        return None;
    }
    for prefix in [".claude-", ".claude_"] {
        if paths.name_has_prefix(&name, prefix) && name.len() > prefix.len() {
            return Some(name[prefix.len()..].to_owned());
        }
    }
    let trimmed = name.strip_prefix('.').unwrap_or(&name);
    Some(if trimmed.is_empty() {
        name.clone()
    } else {
        trimmed.to_owned()
    })
}

/// The name before any collision is considered.
pub fn base_label(folder: &Folder, paths: &Paths) -> String {
    if let Some(email) = folder.email() {
        return match domain_label(Some(email)) {
            Some(word) => format!("Claude {word}"),
            None => format!("Claude {email}"),
        };
    }
    match folder_word(folder, paths) {
        Some(word) => format!("Claude ({word})"),
        None => "Claude".to_owned(),
    }
}

/// Two upper-case letters (or digits) from `text`, if it has any.
pub fn letters(text: &str) -> Option<String> {
    let picked: String = text
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect();
    (!picked.is_empty()).then(|| picked.to_uppercase())
}

/// Badge letters in order of preference.
pub fn monogram_candidates(folder: &Folder, paths: &Paths) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    let mut add = |text: Option<&str>| {
        if let Some(pair) = text.and_then(letters) {
            if !candidates.contains(&pair) {
                candidates.push(pair);
            }
        }
    };
    if let Some(custom) = folder.custom_label.as_deref().filter(|c| !c.is_empty()) {
        add(Some(custom));
    }
    if let Some(email) = folder.email() {
        add(domain_label(Some(email)).as_deref());
        // Swift's split drops empty pieces: `@x.dev` gives `x.dev`.
        add(email.split('@').find(|piece| !piece.is_empty()));
    }
    if let Some(organization) = folder.organization_name() {
        let initials: String = organization
            .split(|c: char| !c.is_alphanumeric())
            .filter_map(|word| word.chars().next())
            .collect();
        if initials.chars().count() >= 2 {
            add(Some(&initials));
        } else {
            add(Some(organization));
        }
    }
    add(folder.display_name());
    add(folder_word(folder, paths).as_deref());
    if candidates.is_empty() {
        candidates.push("CC".to_owned());
    }
    candidates
}

/// A distinct default name and badge for every folder, by id. Folders with a
/// custom name keep it (their badge still comes from it), but take part: a
/// default name that equals someone's custom name is lengthened too.
pub fn assign(folders: &[Folder], paths: &Paths) -> HashMap<String, Names> {
    let mut ordered: Vec<&Folder> = folders.iter().collect();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    let ids: Vec<&str> = ordered.iter().map(|f| f.dir()).collect();

    let mut labels: HashMap<&str, String> = HashMap::new();
    let mut level: HashMap<&str, u8> = HashMap::new();
    let custom: BTreeSet<&str> = ordered
        .iter()
        .filter(|f| f.custom_label.as_deref().is_some_and(|c| !c.is_empty()))
        .map(|f| f.dir())
        .collect();
    for folder in &ordered {
        let label = match folder.custom_label.as_deref().filter(|c| !c.is_empty()) {
            Some(custom) => custom.to_owned(),
            None => base_label(folder, paths),
        };
        labels.insert(folder.dir(), label);
        level.insert(folder.dir(), 0);
    }

    // Lengthen only the names that clash, one step at a time (a step with
    // nothing to add for a folder, like an address it doesn't have, leaves
    // its name as it was for the next one).
    for _ in 0..MAX_LEVEL {
        let clashing: BTreeSet<&str> = clashes(&labels, &ids)
            .into_iter()
            .filter(|id| !custom.contains(id))
            .collect();
        if clashing.is_empty() {
            break;
        }
        for folder in &ordered {
            if !clashing.contains(folder.dir()) {
                continue;
            }
            let next = level.get(folder.dir()).copied().unwrap_or(0) + 1;
            if let Some(longer) = label_at(folder, next, paths) {
                labels.insert(folder.dir(), longer);
            }
            level.insert(folder.dir(), next);
        }
    }

    let preferred: HashMap<&str, Vec<String>> = ordered
        .iter()
        .map(|f| (f.dir(), monogram_candidates(f, paths)))
        .collect();
    let options_of = |id: &str| -> Vec<String> {
        preferred
            .get(id)
            .cloned()
            .unwrap_or_else(|| vec!["CC".to_owned()])
    };
    let mut monograms: HashMap<&str, String> = HashMap::new();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let pick = |options: &[String], taken: &BTreeSet<String>| -> String {
        if let Some(free) = options.iter().find(|o| !taken.contains(*o)) {
            return free.clone();
        }
        let lead: String = options
            .first()
            .map(String::as_str)
            .unwrap_or("C")
            .chars()
            .take(1)
            .collect();
        (1..=99)
            .map(|n| format!("{lead}{n}"))
            .find(|candidate| !taken.contains(candidate))
            .unwrap_or(lead)
    };
    // A name the user chose keeps its own letters: they pick first (a second
    // custom name with the same letters takes its next choice).
    for folder in &ordered {
        if custom.contains(folder.dir()) {
            let chosen = pick(&options_of(folder.dir()), &taken);
            taken.insert(chosen.clone());
            monograms.insert(folder.dir(), chosen);
        }
    }
    let defaults: Vec<&&Folder> = ordered
        .iter()
        .filter(|f| !custom.contains(f.dir()))
        .collect();
    let first_choices: Vec<String> = defaults
        .iter()
        .map(|f| {
            options_of(f.dir())
                .into_iter()
                .next()
                .unwrap_or_else(|| "CC".to_owned())
        })
        .collect();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for first in &first_choices {
        *counts.entry(first.as_str()).or_default() += 1;
    }
    // Folders whose first choice is theirs alone (and free) keep it.
    for (folder, first) in defaults.iter().zip(&first_choices) {
        if counts.get(first.as_str()).copied().unwrap_or(0) <= 1 && !taken.contains(first) {
            taken.insert(first.clone());
            monograms.insert(folder.dir(), first.clone());
        }
    }
    // The rest avoid the letters they would have shared: their other
    // candidates first, the shared one only if nothing else is free.
    for folder in &defaults {
        if monograms.contains_key(folder.dir()) {
            continue;
        }
        let options = options_of(folder.dir());
        let mut reordered: Vec<String> = options.iter().skip(1).cloned().collect();
        reordered.extend(options.first().cloned());
        let chosen = pick(&reordered, &taken);
        taken.insert(chosen.clone());
        monograms.insert(folder.dir(), chosen);
    }

    ordered
        .iter()
        .map(|folder| {
            let names = Names {
                label: labels
                    .get(folder.dir())
                    .cloned()
                    .unwrap_or_else(|| base_label(folder, paths)),
                monogram: monograms
                    .get(folder.dir())
                    .cloned()
                    .unwrap_or_else(|| "CC".to_owned()),
            };
            (folder.dir().to_owned(), names)
        })
        .collect()
}

/// The name at one step of lengthening, or `None` when that step adds
/// nothing.
fn label_at(folder: &Folder, level: u8, paths: &Paths) -> Option<String> {
    let email = folder.email();
    let place = paths.abbreviate(folder.dir());
    match level {
        1 => email.map(|e| format!("Claude {e}")),
        2 => {
            let email = email?;
            if let Some(organization) = folder.organization_name() {
                return Some(format!("Claude {email} · {organization}"));
            }
            folder
                .plan_name()
                .map(|plan| format!("Claude {email} · {plan}"))
        }
        3 => Some(match email {
            Some(email) => format!("Claude {email} · {place}"),
            None => format!("Claude ({place})"),
        }),
        _ => None,
    }
}

/// The ids whose names equal another's, ignoring case.
fn clashes<'a>(labels: &HashMap<&str, String>, ids: &[&'a str]) -> BTreeSet<&'a str> {
    let mut by_name: BTreeMap<String, Vec<&'a str>> = BTreeMap::new();
    for id in ids {
        let name = labels.get(id).map(|l| l.to_lowercase()).unwrap_or_default();
        by_name.entry(name).or_default().push(id);
    }
    by_name
        .into_values()
        .filter(|members| members.len() > 1)
        .flatten()
        .collect()
}

/// Every folder with its default name and badge among `folders`. Only
/// tracked folders count for collisions; an untracked one is named as if it
/// joined them.
pub fn named(folders: &[Folder], paths: &Paths) -> Vec<Folder> {
    let tracked: Vec<Folder> = folders.iter().filter(|f| !f.is_hidden).cloned().collect();
    let mut names = assign(&tracked, paths);
    for folder in folders.iter().filter(|f| f.is_hidden) {
        let mut with_it = tracked.clone();
        with_it.push(folder.clone());
        if let Some(name) = assign(&with_it, paths).remove(folder.dir()) {
            names.insert(folder.dir().to_owned(), name);
        }
    }
    folders
        .iter()
        .map(|folder| {
            let mut copy = folder.clone();
            let name = names.get(folder.dir());
            copy.default_label = name.map(|n| n.label.clone());
            copy.default_monogram = name.map(|n| n.monogram.clone());
            copy
        })
        .collect()
}

/// Labels compared the way the Mac's `localizedCaseInsensitiveCompare`
/// orders them (case folded), then as written.
pub fn compare_labels(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}
