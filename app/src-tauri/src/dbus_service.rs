// org.fedo.Fedo on the session bus: what the GNOME Shell extension (and
// `busctl`, and later the systemd user service) talks to.
//
//   object    /org/fedo/Fedo
//   methods   Toggle()  Show(s view)  Hide()  FocusSession(s id)  PushToTalk()  Quit()
//   props     State       s   "idle" | "working" | "needs-you"  (the icon's dot)
//             Visibility  s   "hidden" | "peek" | "open"        (the island)
//             Muted       b   sounds off; read/write, saved in settings.json
//   signal    StateChanged(s state)
//
// Event-driven: the thread sleeps in libdbus's poll until a call arrives. The
// island page is the source of truth for both properties (it owns the tasks and
// the approval card); it reports them through `report_state` in lib.rs.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dbus::blocking::SyncConnection;
use dbus::channel::{MatchingReceiver, Sender};
use dbus::message::{MatchRule, SignalArgs};
use dbus::Message;
use dbus_crossroads::{Crossroads, MethodErr};
use tauri::AppHandle;

use crate::log;

pub const BUS_NAME: &str = "org.fedo.Fedo";
pub const OBJECT_PATH: &str = "/org/fedo/Fedo";
const INTERFACE: &str = "org.fedo.Fedo";

#[derive(Clone, PartialEq, Debug)]
struct Snapshot {
    state: String,
    visibility: String,
    muted: bool,
}

/// Held in Tauri's managed state so `report` can reach the connection.
pub struct Service {
    conn: Arc<SyncConnection>,
    current: Arc<Mutex<Snapshot>>,
}

/// Who may be named in `Show`. Anything else falls back to the default view.
fn known_view(view: &str) -> bool {
    matches!(view, "" | "overview" | "settings" | "mail" | "prompt")
}

/// Registers the name and serves calls on a thread of its own. Quietly logs and
/// carries on when the bus or the name is not available: Fedo still works
/// without the extension.
pub fn start(app: &AppHandle) {
    use tauri::Manager;

    let conn = match SyncConnection::new_session() {
        Ok(c) => Arc::new(c),
        Err(err) => {
            log::line(format!("dbus: no session bus: {err}"));
            return;
        }
    };
    if let Err(err) = conn.request_name(BUS_NAME, false, false, true) {
        log::line(format!("dbus: could not own {BUS_NAME}: {err}"));
        return;
    }

    let muted = app
        .try_state::<crate::Shared>()
        .map(|s| !s.settings.lock().unwrap().sound_enabled)
        .unwrap_or(false);
    let current = Arc::new(Mutex::new(Snapshot { state: "idle".into(), visibility: "hidden".into(), muted }));
    let mut cr = Crossroads::new();

    let iface = cr.register(INTERFACE, {
        let app = app.clone();
        move |b| {
            b.signal::<(String,), _>("StateChanged", ("state",));

            let a = app.clone();
            b.method("Toggle", (), (), move |_, _, _: ()| {
                crate::island::request(&a, "toggle", None);
                Ok(())
            });
            let a = app.clone();
            b.method("Show", ("view",), (), move |_, _, (view,): (String,)| {
                if !known_view(&view) {
                    return Err(MethodErr::invalid_arg("view"));
                }
                crate::island::request(&a, "show", Some(view));
                Ok(())
            });
            // Push-to-talk (the extension's keybinding): listen, or end now.
            let a = app.clone();
            b.method("PushToTalk", (), (), move |_, _, _: ()| {
                crate::voice::toggle(&a, crate::voice::Mode::Command);
                Ok(())
            });
            let a = app.clone();
            b.method("Hide", (), (), move |_, _, _: ()| {
                crate::island::request(&a, "hide", None);
                Ok(())
            });
            let a = app.clone();
            b.method("FocusSession", ("id",), (), move |_, _, (id,): (String,)| {
                if id.is_empty() {
                    return Err(MethodErr::invalid_arg("id"));
                }
                crate::open_session(Some(id), None);
                let _ = &a;
                Ok(())
            });

            let a = app.clone();
            b.method("Quit", (), (), move |_, _, _: ()| {
                // `exit` with a code is the one way past the "stay alive with
                // no windows" guard in lib.rs.
                a.exit(0);
                Ok(())
            });

            b.property::<String, _>("State")
                .emits_changed_true()
                .get(|_, snap: &mut Arc<Mutex<Snapshot>>| Ok(snap.lock().unwrap().state.clone()));
            b.property::<String, _>("Visibility")
                .emits_changed_true()
                .get(|_, snap: &mut Arc<Mutex<Snapshot>>| Ok(snap.lock().unwrap().visibility.clone()));
            let a = app.clone();
            b.property::<bool, _>("Muted")
                .emits_changed_true()
                .get(|_, snap: &mut Arc<Mutex<Snapshot>>| Ok(snap.lock().unwrap().muted))
                .set(move |_, snap, muted| {
                    snap.lock().unwrap().muted = muted;
                    crate::set_sound_enabled(&a, !muted);
                    Ok(Some(muted))
                });
        }
    });
    cr.insert(OBJECT_PATH, &[iface, cr.introspectable(), cr.properties()], current.clone());

    let cr = Mutex::new(cr);
    conn.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |msg, c| {
            let _ = cr.lock().unwrap().handle_message(msg, c);
            true
        }),
    );

    app.manage(Service { conn: conn.clone(), current });
    std::thread::Builder::new()
        .name("fedo-dbus".into())
        .spawn(move || loop {
            // Blocks in poll(2) until a message arrives; the hour is only the
            // cap on one wait, not a polling period.
            if conn.process(Duration::from_secs(3600)).is_err() {
                log::line("dbus: connection lost".to_string());
                break;
            }
        })
        .ok();
}

/// "hidden" | "peek" | "open", as the island last reported it.
pub fn visibility(app: &AppHandle) -> String {
    use tauri::Manager;
    app.try_state::<Service>()
        .map(|s| s.current.lock().unwrap().visibility.clone())
        .unwrap_or_else(|| "hidden".into())
}

/// The island page's view of itself, pushed whenever it changes. Emits
/// `StateChanged` and `PropertiesChanged` only on a real change.
pub fn report(app: &AppHandle, state: &str, visibility: &str) {
    update(app, |snap| {
        snap.state = state.to_string();
        snap.visibility = visibility.to_string();
    });
}

/// Settings changed the sound switch from the island's own UI.
pub fn report_muted(app: &AppHandle, muted: bool) {
    update(app, |snap| snap.muted = muted);
}

fn update(app: &AppHandle, change: impl FnOnce(&mut Snapshot)) {
    use tauri::Manager;
    let Some(svc) = app.try_state::<Service>() else { return };
    let (prev, next) = {
        let mut cur = svc.current.lock().unwrap();
        let prev = cur.clone();
        change(&mut cur);
        if *cur == prev {
            return;
        }
        (prev, cur.clone())
    };

    let mut changed = dbus::arg::PropMap::new();
    if prev.state != next.state {
        changed.insert("State".into(), dbus::arg::Variant(Box::new(next.state.clone())));
    }
    if prev.visibility != next.visibility {
        changed.insert("Visibility".into(), dbus::arg::Variant(Box::new(next.visibility.clone())));
    }
    if prev.muted != next.muted {
        changed.insert("Muted".into(), dbus::arg::Variant(Box::new(next.muted)));
    }
    let props = dbus::blocking::stdintf::org_freedesktop_dbus::PropertiesPropertiesChanged {
        interface_name: INTERFACE.into(),
        changed_properties: changed,
        invalidated_properties: vec![],
    };
    let _ = svc.conn.send(props.to_emit_message(&OBJECT_PATH.into()));

    if prev.state != next.state {
        if let Ok(msg) = Message::new_signal(OBJECT_PATH, INTERFACE, "StateChanged") {
            let _ = svc.conn.send(msg.append1(next.state.as_str()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::known_view;

    #[test]
    fn show_only_takes_views_the_island_has() {
        assert!(known_view(""));
        assert!(known_view("settings"));
        assert!(!known_view("../etc/passwd"));
        assert!(!known_view("approval"));
    }
}
