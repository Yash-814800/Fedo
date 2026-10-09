// Hook installer for every agent other than Claude Code (hooks.rs) that this
// Fedora/GNOME fork keeps: Antigravity. FEDO.md §0/Phase 0 narrows the Mac and
// upstream builds' nine third-party agents (Gemini CLI, Cursor,
// Codex, Copilot CLI, Muse, OpenCode, Amp, Hermes, Antigravity) down to just
// this one — Claude Code and Antigravity are the only two agents this fork's
// owner actually uses.
//
// What lives here is only *what* changes in Antigravity's config, and the
// command line that starts the relay. The edit itself goes through
// config_file.rs, which applies the CLAUDE.md rules: strict read, refuse
// unexpected types, diff, dated backup that must succeed, fingerprint check,
// atomic write.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::config_file::{self, FileEdit, Plan};
use crate::{platform, settings};

/// Marker that identifies a Fedo entry: the relay's file name.
const MARKER: &str = "fedo-hook";

// ── The relay command line ────────────────────────────────────────────────────

/// The shell an agent runs its hook commands through. Every agent on Linux
/// uses `sh` (or a process started directly) — this only matters at all on
/// Windows, which this fork does not build for; kept so the command-building
/// logic below needs no platform split.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shell {
    Sh,
    PowerShell,
    Cmd,
}

/// Where the relay is and which OS the command is for. Kept apart from the
/// real install so tests can build the command without touching the filesystem.
pub struct Relay {
    pub exe: String,
    pub windows: bool,
}

impl Relay {
    pub fn current() -> Self {
        Self {
            exe: settings::hook_exe_path().to_string_lossy().to_string(),
            windows: cfg!(windows),
        }
    }

    /// The relay followed by `args`, as one command line for `shell`.
    pub fn command(&self, shell: Shell, args: &str) -> String {
        if !self.windows {
            return format!("{} {args}", sh_quote(&self.exe));
        }
        let exe = match shell {
            Shell::Sh => format!("\"{}\"", self.exe.replace('\\', "/")),
            Shell::PowerShell => format!("& '{}'", self.exe.replace('\'', "''")),
            Shell::Cmd if is_plain(&self.exe) => self.exe.clone(),
            Shell::Cmd => format!("\"{}\"", self.exe),
        };
        format!("{exe} {args}")
    }
}

/// The relay command for the running OS, for `args` — Claude Code's own hooks
/// (hooks.rs) use this rather than building a `Relay` themselves.
pub fn relay_command(shell: Shell, args: &str) -> String {
    Relay::current().command(shell, args)
}

/// `s` as one single-quoted POSIX shell word: `'` becomes `'\''`, nothing else
/// is special inside single quotes.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A path no Windows shell would split or interpret. Unreachable on Linux.
fn is_plain(path: &str) -> bool {
    path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '\\' | ':' | '.' | '_' | '-'))
}

// ── Agents ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Agent {
    Antigravity,
}

impl Agent {
    pub const ALL: &'static [Agent] = &[Agent::Antigravity];

    /// The `--agent` name; the pill is `agent_<id>`.
    pub fn id(self) -> &'static str {
        match self {
            Agent::Antigravity => "antigravity",
        }
    }

    pub fn from_id(id: &str) -> Option<Agent> {
        Self::ALL.iter().copied().find(|a| a.id() == id)
    }

    /// The files this agent's install touches, under `home`.
    fn files(self, home: &Path) -> Vec<PathBuf> {
        match self {
            Agent::Antigravity => vec![home.join(".gemini").join("config").join("hooks.json")],
        }
    }

    /// The edits that install (or remove) Fedo for this agent.
    fn edits(self, home: &Path, relay: &Relay, install: bool) -> Vec<FileEdit<'static>> {
        let files = self.files(home);
        let path = files[0].clone();
        let label = path.display().to_string();
        let change = self.json_change(relay, install);
        vec![FileEdit { path, edit: config_file::json_edit(label, change) }]
    }

    /// The change to this agent's JSON config: the new object, or an error to
    /// refuse. Command lines are built here, up front, so the change itself
    /// is pure.
    fn json_change(self, relay: &Relay, install: bool) -> Box<dyn Fn(&Value) -> Result<Option<Value>, String>> {
        match (self, install) {
            (Agent::Antigravity, false) => Box::new(|v| antigravity_uninstall(v).map(Some)),
            (Agent::Antigravity, true) => {
                let block = antigravity_block(relay);
                Box::new(move |v| antigravity_install(v, &block).map(Some))
            }
        }
    }

    /// True when the agent's config already routes to Fedo. Never fails: a
    /// file we cannot read just reads as "not installed".
    fn installed(self, home: &Path) -> bool {
        let files = self.files(home);
        let json = || {
            config_file::read(&files[0])
                .ok()
                .flatten()
                .and_then(|b| config_file::parse_json(Some(&b), "").ok())
                .unwrap_or_else(|| json!({}))
        };
        match self {
            Agent::Antigravity => json().get("fedo").is_some_and(antigravity_is_ours),
        }
    }

    /// Whether the island can answer this agent's permission requests. Must
    /// match `takes_decisions` in the relay (hook/src/reply.rs). Antigravity
    /// has none: PreToolUse is observer-only, see `antigravity_block`.
    fn approvals(self) -> bool {
        false
    }
}

// ── Strings shown in Settings ─────────────────────────────────────────────────

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Antigravity => "Antigravity",
        }
    }

    /// What to do once the file is written, shown with the result.
    fn note(self) -> String {
        use crate::i18n::t;
        match self {
            Agent::Antigravity => t("Start a new Antigravity conversation to pick the hooks up."),
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub installed: bool,
    /// The file (or files, one per line) Fedo writes.
    pub path: String,
    pub hook_ready: bool,
    /// The island can allow or deny this agent's permission requests.
    pub approvals: bool,
    pub note: String,
}

fn find(id: &str) -> Result<Agent, String> {
    Agent::from_id(id).ok_or_else(|| crate::i18n::tf("Unknown agent \"{id}\".", &[("id", id)]))
}

pub fn list() -> Vec<AgentStatus> {
    let home = platform::home_dir();
    let hook_ready = settings::hook_exe_path().exists();
    Agent::ALL
        .iter()
        .map(|&a| AgentStatus {
            id: a.id(),
            name: a.name(),
            installed: a.installed(&home),
            path: a.files(&home).iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"),
            hook_ready,
            approvals: a.approvals(),
            note: a.note(),
        })
        .collect()
}

pub fn preview(id: &str, install: bool) -> Result<Plan, String> {
    let agent = find(id)?;
    config_file::preview(&agent.edits(&platform::home_dir(), &Relay::current(), install))
}

/// Only ever called from an explicit click. Returns the backups taken, one per line.
pub fn apply(id: &str, install: bool, fingerprint: &str) -> Result<String, String> {
    let agent = find(id)?;
    let backups = config_file::apply(&agent.edits(&platform::home_dir(), &Relay::current(), install), fingerprint)?;
    Ok(backups.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"))
}

// ── Antigravity — ~/.gemini/config/hooks.json ─────────────────────────────────
//
// Hooks are named groups at the top level; Fedo's is `fedo`. Tool events take
// matcher groups, lifecycle events take handlers directly; timeouts are in
// seconds. The relay answers PreToolUse with "{}" — no decision — never with
// an allow: Antigravity's own permission rules stay in charge.

const ANTIGRAVITY_TOOL_EVENTS: &[&str] = &["PreToolUse", "PostToolUse"];
const ANTIGRAVITY_LIFECYCLE_EVENTS: &[&str] = &["PreInvocation", "PostInvocation", "Stop"];

fn antigravity_block(relay: &Relay) -> Value {
    let handler = |event: &str| {
        json!({
            "type": "command",
            "command": relay.command(Shell::Cmd, &format!("--agent antigravity {event}")),
            "timeout": 10,
        })
    };
    let mut block = Map::new();
    for event in ANTIGRAVITY_TOOL_EVENTS {
        block.insert((*event).into(), json!([{ "matcher": "*", "hooks": [handler(event)] }]));
    }
    for event in ANTIGRAVITY_LIFECYCLE_EVENTS {
        block.insert((*event).into(), json!([handler(event)]));
    }
    Value::Object(block)
}

/// A `fedo` group Fedo wrote (it runs the relay as `--agent antigravity`).
fn antigravity_is_ours(group: &Value) -> bool {
    let text = group.to_string();
    text.contains(MARKER) && text.contains("--agent antigravity")
}

fn antigravity_install(root: &Value, block: &Value) -> Result<Value, String> {
    let mut root = root.as_object().cloned().unwrap_or_default();
    if root.get("fedo").is_some_and(|g| !antigravity_is_ours(g)) {
        return Err(crate::i18n::t("A hook group named \"fedo\" that Fedo did not write is already there — Fedo has not touched it."));
    }
    root.insert("fedo".into(), block.clone());
    Ok(Value::Object(root))
}

/// Removes Fedo's group, and only if it is Fedo's.
fn antigravity_uninstall(root: &Value) -> Result<Value, String> {
    let mut root = root.as_object().cloned().unwrap_or_default();
    if root.get("fedo").is_some_and(antigravity_is_ours) {
        root.remove("fedo");
    }
    Ok(Value::Object(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_file::tests::scratch;

    fn linux() -> Relay {
        Relay { exe: "/home/me/.local/share/fedo/bin/fedo-hook".into(), windows: false }
    }

    fn windows(exe: &str) -> Relay {
        Relay { exe: exe.into(), windows: true }
    }

    const WIN: &str = r"C:\Users\me\AppData\Local\Fedo\bin\fedo-hook.exe";
    const WIN_SPACE: &str = r"C:\Users\Jane O'Neil\AppData\Local\Fedo\bin\fedo-hook.exe";

    /// Installs then uninstalls `agent` in a fresh home holding `existing` in
    /// its first file, and returns (installed file, uninstalled file).
    fn round_trip(agent: Agent, existing: Option<&str>) -> (PathBuf, Value, Option<Value>) {
        static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let run = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let home = scratch(&format!("agent-{}-{run}", agent.id()));
        let file = agent.files(&home)[0].clone();
        if let Some(text) = existing {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, text).unwrap();
        }
        let relay = linux();
        let plan = config_file::preview(&agent.edits(&home, &relay, true)).unwrap();
        assert!(plan.diff.contains(MARKER), "{}", plan.diff);
        config_file::apply(&agent.edits(&home, &relay, true), &plan.fingerprint).unwrap();
        assert!(agent.installed(&home), "{agent:?} should read as installed");
        let installed = read_json(&file);

        let plan = config_file::preview(&agent.edits(&home, &relay, false)).unwrap();
        config_file::apply(&agent.edits(&home, &relay, false), &plan.fingerprint).unwrap();
        assert!(!agent.installed(&home), "{agent:?} should read as removed");
        let removed = file.exists().then(|| read_json(&file));
        (home, installed, removed)
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn the_command_is_quoted_for_the_shell_that_runs_it() {
        assert_eq!(
            linux().command(Shell::Cmd, "--agent antigravity"),
            "'/home/me/.local/share/fedo/bin/fedo-hook' --agent antigravity"
        );
        let w = windows(WIN);
        assert_eq!(
            w.command(Shell::Sh, "Stop"),
            "\"C:/Users/me/AppData/Local/Fedo/bin/fedo-hook.exe\" Stop"
        );
        assert_eq!(
            w.command(Shell::PowerShell, "--agent antigravity Stop"),
            format!("& '{WIN}' --agent antigravity Stop")
        );
        assert_eq!(w.command(Shell::Cmd, "--agent antigravity"), format!("{WIN} --agent antigravity"));

        let spaced = windows(WIN_SPACE);
        assert_eq!(spaced.command(Shell::Cmd, "x"), format!("\"{WIN_SPACE}\" x"));
        assert_eq!(
            spaced.command(Shell::PowerShell, "x"),
            r"& 'C:\Users\Jane O''Neil\AppData\Local\Fedo\bin\fedo-hook.exe' x"
        );
    }

    #[test]
    fn the_hook_path_is_one_shell_word_whatever_it_contains() {
        assert_eq!(sh_quote("/home/a b/x"), "'/home/a b/x'");
        assert_eq!(sh_quote(r#"/h/$(id)`x`\"y"#), r#"'/h/$(id)`x`\"y'"#);
        assert_eq!(sh_quote("/h/it's"), r"'/h/it'\''s'");
    }

    #[test]
    fn every_agent_id_is_a_valid_pill_name() {
        for agent in Agent::ALL {
            let id = agent.id();
            assert!(id.len() <= 24 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
            assert_eq!(Agent::from_id(id), Some(*agent));
        }
        assert!(find("claude").is_err());
    }

    #[test]
    fn antigravity_gets_its_own_named_group_and_nothing_else_changes() {
        let existing = r#"{"my-guard":{"PreToolUse":[{"matcher":"run_command","hooks":[{"command":"/bin/guard"}]}]}}"#;
        let (home, installed, removed) = round_trip(Agent::Antigravity, Some(existing));
        assert_eq!(installed["my-guard"]["PreToolUse"][0]["matcher"], "run_command");
        let ours = &installed["fedo"];
        for event in ANTIGRAVITY_TOOL_EVENTS {
            assert_eq!(ours[event][0]["matcher"], "*");
            let cmd = ours[event][0]["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.ends_with(&format!("--agent antigravity {event}")), "{cmd}");
        }
        for event in ANTIGRAVITY_LIFECYCLE_EVENTS {
            assert_eq!(ours[event][0]["timeout"], 10);
            assert!(ours[event][0]["command"].as_str().unwrap().contains(MARKER));
        }
        assert_eq!(removed.unwrap(), serde_json::from_str::<Value>(existing).unwrap());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn a_fedo_group_someone_else_wrote_is_neither_replaced_nor_removed() {
        let theirs = json!({ "fedo": { "Stop": [{ "command": "/bin/notify" }] } });
        assert!(antigravity_install(&theirs, &antigravity_block(&linux())).is_err());
        assert_eq!(antigravity_uninstall(&theirs).unwrap(), theirs);
    }

    #[test]
    fn on_windows_antigravity_runs_the_relay_path_quoted_only_when_needed() {
        let block = antigravity_block(&windows(WIN));
        assert_eq!(block["Stop"][0]["command"], format!("{WIN} --agent antigravity Stop"));
        let block = antigravity_block(&windows(WIN_SPACE));
        assert_eq!(block["Stop"][0]["command"], format!("\"{WIN_SPACE}\" --agent antigravity Stop"));
    }

    #[test]
    fn no_agent_takes_approvals_in_this_build() {
        assert!(Agent::ALL.iter().all(|a| !a.approvals()));
    }
}
