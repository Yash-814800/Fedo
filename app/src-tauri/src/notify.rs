// A desktop notification through org.freedesktop.Notifications — GNOME Shell
// shows it as a banner and keeps it in the calendar drawer. Used for "done"
// when the island isn't already showing the finished card.

use std::time::Duration;

use dbus::blocking::Connection;

/// Sends one notification; the body is short plain text. Best effort: no
/// notification daemon means no notification, never an error for the caller.
pub fn send(title: &str, body: &str) -> bool {
    let Ok(conn) = Connection::new_session() else { return false };
    let proxy = conn.with_proxy(
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        Duration::from_secs(2),
    );
    let r: Result<(u32,), _> = proxy.method_call(
        "org.freedesktop.Notifications",
        "Notify",
        (
            "Fedo",                 // app_name
            0u32,                   // replaces_id
            "dialog-information",   // app_icon (themed; the ghost has no file)
            title,                  // summary
            body,                   // body
            Vec::<String>::new(),   // actions
            std::collections::HashMap::<String, dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>>::new(),
            6000i32,                // expire_timeout ms
        ),
    );
    r.is_ok()
}

#[tauri::command]
pub fn notify_done(title: String, body: String) -> bool {
    // Never a long essay in the corner of the screen.
    let cut = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    send(&cut(&title, 80), &cut(&body, 200))
}

#[cfg(test)]
mod tests {
    /// Pops a real banner: `cargo test -p fedo notify -- --ignored`.
    #[test]
    #[ignore]
    fn a_banner_is_shown() {
        assert!(super::send("Fedo test", "Notification path works."));
    }
}
