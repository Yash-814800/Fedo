// The chat's conversation state. No provider answers it yet: Phase 0 removed
// every API-key chat provider (claude.rs, openai_compat.rs, local_chat.rs) per
// FEDO.md §0 ("no API keys anywhere"). Phase 3 wires `send` to `claude -p`
// instead — the Conversation/Turn bookkeeping below is kept because that
// rewrite still needs a plain-text history, just no longer one kept twice in
// two wire formats.

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

/// A model a provider offers, for the picker in the chat view. Unused until
/// Phase 3 — no provider lists any today.
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
}

#[derive(Default)]
struct Conversation {
    /// Bumped by every reset, so an answer that lands after "New chat" is dropped.
    epoch: u64,
    /// `{"role", "content": text}` turns.
    plain: Vec<Value>,
}

pub struct Turn {
    epoch: u64,
    /// No earlier turn: the file or window context rides along with this one.
    pub first: bool,
}

impl Chat {
    pub fn reset(&self) {
        let mut c = self.inner.lock().unwrap();
        let epoch = c.epoch + 1;
        *c = Conversation { epoch, ..Default::default() };
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

/// Not wired to anything yet — Phase 3 routes this through `claude -p`.
pub async fn send(
    _app: &AppHandle,
    _chat: &Chat,
    _settings: &Settings,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let _ = query;
    if let Some(context) = context {
        checked_context(context)?;
    }
    Err(crate::i18n::t("Chat isn't connected yet in this build."))
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
}
