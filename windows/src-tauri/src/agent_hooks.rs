// Whether each hook-driven pill is connected.
//
// A pill fed by hook events has no key: it is connected once the agent's config
// routes its events to fedo-hook. Only reads; nothing here ever writes.
//
// Claude Code is read here. Antigravity is read by agents.rs — the code that
// writes its config is the one source of truth for where that file is and
// what Fedo's entries look like, so the pill and Settings → Agents always agree.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;


/// True when a parsed `~/.claude/settings.json` routes Claude Code's
/// SessionStart events to Fedo's relay.
pub fn claude_hooks_present(settings: &Value) -> bool {
    let Some(groups) = settings
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    groups.iter().any(|group| {
        group.get("hooks").and_then(Value::as_array).is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command").and_then(Value::as_str).is_some_and(|c| c.contains("fedo"))
            })
        })
    })
}

/// A JSON file as a value; anything missing or unreadable is "nothing there".
fn read_json(path: &PathBuf) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| {
            let text = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes).to_vec();
            serde_json::from_slice(&text).ok()
        })
        .unwrap_or(Value::Null)
}

/// Pill ID → connected, for every hook-driven pill.
pub fn status() -> HashMap<String, bool> {
    let claude = claude_hooks_present(&read_json(&crate::hooks::settings_path()));
    let mut out = HashMap::new();
    out.insert("integration_claude".to_string(), claude);
    for agent in crate::agents::list() {
        out.insert(format!("agent_{}", agent.id), agent.installed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(json: &str) -> Value {
        serde_json::from_str(json).unwrap_or(Value::Null)
    }

    #[test]
    fn the_hook_fedo_writes_is_installed() {
        assert!(claude_hooks_present(&settings(
            r#"{"hooks":{"SessionStart":[{"hooks":[
              {"type":"command","command":"'/home/me/.local/share/fedo/bin/fedo-hook' SessionStart"}]}]}}"#
        )));
    }

    #[test]
    fn our_hook_next_to_somebody_elses_is_installed() {
        assert!(claude_hooks_present(&settings(
            r#"{"hooks":{"SessionStart":[
              {"hooks":[{"type":"command","command":"/usr/local/bin/other-tool"}]},
              {"hooks":[{"type":"command","command":"'/home/me/.local/share/fedo/bin/fedo-hook' SessionStart"}]}]}}"#
        )));
    }

    #[test]
    fn other_tools_only_is_not_installed() {
        assert!(!claude_hooks_present(&settings(
            r#"{"hooks":{"SessionStart":[{"hooks":[
              {"type":"command","command":"$HOME/.vibe-island/bin/vibe-island-bridge"},
              {"type":"command","command":"python3 /Users/me/.claude/skills/harness/hook.py"}]}]}}"#
        )));
    }

    #[test]
    fn hooks_for_other_events_do_not_count() {
        assert!(!claude_hooks_present(&settings(
            r#"{"hooks":{"PreToolUse":[{"hooks":[
              {"type":"command","command":"$HOME/.local/share/fedo/bin/fedo-hook"}]}]}}"#
        )));
    }

    #[test]
    fn malformed_or_empty_settings_never_read_as_installed() {
        for json in [
            "{}",
            r#"{"hooks":{"SessionStart":[]}}"#,
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command"}]}]}}"#,
            r#"{"hooks":{"SessionStart":"not-an-array"}}"#,
            r#"{"hooks":"not-an-object"}"#,
            "not json",
        ] {
            assert!(!claude_hooks_present(&settings(json)), "{json}");
        }
    }
}
