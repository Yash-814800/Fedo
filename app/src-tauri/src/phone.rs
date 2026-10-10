// The phone (FEDO.md Phase 6), through KDE Connect.
//
// Reading goes over its D-Bus API (`org.kde.kdeconnect`): which devices exist,
// which are paired and reachable, the phone's active notifications, and signals
// for new notifications and incoming calls. Doing goes through `kdeconnect-cli`
// with arguments passed one by one (never a shell): share a file or a `tel:`
// link, send an SMS. Nothing here runs without the island's confirmation — the
// callers are `assist.rs` actions the user pressed Enter on.
//
// Plugin objects (/notifications, /sms, /share) only exist while the phone is
// reachable, so every call says plainly when it isn't.

use std::process::{Command, Stdio};
use std::time::Duration;

use dbus::arg::{RefArg, Variant};
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use serde::Serialize;
use tauri::AppHandle;

const NAME: &str = "org.kde.kdeconnect";
const DAEMON_PATH: &str = "/modules/kdeconnect";
const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub paired: bool,
    pub reachable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneNotification {
    pub id: String,
    pub app: String,
    pub title: String,
    pub text: String,
}

fn conn() -> Result<Connection, String> {
    Connection::new_session().map_err(|_| "No session bus.".to_string())
}

fn is_hex_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'_')
}

fn device_path(id: &str) -> String {
    format!("{DAEMON_PATH}/devices/{id}")
}

/// Every device KDE Connect knows, paired or not, reachable or not.
pub fn devices() -> Result<Vec<Device>, String> {
    let c = conn()?;
    let daemon = c.with_proxy(NAME, DAEMON_PATH, TIMEOUT);
    let ids: Result<(Vec<String>,), _> = daemon.method_call("org.kde.kdeconnect.daemon", "devices", (false, false));
    let (ids,) = ids.map_err(|_| "KDE Connect isn't running. Start it from your app menu, then try again.".to_string())?;
    let mut out = Vec::new();
    for id in ids.into_iter().filter(|i| is_hex_id(i)) {
        let p = c.with_proxy(NAME, device_path(&id), TIMEOUT);
        let all: std::collections::HashMap<String, Variant<Box<dyn RefArg>>> =
            p.get_all("org.kde.kdeconnect.device").unwrap_or_default();
        let name = all.get("name").and_then(|v| v.0.as_str().map(str::to_string)).unwrap_or_else(|| id.clone());
        let flag = |k: &str| all.get(k).and_then(|v| v.0.as_i64()).unwrap_or(0) != 0;
        let (paired, reachable) = (flag("isPaired"), flag("isReachable"));
        out.push(Device { id, name, paired, reachable });
    }
    Ok(out)
}

/// The phone to act on: paired and reachable. If none is, says what is the case
/// so the card can tell the user what to fix.
pub fn usable(devs: &[Device]) -> Result<Device, String> {
    if let Some(d) = devs.iter().find(|d| d.paired && d.reachable) {
        return Ok(d.clone());
    }
    let paired_off: Vec<&str> = devs.iter().filter(|d| d.paired && !d.reachable).map(|d| d.name.as_str()).collect();
    let unpaired_on: Vec<&str> = devs.iter().filter(|d| !d.paired && d.reachable).map(|d| d.name.as_str()).collect();
    let mut why = String::from("No paired phone is reachable.");
    if !paired_off.is_empty() {
        why.push_str(&format!(" {} is paired but not on the network.", paired_off.join(", ")));
    }
    if !unpaired_on.is_empty() {
        why.push_str(&format!(" {} is on the network but not paired — pair it in KDE Connect.", unpaired_on.join(", ")));
    }
    if devs.is_empty() {
        why.push_str(" KDE Connect has no devices yet.");
    }
    Err(why)
}

pub fn usable_device() -> Result<Device, String> {
    usable(&devices()?)
}

/// The phone's active notifications (app, title, text).
pub fn notifications(device_id: &str) -> Result<Vec<PhoneNotification>, String> {
    if !is_hex_id(device_id) {
        return Err("bad device".into());
    }
    let c = conn()?;
    let base = format!("{}/notifications", device_path(device_id));
    let p = c.with_proxy(NAME, base.clone(), TIMEOUT);
    let r: Result<(Vec<String>,), _> = p.method_call("org.kde.kdeconnect.device.notifications", "activeNotifications", ());
    let (ids,) = r.map_err(|_| "The phone's notifications aren't available (is the Notifications plugin on?).".to_string())?;
    let mut out = Vec::new();
    for id in ids.into_iter().filter(|i| i.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-.:|".contains(&b))) {
        if let Some(n) = notification(&c, device_id, &id) {
            out.push(n);
        }
    }
    Ok(out)
}

fn notification(c: &Connection, device_id: &str, id: &str) -> Option<PhoneNotification> {
    let path = format!("{}/notifications/{}", device_path(device_id), id);
    let p = c.with_proxy(NAME, path, TIMEOUT);
    let all: std::collections::HashMap<String, Variant<Box<dyn RefArg>>> =
        p.get_all("org.kde.kdeconnect.device.notifications.notification").ok()?;
    let s = |k: &str| all.get(k).and_then(|v| v.0.as_str()).unwrap_or("").to_string();
    Some(PhoneNotification { id: id.to_string(), app: s("appName"), title: s("title"), text: s("text") })
}

/// "Messages — Aparna: I'll be there" lines for the island.
pub fn describe(list: &[PhoneNotification]) -> String {
    if list.is_empty() {
        return "No notifications on your phone.".into();
    }
    let mut out = format!("{} on your phone:\n", list.len());
    for n in list.iter().take(12) {
        let head = if n.title.is_empty() { n.app.clone() } else { format!("{} — {}", n.app, n.title) };
        let text: String = n.text.chars().take(120).collect();
        out.push_str(&format!("• {head}{}\n", if text.is_empty() { String::new() } else { format!(": {text}") }));
    }
    out.trim_end().to_string()
}

// ── Doing: kdeconnect-cli, one argument at a time ─────────────────────────────

fn cli(device_id: &str, args: &[&str]) -> Result<(), String> {
    if !is_hex_id(device_id) {
        return Err("bad device".into());
    }
    let out = Command::new("kdeconnect-cli")
        .arg("-d")
        .arg(device_id)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "kdeconnect-cli isn't installed.".to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr);
        Err(msg.lines().next().filter(|l| !l.trim().is_empty()).unwrap_or("KDE Connect refused.").to_string())
    }
}

/// Asks the phone to open the dialer on `number` (a shared `tel:` link).
/// Android may still need one tap on Call.
pub fn dial(device_id: &str, number: &str) -> Result<(), String> {
    let n = crate::contacts::dialable(number);
    if n.len() < 3 {
        return Err("That number doesn't look right.".into());
    }
    cli(device_id, &["--share", &format!("tel:{n}")])
}

pub fn send_sms(device_id: &str, number: &str, text: &str) -> Result<(), String> {
    let n = crate::contacts::dialable(number);
    if n.len() < 3 || text.trim().is_empty() || text.len() > 1500 {
        return Err("Nothing to send.".into());
    }
    cli(device_id, &["--send-sms", text, "--destination", &n])
}

pub fn share_file(device_id: &str, path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() || !p.is_file() {
        return Err("That file isn't there any more.".into());
    }
    cli(device_id, &["--share", path])
}

// ── Listening: new notifications and incoming calls ───────────────────────────

/// Whether a phone notification from `app` should peek the island.
pub fn wanted(app: &str, apps: &[String]) -> bool {
    let a = app.to_lowercase();
    apps.iter().any(|x| !x.trim().is_empty() && a.contains(&x.trim().to_lowercase()))
}

fn device_of(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/modules/kdeconnect/devices/")?;
    let id = rest.split('/').next()?;
    is_hex_id(id).then(|| id.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneEvent {
    /// "notification" or "call".
    pub kind: String,
    pub title: String,
    pub text: String,
}

/// Starts the signal listener (event-driven; the thread sleeps in poll).
pub fn start_listener(app: AppHandle) {
    let spawned = std::thread::Builder::new().name("fedo-phone".into()).spawn(move || {
        let Ok(c) = Connection::new_session() else { return };
        let app_n = app.clone();
        let note = MatchRule::new_signal("org.kde.kdeconnect.device.notifications", "notificationPosted");
        let _ = c.add_match(note, move |(): (), conn, msg| {
            handle_notification(&app_n, conn, msg);
            true
        });
        let app_c = app.clone();
        let call = MatchRule::new_signal("org.kde.kdeconnect.device.telephony", "callReceived");
        let _ = c.add_match(call, move |(): (), _, msg| {
            handle_call(&app_c, msg);
            true
        });
        loop {
            if c.process(Duration::from_secs(3600)).is_err() {
                break;
            }
        }
    });
    if spawned.is_err() {
        crate::log::line("phone listener did not start");
    }
}

fn settings_of(app: &AppHandle) -> crate::settings::Settings {
    use tauri::Manager;
    app.state::<crate::Shared>().settings.lock().unwrap().clone()
}

fn handle_notification(app: &AppHandle, conn: &Connection, msg: &dbus::Message) {
    let s = settings_of(app);
    if !s.phone_notify {
        return;
    }
    let (Some(path), Some(id)) = (msg.path(), msg.get1::<String>()) else { return };
    let Some(dev) = device_of(&path) else { return };
    let Some(n) = notification(conn, &dev, &id) else { return };
    if !wanted(&n.app, &s.phone_notify_apps) {
        return;
    }
    let title = if n.title.is_empty() { n.app.clone() } else { format!("{} — {}", n.app, n.title) };
    crate::island::emit(app, "phone-event", PhoneEvent { kind: "notification".into(), title, text: n.text });
}

fn handle_call(app: &AppHandle, msg: &dbus::Message) {
    let s = settings_of(app);
    if !s.phone_notify {
        return;
    }
    // callReceived(s event, s phoneNumber, s contactName)
    let Ok((event, number, name)) = msg.read3::<String, String, String>() else { return };
    if !event.to_lowercase().contains("ring") {
        return;
    }
    let who = if name.is_empty() || name == number { number } else { name };
    crate::island::emit(
        app,
        "phone-event",
        PhoneEvent { kind: "call".into(), title: "Incoming call".into(), text: who },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(name: &str, paired: bool, reachable: bool) -> Device {
        Device { id: format!("{:032x}", name.len()), name: name.into(), paired, reachable }
    }

    #[test]
    fn the_phone_to_use_is_paired_and_reachable_and_the_error_says_what_is_wrong() {
        assert_eq!(usable(&[d("Pixel", false, true), d("OnePlus", true, true)]).unwrap().name, "OnePlus");
        let e = usable(&[d("Pixel 9", false, true), d("OnePlus 11R", true, false)]).unwrap_err();
        assert!(e.contains("OnePlus 11R is paired but not on the network"), "{e}");
        assert!(e.contains("Pixel 9 is on the network but not paired"), "{e}");
        assert!(usable(&[]).unwrap_err().contains("no devices"));
    }

    #[test]
    fn notifications_read_like_a_list() {
        let list = vec![
            PhoneNotification { id: "1".into(), app: "Messages".into(), title: "Aparna".into(), text: "On my way".into() },
            PhoneNotification { id: "2".into(), app: "Gmail".into(), title: "".into(), text: "".into() },
        ];
        let t = describe(&list);
        assert!(t.starts_with("2 on your phone:"));
        assert!(t.contains("• Messages — Aparna: On my way"));
        assert!(t.contains("• Gmail"));
        assert_eq!(describe(&[]), "No notifications on your phone.");
    }

    #[test]
    fn only_wanted_apps_peek() {
        let apps = vec!["Messages".to_string(), "phone".to_string(), " ".to_string()];
        assert!(wanted("Google Messages", &apps));
        assert!(wanted("Phone", &apps));
        assert!(!wanted("Instagram", &apps));
        assert!(!wanted("Anything", &[]));
    }

    #[test]
    fn paths_and_ids_are_checked_before_use() {
        assert_eq!(
            device_of("/modules/kdeconnect/devices/29c0302ef69748d1821bb28ebb3ac4d6/notifications").as_deref(),
            Some("29c0302ef69748d1821bb28ebb3ac4d6")
        );
        assert_eq!(device_of("/modules/kdeconnect/devices/../../etc"), None);
        assert!(!is_hex_id("abc; rm"));
        assert!(dial("not hex!", "123456").is_err());
        assert!(dial("29c0302e", "x").is_err());
    }

    /// Against the running daemon: lists the devices (no phone needed).
    /// `cargo test -p fedo --lib phone -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_devices_and_the_reason_nothing_is_usable() {
        let devs = devices().unwrap();
        println!("{devs:#?}");
        println!("{:?}", usable(&devs));
        assert!(!devs.is_empty());
    }
}
