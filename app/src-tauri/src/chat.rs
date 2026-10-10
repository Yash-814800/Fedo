// The chat: every turn is a `claude -p` run (the CLI the user is logged into —
// no API keys anywhere, FEDO.md §0). The first turn starts a Claude Code
// session; follow-ups `--resume` it, so the conversation has its context. The
// reply streams to the island as `chat-delta`; the plain-text history kept
// here is what "New chat" clears.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::settings::Settings;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatContext {
    File { name: String, path: String },
    Window { app_name: String, title: String, url: Option<String> },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub text: String,
}

/// A model the chat could be pointed at. Unused: the chat uses the CLI's own default.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
}

// ── Conversation ──────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct Chat {
    inner: Mutex<Conversation>,
    /// The running `claude` process, if a turn is in flight.
    pid: std::sync::Arc<Mutex<Option<u32>>>,
}

#[derive(Default)]
struct Conversation {
    /// Bumped by every reset, so an answer that lands after "New chat" is dropped.
    epoch: u64,
    /// `{"role", "content": text}` turns.
    plain: Vec<Value>,
    /// Claude Code's own id for this chat, so a follow-up `--resume`s it.
    session_id: Option<String>,
}

pub struct Turn {
    epoch: u64,
    /// No earlier turn: the file or window context rides along with this one.
    pub first: bool,
}

impl Chat {
    pub fn reset(&self) {
        // "New chat" ends a turn that is still being written.
        if let Some(pid) = *self.pid.lock().unwrap() {
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
        let mut c = self.inner.lock().unwrap();
        let epoch = c.epoch + 1;
        *c = Conversation { epoch, ..Default::default() };
    }

    pub fn session_id(&self) -> Option<String> {
        self.inner.lock().unwrap().session_id.clone()
    }

    pub fn set_session_id(&self, turn: &Turn, id: String) {
        let mut c = self.inner.lock().unwrap();
        if c.epoch == turn.epoch {
            c.session_id = Some(id);
        }
    }

    pub fn pid_slot(&self) -> std::sync::Arc<Mutex<Option<u32>>> {
        self.pid.clone()
    }

    pub fn begin(&self) -> Turn {
        let c = self.inner.lock().unwrap();
        Turn { epoch: c.epoch, first: c.plain.is_empty() }
    }

    /// Records a finished turn's plain text. Only a successful turn is
    /// recorded, so the history always matches what the model saw.
    pub fn commit(&self, turn: &Turn, user_text: &str, answer: &str) {
        let mut c = self.inner.lock().unwrap();
        if c.epoch != turn.epoch {
            return;
        }
        c.plain.push(json!({ "role": "user", "content": user_text }));
        c.plain.push(json!({ "role": "assistant", "content": answer }));
    }
}

// ── System prompt ─────────────────────────────────────────────────────────────

/// The ghost's instructions. Greets the user by their first name when the
/// account has one worth using (identity.rs).
pub fn system_prompt() -> String {
    system_prompt_for(crate::identity::first_name())
}

fn system_prompt_for(first_name: Option<&str>) -> String {
    match first_name {
        Some(name) => format!("You are Fedo, {name}'s personal assistant living at the top of their screen."),
        None => "You are Fedo, a personal assistant living at the top of the user's screen.".to_string(),
    }
}

/// The window context line, as the chat view sends it along.
pub fn window_line(app_name: &str, title: &str, url: Option<&str>) -> String {
    let mut text = format!("Context — App: {app_name}, Window: {title}");
    if let Some(url) = url {
        text.push_str(&format!(", URL: {url}"));
    }
    text
}

/// The plain-text record of what the user asked, context included.
pub fn plain_question(first: bool, context: Option<&ChatContext>, query: &str) -> String {
    match context.filter(|_| first) {
        Some(ChatContext::File { name, .. }) => format!("File: {name}\n\n{query}"),
        Some(ChatContext::Window { app_name, title, url }) => {
            format!("{}\n\n{query}", window_line(app_name, title, url.as_deref()))
        }
        None => query.to_string(),
    }
}

/// A file rides along only if it is one of Fedo's own copies of a dropped
/// file (files.rs puts them in the inbox). The page names the path, so without
/// this any file the user can read could be sent along with a question.
fn checked_context(context: ChatContext) -> Result<ChatContext, String> {
    match context {
        ChatContext::File { name, path } => {
            let inbox = crate::files::inbox_dir();
            if !is_inside(&inbox, std::path::Path::new(&path)) {
                return Err(crate::i18n::t("Only a file dropped on the island can be sent with a question."));
            }
            Ok(ChatContext::File { name, path })
        }
        other => Ok(other),
    }
}

/// True when `path` is a regular file directly inside `dir`, both resolved
/// (no `..`, no symlink pointing out of it).
fn is_inside(dir: &std::path::Path, path: &std::path::Path) -> bool {
    let (Ok(dir), Ok(file)) = (dir.canonicalize(), path.canonicalize()) else { return false };
    file.parent() == Some(dir.as_path())
        && std::fs::symlink_metadata(&file).map(|m| m.is_file()).unwrap_or(false)
}

// ── claude -p ─────────────────────────────────────────────────────────────────

/// Tools the chat may use without asking: read-only. `claude -p` has nobody to
/// ask, so anything else is simply refused — nothing on the machine changes
/// from the chat (FEDO.md §0).
const CHAT_TOOLS: &str = "Read,Glob,Grep,WebFetch,WebSearch";
/// A turn that has produced nothing for this long is given up on.
const TURN_LIMIT: std::time::Duration = std::time::Duration::from_secs(300);

/// What one stream-json line means for the chat.
#[derive(Debug, PartialEq)]
pub enum Line {
    /// More of the answer (the text so far is the caller's to keep).
    Delta(String),
    /// Plan windows, as `rate_limits` of a status line call.
    Usage(Value),
    /// The turn is over.
    Result { text: String, session_id: Option<String>, is_error: bool },
    Other,
}

/// Reads one line of `claude -p --output-format stream-json` output.
pub fn parse_line(line: &str) -> Line {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Line::Other };
    match v["type"].as_str() {
        Some("stream_event") => {
            let d = &v["event"]["delta"];
            match (v["event"]["type"].as_str(), d["type"].as_str(), d["text"].as_str()) {
                (Some("content_block_delta"), Some("text_delta"), Some(t)) => Line::Delta(t.to_string()),
                _ => Line::Other,
            }
        }
        Some("rate_limit_event") => usage_from(&v["rate_limit_info"]["unifiedWindows"])
            .map(Line::Usage)
            .unwrap_or(Line::Other),
        Some("result") => Line::Result {
            text: v["result"].as_str().unwrap_or_default().to_string(),
            session_id: v["session_id"].as_str().map(str::to_string),
            is_error: v["is_error"].as_bool().unwrap_or(false),
        },
        _ => Line::Other,
    }
}

/// `unifiedWindows` ({five_hour: {utilization 0–1, resetsAt s}, seven_day}) as
/// the status line's `rate_limits` ({used_percentage 0–100, resets_at s}), so
/// the island reads both the same way (core/plan.ts parseClaudePlan).
fn usage_from(windows: &Value) -> Option<Value> {
    let mut out = serde_json::Map::new();
    for key in ["five_hour", "seven_day"] {
        let w = &windows[key];
        if let (Some(u), Some(r)) = (w["utilization"].as_f64(), w["resetsAt"].as_i64()) {
            out.insert(key.to_string(), json!({ "used_percentage": u * 100.0, "resets_at": r }));
        }
    }
    (!out.is_empty()).then(|| Value::Object(out))
}

/// The arguments of one turn, after `claude`.
pub fn turn_args(prompt: &str, system: &str, resume: Option<&str>, add_dir: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = ["-p", prompt, "--output-format", "stream-json", "--verbose", "--include-partial-messages"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(["--append-system-prompt".into(), system.into(), "--allowedTools".into(), CHAT_TOOLS.into()]);
    if let Some(id) = resume {
        a.extend(["--resume".into(), id.into()]);
    }
    if let Some(dir) = add_dir {
        a.extend(["--add-dir".into(), dir.into()]);
    }
    a
}

/// The prompt as Claude Code sees it: a dropped file is named by its path, so
/// it can be read.
pub fn prompt_for(first: bool, context: Option<&ChatContext>, query: &str) -> String {
    match context.filter(|_| first) {
        Some(ChatContext::File { name, path }) => {
            format!("The user dropped a file on you: {name} (at {path}). Read it if the question needs it.\n\n{query}")
        }
        Some(ChatContext::Window { .. }) => plain_question(true, context, query),
        None => query.to_string(),
    }
}

/// One chat turn, through the `claude` CLI the user is logged into. The reply
/// streams to the island as `chat-delta` (the text so far); plan usage seen on
/// the way goes out as `plan-usage`.
pub async fn send(
    app: &AppHandle,
    chat: &Chat,
    _settings: &Settings,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let context = match context {
        Some(c) => Some(checked_context(c)?),
        None => None,
    };
    let turn = chat.begin();
    let resume = chat.session_id();
    let prompt = prompt_for(turn.first, context.as_ref(), &query);
    let inbox = crate::files::inbox_dir();
    let args = turn_args(&prompt, &system_prompt(), resume.as_deref(), inbox.to_str());
    let app2 = app.clone();
    let pid_slot = chat.pid_slot();

    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut so_far = String::new();
        run_claude(&args, &pid_slot, |line| match line {
            Line::Delta(t) => {
                so_far.push_str(t);
                let _ = tauri::Emitter::emit(&app2, "chat-delta", so_far.clone());
            }
            Line::Usage(v) => crate::island::emit(&app2, "plan-usage", v.clone()),
            _ => {}
        })
    })
        .await
        .map_err(|e| e.to_string())??;

    chat.commit(&turn, &plain_question(turn.first, context.as_ref(), &query), &outcome.text);
    if let Some(id) = outcome.session_id {
        chat.set_session_id(&turn, id);
    }
    Ok(ChatReply { text: outcome.text })
}

struct Outcome {
    text: String,
    session_id: Option<String>,
}

/// Runs one turn; `on_line` sees every parsed line as it arrives.
fn run_claude(
    args: &[String],
    pid_slot: &std::sync::Arc<Mutex<Option<u32>>>,
    mut on_line: impl FnMut(&Line),
) -> Result<Outcome, String> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let mut child = Command::new("claude")
        .args(args)
        .current_dir(crate::platform::home_dir())
        .env("FEDO_CHAT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                crate::i18n::t("The `claude` command was not found. Install Claude Code and log in first.")
            } else {
                e.to_string()
            }
        })?;
    *pid_slot.lock().unwrap() = Some(child.id());

    // A watchdog, so a turn that hangs doesn't hold the chat forever.
    let id = child.id();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = done.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            if start.elapsed() > TURN_LIMIT {
                unsafe { libc::kill(id as i32, libc::SIGTERM) };
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });

    let stderr = child.stderr.take();
    let stdout = child.stdout.take().ok_or("no output")?;
    let mut so_far = String::new();
    let mut result = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let parsed = parse_line(&line);
        on_line(&parsed);
        match parsed {
            Line::Delta(t) => so_far.push_str(&t),
            Line::Result { text, session_id, is_error } => result = Some((text, session_id, is_error)),
            Line::Usage(_) | Line::Other => {}
        }
    }
    let status = child.wait();
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    *pid_slot.lock().unwrap() = None;

    match result {
        Some((text, session_id, false)) => Ok(Outcome { text: if text.is_empty() { so_far } else { text }, session_id }),
        Some((text, _, true)) => Err(if text.is_empty() { "Claude Code reported an error.".into() } else { text }),
        None => {
            let mut err = String::new();
            if let Some(mut e) = stderr {
                let _ = std::io::Read::read_to_string(&mut e, &mut err);
            }
            let err = err.trim();
            Err(if err.is_empty() {
                format!("Claude Code exited without an answer ({})", status.map(|s| s.to_string()).unwrap_or_default())
            } else {
                err.chars().take(400).collect()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_recorded_only_once_it_succeeds() {
        let chat = Chat::default();
        let t = chat.begin();
        assert!(t.first);
        chat.commit(&t, "hi", "hello");
        let t = chat.begin();
        assert!(!t.first);
    }

    #[test]
    fn an_answer_that_lands_after_a_reset_is_dropped() {
        let chat = Chat::default();
        let t = chat.begin();
        chat.reset();
        chat.commit(&t, "q", "a");
        assert!(chat.begin().first);
    }

    #[test]
    fn the_prompt_greets_by_first_name() {
        let p = system_prompt_for(Some("Louis"));
        assert!(p.starts_with("You are Fedo, Louis's personal assistant living at the top of their screen."));
        let p = system_prompt_for(None);
        assert!(p.starts_with("You are Fedo, a personal assistant living at the top of the user's screen."));
    }

    #[test]
    fn context_goes_with_the_first_question_only() {
        let file = ChatContext::File { name: "a.txt".into(), path: "/x/a.txt".into() };
        assert_eq!(plain_question(true, Some(&file), "why?"), "File: a.txt\n\nwhy?");
        assert_eq!(plain_question(false, Some(&file), "why?"), "why?");
        let win = ChatContext::Window { app_name: "Code".into(), title: "main.rs".into(), url: None };
        assert_eq!(plain_question(true, Some(&win), "q"), "Context — App: Code, Window: main.rs\n\nq");
        assert_eq!(window_line("Edge", "Docs", Some("https://x.dev")), "Context — App: Edge, Window: Docs, URL: https://x.dev");
    }

    #[test]
    fn only_files_in_the_inbox_ride_along() {
        let base = std::env::temp_dir().join(format!("fedo-chat-ctx-{}", std::process::id()));
        let inbox = base.join("inbox");
        std::fs::create_dir_all(inbox.join("sub")).unwrap();
        std::fs::write(inbox.join("a.txt"), b"a").unwrap();
        std::fs::write(base.join("secret.txt"), b"s").unwrap();
        std::fs::write(inbox.join("sub").join("b.txt"), b"b").unwrap();
        assert!(is_inside(&inbox, &inbox.join("a.txt")));
        assert!(!is_inside(&inbox, &base.join("secret.txt")));
        assert!(!is_inside(&inbox, &inbox.join("..").join("secret.txt")));
        assert!(!is_inside(&inbox, &inbox.join("sub").join("b.txt")));
        assert!(!is_inside(&inbox, &inbox.join("missing.txt")));
        assert!(!is_inside(&inbox, &inbox));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(base.join("secret.txt"), inbox.join("link.txt")).unwrap();
            assert!(!is_inside(&inbox, &inbox.join("link.txt")));
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn stream_lines_are_read_as_they_come() {
        let d = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"po"}}}"#;
        assert_eq!(parse_line(d), Line::Delta("po".into()));
        let stop = r#"{"type":"stream_event","event":{"type":"message_stop"}}"#;
        assert_eq!(parse_line(stop), Line::Other);
        let r = r#"{"type":"result","result":"pong","session_id":"abc","is_error":false}"#;
        assert_eq!(
            parse_line(r),
            Line::Result { text: "pong".into(), session_id: Some("abc".into()), is_error: false }
        );
        assert_eq!(parse_line("not json"), Line::Other);
    }

    #[test]
    fn rate_limit_events_become_status_line_limits() {
        let l = r#"{"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"five_hour":{"utilization":0.23,"resetsAt":1791649800},"seven_day":{"utilization":0.54,"resetsAt":1791975600}}}}"#;
        let Line::Usage(v) = parse_line(l) else { panic!("not usage") };
        assert_eq!(v["five_hour"]["resets_at"], 1791649800);
        assert!((v["seven_day"]["used_percentage"].as_f64().unwrap() - 54.0).abs() < 1e-9);
        let none = r#"{"type":"rate_limit_event","rate_limit_info":{}}"#;
        assert_eq!(parse_line(none), Line::Other);
    }

    #[test]
    fn a_follow_up_resumes_and_may_not_write() {
        let first = turn_args("hi", "sys", None, None);
        assert!(!first.contains(&"--resume".to_string()));
        let next = turn_args("again", "sys", Some("id-1"), Some("/inbox"));
        let at = next.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(next[at + 1], "id-1");
        let tools = next.iter().position(|a| a == "--allowedTools").unwrap();
        assert!(!next[tools + 1].contains("Bash") && !next[tools + 1].contains("Write") && !next[tools + 1].contains("Edit"));
    }

    #[test]
    fn a_dropped_file_is_named_by_path_in_the_first_turn_only() {
        let f = ChatContext::File { name: "a.pdf".into(), path: "/inbox/a.pdf".into() };
        assert!(prompt_for(true, Some(&f), "summarise").contains("/inbox/a.pdf"));
        assert_eq!(prompt_for(false, Some(&f), "and?"), "and?");
    }

    /// Real `claude`, real quota (a few tokens): `cargo test -p fedo chat -- --ignored`.
    #[test]
    #[ignore]
    fn a_real_turn_streams_resumes_and_reports_usage() {
        let slot = std::sync::Arc::new(Mutex::new(None));
        let sys = system_prompt_for(None);
        let mut deltas = 0;
        let mut usage = None;
        let mut a = turn_args("Remember the word MANGO. Reply with just: ok", &sys, None, None);
        a.extend(["--model".into(), "haiku".into()]);
        let first = run_claude(&a, &slot, |l| match l {
            Line::Delta(_) => deltas += 1,
            Line::Usage(v) => usage = Some(v.clone()),
            _ => {}
        })
        .expect("first turn");
        assert!(deltas > 0, "no streamed deltas");
        let usage = usage.expect("no rate_limit_event");
        assert!(usage["five_hour"]["used_percentage"].as_f64().is_some());
        let id = first.session_id.expect("session id");
        let mut b = turn_args("What word did I ask you to remember? One word.", &sys, Some(&id), None);
        b.extend(["--model".into(), "haiku".into()]);
        let second = run_claude(&b, &slot, |_| {}).expect("second turn");
        assert!(second.text.to_uppercase().contains("MANGO"), "no context: {}", second.text);
        assert_eq!(second.session_id.as_deref(), Some(id.as_str()));
        assert!(slot.lock().unwrap().is_none());
    }
}
