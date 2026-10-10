// "Monday morning on first unlock": GNOME's screen saver says when the session
// is unlocked (org.gnome.ScreenSaver.ActiveChanged(false)). Event-driven — the
// thread sleeps in poll(2) between signals. On an unlock that is the first one
// of a Monday after 8:00 with no recap shown yet this week, the island is told
// to run its recap check (src/recap/recap.ts decides what to show).
//
// Anything else — a Wayland session without GNOME's screen saver, no bus —
// just means the other triggers (launch, an agent starting work, the island
// waking) are the ones that apply.

use std::time::Duration;

use dbus::blocking::Connection;
use dbus::message::MatchRule;
use tauri::AppHandle;

use crate::{log, recap};

pub fn start(app: AppHandle) {
    let spawned = std::thread::Builder::new().name("fedo-unlock".into()).spawn(move || {
        let Ok(conn) = Connection::new_session() else { return };
        let rule = MatchRule::new_signal("org.gnome.ScreenSaver", "ActiveChanged");
        let app2 = app.clone();
        let added = conn.add_match(rule, move |(active,): (bool,), _, _| {
            if !active && recap::due_now(&app2) {
                log::line("unlock: Monday recap is due");
                crate::island::emit(&app2, "recap-check", ());
            }
            true
        });
        if added.is_err() {
            return;
        }
        loop {
            if conn.process(Duration::from_secs(3600)).is_err() {
                break;
            }
        }
    });
    if spawned.is_err() {
        log::line("unlock watcher did not start");
    }
}
