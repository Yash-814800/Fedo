// The local model backend (Ollama), an alternative to the `claude` CLI for every
// request Fedo makes itself: the router, the chat, file plans, the prompt writer
// and email drafts. Chosen by one setting (Settings → AI); Claude Code's own
// sessions and hooks are unaffected, they are Claude Code.
//
// Talks to Ollama's HTTP API on the address the user configured (default
// 127.0.0.1:11434). Nothing is sent anywhere else. A request that needs a
// *structured* answer asks Ollama for JSON that fits a schema (`format`), and
// the caller validates it again, exactly as it does for Claude.
//
// What a small local model cannot do, Fedo does for it: it has no tools, so the
// file search and reading a dropped file happen in Rust and the results are put
// in the prompt.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

use crate::settings::Settings;

#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    pub url: String,
    pub model: String,
}

static CONFIG: Mutex<Option<Local>> = Mutex::new(None);

/// Applies the settings: `Some` when requests should go to the local model.
pub fn configure(s: &Settings) {
    let local = (s.ai_backend == "local" && !s.local_model.trim().is_empty()).then(|| Local {
        url: normalize_url(&s.ollama_url),
        model: s.local_model.trim().to_string(),
    });
    *CONFIG.lock().unwrap() = local;
}

/// The local backend, if it is the one chosen.
pub fn local() -> Option<Local> {
    CONFIG.lock().unwrap().clone()
}

pub fn normalize_url(u: &str) -> String {
    let u = u.trim().trim_end_matches('/');
    if u.is_empty() {
        return "http://127.0.0.1:11434".into();
    }
    if u.starts_with("http://") || u.starts_with("https://") {
        u.to_string()
    } else {
        format!("http://{u}")
    }
}

fn run<T>(f: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(f)
}

fn client(limit: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder().timeout(limit).build().map_err(|e| e.to_string())
}

fn friendly(url: &str, e: &reqwest::Error) -> String {
    if e.is_connect() {
        format!("Ollama isn't answering at {url}. Start it with `ollama serve` (or `systemctl start ollama`).")
    } else if e.is_timeout() {
        "The local model took too long to answer.".to_string()
    } else {
        format!("Local model error: {e}")
    }
}

async fn check(url: &str, model: &str, r: reqwest::Response) -> Result<reqwest::Response, String> {
    if r.status().is_success() {
        return Ok(r);
    }
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    let msg = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or(body);
    if status.as_u16() == 404 || msg.contains("not found") {
        return Err(format!("The model “{model}” isn't installed. Run: ollama pull {model}"));
    }
    Err(format!("Ollama ({url}): {}", msg.chars().take(200).collect::<String>()))
}

/// The body of an /api/chat request.
pub fn chat_body(model: &str, messages: &[Value], stream: bool, format: Option<Value>, predict: u32, ctx: u32) -> Value {
    let mut b = json!({
        "model": model,
        "messages": messages,
        "stream": stream,
        // Thinking models (Qwen3…) would spend their budget thinking.
        "think": false,
        "options": { "temperature": 0.2, "num_ctx": ctx, "num_predict": predict },
    });
    if let Some(f) = format {
        b["format"] = f;
        b["options"]["temperature"] = json!(0);
    }
    b
}

/// One request that must come back as JSON fitting `schema`.
pub fn json(l: &Local, system: &str, prompt: &str, schema: &str, limit: Duration) -> Result<Value, String> {
    let schema: Value = serde_json::from_str(schema).map_err(|e| e.to_string())?;
    let msgs = [json!({"role":"system","content":system}), json!({"role":"user","content":prompt})];
    let body = chat_body(&l.model, &msgs, false, Some(schema), 700, 4096);
    let out = run(async {
        let c = client(limit)?;
        let r = c.post(format!("{}/api/chat", l.url)).json(&body).send().await.map_err(|e| friendly(&l.url, &e))?;
        let r = check(&l.url, &l.model, r).await?;
        r.json::<Value>().await.map_err(|e| e.to_string())
    })?;
    let content = out["message"]["content"].as_str().unwrap_or("");
    parse_json_object(content).ok_or_else(|| "The local model didn't return the JSON asked for.".to_string())
}

/// Plain text from the model.
pub fn text(l: &Local, system: &str, prompt: &str, limit: Duration) -> Result<String, String> {
    let msgs = [json!({"role":"system","content":system}), json!({"role":"user","content":prompt})];
    let body = chat_body(&l.model, &msgs, false, None, 900, 4096);
    let out = run(async {
        let c = client(limit)?;
        let r = c.post(format!("{}/api/chat", l.url)).json(&body).send().await.map_err(|e| friendly(&l.url, &e))?;
        let r = check(&l.url, &l.model, r).await?;
        r.json::<Value>().await.map_err(|e| e.to_string())
    })?;
    let t = strip_think(out["message"]["content"].as_str().unwrap_or("")).trim().to_string();
    if t.is_empty() {
        Err("The local model returned nothing.".into())
    } else {
        Ok(t)
    }
}

/// A conversation turn, streamed: `on_delta` gets each piece as it arrives.
pub fn chat_stream(
    l: &Local,
    system: &str,
    history: &[Value],
    user: &str,
    limit: Duration,
    mut on_delta: impl FnMut(&str),
) -> Result<String, String> {
    let mut msgs = vec![json!({"role":"system","content":system})];
    // The last few turns only: a 4k window is all a small model has to spare.
    let keep = history.len().saturating_sub(8);
    msgs.extend(history[keep..].iter().cloned());
    msgs.push(json!({"role":"user","content":user}));
    let body = chat_body(&l.model, &msgs, true, None, 1200, 8192);
    run(async {
        let c = client(limit)?;
        let mut r = c.post(format!("{}/api/chat", l.url)).json(&body).send().await.map_err(|e| friendly(&l.url, &e))?;
        r = check(&l.url, &l.model, r).await?;
        let mut buf = String::new();
        let mut all = String::new();
        loop {
            match r.chunk().await {
                Ok(Some(chunk)) => {
                    buf.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(i) = buf.find('\n') {
                        let line: String = buf.drain(..=i).collect();
                        if let Some(piece) = delta_of(&line) {
                            all.push_str(&piece);
                            on_delta(&piece);
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(friendly(&l.url, &e)),
            }
        }
        if let Some(piece) = delta_of(&buf) {
            all.push_str(&piece);
            on_delta(&piece);
        }
        Ok(strip_think(&all).trim().to_string())
    })
}

/// The text piece of one NDJSON line of a streamed chat.
pub fn delta_of(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    v["message"]["content"].as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// `<think>…</think>` blocks some models emit even when asked not to.
pub fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if b < a {
            break;
        }
        out.replace_range(a..b + "</think>".len(), "");
    }
    out
}

/// A JSON object out of model text: the whole string, or the outermost braces.
pub fn parse_json_object(s: &str) -> Option<Value> {
    let s = strip_think(s);
    let s = s.trim();
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        if v.is_object() {
            return Some(v);
        }
    }
    let (a, b) = (s.find('{')?, s.rfind('}')?);
    serde_json::from_str::<Value>(&s[a..=b]).ok().filter(Value::is_object)
}

// ── Status, for Settings ──────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub models: Vec<String>,
    pub error: Option<String>,
}

pub fn status(url: &str) -> Status {
    let url = normalize_url(url);
    let r = run(async {
        let c = client(Duration::from_secs(3))?;
        let r = c.get(format!("{url}/api/tags")).send().await.map_err(|e| friendly(&url, &e))?;
        r.json::<Value>().await.map_err(|e| e.to_string())
    });
    match r {
        Ok(v) => Status {
            running: true,
            models: v["models"]
                .as_array()
                .map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            error: None,
        },
        Err(e) => Status { running: false, models: vec![], error: Some(e) },
    }
}

#[tauri::command]
pub async fn local_ai_status(url: String) -> Status {
    tauri::async_runtime::spawn_blocking(move || status(&url))
        .await
        .unwrap_or(Status { running: false, models: vec![], error: Some("status failed".into()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_normalised() {
        assert_eq!(normalize_url(""), "http://127.0.0.1:11434");
        assert_eq!(normalize_url("localhost:11434/"), "http://localhost:11434");
        assert_eq!(normalize_url("https://x.local"), "https://x.local");
    }

    #[test]
    fn model_text_becomes_json_even_when_wrapped() {
        assert_eq!(parse_json_object(r#"{"a":1}"#).unwrap()["a"], 1);
        assert_eq!(parse_json_object("Sure!\n```json\n{\"a\": 2}\n```").unwrap()["a"], 2);
        assert_eq!(parse_json_object("<think>hmm {x}</think>{\"a\":3}").unwrap()["a"], 3);
        assert!(parse_json_object("no json here").is_none());
        assert!(parse_json_object("[1,2]").is_none());
    }

    #[test]
    fn streamed_lines_give_their_text() {
        assert_eq!(delta_of(r#"{"message":{"role":"assistant","content":"Hel"},"done":false}"#).as_deref(), Some("Hel"));
        assert_eq!(delta_of(r#"{"message":{"content":""},"done":true}"#), None);
        assert_eq!(delta_of("not json"), None);
        assert_eq!(strip_think("a<think>x</think>b"), "ab");
    }

    #[test]
    fn the_request_asks_for_no_thinking_and_a_small_window() {
        let b = chat_body("m", &[], false, Some(json!({"type":"object"})), 100, 4096);
        assert_eq!(b["think"], false);
        assert_eq!(b["options"]["num_ctx"], 4096);
        assert_eq!(b["format"]["type"], "object");
        assert_eq!(b["options"]["temperature"], 0);
    }

    #[test]
    fn only_the_local_backend_setting_turns_it_on() {
        let mut s = Settings::default();
        configure(&s);
        assert!(local().is_none());
        s.ai_backend = "local".into();
        s.local_model = "qwen3:4b-instruct-2507-q4_K_M".into();
        configure(&s);
        assert_eq!(local().unwrap().model, "qwen3:4b-instruct-2507-q4_K_M");
        s.local_model = "  ".into();
        configure(&s);
        assert!(local().is_none());
        s.ai_backend = "claude".into();
        configure(&s);
    }
}
