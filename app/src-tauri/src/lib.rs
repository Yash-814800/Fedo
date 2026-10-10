// Fedo — app wiring and the commands the island calls.

mod agent_hooks;
mod agents;
mod actions;
mod assist;
mod calendar;
mod contacts;
mod chat;
mod config_file;
mod dbus_service;
mod desktop;
mod files;
mod hooks;
mod i18n;
mod identity;
mod island;
mod llm;
mod log;
mod net;
mod pipe;
mod platform;
#[cfg(target_os = "linux")]
mod portal;
mod recap;
mod session_window;
mod settings;
mod shortcuts;
mod sounds;
mod notify;
mod phone;
mod phoneplan;
mod scheduler;
mod spotify;
mod voice;
mod extension_keys;
mod timeplan;
mod unlock;
#[cfg(target_os = "linux")]
mod stats;
mod tray;

use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::{ManagerExt, MacosLauncher};

use chat::{Chat, ChatContext, ChatReply};
use files::DroppedFile;
use hooks::{HookPreview, HookStatus};
use island::{PollGate, ScreenInfo};
use pipe::Pending;
use settings::Settings;

pub struct Shared {
    pub settings: Mutex<Settings>,
    pub gate: Arc<PollGate>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootInfo {
    settings: Settings,
    screen: ScreenInfo,
    version: String,
    hook_path: String,
    /// False where the OS has no global cursor (Wayland): the page then reports
    /// the cursor from its own mouse events.
    cursor_poll: bool,
}

#[tauri::command]
fn boot(app: AppHandle, shared: State<Shared>) -> BootInfo {
    let mut settings = shared.settings.lock().unwrap().clone();
    // The real state of ~/.claude/settings.json wins over whatever we stored.
    let hooks_status = hooks::status();
    settings.hooks_installed = hooks_status.installed;
    settings.plan_relay_installed = hooks_status.plan_relay_installed;
    let screen = island::screen_info(&app, &settings.screen);
    BootInfo {
        settings,
        screen,
        version: env!("CARGO_PKG_VERSION").to_string(),
        hook_path: settings::hook_exe_path().to_string_lossy().to_string(),
        cursor_poll: platform::CURSOR_POLL,
    }
}

#[tauri::command]
fn save_settings(app: AppHandle, shared: State<Shared>, settings: Settings) {
    let (screen_changed, autostart_changed, shortcuts_changed) = {
        let mut current = shared.settings.lock().unwrap();
        let screen_changed = current.screen != settings.screen;
        let autostart_changed = current.autostart != settings.autostart;
        let shortcuts_changed = current.shortcuts != settings.shortcuts;
        // Where the ghost sits on the desktop is desktop.rs's to say, not a webview's.
        let mut settings = settings.clone();
        settings.desktop_ghost = current.desktop_ghost.clone();
        *current = settings;
        (screen_changed, autostart_changed, shortcuts_changed)
    };
    let settings = shared.settings.lock().unwrap().clone();
    llm::configure(&settings);
    phone::set_preferred(&settings.default_phone_device);
    if let Err(err) = settings::save(&settings) {
        log::line(format!("could not save settings: {err}"));
    }
    if autostart_changed {
        let manager = app.autolaunch();
        let result = if settings.autostart { manager.enable() } else { manager.disable() };
        if let Err(err) = result {
            eprintln!("[fedo] autostart: {err}");
        }
    }
    if screen_changed {
        let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
        island::apply_geometry(&app, &settings.screen, collapsed);
    }
    spotify::sync(&app, &settings.active_integrations);
    voice::sync_wake(&app, settings.wake_word);
    if shortcuts_changed {
        shortcuts::apply(&app, &settings.shortcuts);
    }
    if i18n::set_picked(&settings.language) {
        language_changed(&app);
    }
    dbus_service::report_muted(&app, !settings.sound_enabled);
    // Keep the other window in step (island ⇄ settings window).
    let _ = app.emit("settings-changed", settings);
}

/// The island reports the system's languages at launch, for "System" in
/// Settings → Language (WebView2 and WebKitGTK know them best).
#[tauri::command]
fn set_system_languages(app: AppHandle, languages: Vec<String>) {
    if i18n::set_system(languages) {
        language_changed(&app);
    }
}

/// The tray/extension's "Mute sounds": flips the one setting and tells the
/// island window, which plays the sounds.
pub(crate) fn set_sound_enabled(app: &AppHandle, enabled: bool) {
    let shared = app.state::<Shared>();
    let updated = {
        let mut current = shared.settings.lock().unwrap();
        if current.sound_enabled == enabled {
            return;
        }
        current.sound_enabled = enabled;
        if let Err(err) = settings::save(&current) {
            log::line(format!("could not save settings: {err}"));
        }
        current.clone()
    };
    let _ = app.emit("settings-changed", updated);
}

/// What Rust labels itself follows the new language: the tray menu and the
/// settings window's title. The webviews switch on their own.
fn language_changed(app: &AppHandle) {
    tray::retitle(app);
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.set_title(&i18n::t("Settings — Fedo"));
    }
}

/// The island page reports what the top-bar icon should show (`state`: idle /
/// working / needs-you) and how much of the island is out (`visibility`:
/// hidden / peek / open). Both go out on D-Bus for the extension.
#[tauri::command]
fn report_state(app: AppHandle, state: String, visibility: String) {
    let state = match state.as_str() {
        "working" | "needs-you" => state,
        _ => "idle".to_string(),
    };
    let visibility = match visibility.as_str() {
        "peek" | "open" => visibility,
        _ => "hidden".to_string(),
    };
    island::note_activity(&app, &state, &visibility);
    dbus_service::report(&app, &state, &visibility);
}

/// The island page has registered all its listeners and calls this once, last
/// thing in its boot: queued events are delivered now (island.rs).
#[tauri::command]
fn island_ready(app: AppHandle) {
    island::page_ready(&app);
}

/// Hidden island → shrink the window to the invisible wake strip and park the
/// cursor poll; anything else → full panel and 60 Hz polling.
#[tauri::command]
fn set_collapsed(app: AppHandle, shared: State<Shared>, collapsed: bool) {
    let pref = shared.settings.lock().unwrap().screen.clone();
    shared.gate.collapsed.store(collapsed, Ordering::Relaxed);
    island::apply_geometry(&app, &pref, collapsed);
    // The wake strip must always take the mouse, and a resize invalidates the flag.
    island::refresh_click_through(&app, &shared.gate);
    shared.gate.set_active(!collapsed);
    platform::set_pointer_watch(!collapsed);
}

/// The front end pushes the island shape; Rust decides click-through from it.
#[tauri::command]
fn set_island_rect(app: AppHandle, shared: State<Shared>, x: f64, y: f64, width: f64, height: f64) {
    shared.gate.set_rect(island::IslandRect { x, y, w: width, h: height });
    // Without the cursor poll the input region is the click-through: it follows the island.
    if !platform::CURSOR_POLL {
        island::refresh_click_through(&app, &shared.gate);
    }
}

#[tauri::command]
fn focus_window(app: AppHandle, focused: bool) {
    let Some(win) = island::window(&app) else { return };
    platform::set_activating(&win, focused);
    if focused {
        let _ = win.set_focus();
    }
}

#[tauri::command]
fn reposition(app: AppHandle, shared: State<Shared>) {
    let pref = shared.settings.lock().unwrap().screen.clone();
    let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
    island::apply_geometry(&app, &pref, collapsed);
}

/// The displays the island can be pinned to, for Settings.
#[tauri::command]
fn list_monitors(app: AppHandle) -> Vec<island::MonitorChoice> {
    island::monitor_choices(&app)
}

#[tauri::command]
fn open_url(url: String) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    platform::open_url(&url);
}

/// "Open terminal" opens the working folder in VS Code when `code` is on PATH,
/// and falls back to the file manager otherwise.
#[tauri::command]
fn open_in_vscode(path: Option<String>) -> bool {
    // No shell anywhere near this. The path is a project folder chosen by
    // whoever is using Claude Code, and a shell would happily read `&`, `^`, `%`
    // or `$` in a folder name as syntax. Finding the launcher ourselves and
    // handing the path over as a separate argument keeps it a path.
    let path = path.filter(|p| !p.is_empty());
    // It arrives in a hook payload: only an existing folder, given by its full
    // path, goes any further. `code` would read `--something` as an option, and
    // xdg-open would launch a file with whatever handles its type.
    if let Some(p) = path.as_deref() {
        let p = std::path::Path::new(p);
        if !(p.is_absolute() && p.is_dir()) {
            return false;
        }
    }
    if let Some(code) = platform::find_on_path("code") {
        let mut cmd = Command::new(code);
        if let Some(p) = path.as_deref() {
            cmd.arg(p);
        }
        if platform::no_console(&mut cmd).spawn().is_ok() {
            return true;
        }
    }
    if let Some(p) = path.as_deref() {
        platform::reveal_folder(p);
    }
    false
}

/// "Open terminal": brings forward the terminal or editor window the session
/// runs in, when it was found (see session_window.rs); otherwise opens the
/// folder in VS Code, as before.
#[tauri::command]
fn open_session(session_id: Option<String>, path: Option<String>) -> bool {
    let Some(owners) = session_id.as_deref().and_then(session_window::lookup) else {
        return open_in_vscode(path);
    };
    let folder = path.as_deref().map(session_window::folder_name).unwrap_or_default().to_string();
    #[cfg(windows)]
    {
        if platform::focus_session_window(&owners, &folder) {
            return true;
        }
        open_in_vscode(path)
    }
    // Linux asks the display server, KWin or the terminal, which can take a
    // moment: never on the UI thread a sync command runs on. VS Code still
    // opens when none of them could bring the window forward.
    #[cfg(target_os = "linux")]
    {
        std::thread::spawn(move || {
            if !platform::focus_session_window(&owners, &folder) {
                open_in_vscode(path);
            }
        });
        true
    }
}

/// The Claude Desktop pill's target: the Claude app (Windows only — it has no
/// Linux build).
#[tauri::command]
fn open_claude_desktop() -> bool {
    platform::open_claude_desktop()
}

/// The file behind a live diff, if it may be handed to the editor: an existing
/// regular file given by its full path. Anything else — a relative path, a
/// folder, a path `code` could read as an option — goes no further.
fn diff_file(path: &str) -> Option<&std::path::Path> {
    let p = std::path::Path::new(path);
    (p.is_absolute() && p.is_file()).then_some(p)
}

/// The diff card's ↗: opens the edited file in VS Code when `code` is on PATH,
/// otherwise shows its folder. The file itself is never opened by its type —
/// xdg-open or Explorer would run a script that Claude just wrote.
#[tauri::command]
fn open_file_in_vscode(path: String) -> bool {
    let Some(file) = diff_file(&path) else { return false };
    if let Some(code) = platform::find_on_path("code") {
        let mut cmd = Command::new(code);
        cmd.arg(file);
        if platform::no_console(&mut cmd).spawn().is_ok() {
            return true;
        }
    }
    if let Some(folder) = file.parent().filter(|d| d.is_dir()) {
        platform::reveal_folder(&folder.to_string_lossy());
    }
    false
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Tray → Pause: no network traffic (currently just Spotify artwork) while paused.
#[tauri::command]
fn set_paused(paused: bool) {
    spotify::PAUSED.store(paused, Ordering::Relaxed);
}

// ── Claude Code hooks ─────────────────────────────────────────────────────────

#[tauri::command]
fn hooks_status() -> HookStatus {
    hooks::status()
}

/// Pill ID → whether that agent's hooks reach Fedo. Read-only.
#[tauri::command]
fn agent_hooks_status() -> std::collections::HashMap<String, bool> {
    agent_hooks::status()
}

/// Returns the diff the user has to look at before anything is written.
#[tauri::command]
fn hooks_preview(install: bool) -> Result<HookPreview, String> {
    hooks::preview(install)
}

/// Only ever called from an explicit click in the settings window.
#[tauri::command]
fn hooks_apply(
    app: AppHandle,
    shared: State<Shared>,
    install: bool,
    fingerprint: String,
) -> Result<String, String> {
    // The fingerprint comes from the preview the user actually looked at, so a
    // settings.json that changed in between is refused rather than overwritten.
    let backup = hooks::write(install, &fingerprint)?;
    let updated = {
        let mut current = shared.settings.lock().unwrap();
        current.hooks_installed = install;
        if let Err(err) = settings::save(&current) {
            log::line(format!("could not save settings: {err}"));
        }
        current.clone()
    };
    let _ = app.emit("settings-changed", updated);
    Ok(backup)
}

// ── Other agents' hooks and plugins ──────────────────────────────────────────

#[tauri::command]
fn agent_hooks_list() -> Vec<agents::AgentStatus> {
    agents::list()
}

/// The diff the user has to look at before anything is written.
#[tauri::command]
fn agent_hooks_preview(agent: String, install: bool) -> Result<config_file::Plan, String> {
    agents::preview(&agent, install)
}

/// Only ever called from an explicit click in the settings window, with the
/// fingerprint of the preview the user looked at.
#[tauri::command]
fn agent_hooks_apply(agent: String, install: bool, fingerprint: String) -> Result<String, String> {
    let backups = agents::apply(&agent, install, &fingerprint)?;
    let done = if install { "installed" } else { "removed" };
    log::line(format!("agent hooks {done} for {agent}"));
    Ok(backups)
}

// ── Plan usage ────────────────────────────────────────────────────────────────

/// The diff of putting the plan usage relay into (or taking it out of) the
/// status line, before anything is written.
#[tauri::command]
fn status_line_preview(install: bool) -> Result<HookPreview, String> {
    hooks::status_line_preview(install)
}

/// Only ever called from an explicit click in the settings window.
#[tauri::command]
fn status_line_apply(
    app: AppHandle,
    shared: State<Shared>,
    install: bool,
    fingerprint: String,
) -> Result<String, String> {
    let backup = hooks::status_line_write(install, &fingerprint)?;
    let updated = {
        let mut current = shared.settings.lock().unwrap();
        current.plan_relay_installed = install;
        // As on the Mac: taking the relay out turns the pill off with it.
        if !install {
            current.show_plan_in_notch = false;
        }
        if let Err(err) = settings::save(&current) {
            log::line(format!("could not save settings: {err}"));
        }
        current.clone()
    };
    let _ = app.emit("settings-changed", updated);
    Ok(backup)
}

#[tauri::command]
fn approval_decision(app: AppHandle, request_id: String, decision: String) {
    recap::record_decision(&app, &request_id, &decision);
    pipe::answer(&app, &request_id, &decision);
}

/// An option picked on the island for a question Claude Code asked.
#[tauri::command]
fn approval_answer(
    app: AppHandle,
    request_id: String,
    answers: std::collections::HashMap<String, serde_json::Value>,
) {
    // An answered question is not an Allow / Deny: nothing for the recap.
    recap::forget_request(&app, &request_id);
    pipe::answer_question(&app, &request_id, &answers);
}

/// The island has the card on screen, so the long wait for a human may begin.
/// Until this arrives the relay only waits a few hundred milliseconds, which is
/// what stops a paused or unresponsive island from freezing Claude Code.
#[tauri::command]
fn approval_ack(app: AppHandle, request_id: String) {
    pipe::acknowledge(&app, &request_id);
}

/// Nobody can act on this request — the island is paused, or another card is
/// already up. Claude Code falls back to asking in the terminal immediately.
#[tauri::command]
fn approval_decline(app: AppHandle, request_id: String) {
    recap::forget_request(&app, &request_id);
    pipe::decline(&app, &request_id);
}

// ── Chat and files ────────────────────────────────────────────────────────────

/// One chat turn, run through `claude -p` (chat.rs).
#[tauri::command]
async fn chat_send(
    app: AppHandle,
    shared: State<'_, Shared>,
    chat: State<'_, Chat>,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let settings = shared.settings.lock().unwrap().clone();
    chat::send(&app, &chat, &settings, query, context).await
}

#[tauri::command]
fn chat_reset(chat: State<Chat>) {
    chat.reset();
}

/// Copies a dropped file into the inbox and reports its name back.
#[tauri::command]
fn ingest_file(path: String) -> Result<DroppedFile, String> {
    files::ingest(&path)
}

/// Lets the island write to the same log as the Rust side.
#[tauri::command]
fn log_line(message: String) {
    log::line(format!("ui  {message}"));
}

// ── Global shortcuts ──────────────────────────────────────────────────────────

/// How each global shortcut went: registered, taken by another app, and so on.
#[tauri::command]
fn shortcuts_status(app: AppHandle) -> shortcuts::Report {
    shortcuts::status(&app)
}

/// Settings is recording a new combination: let go of ours meanwhile, so the
/// keys reach the recorder instead of running an action. `false` takes them back.
#[tauri::command]
fn shortcuts_suspend(app: AppHandle, shared: State<Shared>, suspended: bool) {
    if suspended {
        shortcuts::suspend(&app);
    } else {
        let stored = shared.settings.lock().unwrap().shortcuts.clone();
        shortcuts::apply(&app, &stored);
    }
}

// ── Settings window ───────────────────────────────────────────────────────────

/// WebView2 allows exactly one browser environment per app, and its options are
/// fixed by whichever webview is created first. Every window must therefore ask
/// for the *same* arguments as the island (see `additionalBrowserArgs` in
/// tauri.conf.json) — a mismatch makes the second window come up blank, with no
/// error anywhere.
pub(crate) const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required";

/// In a dev build the pages are served by Vite, so a window created in code
/// needs the absolute dev URL; a bundled build resolves it inside the app bundle.
pub(crate) fn page_url(app: &AppHandle, page: &str) -> WebviewUrl {
    #[cfg(dev)]
    if let Some(mut base) = app.config().build.dev_url.clone() {
        base.set_path(&format!("/{page}"));
        return WebviewUrl::External(base);
    }
    let _ = app;
    WebviewUrl::App(page.into())
}

/// The settings window is built when it is asked for and destroyed when it is
/// closed, so it costs nothing in between (FEDO.md §3).
fn build_settings_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    match WebviewWindowBuilder::new(app, "settings", page_url(app, "settings.html"))
        .additional_browser_args(BROWSER_ARGS)
        .title(i18n::t("Settings — Fedo"))
        .inner_size(560.0, 680.0)
        .min_inner_size(460.0, 480.0)
        .resizable(true)
        .center()
        .build()
    {
        Ok(win) => {
            let handle = app.clone();
            win.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    island::reap_soon(&handle);
                }
            });
            Some(win)
        }
        Err(err) => {
            log::line(format!("settings window failed: {err}"));
            None
        }
    }
}

pub fn show_settings_window(app: &AppHandle) {
    let win = match app.get_webview_window("settings") {
        Some(w) => w,
        None => {
            let handle = app.clone();
            // Window builders belong on the main thread; the tray, the D-Bus
            // thread and commands all arrive here from somewhere else.
            let _ = app.run_on_main_thread(move || {
                if let Some(w) = build_settings_window(&handle) {
                    let _ = w.set_focus();
                }
            });
            return;
        }
    };
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
}

#[tauri::command]
fn open_settings_window(app: AppHandle) {
    show_settings_window(&app);
}

/// `fedo stats`: prints what the running daemon costs and returns the exit code.
pub fn stats() -> i32 {
    #[cfg(target_os = "linux")]
    return stats::run();
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("fedo stats reads /proc and is Linux only");
        1
    }
}

/// `fedo hooks status|install|uninstall [--yes]`: the one-script install's
/// hook step. Shows the diff and the backup it will take and writes only after
/// a "y" (the same preview → fingerprint → dated backup path as Settings).
pub fn hooks_cli(args: &[String]) -> i32 {
    use std::io::Write;
    let sub = args.first().map(String::as_str).unwrap_or("status");
    let yes = args.iter().any(|a| a == "--yes");
    let ours = |ok: bool| if ok { "installed" } else { "not installed" };
    let claude = hooks::status();
    let agy = agents::list().into_iter().find(|a| a.id == "antigravity");
    if sub == "status" {
        println!("Claude Code hooks: {}", ours(claude.installed));
        match &agy {
            Some(a) => println!("Antigravity hooks: {}", ours(a.installed)),
            None => println!("Antigravity hooks: unknown"),
        }
        return 0;
    }
    let install = match sub {
        "install" => true,
        "uninstall" => false,
        _ => {
            eprintln!("usage: fedo hooks status|install|uninstall [--yes]");
            return 2;
        }
    };

    // The relay Claude Code runs lives in our own folder; put it there first.
    if install {
        let dest = settings::hook_exe_path();
        let exe = std::env::current_exe().ok();
        let src = exe.as_ref().and_then(|e| {
            let dir = e.parent()?;
            [
                dir.join(platform::HOOK_EXE),
                dir.join("../release").join(platform::HOOK_EXE),
                std::path::Path::new("/usr/lib/Fedo").join(platform::HOOK_EXE),
            ]
            .into_iter()
            .find(|p| p.exists())
        });
        match src {
            Some(src) => {
                let ok = platform::ensure_private_dir(&settings::local_dir()).is_ok()
                    && dest.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok())
                    && std::fs::copy(&src, &dest).is_ok();
                if !ok {
                    eprintln!("could not copy the relay to {}", dest.display());
                    return 1;
                }
            }
            None if !dest.exists() => {
                eprintln!("fedo-hook not found next to {}", exe.map(|e| e.display().to_string()).unwrap_or_default());
                return 1;
            }
            None => {}
        }
    }

    let mut failed = false;
    // (label, diff, backup, path, fingerprint, apply)
    type Apply = Box<dyn Fn(&str) -> Result<String, String>>;
    let mut steps: Vec<(&str, config_file::Plan, Apply)> = Vec::new();
    match hooks::preview(install) {
        Ok(p) => steps.push((
            "Claude Code (~/.claude/settings.json)",
            config_file::Plan { diff: p.diff, backup: p.backup, path: p.settings_path, fingerprint: p.fingerprint },
            Box::new(move |fp| hooks::write(install, fp)),
        )),
        Err(e) => {
            eprintln!("Claude Code: {e}");
            failed = true;
        }
    }
    if agy.as_ref().is_some_and(|a| a.installed || install) {
        match agents::preview("antigravity", install) {
            Ok(p) => steps.push(("Antigravity (~/.gemini/config/hooks.json)", p, Box::new(move |fp| agents::apply("antigravity", install, fp)))),
            Err(e) => {
                eprintln!("Antigravity: {e}");
                failed = true;
            }
        }
    }
    for (label, plan, apply) in steps {
        if plan.diff.trim().is_empty() || plan.diff.trim() == "No change." {
            println!("{label}: already {}", if install { "installed" } else { "clean" });
            continue;
        }
        println!("── {label}\n{}\nbackup: {}", plan.diff, if plan.backup.is_empty() { "(nothing to back up)" } else { &plan.backup });
        let go = yes || {
            print!("Apply this change? [y/N] ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
        };
        if !go {
            println!("skipped");
            continue;
        }
        match apply(&plan.fingerprint) {
            Ok(_) => println!("done"),
            Err(e) => {
                eprintln!("failed: {e}");
                failed = true;
            }
        }
    }
    i32::from(failed)
}

pub fn run() {
    platform::prepare_environment();
    let loaded = settings::load();
    llm::configure(&loaded);
    phone::set_preferred(&loaded.default_phone_device);
    i18n::set_picked(&loaded.language);
    let gate = Arc::new(PollGate::new());

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // `fedo --shortcut <action>`: what a desktop's own keyboard
            // settings run where we can't listen for keys ourselves (Wayland).
            match shortcuts::from_args(&argv) {
                Some(action) => shortcuts::dispatch(app, action),
                None => island::request(app, "show", None),
            }
        }))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None));
    // Where no global shortcut can work, the plugin isn't even started.
    if platform::global_shortcuts_blocked().is_none() {
        builder = builder.plugin(shortcuts::plugin());
    }

    builder
        .manage(Shared {
            settings: Mutex::new(loaded.clone()),
            gate: gate.clone(),
        })
        .manage(Pending::default())
        .manage(island::Lifecycle::default())
        .manage(Chat::default())
        .manage(shortcuts::Registry::default())
        .manage(recap::load())
        .manage(scheduler::Scheduler::load())
        .invoke_handler(tauri::generate_handler![
            boot,
            save_settings,
            set_system_languages,
            set_collapsed,
            report_state,
            island_ready,
            set_island_rect,
            focus_window,
            reposition,
            list_monitors,
            open_url,
            open_in_vscode,
            open_session,
            open_claude_desktop,
            open_file_in_vscode,
            quit_app,
            set_paused,
            hooks_status,
            agent_hooks_status,
            hooks_preview,
            hooks_apply,
            agent_hooks_list,
            agent_hooks_preview,
            agent_hooks_apply,
            status_line_preview,
            status_line_apply,
            approval_decision,
            approval_answer,
            approval_ack,
            approval_decline,
            log_line,
            chat_send,
            chat_reset,
            assist::assist_route,
            assist::assist_run,
            assist::assist_copy,
            assist::assist_do,
            llm::local_ai_status,
            assist::assist_create_event,
            notify::notify_done,
            scheduler::schedule_list,
            scheduler::schedule_pause,
            scheduler::schedule_resume,
            scheduler::schedule_cancel,
            ingest_file,
            open_settings_window,
            shortcuts_status,
            shortcuts_suspend,
            recap::recap_history,
            recap::recap_prefs,
            recap::recap_set_enabled,
            recap::recap_set_hide_projects,
            recap::recap_mark_shown,
            recap::recap_clear,
            recap::recap_save_png,
            recap::recap_reveal_saved,
            desktop::desktop_ghost_info,
            desktop::desktop_ghost_pick_up,
            desktop::desktop_ghost_carry,
            desktop::desktop_ghost_carry_end,
            desktop::desktop_ghost_drag_begin,
            desktop::desktop_ghost_drag_move,
            desktop::desktop_ghost_drag_end,
            desktop::desktop_ghost_fly_out,
            desktop::desktop_ghost_fly_home,
            desktop::desktop_ghost_set_asleep,
            sounds::custom_sounds,
            sounds::custom_sound,
            sounds::reveal_sounds_folder,
            sounds::reload_sounds,
            spotify::spotify_refresh,
            spotify::spotify_control,
            spotify::spotify_open,
            spotify::spotify_installed,
            phone::phone_list_devices,
            extension_keys::extension_keys_get,
            extension_keys::extension_key_set,
            voice::voice_status,
            voice::voice_toggle,
            voice::voice_cancel,
            voice::voice_prepare,
            voice::voice_speak,
            voice::voice_stop_speaking,
            voice::voice_confirmation,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            tray::build(&handle)?;
            // Same rule for the ghost's desktop window.
            desktop::setup(&handle);

            // No island window yet: it is built by the first thing that needs it
            // (island.rs), and the icon in the top bar is the way to ask.
            island::spawn_cursor_poll(handle.clone(), gate.clone());

            log::line(format!("--- Fedo {} started ---", env!("CARGO_PKG_VERSION")));
            hooks::ensure_hook_exe(&handle);
            pipe::start(handle.clone());
            dbus_service::start(&handle);
            unlock::start(handle.clone());
            scheduler::start(handle.clone());
            phone::start_listener(handle.clone());
            spotify::sync(&handle, &loaded.active_integrations);
            voice::sync_wake(&handle, loaded.wake_word);
            shortcuts::apply(&handle, &loaded.shortcuts);
            // Startup is a burst of one-off work (GTK, the tray, the portal);
            // let it finish, then drop what it left resident.
            tauri::async_runtime::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                island::settle_idle();
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Fedo")
        .run(|_app, event| {
            // The daemon outlives its windows: the island's webview is torn
            // down when idle and the settings window closes for good, and
            // Tauri would otherwise exit with the last of them. An explicit
            // quit (`app.exit(code)`) carries a code and still goes through.
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::diff_file;

    #[test]
    fn only_an_existing_file_by_its_full_path_reaches_the_editor() {
        let dir = std::env::temp_dir().join(format!("fedo-diff-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("edited.ts");
        std::fs::write(&file, "x").unwrap();

        assert!(diff_file(&file.to_string_lossy()).is_some());
        // A folder, a missing file, a relative path or an option never pass.
        assert!(diff_file(&dir.to_string_lossy()).is_none());
        assert!(diff_file(&dir.join("missing.ts").to_string_lossy()).is_none());
        assert!(diff_file("edited.ts").is_none());
        assert!(diff_file("--help").is_none());
        assert!(diff_file("").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
