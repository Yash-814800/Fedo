// The keys the GNOME Shell extension binds (push-to-talk, approve, …): they
// live in the extension's GSettings schema, so Settings reads and writes them
// with the `gsettings` CLI. The extension picks a change up immediately.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Serialize;

const SCHEMA: &str = "org.gnome.shell.extensions.fedo";
/// Every key the schema defines; nothing else is ever passed to gsettings.
pub const KEYS: &[&str] = &["push-to-talk", "toggle-island", "open-chat", "approve-latest", "jump-to-latest", "mute-toggle"];

fn schema_dir() -> Option<PathBuf> {
    let dir = crate::platform::home_dir().join(".local/share/gnome-shell/extensions/fedo@yash/schemas");
    dir.join("gschemas.compiled").is_file().then_some(dir)
}

#[derive(Serialize)]
pub struct ExtensionKey {
    pub name: String,
    pub accelerator: String,
}

/// `['<Super><Alt>v']` → `<Super><Alt>v` (one binding per key here).
pub fn parse_value(raw: &str) -> String {
    // An empty list prints as `@as []`.
    if raw.trim_start().starts_with('@') {
        return String::new();
    }
    raw.trim().trim_start_matches('[').trim_end_matches(']').split(',').next().unwrap_or("").trim().trim_matches('\'').to_string()
}

/// A GTK accelerator the shell accepts: modifiers in angle brackets, then a key
/// name. Empty clears the binding.
pub fn valid_accelerator(a: &str) -> bool {
    if a.is_empty() {
        return true;
    }
    let mut rest = a;
    let mut mods = 0;
    while let Some(r) = rest.strip_prefix('<') {
        let Some(end) = r.find('>') else { return false };
        if !matches!(&r[..end], "Control" | "Ctrl" | "Alt" | "Super" | "Shift" | "Primary") {
            return false;
        }
        mods += 1;
        rest = &r[end + 1..];
    }
    mods > 0 && !rest.is_empty() && rest.len() <= 24 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[tauri::command]
pub fn extension_keys_get() -> Vec<ExtensionKey> {
    let Some(dir) = schema_dir() else { return Vec::new() };
    KEYS.iter()
        .filter_map(|name| {
            let out = Command::new("gsettings")
                .arg("--schemadir")
                .arg(&dir)
                .args(["get", SCHEMA, name])
                .stderr(Stdio::null())
                .output()
                .ok()?;
            out.status.success().then(|| ExtensionKey {
                name: (*name).to_string(),
                accelerator: parse_value(&String::from_utf8_lossy(&out.stdout)),
            })
        })
        .collect()
}

#[tauri::command]
pub fn extension_key_set(name: String, accelerator: String) -> Result<(), String> {
    if !KEYS.contains(&name.as_str()) {
        return Err("Unknown key.".into());
    }
    let accelerator = accelerator.trim().to_string();
    if !valid_accelerator(&accelerator) {
        return Err("Use modifiers and a key, like <Super><Alt>v.".into());
    }
    let dir = schema_dir().ok_or("The GNOME extension isn't installed (run scripts/install.sh).")?;
    let value = if accelerator.is_empty() { "[]".to_string() } else { format!("['{accelerator}']") };
    let ok = Command::new("gsettings")
        .arg("--schemadir")
        .arg(&dir)
        .args(["set", SCHEMA, &name, &value])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok { Ok(()) } else { Err("gsettings refused that key.".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_accelerators() {
        assert_eq!(parse_value("['<Super><Alt>v']\n"), "<Super><Alt>v");
        assert_eq!(parse_value("@as []\n"), "");
        assert!(valid_accelerator("<Super><Alt>v"));
        assert!(valid_accelerator("<Control><Shift>F12"));
        assert!(valid_accelerator(""));
        assert!(!valid_accelerator("v"));
        assert!(!valid_accelerator("<Meta>v"));
        assert!(!valid_accelerator("<Super>v'; rm -rf ~"));
        assert!(!valid_accelerator("<Super>"));
    }
}
