// Confirmable actions that reach other people or other devices (FEDO.md
// Phase 6): call, SMS, WhatsApp (ZapZap), Gmail, send a file to the phone.
// `build_*` make the card the user reads; `do_action` runs it once they press
// Enter. The card carries only plain parameters, and `do_action` checks every
// one again — nothing arrives in a shell, a number is digits, a file must be
// one of Fedo's own copies (the inbox).

use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::contacts::{self, Contact, Pick};
use crate::phone;
use crate::phoneplan::Channel;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    /// call | sms | whatsapp | whatsapp-file | gmail | share-phone
    pub kind: String,
    pub title: String,
    /// What will happen, line by line (who, which number, the message).
    pub lines: Vec<String>,
    /// Something the user should know ("Android may need one tap").
    pub note: Option<String>,
    /// Something that blocks or risks it ("no country code").
    pub warning: Option<String>,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionChoice {
    pub label: String,
    pub detail: String,
    pub action: Action,
}

pub enum Built {
    One(Action),
    Choose(String, Vec<ActionChoice>),
    Say(String),
}

// ── URLs ──────────────────────────────────────────────────────────────────────

pub fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `whatsapp://send?phone=…&text=…`: ZapZap is this scheme's handler, and
/// opens the chat with the message typed but not sent.
pub fn whatsapp_url(digits: &str, text: &str) -> String {
    if text.is_empty() {
        format!("whatsapp://send?phone={digits}")
    } else {
        format!("whatsapp://send?phone={digits}&text={}", pct(text))
    }
}

pub fn gmail_url(to: &str, subject: &str, body: &str) -> String {
    format!(
        "https://mail.google.com/mail/?view=cm&fs=1&to={}&su={}&body={}",
        pct(to),
        pct(subject),
        pct(body)
    )
}

/// What GNOME's file manager puts on the clipboard for a copied file; browsers
/// and chat apps take a paste of it as the file.
pub fn copied_file_payload(path: &str) -> String {
    let mut uri = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    format!("copy\n{uri}")
}

fn valid_email(s: &str) -> bool {
    let s = s.trim();
    s.len() <= 254
        && s.matches('@').count() == 1
        && !s.starts_with('@')
        && !s.ends_with('@')
        && s.split('@').nth(1).is_some_and(|d| d.contains('.'))
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b))
}

// ── Building cards ────────────────────────────────────────────────────────────

fn default_cc(configured: &str) -> String {
    let c: String = configured.chars().filter(|c| c.is_ascii_digit()).collect();
    if !c.is_empty() {
        return c;
    }
    // No setting: the time zone tells (only the one this machine is in).
    let tz = std::fs::read_link("/etc/localtime").map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    if tz.ends_with("Asia/Kolkata") || tz.ends_with("Asia/Calcutta") {
        "91".into()
    } else {
        String::new()
    }
}

/// One distinct number per person, shown as written.
fn distinct_numbers(c: &Contact) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for n in &c.numbers {
        let key: String = n.chars().filter(|c| c.is_ascii_digit()).collect();
        let key = key.trim_start_matches('0').to_string();
        let key = key.strip_prefix("91").filter(|k| k.len() == 10).map(str::to_string).unwrap_or(key);
        if !seen.contains(&key) {
            seen.push(key);
            out.push(n.clone());
        }
    }
    out
}

/// People → (contact, number) candidates, as one action each.
fn people_choices(
    who: &str,
    all: &[Contact],
    make: &dyn Fn(&Contact, &str) -> Action,
) -> Result<Built, String> {
    let picked: Vec<Contact> = match contacts::pick(all, who) {
        Pick::None => return Ok(Built::Say(format!("I couldn't find “{who}” in your phone's contacts."))),
        Pick::One(c) => vec![c],
        Pick::Many(v) => v,
    };
    let mut options: Vec<ActionChoice> = Vec::new();
    for c in &picked {
        for n in distinct_numbers(c) {
            options.push(ActionChoice { label: c.name.clone(), detail: n.clone(), action: make(c, &n) });
        }
    }
    match options.len() {
        0 => Ok(Built::Say(format!("{} has no phone number saved.", picked[0].name))),
        1 => Ok(Built::One(options.remove(0).action)),
        _ => Ok(Built::Choose(format!("Which {who}?"), options)),
    }
}

pub fn build_call(who: &str, all: &[Contact], device: &phone::Device) -> Result<Built, String> {
    let dev = device.clone();
    people_choices(who, all, &move |c, n| Action {
        kind: "call".into(),
        title: format!("Call {}", c.name),
        lines: vec![format!("{} · {n}", c.name), format!("from {}", dev.name)],
        note: Some("Your phone opens the dialer with the number; Android may still need one tap on Call.".into()),
        warning: None,
        params: json!({ "device": dev.id, "number": n, "name": c.name }),
    })
}

pub fn build_text(
    who: &str,
    message: &str,
    channel: Channel,
    all: &[Contact],
    device: Option<&phone::Device>,
    cc_setting: &str,
) -> Result<Built, String> {
    let cc = default_cc(cc_setting);
    let msg = message.to_string();
    let dev = device.cloned();
    match channel {
        Channel::Sms => {
            let Some(dev) = dev else {
                return Err("SMS goes through your phone, and no paired phone is reachable.".into());
            };
            people_choices(who, all, &move |c, n| Action {
                kind: "sms".into(),
                title: format!("Text {}", c.name),
                lines: vec![format!("To {} · {n}", c.name), format!("“{msg}”"), format!("via {}", dev.name)],
                note: Some("Sent as an SMS from your phone, right away.".into()),
                warning: None,
                params: json!({ "device": dev.id, "number": n, "text": msg, "name": c.name }),
            })
        }
        Channel::WhatsApp => people_choices(who, all, &move |c, n| {
            let digits = contacts::international_digits(n, &cc);
            Action {
                kind: "whatsapp".into(),
                title: format!("WhatsApp {}", c.name),
                lines: vec![format!("To {} · {n}", c.name), format!("“{msg}”")],
                note: Some("ZapZap opens the chat with this typed in. Press Enter there to send.".into()),
                warning: digits.is_none().then(|| {
                    "This number has no country code, and WhatsApp needs one. Set it in Settings → Phone, or save the contact with +country code.".to_string()
                }),
                params: json!({ "digits": digits.unwrap_or_default(), "text": msg, "name": c.name }),
            }
        }),
    }
}

fn inbox_file(path: &str) -> Result<std::path::PathBuf, String> {
    let p = Path::new(path);
    let inbox = crate::files::inbox_dir();
    let (Ok(dir), Ok(file)) = (inbox.canonicalize(), p.canonicalize()) else {
        return Err("That file isn't there any more.".into());
    };
    if file.parent() != Some(dir.as_path()) || !file.is_file() {
        return Err("Only a file dropped on the island can be sent.".into());
    }
    Ok(file)
}

pub fn build_share_phone(file_path: &str, device: &phone::Device) -> Result<Built, String> {
    let f = inbox_file(file_path)?;
    let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    Ok(Built::One(Action {
        kind: "share-phone".into(),
        title: format!("Send {name} to your phone"),
        lines: vec![name, format!("to {}", device.name)],
        note: Some("Your phone may ask to accept it; it lands in Downloads.".into()),
        warning: None,
        params: json!({ "device": device.id, "path": f.to_string_lossy() }),
    }))
}

pub fn build_whatsapp_file(who: Option<&str>, file_path: &str, all: &[Contact], cc_setting: &str) -> Result<Built, String> {
    let f = inbox_file(file_path)?;
    let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    let path = f.to_string_lossy().into_owned();
    let cc = default_cc(cc_setting);
    let make = move |c: Option<&Contact>, n: Option<&str>| {
        let digits = n.and_then(|n| contacts::international_digits(n, &cc)).unwrap_or_default();
        Action {
            kind: "whatsapp-file".into(),
            title: match c {
                Some(c) => format!("Send {name} to {} on WhatsApp", c.name),
                None => format!("Send {name} on WhatsApp"),
            },
            lines: vec![name.clone(), c.map(|c| format!("to {}", c.name)).unwrap_or_else(|| "choose the chat in ZapZap".into())],
            note: Some("ZapZap opens the chat and the file is on your clipboard: press Ctrl+V there, then Enter.".into()),
            warning: None,
            params: json!({ "digits": digits, "path": path }),
        }
    };
    match who {
        None => Ok(Built::One(make(None, None))),
        Some(w) => people_choices(w, all, &|c, n| make(Some(c), Some(n))),
    }
}

pub fn build_gmail(
    to_or_name: &str,
    subject: &str,
    body: &str,
    file_path: Option<&str>,
    all: &[Contact],
) -> Result<Built, String> {
    let file = match file_path {
        Some(p) => Some(inbox_file(p)?),
        None => None,
    };
    let make = |email: &str, who: Option<&str>| {
        let mut lines = vec![format!("To {}{email}", who.map(|w| format!("{w} · ")).unwrap_or_default())];
        if !subject.is_empty() {
            lines.push(format!("Subject: {subject}"));
        }
        if !body.is_empty() {
            lines.push(body.chars().take(240).collect::<String>());
        }
        if let Some(f) = &file {
            lines.push(format!("Attachment: {}", f.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
        }
        Action {
            kind: "gmail".into(),
            title: "Compose in Gmail".into(),
            lines,
            note: Some(if file.is_some() {
                "Gmail opens with this drafted. The file is selected in Files and on your clipboard: paste or drag it into the message, then send."
            } else {
                "Gmail opens with this drafted. Review and press Send there."
            }
            .into()),
            warning: None,
            params: json!({
                "to": email, "subject": subject, "body": body,
                "path": file.as_ref().map(|f| f.to_string_lossy().into_owned()),
            }),
        }
    };
    if valid_email(to_or_name) {
        return Ok(Built::One(make(to_or_name.trim(), None)));
    }
    let picked: Vec<Contact> = match contacts::pick(all, to_or_name) {
        Pick::None => return Ok(Built::Say(format!("I couldn't find “{to_or_name}” in your contacts, and it isn't an email address."))),
        Pick::One(c) => vec![c],
        Pick::Many(v) => v,
    };
    let mut options = Vec::new();
    for c in &picked {
        for e in &c.emails {
            options.push(ActionChoice { label: c.name.clone(), detail: e.clone(), action: make(e, Some(&c.name)) });
        }
    }
    match options.len() {
        0 => Ok(Built::Say(format!("{} has no email address saved.", picked[0].name))),
        1 => Ok(Built::One(options.remove(0).action)),
        _ => Ok(Built::Choose(format!("Which {to_or_name}?"), options)),
    }
}

// ── Doing ─────────────────────────────────────────────────────────────────────

fn open_url(url: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|_| "xdg-open isn't available.".to_string())
}

fn put_file_on_clipboard(path: &str) -> bool {
    use std::io::Write;
    let Ok(mut child) = Command::new("wl-copy")
        .args(["--type", "x-special/gnome-copied-files"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(copied_file_payload(path).as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

fn select_in_files(path: &str) {
    let ok = Command::new("nautilus")
        .arg("--select")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok();
    if !ok {
        if let Some(dir) = Path::new(path).parent() {
            let _ = open_url(&dir.to_string_lossy());
        }
    }
}

fn param<'a>(a: &'a Action, k: &str) -> &'a str {
    a.params[k].as_str().unwrap_or("")
}

/// "Ask Antigravity …": the card the user confirms before `agy -p` runs.
/// Antigravity's print mode edits files and runs commands on its own (a probe
/// with `--mode plan --sandbox` still created a file), so it is never started
/// without this card.
pub fn build_antigravity(prompt: &str) -> Built {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Built::Say("What should I ask Antigravity?".into());
    }
    if prompt.len() > 4000 {
        return Built::Say("That's too long to send to Antigravity from here.".into());
    }
    if crate::platform::find_on_path("agy").is_none() {
        return Built::Say("The `agy` command isn't on your PATH.".into());
    }
    Built::One(Action {
        kind: "antigravity".into(),
        title: "Ask Antigravity".into(),
        lines: vec![format!("“{prompt}”"), "Runs: agy -p … in your home folder".into()],
        note: Some("Antigravity may edit files and run commands to do this. Its answer comes back here.".into()),
        warning: None,
        params: json!({ "prompt": prompt }),
    })
}

fn run_antigravity(prompt: &str) -> Result<String, String> {
    use std::io::Read;
    let prompt = prompt.trim();
    if prompt.is_empty() || prompt.len() > 4000 {
        return Err("Nothing to ask.".into());
    }
    let agy = crate::platform::find_on_path("agy").ok_or("The `agy` command isn't on your PATH.")?;
    let mut child = Command::new(agy)
        .args(["-p", prompt, "--output-format", "json"])
        .current_dir(crate::platform::home_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start agy: {e}"))?;
    let mut out = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Antigravity took longer than 5 minutes; I stopped waiting.".into());
            }
        }
    }
    let raw = reader.join().unwrap_or_default();
    antigravity_answer(&raw)
}

/// The answer in `agy --output-format json`'s output.
pub fn antigravity_answer(raw: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(raw.trim()).map_err(|_| "Antigravity returned something unreadable.".to_string())?;
    let text = v["response"].as_str().unwrap_or("").trim();
    if v["status"].as_str().is_some_and(|s| s != "SUCCESS") {
        return Err(if text.is_empty() { format!("Antigravity reported {}.", v["status"]) } else { text.to_string() });
    }
    if text.is_empty() {
        return Err("Antigravity gave no answer.".into());
    }
    Ok(text.to_string())
}

/// Runs a confirmed action. Returns what to tell the user.
pub fn do_action(a: &Action) -> Result<String, String> {
    let result = do_action_inner(a);
    let detail = match a.kind.as_str() {
        "antigravity" => param(a, "prompt").to_string(),
        _ => a.lines.join(" | "),
    };
    crate::log::action(
        &a.kind,
        &detail,
        &match &result {
            Ok(_) => "ok".to_string(),
            Err(e) => format!("failed: {e}"),
        },
    );
    result
}

fn do_action_inner(a: &Action) -> Result<String, String> {
    match a.kind.as_str() {
        "antigravity" => run_antigravity(param(a, "prompt")),
        "call" => {
            let dev = reachable(param(a, "device"))?;
            phone::dial(&dev, param(a, "number"))?;
            Ok(format!("Asked your phone to dial {}. Tap Call on the phone if it asks.", param(a, "name")))
        }
        "sms" => {
            let dev = reachable(param(a, "device"))?;
            phone::send_sms(&dev, param(a, "number"), param(a, "text"))?;
            Ok(format!("SMS sent to {} from your phone.", param(a, "name")))
        }
        "share-phone" => {
            let dev = reachable(param(a, "device"))?;
            let f = inbox_file(param(a, "path"))?;
            phone::share_file(&dev, &f.to_string_lossy())?;
            Ok("Sent to your phone.".into())
        }
        "whatsapp" => {
            let digits = param(a, "digits");
            if !(7..=15).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err("That number needs a country code before WhatsApp can open it.".into());
            }
            open_url(&whatsapp_url(digits, param(a, "text")))?;
            Ok("ZapZap is opening the chat with your message typed. Press Enter there to send it.".into())
        }
        "whatsapp-file" => {
            let f = inbox_file(param(a, "path"))?;
            let p = f.to_string_lossy().into_owned();
            let digits = param(a, "digits");
            let clip = put_file_on_clipboard(&p);
            if (7..=15).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
                open_url(&whatsapp_url(digits, ""))?;
            } else {
                open_url("whatsapp://send")?;
            }
            Ok(if clip {
                "ZapZap is opening. The file is on your clipboard: press Ctrl+V in the chat, then Enter.".into()
            } else {
                select_in_files(&p);
                "ZapZap is opening, and the file is selected in Files: drag it into the chat (wl-copy isn't installed for the clipboard route).".into()
            })
        }
        "gmail" => {
            let to = param(a, "to");
            if !valid_email(to) {
                return Err("That isn't an email address.".into());
            }
            open_url(&gmail_url(to, param(a, "subject"), param(a, "body")))?;
            if let Some(p) = a.params["path"].as_str() {
                let f = inbox_file(p)?;
                let p = f.to_string_lossy().into_owned();
                let clip = put_file_on_clipboard(&p);
                select_in_files(&p);
                return Ok(if clip {
                    "Gmail is opening. The file is selected in Files and on your clipboard: paste it into the message.".into()
                } else {
                    "Gmail is opening, and the file is selected in Files: drag it into the message.".into()
                });
            }
            Ok("Gmail is opening with your draft. Press Send there.".into())
        }
        other => Err(format!("I don't know how to “{other}”.")),
    }
}

/// The device id must be one that is paired and reachable right now.
fn reachable(id: &str) -> Result<String, String> {
    let devs = phone::devices()?;
    match devs.iter().find(|d| d.id == id && d.paired && d.reachable) {
        Some(d) => Ok(d.id.clone()),
        // Never quietly act on a different phone than the card named.
        None => Err(phone::usable(&devs)
            .err()
            .map(|why| format!("That phone isn't reachable right now. {why}"))
            .unwrap_or_else(|| "That phone isn't reachable right now.".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn antigravity_answers_come_out_of_the_json() {
        let ok = r#"{"conversation_id":"x","status":"SUCCESS","response":"done\n","num_turns":1}"#;
        assert_eq!(antigravity_answer(ok).unwrap(), "done");
        assert!(antigravity_answer(r#"{"status":"ERROR","response":"quota exceeded"}"#).unwrap_err().contains("quota"));
        assert!(antigravity_answer(r#"{"status":"SUCCESS","response":""}"#).is_err());
        assert!(antigravity_answer("not json").is_err());
    }

    /// Live: really runs `agy -p` through the confirmed-action path.
    /// `cargo test -p fedo --lib live_antigravity -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_antigravity_answers() {
        let Built::One(action) = build_antigravity("Reply with exactly the word: pong. Do not use any tools.") else {
            panic!("no card");
        };
        assert_eq!(action.kind, "antigravity");
        let answer = do_action(&action).unwrap();
        println!("{answer:?}");
        assert!(answer.to_lowercase().contains("pong"));
    }

    #[test]
    fn nothing_is_asked_of_antigravity_without_words() {
        assert!(matches!(build_antigravity("   "), Built::Say(_)));
        assert!(matches!(build_antigravity(&"x".repeat(5000)), Built::Say(_)));
    }

    fn people() -> Vec<Contact> {
        let c = |n: &str, t: &[&str], e: &[&str]| Contact {
            name: n.into(),
            numbers: t.iter().map(|s| s.to_string()).collect(),
            emails: e.iter().map(|s| s.to_string()).collect(),
        };
        vec![
            c("Aparna", &["+91 98115 18797", "09811518797"], &[]),
            c("Pulkit Sharma", &["+911111111111"], &["p@example.com"]),
            c("Pulkit Dad", &["+912222222222"], &[]),
            c("HR Team", &[], &["hr@x.com", "careers@x.com"]),
        ]
    }

    fn phone_dev() -> phone::Device {
        phone::Device { id: "29c0302ef69748d1821bb28ebb3ac4d6".into(), name: "OnePlus".into(), paired: true, reachable: true }
    }

    #[test]
    fn links_are_built_and_encoded() {
        assert_eq!(whatsapp_url("919811518797", "I'll be right there"), "whatsapp://send?phone=919811518797&text=I%27ll%20be%20right%20there");
        assert_eq!(whatsapp_url("919811518797", ""), "whatsapp://send?phone=919811518797");
        let g = gmail_url("hr@x.com", "DS role", "Hi, I'm applying & attaching my CV.");
        assert!(g.starts_with("https://mail.google.com/mail/?view=cm&fs=1&to=hr%40x.com&su=DS%20role&body="));
        assert!(g.contains("%26") && !g.contains(' '));
        assert_eq!(copied_file_payload("/home/a b/x.pdf"), "copy\nfile:///home/a%20b/x.pdf");
    }

    #[test]
    fn a_call_card_names_the_number_and_the_tap() {
        let Built::One(a) = build_call("aparna", &people(), &phone_dev()).unwrap() else { panic!("one number expected") };
        assert_eq!(a.kind, "call");
        assert!(a.lines[0].contains("Aparna"));
        assert!(a.note.unwrap().contains("one tap"));
    }

    #[test]
    fn the_same_number_written_twice_is_one_choice_and_two_people_ask() {
        // Aparna's two spellings of one number collapse.
        assert!(matches!(build_call("aparna", &people(), &phone_dev()).unwrap(), Built::One(_)));
        match build_call("pulkit", &people(), &phone_dev()).unwrap() {
            Built::Choose(_, opts) => assert_eq!(opts.len(), 2),
            _ => panic!("two Pulkits should ask"),
        }
        assert!(matches!(build_call("zorg", &people(), &phone_dev()).unwrap(), Built::Say(_)));
    }

    #[test]
    fn whatsapp_without_a_country_code_says_so_instead_of_pretending() {
        let all = vec![Contact { name: "Local".into(), numbers: vec!["98115 18797".into()], emails: vec![] }];
        let Built::One(a) = build_text("local", "hi", Channel::WhatsApp, &all, None, "").unwrap() else { panic!() };
        // (In a time zone with no known default the digits are empty and a warning shows.)
        if a.params["digits"] == "" {
            assert!(a.warning.unwrap().contains("country code"));
        }
        let Built::One(b) = build_text("local", "hi", Channel::WhatsApp, &all, None, "91").unwrap() else { panic!() };
        assert_eq!(b.params["digits"], "919811518797");
        assert!(b.warning.is_none());
        assert!(do_action(&Action { params: json!({ "digits": "", "text": "hi" }), ..b.clone() }).is_err());
    }

    #[test]
    fn an_sms_needs_a_reachable_phone() {
        assert!(build_text("aparna", "hi", Channel::Sms, &people(), None, "").is_err());
        let Built::One(a) = build_text("aparna", "On my way", Channel::Sms, &people(), Some(&phone_dev()), "").unwrap() else { panic!() };
        assert_eq!(a.kind, "sms");
        assert!(a.lines.iter().any(|l| l.contains("On my way")));
    }

    #[test]
    fn email_goes_to_an_address_or_a_contacts_address() {
        let Built::One(a) = build_gmail("hr@x.com", "DS role", "hello", None, &people()).unwrap() else { panic!() };
        assert_eq!(a.params["to"], "hr@x.com");
        match build_gmail("hr team", "s", "b", None, &people()).unwrap() {
            Built::Choose(_, o) => assert_eq!(o.len(), 2),
            _ => panic!("two addresses should ask"),
        }
        assert!(matches!(build_gmail("aparna", "s", "b", None, &people()).unwrap(), Built::Say(_)));
        assert!(!valid_email("a@b"));
        assert!(!valid_email("x@y.com; rm"));
    }

    #[test]
    fn only_files_in_the_inbox_can_be_sent() {
        assert!(build_share_phone("/etc/passwd", &phone_dev()).is_err());
        assert!(build_gmail("hr@x.com", "s", "b", Some("/etc/passwd"), &people()).is_err());
        assert!(build_whatsapp_file(None, "/home/nobody/x", &people(), "").is_err());
    }

    #[test]
    fn running_a_card_rechecks_its_parameters() {
        let bad = Action {
            kind: "gmail".into(), title: String::new(), lines: vec![], note: None, warning: None,
            params: json!({ "to": "x@y.com\nBcc: z@q.com", "subject": "", "body": "" }),
        };
        assert!(do_action(&bad).is_err());
        let unknown = Action { kind: "format-disk".into(), ..bad };
        assert!(do_action(&unknown).is_err());
    }

    /// Opens real windows (ZapZap, a browser tab, Files) with a harmless
    /// draft, then checks the clipboard holds the file.
    /// `cargo test -p fedo --lib actions::tests::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_launchers() {
        let inbox = crate::files::inbox_dir();
        std::fs::create_dir_all(&inbox).unwrap();
        let file = inbox.join("fedo-live-test.txt");
        std::fs::write(&file, b"hello from fedo").unwrap();
        let path = file.to_string_lossy().into_owned();

        let wa = Action {
            kind: "whatsapp".into(), title: String::new(), lines: vec![], note: None, warning: None,
            params: json!({ "digits": "919999999999", "text": "Fedo test - do not send" }),
        };
        println!("whatsapp: {:?}", do_action(&wa));

        let gm = Action {
            kind: "gmail".into(), title: String::new(), lines: vec![], note: None, warning: None,
            params: json!({ "to": "fedo-test@example.com", "subject": "Fedo test", "body": "Draft only", "path": path }),
        };
        println!("gmail: {:?}", do_action(&gm));
        std::thread::sleep(std::time::Duration::from_millis(800));
        let out = Command::new("wl-paste").args(["--type", "x-special/gnome-copied-files"]).output().unwrap();
        let clip = String::from_utf8_lossy(&out.stdout).to_string();
        println!("clipboard: {clip:?}");
        assert!(clip.contains("fedo-live-test.txt"));
        let _ = std::fs::remove_file(&file);
    }
}
