//! The ToolUseIdCache (HookSocketServer.swift, HS§4.3): FIFO queues of
//! PreToolUse `tool_use_id`s per (session, tool, input), so a
//! PermissionRequest, which carries no id, gets the id of the call it is
//! about and its own PostToolUse resolves it.
//!
//! The key is structured (session, tool, canonical input) rather than the
//! Mac's joined string, so a session id or tool name holding the separator
//! can never match another's calls; the canonical input is the same for the
//! PreToolUse and the PermissionRequest of a call because both come through
//! the hook's identical truncation (HS§4.3 WIN).

use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

/// Entries older than this are dropped: their tool resolved long ago.
pub const MAX_AGE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Key {
    session: String,
    tool: String,
    input: String,
}

#[derive(Debug, Clone)]
struct Entry {
    tool_use_id: String,
    recorded_at: SystemTime,
    /// The subagent that made the call; `None` for the main session.
    agent_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolUseIdCache {
    max_age: Duration,
    queues: BTreeMap<Key, Vec<Entry>>,
    /// Reverse index: tool_use_id → its queue.
    key_by_id: HashMap<String, Key>,
}

impl Default for ToolUseIdCache {
    fn default() -> Self {
        ToolUseIdCache::new(MAX_AGE)
    }
}

impl ToolUseIdCache {
    pub fn new(max_age: Duration) -> Self {
        ToolUseIdCache {
            max_age,
            queues: BTreeMap::new(),
            key_by_id: HashMap::new(),
        }
    }

    /// Ids held.
    pub fn len(&self) -> usize {
        self.key_by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.key_by_id.is_empty()
    }

    /// A PreToolUse: remembers its id at the end of its call's queue. An id
    /// already known is kept where it is (a PreToolUse delivered twice).
    pub fn record(
        &mut self,
        session: &str,
        tool: Option<&str>,
        input: Option<&Map<String, Value>>,
        tool_use_id: &str,
        agent_id: Option<&str>,
        now: SystemTime,
    ) {
        self.prune(now);
        if self.key_by_id.contains_key(tool_use_id) {
            return;
        }
        let key = key(session, tool, input);
        self.queues.entry(key.clone()).or_default().push(Entry {
            tool_use_id: tool_use_id.to_owned(),
            recorded_at: now,
            agent_id: non_empty(agent_id),
        });
        self.key_by_id.insert(tool_use_id.to_owned(), key);
    }

    /// The oldest id recorded for exactly this call, removed.
    pub fn pop(
        &mut self,
        session: &str,
        tool: Option<&str>,
        input: Option<&Map<String, Value>>,
        now: SystemTime,
    ) -> Option<String> {
        self.prune(now);
        let key = key(session, tool, input);
        let entries = self.queues.get_mut(&key)?;
        if entries.is_empty() {
            return None;
        }
        let entry = entries.remove(0);
        if entries.is_empty() {
            self.queues.remove(&key);
        }
        self.key_by_id.remove(&entry.tool_use_id);
        Some(entry.tool_use_id)
    }

    /// The id of the one call of `tool` still in flight in the session by
    /// the same agent, removed; `None` when there are none or several (then
    /// which call the request is about can't be told). Another hook may have
    /// rewritten the input between PreToolUse and PermissionRequest, which
    /// is why the input is not compared here.
    pub fn pop_only_in_flight(
        &mut self,
        session: &str,
        tool: Option<&str>,
        agent_id: Option<&str>,
        now: SystemTime,
    ) -> Option<String> {
        self.prune(now);
        let tool = tool.unwrap_or(UNKNOWN_TOOL);
        let agent = non_empty(agent_id);
        let mut found: Option<String> = None;
        for (key, entries) in &self.queues {
            if key.session != session || key.tool != tool {
                continue;
            }
            for entry in entries.iter().filter(|e| e.agent_id == agent) {
                if found.is_some() {
                    return None;
                }
                found = Some(entry.tool_use_id.clone());
            }
        }
        let id = found?;
        self.remove(&id);
        Some(id)
    }

    /// The call resolved (PostToolUse, PostToolUseFailure, PermissionDenied):
    /// its id must never be handed to a later identical call.
    pub fn remove(&mut self, tool_use_id: &str) {
        let Some(key) = self.key_by_id.remove(tool_use_id) else {
            return;
        };
        if let Some(entries) = self.queues.get_mut(&key) {
            entries.retain(|e| e.tool_use_id != tool_use_id);
            if entries.is_empty() {
                self.queues.remove(&key);
            }
        }
    }

    /// Forgets a session's calls; with `main_session_only`, background
    /// subagents' calls (still running after the main Stop) are kept.
    pub fn remove_session(&mut self, session: &str, main_session_only: bool) {
        let keys: Vec<Key> = self
            .queues
            .keys()
            .filter(|k| k.session == session)
            .cloned()
            .collect();
        for key in keys {
            let Some(entries) = self.queues.remove(&key) else {
                continue;
            };
            let (kept, dropped): (Vec<Entry>, Vec<Entry>) = entries
                .into_iter()
                .partition(|e| main_session_only && e.agent_id.is_some());
            for entry in dropped {
                self.key_by_id.remove(&entry.tool_use_id);
            }
            if !kept.is_empty() {
                self.queues.insert(key, kept);
            }
        }
    }

    fn prune(&mut self, now: SystemTime) {
        let Some(cutoff) = now.checked_sub(self.max_age) else {
            return;
        };
        let key_by_id = &mut self.key_by_id;
        self.queues.retain(|_, entries| {
            entries.retain(|entry| {
                let keep = entry.recorded_at >= cutoff;
                if !keep {
                    key_by_id.remove(&entry.tool_use_id);
                }
                keep
            });
            !entries.is_empty()
        });
    }
}

const UNKNOWN_TOOL: &str = "unknown";

fn non_empty(agent_id: Option<&str>) -> Option<String> {
    agent_id.filter(|a| !a.is_empty()).map(str::to_owned)
}

fn key(session: &str, tool: Option<&str>, input: Option<&Map<String, Value>>) -> Key {
    Key {
        session: session.to_owned(),
        tool: tool.unwrap_or(UNKNOWN_TOOL).to_owned(),
        input: match input {
            Some(map) => canonical(&Value::Object(map.clone())),
            None => "{}".into(),
        },
    }
}

/// JSON with every object's keys sorted (the Mac's `.sortedKeys` encoder),
/// whatever order serde_json's map keeps in this build.
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000 + seconds)
    }

    /// HookEventDecodingTests.toolUseIdCacheHygiene.
    #[test]
    fn hygiene() {
        let mut cache = ToolUseIdCache::default();
        let ls = input(json!({"command": "ls"}));
        cache.record("s", Some("Bash"), Some(&ls), "toolu_1", None, at(0));
        cache.record("s", Some("Bash"), Some(&ls), "toolu_2", None, at(0));
        // toolu_1 was auto-allowed and ran: its id must not be handed out later.
        cache.remove("toolu_1");
        assert_eq!(
            cache.pop("s", Some("Bash"), Some(&ls), at(1)).as_deref(),
            Some("toolu_2")
        );
        assert_eq!(cache.pop("s", Some("Bash"), Some(&ls), at(1)), None);

        cache.record("s", Some("Bash"), Some(&ls), "toolu_3", None, at(2));
        cache.record("other", Some("Bash"), Some(&ls), "toolu_4", None, at(2));
        cache.remove_session("s", false);
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache
                .pop("other", Some("Bash"), Some(&ls), at(3))
                .as_deref(),
            Some("toolu_4")
        );

        // An hour-old entry is gone as soon as anything newer is recorded.
        cache.record("s", Some("Bash"), Some(&ls), "toolu_old", None, at(10));
        cache.record(
            "s",
            Some("Read"),
            None,
            "toolu_new",
            None,
            at(10 + 2 * 3600),
        );
        assert_eq!(
            cache.pop("s", Some("Bash"), Some(&ls), at(10 + 2 * 3600)),
            None
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn key_order_does_not_matter() {
        let mut cache = ToolUseIdCache::default();
        let a = input(json!({"b": 1, "a": {"y": [1, {"d": 2, "c": 3}], "x": null}}));
        let b = input(json!({"a": {"x": null, "y": [1, {"c": 3, "d": 2}]}, "b": 1}));
        cache.record("s", Some("Edit"), Some(&a), "toolu_e", None, at(0));
        assert_eq!(
            cache.pop("s", Some("Edit"), Some(&b), at(1)).as_deref(),
            Some("toolu_e")
        );
    }

    #[test]
    fn fifo_per_call() {
        let mut cache = ToolUseIdCache::default();
        let make = input(json!({"command": "make"}));
        for id in ["t1", "t2", "t3"] {
            cache.record("s", Some("Bash"), Some(&make), id, None, at(0));
        }
        // A duplicate PreToolUse keeps its place.
        cache.record("s", Some("Bash"), Some(&make), "t1", None, at(0));
        let popped: Vec<_> = (0..4)
            .map(|_| cache.pop("s", Some("Bash"), Some(&make), at(1)))
            .collect();
        assert_eq!(
            popped,
            [
                Some("t1".into()),
                Some("t2".into()),
                Some("t3".into()),
                None
            ]
        );
    }

    #[test]
    fn only_in_flight_needs_exactly_one_by_the_same_agent() {
        let mut cache = ToolUseIdCache::default();
        let one = input(json!({"command": "one"}));
        let two = input(json!({"command": "two"}));
        cache.record("s", Some("Bash"), Some(&one), "main", None, at(0));
        cache.record("s", Some("Bash"), Some(&two), "sub", Some("agent-1"), at(0));
        cache.record("s", Some("Read"), None, "read", None, at(0));
        cache.record("t", Some("Bash"), Some(&one), "other-session", None, at(0));
        // The main agent has exactly one Bash in flight.
        assert_eq!(
            cache
                .pop_only_in_flight("s", Some("Bash"), None, at(1))
                .as_deref(),
            Some("main")
        );
        // The subagent has its own; an empty agent id is the main session.
        assert_eq!(
            cache.pop_only_in_flight("s", Some("Bash"), Some(""), at(1)),
            None
        );
        assert_eq!(
            cache
                .pop_only_in_flight("s", Some("Bash"), Some("agent-1"), at(1))
                .as_deref(),
            Some("sub")
        );
        // Two in flight: can't tell which.
        cache.record("s", Some("Bash"), Some(&one), "a", None, at(2));
        cache.record("s", Some("Bash"), Some(&two), "b", None, at(2));
        assert_eq!(
            cache.pop_only_in_flight("s", Some("Bash"), None, at(3)),
            None
        );
        assert_eq!(cache.len(), 4);
    }

    #[test]
    fn main_stop_keeps_subagents_calls() {
        let mut cache = ToolUseIdCache::default();
        let x = input(json!({"command": "x"}));
        cache.record("s", Some("Bash"), Some(&x), "main", None, at(0));
        cache.record("s", Some("Bash"), Some(&x), "sub", Some("agent-1"), at(0));
        cache.remove_session("s", true);
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.pop("s", Some("Bash"), Some(&x), at(1)).as_deref(),
            Some("sub")
        );
    }

    #[test]
    fn separators_in_ids_never_collide() {
        let mut cache = ToolUseIdCache::default();
        // The Mac's joined key would read both as "a:b:c:{}".
        cache.record("a:b", Some("c"), None, "one", None, at(0));
        assert_eq!(cache.pop("a", Some("b:c"), None, at(1)), None);
        cache.remove_session("a", false);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn canonical_json() {
        assert_eq!(
            canonical(&json!({"b": [true, null, 1.5], "a": "é\"x"})),
            r#"{"a":"é\"x","b":[true,null,1.5]}"#
        );
    }
}
