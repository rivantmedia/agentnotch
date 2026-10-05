//! What a settings.json should become (the pure half of HookInstaller.swift:
//! `planInstall`, `planUninstall`, the status line planning). Given the
//! bytes on disk, a plan either refuses, finds nothing to change, or holds
//! the new bytes; no file is touched here, so every rule can be exercised
//! without a config folder.
//!
//! settings.json is the user's file, so a plan:
//! - refuses a file that isn't a JSON object, or whose `hooks` isn't one;
//! - changes only `hooks` and `statusLine`, by splicing (`SettingsDocument`);
//! - compares as JSON, so a file someone merely reformatted is not rewritten;
//! - touches only entries that run this app's hook exe (`Recogniser`): other
//!   tools' entries, the official Codenotch's included, keep their place.

use super::commands::{is_upstream_hook, takeover, Recogniser, Takeover};
use super::events::event_groups;
use crate::core::settings_doc::{Json, Member, SettingsDocument};
use crate::runtime_types::{CommandForm, StatusLineIntent};

/// What the saved-status-line file holds when ours chains to nothing. Unlike
/// a missing file (a lost one, which the backups stand in for), this says the
/// user had no status line, so an older one in a backup never comes back.
pub const CHAINS_NOTHING: &[u8] = b"{}\n";

/// A change to the saved previous-status-line file that goes with a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviousStatusLineChange {
    /// Write these bytes (the `statusLine` object being taken over) before
    /// settings.json stops pointing at it.
    Save(Vec<u8>),
    /// Write these bytes where the saved file was lost: what the backups say
    /// ours chains to. Applies even when settings.json doesn't change, or
    /// the running wrapper would keep chaining nothing.
    Recover(Vec<u8>),
    /// Write the empty object: ours chains to nothing.
    ChainNothing,
    /// Delete the file once settings.json is written (the status line was
    /// given back, so there is nothing left to chain).
    Remove,
}

/// Why a plan refuses to touch settings.json.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not a JSON object (truncated, an array, binary garbage, …).
    Unreadable,
    /// `hooks` exists but isn't an object.
    HooksNotAnObject,
}

impl Refusal {
    /// What Settings shows.
    pub fn message(self) -> &'static str {
        match self {
            Refusal::Unreadable => {
                "settings.json isn't valid JSON, so it was left alone. Fix it and try again."
            }
            Refusal::HooksNotAnObject => {
                "settings.json has a \"hooks\" value that isn't an object, so it was left alone."
            }
        }
    }
}

/// What a plan decided for settings.json itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsWrite {
    Write(Vec<u8>),
    AlreadyCurrent,
    Refuse(Refusal),
}

/// A planned change to one settings.json.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsPlan {
    pub settings: SettingsWrite,
    pub previous_status_line: Option<PreviousStatusLineChange>,
    /// What was decided for the status line that is there: wrapped, kept
    /// current, given back, left alone (with the reason), or not looked at.
    pub status_line: StatusLineIntent,
}

impl SettingsPlan {
    fn refuse(refusal: Refusal) -> SettingsPlan {
        SettingsPlan {
            settings: SettingsWrite::Refuse(refusal),
            previous_status_line: None,
            status_line: StatusLineIntent::Nothing,
        }
    }

    fn nothing() -> SettingsPlan {
        SettingsPlan {
            settings: SettingsWrite::AlreadyCurrent,
            previous_status_line: None,
            status_line: StatusLineIntent::Nothing,
        }
    }
}

/// What to do with the folder's `statusLine` during an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLineWish<'a> {
    /// Make ours the status line where that is safe (`takeover`), saving
    /// whatever was there to chain to.
    Wrap { command: &'a str, git_bash: bool },
    /// If ours is the status line, put the saved one back.
    Unwrap,
    /// Don't touch the status line.
    Leave,
}

/// What a plan is given besides the bytes: the status line saved by an
/// earlier install (only consulted while ours is the status line), and the
/// newest other status line in our backups, for when the saved one is gone
/// (read only when it is needed).
pub struct Saved<'a> {
    pub previous_status_line: Option<&'a Json>,
    pub backup_status_line: &'a dyn Fn() -> Option<Json>,
}

impl Saved<'_> {
    /// Nothing saved, no backups.
    pub const NONE: Saved<'static> = Saved {
        previous_status_line: None,
        backup_status_line: &|| None,
    };
}

/// The settings.json wanted for an install, or a refusal.
///
/// `existing` is the file's bytes, or `None` when there is no file yet (the
/// only case where starting from `{}` is right).
pub fn plan_install(
    existing: Option<&[u8]>,
    form: &CommandForm,
    events: &[String],
    status_line: StatusLineWish,
    saved: &Saved,
    recogniser: &Recogniser,
) -> SettingsPlan {
    let Some(mut document) = SettingsDocument::new(existing) else {
        return SettingsPlan::refuse(Refusal::Unreadable);
    };
    let original = document.value().clone();
    let Some(before) = hooks_object(&document) else {
        return SettingsPlan::refuse(Refusal::HooksNotAnObject);
    };

    // Ours come out of every event first, even events no longer registered,
    // so stale keys from an older Claude Code never linger and entries in
    // another form are replaced, not kept beside the new ones. Only entries
    // running our exe are touched.
    let (mut hooks, _) = removing_hooks(&before, &|entry| recogniser.is_our_hook(entry));
    for event in events {
        let Some(groups) = event_groups(event, form) else {
            continue;
        };
        // An event holding something other than a list of matcher groups is
        // malformed; it is left for the user rather than overwritten.
        let mut all = match hooks.get(event) {
            Some(value) => match value.items() {
                Some(items) => items.to_vec(),
                None => continue,
            },
            None => Vec::new(),
        };
        all.extend(groups);
        hooks.set(event, Some(Json::Array(all)));
    }
    replace_hooks(&before, hooks, &mut document);

    let (change, decided) = apply_status_line_wish(status_line, &mut document, saved, recogniser);
    finish(document, &original, existing, change, decided)
}

/// The settings.json without our hooks and with the previous status line
/// back.
pub fn plan_uninstall(
    existing: Option<&[u8]>,
    saved: &Saved,
    recogniser: &Recogniser,
) -> SettingsPlan {
    if existing.is_none() {
        return SettingsPlan::nothing();
    }
    let Some(mut document) = SettingsDocument::new(existing) else {
        return SettingsPlan::refuse(Refusal::Unreadable);
    };
    let original = document.value().clone();
    let Some(before) = hooks_object(&document) else {
        return SettingsPlan::refuse(Refusal::HooksNotAnObject);
    };
    let (hooks, _) = removing_hooks(&before, &|entry| recogniser.is_our_hook(entry));
    replace_hooks(&before, hooks, &mut document);

    let (change, decided) =
        apply_status_line_wish(StatusLineWish::Unwrap, &mut document, saved, recogniser);
    finish(document, &original, existing, change, decided)
}

/// The settings.json without our hook entries, everything else as it is (the
/// status line included), for a folder no hook command can name any more.
/// An exec-form entry left there would be run by an older Claude Code as a
/// bare string, which Git Bash can fail to parse with exit 2: a block. With
/// none of ours there, nothing is written, not even to tidy up.
pub fn plan_hook_removal(existing: Option<&[u8]>, recogniser: &Recogniser) -> SettingsPlan {
    if existing.is_none() {
        return SettingsPlan::nothing();
    }
    let Some(mut document) = SettingsDocument::new(existing) else {
        return SettingsPlan::refuse(Refusal::Unreadable);
    };
    let original = document.value().clone();
    let Some(before) = hooks_object(&document) else {
        return SettingsPlan::refuse(Refusal::HooksNotAnObject);
    };
    let (hooks, removed) = removing_hooks(&before, &|entry| recogniser.is_our_hook(entry));
    if removed == 0 {
        return SettingsPlan::nothing();
    }
    replace_hooks(&before, hooks, &mut document);
    finish(
        document,
        &original,
        existing,
        None,
        StatusLineIntent::Nothing,
    )
}

/// The settings.json without the official Codenotch's hook entries, and how
/// many went. Its status line is never its own, so nothing else changes.
pub fn plan_upstream_removal(existing: Option<&[u8]>) -> (SettingsPlan, u32) {
    if existing.is_none() {
        return (SettingsPlan::nothing(), 0);
    }
    let Some(mut document) = SettingsDocument::new(existing) else {
        return (SettingsPlan::refuse(Refusal::Unreadable), 0);
    };
    let original = document.value().clone();
    let Some(before) = hooks_object(&document) else {
        return (SettingsPlan::refuse(Refusal::HooksNotAnObject), 0);
    };
    let (hooks, removed) = removing_hooks(&before, &is_upstream_hook);
    replace_hooks(&before, hooks, &mut document);
    (
        finish(
            document,
            &original,
            existing,
            None,
            StatusLineIntent::Nothing,
        ),
        removed,
    )
}

/// Puts `new` in place of `old` as the document's `hooks`, only if it
/// differs (so the value keeps its bytes when nothing changed), and without
/// an empty object the file didn't have.
fn replace_hooks(old: &Json, new: Json, document: &mut SettingsDocument) {
    if new.is_equivalent(old) {
        return;
    }
    let empty = new.members().is_none_or(<[Member]>::is_empty);
    document.set("hooks", (!empty).then_some(new));
}

/// `hooks` as an object: `{}` when absent, `None` when it is something else.
fn hooks_object(document: &SettingsDocument) -> Option<Json> {
    match document.get("hooks") {
        None => Some(Json::Object(Vec::new())),
        Some(hooks) if hooks.is_object() => Some(hooks.clone()),
        Some(_) => None,
    }
}

fn finish(
    document: SettingsDocument,
    original: &Json,
    existing: Option<&[u8]>,
    change: Option<PreviousStatusLineChange>,
    status_line: StatusLineIntent,
) -> SettingsPlan {
    // Compared as JSON, not byte for byte: Claude Code and other tools
    // rewrite settings.json in their own formatting, and reformatting the
    // user's file on every pass would be a pointless write (and backup).
    if existing.is_some() && document.value().is_equivalent(original) {
        // A `Save` without a statusLine change only re-saves what the saved
        // file already says, so nothing is lost by dropping it here;
        // `Recover`, `Remove` and `ChainNothing` still apply.
        let pending = match change {
            Some(PreviousStatusLineChange::Save(_)) | None => None,
            other => other,
        };
        return SettingsPlan {
            settings: SettingsWrite::AlreadyCurrent,
            previous_status_line: pending,
            status_line,
        };
    }
    SettingsPlan {
        settings: SettingsWrite::Write(document.data()),
        previous_status_line: change,
        status_line,
    }
}

// ---- Status line ----

/// Applies a status line wish to the document. Returns the matching change
/// to the saved previous-status-line file, and what was decided.
fn apply_status_line_wish(
    wish: StatusLineWish,
    document: &mut SettingsDocument,
    saved: &Saved,
    recogniser: &Recogniser,
) -> (Option<PreviousStatusLineChange>, StatusLineIntent) {
    let current = document.get("statusLine").cloned();
    let chained = || chain_target(saved, recogniser);

    match wish {
        StatusLineWish::Leave => (None, StatusLineIntent::Nothing),

        StatusLineWish::Unwrap => {
            if !recogniser.is_our_status_line(current.as_ref()) {
                return (None, StatusLineIntent::Nothing);
            }
            let back = chained().map(|previous| restored(&previous, current.as_ref()));
            document.set("statusLine", back);
            (
                Some(PreviousStatusLineChange::Remove),
                StatusLineIntent::Unwrap,
            )
        }

        StatusLineWish::Wrap { command, git_bash } => {
            match (takeover(current.as_ref(), recogniser, git_bash), current) {
                (Takeover::LeaveAlone(reason), _) => (None, StatusLineIntent::LeaveAlone(reason)),

                (Takeover::Update, Some(current)) => {
                    // Keep what the user set on the wrapper entry (padding, …);
                    // only the command is ours to update. (Set only when it
                    // changes, so a hooks-only write leaves the entry's bytes
                    // as they are.)
                    let is_current = current.get("command").and_then(Json::as_str) == Some(command)
                        && current.get("args").is_none();
                    if !is_current {
                        let mut ours = current;
                        ours.set("command", Some(Json::string(command)));
                        ours.set("args", None);
                        document.set("statusLine", Some(ours));
                    }
                    // Keep chaining to what the running wrapper chains to.
                    // That may be saved beside another folder's wrapper (a
                    // settings.json copied from it), so it is saved beside
                    // ours as well.
                    let change = chained().map(|chain| {
                        let bytes = saved_bytes(&chain);
                        // A lost saved copy is put back now, not at the next
                        // change to settings.json: until then the wrapper
                        // chains nothing.
                        if saved.previous_status_line.is_none() {
                            PreviousStatusLineChange::Recover(bytes)
                        } else {
                            PreviousStatusLineChange::Save(bytes)
                        }
                    });
                    (change, StatusLineIntent::UpdateCommand)
                }

                (Takeover::Wrap, Some(current)) => {
                    // The same entry (every key, in its order) running ours.
                    let mut ours = current.clone();
                    ours.set("type", Some(Json::string("command")));
                    ours.set("command", Some(Json::string(command)));
                    document.set("statusLine", Some(ours));
                    (
                        Some(PreviousStatusLineChange::Save(saved_bytes(&current))),
                        StatusLineIntent::Wrap,
                    )
                }

                // No status line before us: nothing to chain to, and any
                // stale saved file from an earlier install must not be run
                // (nor an older status line from a backup brought back).
                (Takeover::Install, _) | (Takeover::Update | Takeover::Wrap, None) => {
                    document.set(
                        "statusLine",
                        Some(Json::object([
                            ("type", Json::string("command")),
                            ("command", Json::string(command)),
                        ])),
                    );
                    (
                        Some(PreviousStatusLineChange::ChainNothing),
                        StatusLineIntent::Wrap,
                    )
                }
            }
        }
    }
}

/// A `statusLine` object as the saved file holds it.
fn saved_bytes(status_line: &Json) -> Vec<u8> {
    let mut text = status_line.serialized();
    text.push('\n');
    text.into_bytes()
}

/// What our wrapper chains to: the saved copy when there is one (the empty
/// object says "nothing"), else what the backups say it was (a lost saved
/// file must not cost the user their status line). The backups are only read
/// when the saved copy is missing.
pub fn chain_target(saved: &Saved, recogniser: &Recogniser) -> Option<Json> {
    let chainable = |value: Json| {
        let has_members = value.members().is_some_and(|members| !members.is_empty());
        (has_members && !is_a_wrapper(&value, recogniser)).then_some(value)
    };
    match saved.previous_status_line {
        Some(previous) => chainable(previous.clone()),
        None => (saved.backup_status_line)().and_then(chainable),
    }
}

/// Our wrapper, in any spelling: never something to chain to.
pub fn is_a_wrapper(status_line: &Json, recogniser: &Recogniser) -> bool {
    recogniser.is_our_status_line(Some(status_line))
        || status_line
            .get("command")
            .and_then(Json::as_str)
            .is_some_and(|command| recogniser.trips_loop_guard(command))
}

/// The status line to put back: the saved one, with any setting the user
/// changed on the wrapper entry while it was wrapped (keys both have), and
/// any they added. A `padding: 0` only the wrapper has is the default
/// anyway, so it isn't carried over.
pub fn restored(previous: &Json, wrapper: Option<&Json>) -> Json {
    if !previous.is_object() {
        return previous.clone();
    }
    let mut restored = previous.clone();
    for member in wrapper.and_then(Json::members).unwrap_or_default() {
        // `args` would be the wrapper's own, never the user's: a status
        // line with `args` is not taken over.
        if matches!(member.key.as_str(), "type" | "command" | "args") {
            continue;
        }
        let only_the_wrapper_has_it = previous.get(&member.key).is_none();
        if only_the_wrapper_has_it
            && member.key == "padding"
            && member.value.is_equivalent(&Json::int(0))
        {
            continue;
        }
        restored.set(&member.key, Some(member.value.clone()));
    }
    restored
}

// ---- Hook entries ----

/// Removes the hook entries that match from every event, and says how many
/// went. Matcher groups left with no hooks are dropped, and so are events
/// left with no groups. Anything of a shape not recognised is kept as it is.
pub fn removing_hooks(hooks: &Json, matches: &dyn Fn(&Json) -> bool) -> (Json, u32) {
    let mut cleaned = Vec::new();
    let mut removed = 0u32;
    for member in hooks.members().unwrap_or_default() {
        let Some(groups) = member.value.items() else {
            cleaned.push(member.clone());
            continue;
        };
        let mut remaining = Vec::new();
        for group in groups {
            let Some(entries) = group.get("hooks").and_then(Json::items) else {
                remaining.push(group.clone());
                continue;
            };
            let kept: Vec<Json> = entries
                .iter()
                .filter(|entry| !matches(entry))
                .cloned()
                .collect();
            if kept.len() == entries.len() {
                remaining.push(group.clone());
                continue;
            }
            removed += (entries.len() - kept.len()) as u32;
            if !kept.is_empty() {
                let mut updated = group.clone();
                updated.set("hooks", Some(Json::Array(kept)));
                remaining.push(updated);
            }
        }
        if !remaining.is_empty() {
            cleaned.push(Member::new(member.key.clone(), Json::Array(remaining)));
        }
    }
    (Json::Object(cleaned), removed)
}

/// The first hook entry anywhere in a parsed settings.json that satisfies
/// `matches`. Tolerant of any shape at any key.
pub fn first_hook_entry<'a>(
    settings: &'a Json,
    matches: &dyn Fn(&Json) -> bool,
) -> Option<&'a Json> {
    hook_entries(settings).find(|entry| matches(entry))
}

/// Every hook entry of every event, in file order.
pub fn hook_entries(settings: &Json) -> impl Iterator<Item = &Json> {
    settings
        .get("hooks")
        .and_then(Json::members)
        .unwrap_or_default()
        .iter()
        .flat_map(|event| event.value.items().unwrap_or_default())
        .flat_map(|group| group.get("hooks").and_then(Json::items).unwrap_or_default())
}
