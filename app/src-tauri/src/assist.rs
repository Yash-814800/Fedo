// The command assistant (FEDO.md Phase 4): every message typed in the chat goes
// through a router first.
//
//   1. a local fast path — regexes, no latency, no usage — for the obvious
//      (mute, play/pause/next, "install obs", "kill vlc", "rewrite this as a
//      prompt", timers…);
//   2. otherwise `claude -p --model haiku --json-schema …`, which classifies the
//      request (and, for a one-off system action, proposes the command).
//
// What comes back is a *card*, never an action: the exact command, a one-line
// explanation and a risk. Nothing here runs a command that changes the machine —
// `run_command` is called by the island only after Enter / the Run button.
// Fedo grounds what the model says: installs are looked up with `dnf` and
// `flatpak`, kills are resolved to real PIDs, file tasks to paths that exist,
// and the risk is raised by Fedo's own rules, never lowered.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

// ── What the island gets back ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Safe,
    Careful,
    Dangerous,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub command: String,
    pub explanation: String,
    pub risk: Risk,
    /// Shown in red/amber under the command.
    pub warnings: Vec<String>,
    /// Extra rows: matching processes, the real paths of a file plan…
    pub details: Vec<String>,
    pub sudo: bool,
    /// Safe to run hidden with output in the island (no sudo, risk safe).
    pub inline_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub label: String,
    pub detail: String,
    pub card: Card,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Reply {
    /// Not an assistant request: the island runs its normal chat turn.
    Chat,
    /// Handled on the spot (mute, play/pause…).
    Done { text: String },
    Card { card: Card },
    Choices { title: String, options: Vec<Choice> },
    /// A rewritten prompt for Claude Code.
    Prompt { text: String },
    /// Nothing to run; just say it.
    Note { text: String },
}

// ── Risk: Fedo's own rules ────────────────────────────────────────────────────

/// Processes whose death takes the session (or Fedo) with it.
const PROTECTED: &[&str] = &[
    "gnome-shell", "systemd", "Xwayland", "pipewire", "wireplumber", "mutter", "dbus-daemon",
    "dbus-broker", "gdm", "gnome-session", "fedo", "pulseaudio", "NetworkManager", "init",
];

/// Splits on whitespace and shell separators, enough to see which words a
/// command contains (not a shell parser).
fn words(command: &str) -> Vec<String> {
    command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '`' | '$' | '"' | '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The risk Fedo's rules give a command, with what to say about it. The model's
/// opinion can only raise it.
pub fn assess(command: &str, model_risk: Risk) -> (Risk, Vec<String>, bool) {
    let w = words(command);
    let has = |s: &str| w.iter().any(|x| x == s);
    let mut risk = model_risk;
    let mut warnings = Vec::new();
    let sudo = has("sudo") || has("pkexec") || has("su");
    if sudo {
        risk = risk.max(Risk::Careful);
        warnings.push("Needs administrator rights: you'll be asked for your password in the terminal.".into());
    }
    let rm_recursive = has("rm") && w.iter().any(|x| x.starts_with('-') && x.contains('r') && !x.starts_with("--"))
        || has("rm") && has("--recursive");
    if rm_recursive {
        risk = risk.max(Risk::Dangerous);
        warnings.push("Deletes files recursively, with no undo.".into());
    }
    for (needle, why) in [
        ("dd", "Writes raw data to a device; a wrong target destroys it."),
        ("mkfs", "Formats a filesystem."),
        ("shred", "Destroys file contents for good."),
        ("wipefs", "Erases filesystem signatures."),
        ("fdisk", "Edits the partition table."),
        ("parted", "Edits the partition table."),
    ] {
        if w.iter().any(|x| x == needle || x.starts_with(&format!("{needle}."))) {
            risk = risk.max(Risk::Dangerous);
            warnings.push(why.into());
        }
    }
    if command.contains("/dev/sd") || command.contains("/dev/nvme") || command.contains("> /dev/") {
        risk = risk.max(Risk::Dangerous);
        warnings.push("Touches a disk device.".into());
    }
    if (has("curl") || has("wget")) && (command.contains("| sh") || command.contains("| bash") || command.contains("|sh") || command.contains("|bash")) {
        risk = risk.max(Risk::Dangerous);
        warnings.push("Runs a script straight from the internet.".into());
    }
    if has("chmod") && has("-R") || has("chown") && has("-R") {
        risk = risk.max(Risk::Careful);
        warnings.push("Changes permissions recursively.".into());
    }
    if has("kill") || has("pkill") || has("killall") {
        for p in PROTECTED {
            if w.iter().any(|x| x == p) {
                risk = Risk::Dangerous;
                warnings.push(format!("`{p}` is part of your desktop session — killing it can log you out or take Fedo down."));
            }
        }
    }
    if has("dnf") && (has("remove") || has("erase")) || has("flatpak") && has("uninstall") {
        risk = risk.max(Risk::Careful);
        warnings.push("Removes software.".into());
    }
    (risk, warnings, sudo)
}

/// A finished card: assessed, with the inline decision made.
pub fn card(command: String, explanation: String, model_risk: Risk, details: Vec<String>) -> Card {
    let (risk, warnings, sudo) = assess(&command, model_risk);
    let inline_ok = risk == Risk::Safe && !sudo && !command.contains('\n');
    Card { command, explanation, risk, warnings, details, sudo, inline_ok }
}

/// One shell word, single-quoted.
pub fn quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-+=:,@%".contains(&b)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ── Local fast path ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Fast {
    Mute(bool),
    Media(&'static str),
    /// Phase 5.
    Timer,
    /// Phase 6.
    Notifications,
    Install(String),
    /// (name, force: SIGKILL)
    Kill(String, bool),
    PromptRewrite(String),
}

/// A program name as one to three plain words ("obs", "visual studio code").
fn program_name(s: &str) -> Option<String> {
    let s = s.trim().trim_matches(|c| c == '.' || c == '"' || c == '\'' || c == '?');
    let s = s.strip_prefix("the ").unwrap_or(s).trim();
    let n = s.split_whitespace().count();
    let ok = !s.is_empty()
        && n <= 3
        && s.len() <= 40
        && s.chars().all(|c| c.is_alphanumeric() || c == ' ' || matches!(c, '-' | '_' | '.' | '+'));
    ok.then(|| s.to_lowercase())
}

pub fn fast_path(text: &str) -> Option<Fast> {
    let t = text.trim();
    let l = t.to_lowercase();
    let l = l.trim_end_matches(|c| matches!(c, '.' | '!' | '?')).trim();

    if let Some(rest) = ["rewrite this as a prompt", "turn this into a prompt", "write a prompt for claude", "prompt:"]
        .iter()
        .find_map(|p| l.strip_prefix(p).map(|_| t[p.len()..].trim_start_matches([':', ' ', '\n']).to_string()))
    {
        return Some(Fast::PromptRewrite(rest));
    }
    match l {
        "mute" | "mute sounds" | "mute fedo" | "be quiet" | "silence" => return Some(Fast::Mute(true)),
        "unmute" | "unmute sounds" | "unmute fedo" | "sounds on" => return Some(Fast::Mute(false)),
        "pause" | "pause music" | "pause the music" | "pause song" | "pause the song" | "pause spotify"
        | "play" | "play music" | "resume" | "resume music" | "resume the music" | "play spotify" => {
            return Some(Fast::Media("playPause"))
        }
        "next" | "next song" | "next track" | "skip" | "skip song" | "skip this song" | "skip track" => {
            return Some(Fast::Media("next"))
        }
        "previous" | "previous song" | "previous track" | "last song" | "go back a song" => {
            return Some(Fast::Media("previous"))
        }
        "read my notifications" | "read notifications" | "what did i miss" | "read my messages" => {
            return Some(Fast::Notifications)
        }
        _ => {}
    }
    if (l.starts_with("timer ") || l.starts_with("set a timer") || l.starts_with("set timer") || l.starts_with("remind me"))
        && (l.chars().any(|c| c.is_ascii_digit()) || l.contains(" a ") || l.contains(" an "))
    {
        return Some(Fast::Timer);
    }
    for p in ["install ", "get me ", "i need ", "download "] {
        if let Some(rest) = l.strip_prefix(p) {
            if p == "i need " || p == "get me " {
                // Too loose to act on without the model; "install X" is the sure one.
                continue;
            }
            if let Some(n) = program_name(rest) {
                return Some(Fast::Install(n));
            }
        }
    }
    for p in ["force kill ", "kill -9 ", "kill -s kill ", "kill ", "force quit ", "force-quit ", "quit ", "terminate "] {
        if let Some(rest) = l.strip_prefix(p) {
            let force = matches!(p, "force kill " | "kill -9 " | "kill -s kill ");
            let rest = rest
                .trim_end_matches(" completely")
                .trim_end_matches(" totally")
                .trim_end_matches(" entirely")
                .trim_end_matches(" for good");
            if let Some(n) = program_name(rest) {
                return Some(Fast::Kill(n, force));
            }
        }
    }
    None
}

// ── Install: grounded in dnf and flatpak ──────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub source: &'static str, // "dnf" | "flatpak"
    pub id: String,           // package name | application id
    pub summary: String,
    pub installed: bool,
}

/// `dnf search` lines: "obs-studio.x86_64 : Open Broadcaster Software Studio".
pub fn parse_dnf_search(out: &str) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    for line in out.lines() {
        let line = line.trim_start();
        if line.starts_with('=') || line.is_empty() {
            continue;
        }
        // dnf5 quiet prints "name.arch<TAB>summary"; older dnf " : ".
        let Some((name, summary)) = line.split_once('\t').or_else(|| line.split_once(" : ")) else { continue };
        let name = name.trim();
        let (pkg, arch) = name.rsplit_once('.').unwrap_or((name, ""));
        if !matches!(arch, "x86_64" | "noarch" | "aarch64" | "") {
            continue;
        }
        if !seen.iter().any(|(p, _): &(String, String)| p == pkg) {
            seen.push((pkg.to_string(), summary.trim().to_string()));
        }
    }
    seen
}

/// `flatpak search --columns=application,name,description`: tab separated,
/// one row per branch (so repeated), with plugins, themes and runtimes mixed in.
/// Returns (application id, display name, description), apps only, no repeats.
pub fn parse_flatpak_search(out: &str) -> Vec<(String, String, String)> {
    let mut seen: Vec<(String, String, String)> = Vec::new();
    for l in out.lines() {
        let mut c = l.split('\t');
        let id = c.next().unwrap_or("").trim();
        let name = c.next().unwrap_or("").trim();
        let desc = c.next().unwrap_or("").trim();
        let noise = [
            ".Plugin.", ".Gtk3theme.", ".Gtk4theme.", ".Platform", ".Sdk", ".Locale", ".Debug", ".Extension",
            ".BaseApp", ".Theme", ".VulkanLayer.", ".GL.", ".Codecs", ".Sources",
        ];
        if !id.contains('.') || id.contains(' ') || noise.iter().any(|n| id.contains(n)) {
            continue;
        }
        if !seen.iter().any(|(i, _, _)| i == id) {
            seen.push((id.to_string(), name.to_string(), desc.to_string()));
        }
    }
    seen
}

/// How well a Flathub app fits the query: its name, or the id's last segment,
/// is the query (0); the query is a whole word of the name (1); only a prefix
/// or substring (2).
fn flatpak_rank(id: &str, name: &str, query: &str) -> u8 {
    let q = query.to_lowercase();
    let n = name.to_lowercase();
    let last = id.rsplit('.').next().unwrap_or("").to_lowercase();
    if n == q || last == q.replace(' ', "") {
        0
    } else if n.split(|c: char| !c.is_alphanumeric()).any(|w| w == q) || n.starts_with(&format!("{q} ")) {
        1
    } else {
        2
    }
}

/// Ranks package names for a query: exact, then prefix, then the rest.
fn rank(name: &str, query: &str) -> u8 {
    let n = name.to_lowercase();
    let noise = ["-plugin", "-devel", "-libs", "-doc", "-data", "-langpack", "-debug", "-static", "-common", "-lang"];
    if noise.iter().any(|x| n.contains(x)) {
        3
    } else if n == query {
        0
    } else if n.starts_with(&format!("{query}-")) || n.starts_with(query) {
        1
    } else {
        2
    }
}

fn run_capture(program: &str, args: &[&str], limit: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    reader.join().ok()
}

fn rpm_installed(pkg: &str) -> bool {
    Command::new("rpm")
        .args(["-q", "--quiet", pkg])
        .stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn flatpak_installed(id: &str) -> bool {
    run_capture("flatpak", &["list", "--app", "--columns=application"], Duration::from_secs(5))
        .map(|o| o.lines().any(|l| l.trim() == id))
        .unwrap_or(false)
}

/// What can be installed for `name`, best first: dnf, then Flathub. Flathub's
/// own top hit names the program properly ("OBS Studio"), which lets the dnf
/// package of that name (`obs-studio`) beat look-alikes (`obs-signd`).
pub fn find_candidates(name: &str) -> Vec<Candidate> {
    let q = name.to_lowercase().replace(' ', "-");

    let mut fp = run_capture(
        "flatpak",
        &["search", "--columns=application,name,description", name],
        Duration::from_secs(25),
    )
    .map(|o| parse_flatpak_search(&o))
    .unwrap_or_default();
    fp.sort_by_key(|(id, n, _)| (flatpak_rank(id, n, name), n.len()));
    let best = fp.first().map(|(id, n, _)| flatpak_rank(id, n, name)).unwrap_or(2);
    let fp: Vec<_> = fp.into_iter().filter(|(id, n, _)| flatpak_rank(id, n, name) <= best.max(1)).take(1).collect();
    // "OBS Studio" → "obs-studio".
    let named: Vec<String> = fp.iter().map(|(_, n, _)| n.to_lowercase().replace(' ', "-")).collect();

    let found = run_capture("dnf", &["-q", "--cacheonly", "search", name], Duration::from_secs(25))
        .filter(|o| !o.trim().is_empty())
        .or_else(|| run_capture("dnf", &["-q", "search", name], Duration::from_secs(40)))
        .unwrap_or_default();
    let mut dnf = parse_dnf_search(&found);
    let score = |p: &str| {
        if named.iter().any(|n| n == p) { 0 } else { rank(p, &q) + 1 }
    };
    dnf.sort_by_key(|(p, _)| (score(p), p.len()));

    let mut out: Vec<Candidate> = Vec::new();
    // Only names that are the program, not everything that mentions it.
    for (pkg, summary) in dnf.into_iter().filter(|(p, _)| score(p) <= 1 || (score(p) == 2 && rank(p, &q) == 1)).take(1) {
        let installed = rpm_installed(&pkg);
        out.push(Candidate { source: "dnf", id: pkg, summary, installed });
    }
    for (id, n, desc) in fp {
        let installed = flatpak_installed(&id);
        out.push(Candidate { source: "flatpak", id, summary: format!("{n} — {desc}"), installed });
    }
    out
}

pub fn install_command(c: &Candidate) -> String {
    match c.source {
        "dnf" => format!("sudo dnf install {}", quote(&c.id)),
        _ => format!("flatpak install flathub {}", quote(&c.id)),
    }
}

fn install_reply(name: &str, cands: Vec<Candidate>) -> Reply {
    if cands.is_empty() {
        return Reply::Note {
            text: format!("I couldn't find “{name}” in the Fedora repositories or on Flathub. Another spelling?"),
        };
    }
    if let Some(c) = cands.iter().find(|c| c.installed) {
        if cands.len() == 1 {
            return Reply::Note { text: format!("{} is already installed ({}).", c.id, c.source) };
        }
    }
    let to_choice = |c: &Candidate| {
        let how = if c.source == "dnf" { "Fedora repo (dnf)" } else { "Flathub (flatpak)" };
        let mut expl = format!("Install {} from {how}.", c.id);
        if c.installed {
            expl = format!("{} is already installed from {how}; this would reinstall it.", c.id);
        }
        Choice {
            label: format!("{} — {how}", c.id),
            detail: c.summary.clone(),
            card: card(install_command(c), expl, Risk::Careful, vec![c.summary.clone()]),
        }
    };
    let options: Vec<Choice> = cands.iter().take(3).map(to_choice).collect();
    if options.len() == 1 {
        return Reply::Card { card: options[0].card.clone() };
    }
    Reply::Choices { title: format!("Where should “{name}” come from?"), options }
}

// ── Kill: real processes ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub comm: String,
    pub rss_kb: u64,
    pub args: String,
}

/// `ps -eo pid=,comm=,rss=,args=` rows.
pub fn parse_ps(out: &str) -> Vec<Proc> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let comm = it.next()?.to_string();
            let rss_kb = it.next()?.parse().ok()?;
            let args = it.collect::<Vec<_>>().join(" ");
            Some(Proc { pid, comm, rss_kb, args })
        })
        .collect()
}

/// Processes of `name`: by command name first, else by program path. Never
/// ourselves (this process, its children, the `ps` asking).
pub fn matching(procs: &[Proc], name: &str, self_pid: u32) -> Vec<Proc> {
    let q = name.to_lowercase();
    let exe = |p: &Proc| p.args.split_whitespace().next().unwrap_or("").rsplit('/').next().unwrap_or("").to_lowercase();
    let by = |f: &dyn Fn(&Proc) -> bool| procs.iter().filter(|p| p.pid != self_pid && f(p)).cloned().collect::<Vec<_>>();
    let hits = by(&|p| p.comm.to_lowercase() == q || exe(p) == q);
    if !hits.is_empty() {
        return hits;
    }
    by(&|p| p.comm.to_lowercase().contains(&q) || exe(p).contains(&q))
}

fn mem(kb: u64) -> String {
    if kb >= 1024 * 1024 {
        format!("{:.1} GB", kb as f64 / 1024.0 / 1024.0)
    } else {
        format!("{} MB", kb / 1024)
    }
}

/// `flatpak ps --columns=application,pid`: which flatpak app owns which PID.
pub fn parse_flatpak_ps(out: &str) -> Vec<(String, u32)> {
    out.lines()
        .filter_map(|l| {
            let mut c = l.split_whitespace();
            let app = c.next()?.to_string();
            let pid = c.next()?.parse().ok()?;
            app.contains('.').then_some((app, pid))
        })
        .collect()
}

fn kill_reply(name: &str, self_pid: u32, force: bool) -> Reply {
    let ps = run_capture("ps", &["-eo", "pid=,comm=,rss=,args="], Duration::from_secs(5)).unwrap_or_default();
    let procs = parse_ps(&ps);
    // Our own children (the claude router, this very ps) are not the target.
    let mut hits = matching(&procs, name, self_pid);
    hits.retain(|p| !(p.comm == "ps" || p.comm == "claude" && p.args.contains("--json-schema")));
    if hits.is_empty() {
        return Reply::Note { text: format!("Nothing named “{name}” is running.") };
    }
    let details: Vec<String> = hits
        .iter()
        .take(12)
        .map(|p| format!("{:>7}  {:<18} {:>8}", p.pid, p.comm, mem(p.rss_kb)))
        .collect();
    let flat = run_capture("flatpak", &["ps", "--columns=application,pid"], Duration::from_secs(4))
        .map(|o| parse_flatpak_ps(&o))
        .unwrap_or_default();
    let apps: Vec<String> = {
        let mut a: Vec<String> = flat
            .iter()
            .filter(|(_, pid)| hits.iter().any(|h| h.pid == *pid))
            .map(|(app, _)| app.clone())
            .collect();
        a.sort();
        a.dedup();
        a
    };
    let (command, expl) = if !apps.is_empty() {
        (
            apps.iter().map(|a| format!("flatpak kill {}", quote(a))).collect::<Vec<_>>().join(" && "),
            format!("Stops the Flatpak app {} and everything it started.", apps.join(", ")),
        )
    } else {
        let pids = hits.iter().map(|p| p.pid.to_string()).collect::<Vec<_>>().join(" ");
        let plural = if hits.len() == 1 { "" } else { "es" };
        if force {
            (
                format!("kill -9 {pids}"),
                format!("Force-kills {} process{plural} named “{name}” (SIGKILL): no chance to save.", hits.len()),
            )
        } else {
            (
                format!("kill {pids}"),
                format!(
                    "Asks {} process{plural} named “{name}” to quit (SIGTERM). If one stays, say “force kill {name}”.",
                    hits.len()
                ),
            )
        }
    };
    // The names go through the same rules as any command, so a protected
    // process earns its red warning.
    let judged = format!("{command} {}", hits.iter().map(|p| p.comm.clone()).collect::<Vec<_>>().join(" "));
    let (_, mut warnings, _) = assess(&judged, Risk::Careful);
    if force {
        warnings.push("Force kill: unsaved work in it is lost.".into());
    }
    let mut c = card(command, expl, Risk::Careful, details);
    for w in warnings {
        if !c.warnings.contains(&w) && (w.contains("desktop session") || w.starts_with("Force kill")) {
            if w.contains("desktop session") {
                c.risk = Risk::Dangerous;
            }
            c.warnings.push(w);
        }
    }
    c.inline_ok = false;
    Reply::Card { card: c }
}

// ── Files: a plan with real paths ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct PlanOp {
    pub op: String, // cp | mv | mkdir
    #[serde(default)]
    pub src: Option<String>,
    pub dst: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilePlan {
    #[serde(default)]
    pub ops: Vec<PlanOp>,
    #[serde(default)]
    pub explanation: String,
    /// The task is ambiguous: these paths all fit `which`.
    #[serde(default)]
    pub ambiguous: Option<Ambiguous>,
    /// The model could not resolve something.
    #[serde(default)]
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ambiguous {
    /// What the options stand for ("the ds resume").
    pub which: String,
    pub options: Vec<String>,
}

fn expand_home(p: &str, home: &Path) -> std::path::PathBuf {
    if p == "~" {
        home.to_path_buf()
    } else if let Some(r) = p.strip_prefix("~/") {
        home.join(r)
    } else {
        std::path::PathBuf::from(p)
    }
}

/// Turns a plan into a card, or says why it can't be one. Sources must exist;
/// a destination's folder must exist or be made by an explicit `mkdir` op,
/// which the card calls out. The command is built here, from the plan, with
/// every path quoted — never taken from the model.
pub fn plan_card(plan: &FilePlan, home: &Path) -> Result<Card, String> {
    if let Some(p) = &plan.problem {
        return Err(p.clone());
    }
    if plan.ops.is_empty() || plan.ops.len() > 12 {
        return Err("I couldn't turn that into a file plan.".into());
    }
    let mut parts = Vec::new();
    let mut details = Vec::new();
    let mut warnings = Vec::new();
    let mut made: Vec<std::path::PathBuf> = Vec::new();
    let mut destructive = false;
    for op in &plan.ops {
        let dst = expand_home(&op.dst, home);
        if !dst.is_absolute() {
            return Err(format!("The destination “{}” isn't an absolute path.", op.dst));
        }
        match op.op.as_str() {
            "mkdir" => {
                if dst.exists() {
                    continue;
                }
                warnings.push(format!("Creates the folder {}", dst.display()));
                made.push(dst.clone());
                details.push(format!("mkdir -p {}", dst.display()));
                parts.push(format!("mkdir -p {}", quote(&dst.to_string_lossy())));
            }
            "cp" | "mv" => {
                let src = expand_home(op.src.as_deref().ok_or("A copy or move had no source.")?, home);
                if !src.is_absolute() || !src.exists() {
                    return Err(format!("{} doesn't exist.", src.display()));
                }
                let dir = if dst.is_dir() { dst.clone() } else { dst.parent().unwrap_or(Path::new("/")).to_path_buf() };
                if !dir.exists() && !made.iter().any(|m| m == &dir || dir.starts_with(m)) {
                    return Err(format!("The folder {} doesn't exist and the plan doesn't create it.", dir.display()));
                }
                if dst.exists() && !dst.is_dir() {
                    warnings.push(format!("{} already exists and would be replaced.", dst.display()));
                    destructive = true;
                }
                let flag = if op.op == "cp" && src.is_dir() { "cp -r --" } else if op.op == "cp" { "cp --" } else { "mv --" };
                details.push(format!("{} {} → {}", op.op, src.display(), dst.display()));
                parts.push(format!("{flag} {} {}", quote(&src.to_string_lossy()), quote(&dst.to_string_lossy())));
            }
            other => return Err(format!("“{other}” isn't something I plan files with.")),
        }
    }
    if parts.is_empty() {
        return Err("There's nothing to do — everything is already in place.".into());
    }
    let command = parts.join(" && ");
    let mut c = card(command, plan.explanation.clone(), if destructive { Risk::Careful } else { Risk::Safe }, details);
    for w in warnings {
        if !c.warnings.contains(&w) {
            c.warnings.push(w);
        }
    }
    // Making folders or replacing files is the user's call, in the terminal view.
    if !c.warnings.is_empty() {
        c.risk = c.risk.max(Risk::Careful);
        c.inline_ok = false;
    }
    Ok(c)
}

// ── Talking to claude -p ──────────────────────────────────────────────────────

const ROUTER_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "intent": {"type": "string", "enum": ["chat", "install", "kill", "files", "command", "prompt", "timer", "media", "mute", "notifications"]},
    "args": {"type": "object"},
    "command": {"type": "string"},
    "explanation": {"type": "string"},
    "risk": {"type": "string", "enum": ["safe", "careful", "dangerous"]}
  },
  "required": ["intent", "explanation", "risk"]
}"#;

const ROUTER_SYSTEM: &str = "You are the intent router for Fedo, a desktop assistant on Fedora Linux (GNOME, Wayland, zsh/bash, dnf, flatpak). \
Classify ONE user message. Reply with JSON only, matching the schema.\n\
intent:\n\
- install: install a program. args {\"name\": the program}. No command.\n\
- kill: stop or force-quit a running program. args {\"name\": the program}. No command.\n\
- files: copy, move, rename or organise the user's files and folders. args {\"task\": the request, verbatim}. No command.\n\
- command: any other one-off system action that needs a shell command (disk usage, network info, restarting a service, changing a setting…). Put ONE command line in `command`; Fedora-correct; prefer read-only forms; no placeholders.\n\
- prompt: the user wants their rough text rewritten as a clear prompt/instruction for Claude Code. args {\"text\": the rough text}.\n\
- timer: set a timer or reminder. args {\"seconds\": number, \"label\": string}.\n\
- media: play, pause, skip or go back in music. args {\"action\": \"playPause\"|\"next\"|\"previous\"}.\n\
- mute: mute or unmute Fedo's sounds. args {\"on\": boolean}.\n\
- notifications: read or summarise the user's notifications or messages.\n\
- chat: anything else — questions, explanations, writing, coding help, conversation. Use chat when unsure.\n\
explanation: one short sentence. risk: how harmful the action could be if it ran (safe = read-only or trivially reversible; careful = changes the system or installs/removes software; dangerous = can destroy data or break the session).";

const FILES_SYSTEM: &str = "You plan file operations for the user on Linux. The user's home directory is given. \
Use ONLY the read-only tools you have (Glob, ls, find) to find the REAL files and folders the request refers to; names in the request are fuzzy, so match them against what exists. \
Never invent a path. All paths in the answer are absolute. \
Reply with JSON only: {\"ops\": [{\"op\": \"cp\"|\"mv\"|\"mkdir\", \"src\": path (cp/mv), \"dst\": path}], \"explanation\": one sentence, \"ambiguous\": {\"which\": \"what is unclear\", \"options\": [paths]} (only if several existing files fit and you cannot tell which; then leave ops empty), \"problem\": string (only if nothing matches)}. \
For a copy/rename use dst = the full new file path. Add a mkdir op only if the destination folder is missing and the user asked for it to exist.";

const PROMPT_SYSTEM: &str = "You turn a developer's raw, frustrated or rambling reaction into a clear, specific instruction for Claude Code (an AI coding agent working in their repository). \
Output ONLY the prompt text, no preface and no quotes. Structure it tightly: what is wrong or wanted (observed vs expected), the precise behaviour wanted, constraints (what must not change), and acceptance criteria as a short checklist. \
Be concise and concrete; keep every specific detail (names, files, numbers) the user gave; do not invent details; no pleasantries, no emotion, no fluff.";

/// One `claude -p` call returning JSON that fits `schema`: Claude Code's own
/// structured output (`--json-schema`), checked again here, retried once.
fn claude_json(
    system: &str,
    prompt: &str,
    schema: &str,
    model: &str,
    allowed: Option<&str>,
    cwd: &Path,
    limit: Duration,
) -> Result<Value, String> {
    let mut last = String::from("no answer");
    for _ in 0..2 {
        let mut cmd = Command::new("claude");
        cmd.args(["-p", prompt, "--model", model, "--output-format", "json", "--json-schema", schema])
            .args(["--append-system-prompt", system]);
        match allowed {
            Some(tools) => {
                cmd.args(["--allowedTools", tools]);
            }
            None => {
                cmd.args(["--tools", ""]);
            }
        }
        cmd.current_dir(cwd).env("FEDO_CHAT", "1");
        let out = run_capture_cmd(cmd, limit).ok_or("The `claude` command wasn't found or timed out.")?;
        let Ok(v) = serde_json::from_str::<Value>(&out) else {
            last = "claude returned something unreadable".into();
            continue;
        };
        if v["is_error"].as_bool() == Some(true) {
            return Err(v["result"].as_str().unwrap_or("Claude Code reported an error.").to_string());
        }
        if let Some(s) = v.get("structured_output").filter(|s| s.is_object()) {
            return Ok(s.clone());
        }
        if let Some(Ok(parsed)) = v["result"].as_str().map(serde_json::from_str::<Value>) {
            if parsed.is_object() {
                return Ok(parsed);
            }
        }
        last = "claude didn't return the JSON asked for".into();
    }
    Err(last)
}

fn run_capture_cmd(mut cmd: Command, limit: Duration) -> Option<String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// Plain text out of `claude -p` (the prompt writer).
fn claude_text(system: &str, prompt: &str, model: &str, cwd: &Path, limit: Duration) -> Result<String, String> {
    let mut cmd = Command::new("claude");
    cmd.args(["-p", prompt, "--model", model, "--output-format", "text", "--tools", ""])
        .args(["--append-system-prompt", system])
        .current_dir(cwd)
        .env("FEDO_CHAT", "1");
    let out = run_capture_cmd(cmd, limit).ok_or("The `claude` command wasn't found or timed out.")?;
    let out = out.trim().to_string();
    if out.is_empty() {
        Err("Claude Code returned nothing.".into())
    } else {
        Ok(out)
    }
}

// ── The router ────────────────────────────────────────────────────────────────

fn risk_of(v: &Value) -> Risk {
    match v["risk"].as_str() {
        Some("safe") => Risk::Safe,
        Some("dangerous") => Risk::Dangerous,
        _ => Risk::Careful,
    }
}

/// A chat message the assistant handles, or `Reply::Chat`. `prompt_mode`
/// skips the router: the text is rewritten as a prompt (the prompt writer's
/// own button).
pub async fn route(app: &AppHandle, query: String, prompt_mode: bool, fast_only: bool) -> Result<Reply, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || route_blocking(&app, &query, prompt_mode, fast_only))
        .await
        .map_err(|e| e.to_string())?
}

fn route_blocking(app: &AppHandle, query: &str, prompt_mode: bool, fast_only: bool) -> Result<Reply, String> {
    let home = crate::platform::home_dir();
    if prompt_mode {
        return write_prompt(query, &home);
    }
    if let Some(fast) = fast_path(query) {
        return match fast {
            Fast::Mute(on) => {
                crate::set_sound_enabled(app, !on);
                Ok(Reply::Done { text: if on { "Sounds muted." } else { "Sounds on." }.into() })
            }
            Fast::Media(action) => {
                let ok = tauri::async_runtime::block_on(crate::spotify::spotify_control(app.clone(), action.into(), None));
                Ok(if ok {
                    Reply::Done { text: "Done.".into() }
                } else {
                    Reply::Note { text: "Spotify isn't running, so there's nothing to control.".into() }
                })
            }
            Fast::Timer => Ok(Reply::Note { text: "Timers and reminders arrive in the next phase.".into() }),
            Fast::Notifications => Ok(Reply::Note { text: "Reading notifications arrives with phone & messages (Phase 6).".into() }),
            Fast::Install(name) => Ok(install_reply(&name, find_candidates(&name))),
            Fast::Kill(name, force) => Ok(kill_reply(&name, std::process::id(), force)),
            Fast::PromptRewrite(text) => write_prompt(&text, &home),
        };
    }
    // Mid-conversation follow-ups are chat: only the obvious ones are caught.
    if fast_only {
        return Ok(Reply::Chat);
    }
    let routed = claude_json(ROUTER_SYSTEM, query, ROUTER_SCHEMA, "haiku", None, &home, Duration::from_secs(45))?;
    dispatch(app, query, &routed, &home)
}

/// What a routed intent does. Public for tests with a fake model answer.
fn dispatch(app: &AppHandle, query: &str, routed: &Value, home: &Path) -> Result<Reply, String> {
    let arg = |k: &str| routed["args"][k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    match routed["intent"].as_str().unwrap_or("chat") {
        "install" => match arg("name").and_then(|n| program_name(&n)) {
            Some(n) => Ok(install_reply(&n, find_candidates(&n))),
            None => Ok(Reply::Chat),
        },
        "kill" => match arg("name").and_then(|n| program_name(&n)) {
            Some(n) => Ok(kill_reply(&n, std::process::id(), false)),
            None => Ok(Reply::Chat),
        },
        "files" => plan_files(&arg("task").unwrap_or_else(|| query.to_string()), home),
        "command" => {
            let Some(cmd) = routed["command"].as_str().map(str::trim).filter(|c| !c.is_empty()) else {
                return Ok(Reply::Chat);
            };
            let expl = routed["explanation"].as_str().unwrap_or("").to_string();
            Ok(Reply::Card { card: card(cmd.to_string(), expl, risk_of(routed), vec![]) })
        }
        "prompt" => write_prompt(&arg("text").unwrap_or_else(|| query.to_string()), home),
        "timer" => Ok(Reply::Note { text: "Timers and reminders arrive in the next phase.".into() }),
        "notifications" => Ok(Reply::Note { text: "Reading notifications arrives with phone & messages (Phase 6).".into() }),
        "mute" => {
            let on = routed["args"]["on"].as_bool().unwrap_or(true);
            crate::set_sound_enabled(app, !on);
            Ok(Reply::Done { text: if on { "Sounds muted." } else { "Sounds on." }.into() })
        }
        "media" => {
            let action = match routed["args"]["action"].as_str() {
                Some("next") => "next",
                Some("previous") => "previous",
                _ => "playPause",
            };
            let ok = tauri::async_runtime::block_on(crate::spotify::spotify_control(app.clone(), action.into(), None));
            Ok(if ok {
                Reply::Done { text: "Done.".into() }
            } else {
                Reply::Note { text: "Spotify isn't running, so there's nothing to control.".into() }
            })
        }
        _ => Ok(Reply::Chat),
    }
}

fn write_prompt(raw: &str, home: &Path) -> Result<Reply, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Reply::Note { text: "Paste what you want rewritten after “rewrite this as a prompt”.".into() });
    }
    let text = claude_text(PROMPT_SYSTEM, raw, "sonnet", home, Duration::from_secs(90))?;
    Ok(Reply::Prompt { text })
}

const FILES_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "ops": {"type": "array", "items": {"type": "object", "properties": {"op": {"type": "string", "enum": ["cp", "mv", "mkdir"]}, "src": {"type": "string"}, "dst": {"type": "string"}}, "required": ["op", "dst"]}},
    "explanation": {"type": "string"},
    "ambiguous": {"type": "object", "properties": {"which": {"type": "string"}, "options": {"type": "array", "items": {"type": "string"}}}, "required": ["which", "options"]},
    "problem": {"type": "string"}
  },
  "required": ["ops", "explanation"]
}"#;

fn plan_files(task: &str, home: &Path) -> Result<Reply, String> {
    let prompt = format!("Home directory: {}\n\nRequest: {task}", home.display());
    let v = claude_json(
        FILES_SYSTEM,
        &prompt,
        FILES_SCHEMA,
        "haiku",
        Some("Glob Bash(ls:*) Bash(find:*)"),
        home,
        Duration::from_secs(90),
    )?;
    let plan: FilePlan = serde_json::from_value(v).map_err(|e| e.to_string())?;
    files_reply(&plan, home)
}

/// A plan, or the question that settles it.
pub fn files_reply(plan: &FilePlan, home: &Path) -> Result<Reply, String> {
    if let Some(amb) = &plan.ambiguous {
        // Which one? Each option becomes the plan's source in turn.
        let options: Vec<Choice> = amb
            .options
            .iter()
            .filter(|p| expand_home(p, home).exists())
            .take(6)
            .filter_map(|p| {
                let mut one = plan.clone();
                one.ambiguous = None;
                // The ops are empty while ambiguous: the model's own op shape is
                // reused by re-planning with the chosen file named.
                one.ops = plan.ops.iter().cloned().map(|mut o| { if o.op != "mkdir" { o.src = Some(p.clone()); } o }).collect();
                plan_card(&one, home).ok().map(|card| Choice {
                    label: p.rsplit('/').next().unwrap_or(p).to_string(),
                    detail: p.clone(),
                    card,
                })
            })
            .collect();
        if options.len() > 1 {
            return Ok(Reply::Choices { title: format!("Which one is {}?", amb.which), options });
        }
        if options.is_empty() {
            return Ok(Reply::Note { text: format!("I found several candidates for {} but couldn't plan with any.", amb.which) });
        }
    }
    match plan_card(plan, home) {
        Ok(card) => Ok(Reply::Card { card }),
        Err(why) => Ok(Reply::Note { text: why }),
    }
}

// ── Running a card ────────────────────────────────────────────────────────────

/// What a run returns for the island.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub ok: bool,
    /// Inline runs: what it printed (trimmed).
    pub output: String,
    pub note: String,
}

/// Arguments for a new terminal window that runs `command` and waits.
pub fn terminal_args(command: &str) -> Vec<String> {
    let script = format!("{command}; echo; read -rp 'Done — press Enter to close'");
    vec!["--new-window".into(), "--".into(), "bash".into(), "-lc".into(), script]
}

/// Opens a Ptyxis window running the command (or GNOME Terminal as a fallback).
/// Only ever called from the island's Run action.
pub fn run_in_terminal(command: &str) -> Result<(), String> {
    let try_spawn = |prog: &str, args: Vec<String>| {
        Command::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    };
    if try_spawn("ptyxis", terminal_args(command)) {
        return Ok(());
    }
    let script = format!("{command}; echo; read -rp 'Done — press Enter to close'");
    if try_spawn("gnome-terminal", vec!["--".into(), "bash".into(), "-lc".into(), script]) {
        return Ok(());
    }
    Err("No terminal found (looked for ptyxis and gnome-terminal).".into())
}

/// Hidden run for a safe, non-sudo command; output comes back to the island.
pub fn run_inline(command: &str) -> RunResult {
    let mut cmd = Command::new("bash");
    cmd.args(["-lc", command]);
    match run_capture_cmd(cmd, Duration::from_secs(30)) {
        Some(out) => {
            let t: String = out.trim().chars().take(4000).collect();
            RunResult { ok: true, output: t, note: String::new() }
        }
        None => RunResult { ok: false, output: String::new(), note: "It took longer than 30 s and was stopped.".into() },
    }
}

/// Puts text on the Wayland clipboard.
pub fn copy_text(text: &str) -> bool {
    use std::io::Write;
    let Ok(mut child) = Command::new("wl-copy").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

#[tauri::command]
pub async fn assist_route(app: AppHandle, query: String, prompt_mode: bool, fast_only: bool) -> Result<Reply, String> {
    route(&app, query, prompt_mode, fast_only).await
}

/// `inline` is honoured only when Fedo's own rules call the command safe.
#[tauri::command]
pub async fn assist_run(command: String, inline: bool) -> Result<RunResult, String> {
    let command = command.trim().to_string();
    if command.is_empty() || command.len() > 4000 {
        return Err("Nothing to run.".into());
    }
    let (risk, _, sudo) = assess(&command, Risk::Safe);
    if inline && risk == Risk::Safe && !sudo && !command.contains('\n') {
        return tauri::async_runtime::spawn_blocking(move || run_inline(&command))
            .await
            .map_err(|e| e.to_string());
    }
    run_in_terminal(&command)?;
    Ok(RunResult { ok: true, output: String::new(), note: "Opened in a terminal window.".into() })
}

#[tauri::command]
pub fn assist_copy(text: String) -> bool {
    copy_text(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obvious_requests_never_reach_the_model() {
        assert_eq!(fast_path("mute"), Some(Fast::Mute(true)));
        assert_eq!(fast_path("Unmute sounds!"), Some(Fast::Mute(false)));
        assert_eq!(fast_path("pause the music"), Some(Fast::Media("playPause")));
        assert_eq!(fast_path("next song"), Some(Fast::Media("next")));
        assert_eq!(fast_path("install obs"), Some(Fast::Install("obs".into())));
        assert_eq!(fast_path("Install Visual Studio Code"), Some(Fast::Install("visual studio code".into())));
        assert_eq!(fast_path("kill vlc completely"), Some(Fast::Kill("vlc".into(), false)));
        assert_eq!(fast_path("force kill vlc"), Some(Fast::Kill("vlc".into(), true)));
        assert_eq!(fast_path("timer 10 min"), Some(Fast::Timer));
        assert_eq!(fast_path("read my notifications"), Some(Fast::Notifications));
        assert_eq!(
            fast_path("rewrite this as a prompt: no the button is wrong"),
            Some(Fast::PromptRewrite("no the button is wrong".into()))
        );
    }

    #[test]
    fn longer_sentences_go_to_the_model() {
        assert_eq!(fast_path("install the latest nvidia driver and reboot when it is done"), None);
        assert_eq!(fast_path("kill the process that is using port 8080"), None);
        assert_eq!(fast_path("how do I install obs on fedora?"), None);
        assert_eq!(fast_path("go to work/resumes and copy the ds resume"), None);
        assert_eq!(fast_path("install $(rm -rf ~)"), None);
    }

    #[test]
    fn risk_is_raised_by_fedos_own_rules_and_never_lowered() {
        assert_eq!(assess("ls -la ~", Risk::Safe).0, Risk::Safe);
        assert_eq!(assess("ls", Risk::Dangerous).0, Risk::Dangerous);
        let (r, w, sudo) = assess("sudo dnf install obs-studio", Risk::Safe);
        assert_eq!((r, sudo), (Risk::Careful, true));
        assert!(!w.is_empty());
        assert_eq!(assess("rm -rf ~/old", Risk::Safe).0, Risk::Dangerous);
        assert_eq!(assess("dd if=/dev/zero of=/dev/sda", Risk::Safe).0, Risk::Dangerous);
        assert_eq!(assess("curl https://x.sh | sh", Risk::Safe).0, Risk::Dangerous);
        let (r, w, _) = assess("pkill -9 gnome-shell", Risk::Careful);
        assert_eq!(r, Risk::Dangerous);
        assert!(w.iter().any(|x| x.contains("gnome-shell")));
        assert_eq!(assess("rm notes.txt", Risk::Safe).0, Risk::Safe);
    }

    #[test]
    fn only_a_plain_safe_command_may_run_hidden() {
        assert!(card("df -h".into(), "".into(), Risk::Safe, vec![]).inline_ok);
        assert!(!card("sudo df -h".into(), "".into(), Risk::Safe, vec![]).inline_ok);
        assert!(!card("df -h".into(), "".into(), Risk::Careful, vec![]).inline_ok);
    }

    #[test]
    fn dnf_and_flatpak_output_is_read() {
        let dnf = "Matched fields: name (exact)\n obs-studio.x86_64 : Open Broadcaster Software Studio\nobs-studio.x86_64 : Open Broadcaster Software Studio\nobs-cli.noarch : A cli\nobs-studio-libs.i686 : x\n";
        let p = parse_dnf_search(dnf);
        assert_eq!(p[0].0, "obs-studio");
        assert_eq!(p.len(), 2);
        // dnf5 quiet: tab separated.
        let q = parse_dnf_search("Matched fields: name, summary\n obs-studio.x86_64\tOpen Broadcaster Software Studio\nobs-studio-plugin-vaapi.x86_64\tVAAPI\n");
        assert_eq!(q[0], ("obs-studio".to_string(), "Open Broadcaster Software Studio".to_string()));
        assert_eq!(rank("obs-studio-plugin-vaapi", "obs"), 3);
        let fp = "com.obsproject.Studio\tOBS Studio\tLive streaming\ncom.obsproject.Studio\tOBS Studio\tLive streaming\norg.gtk.Gtk3theme.Obsidian-2\tObs\tt\ncom.obsproject.Studio.Plugin.x\tX\ty\nmd.obsidian.Obsidian\tObsidian\tz\n";
        let f = parse_flatpak_search(fp);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].0, "com.obsproject.Studio");
        assert_eq!(flatpak_rank("com.obsproject.Studio", "OBS Studio", "obs"), 1);
        assert_eq!(flatpak_rank("md.obsidian.Obsidian", "Obsidian", "obs"), 2);
        assert_eq!(flatpak_rank("org.videolan.VLC", "VLC", "vlc"), 0);
        assert_eq!(rank("obs-studio", "obs"), 1);
        assert_eq!(rank("obs", "obs"), 0);
        assert_eq!(rank("gobs", "obs"), 2);
    }

    #[test]
    fn install_choices_carry_the_exact_commands() {
        let c = vec![
            Candidate { source: "dnf", id: "obs-studio".into(), summary: "Open Broadcaster".into(), installed: false },
            Candidate { source: "flatpak", id: "com.obsproject.Studio".into(), summary: "OBS".into(), installed: false },
        ];
        let Reply::Choices { options, .. } = install_reply("obs", c) else { panic!("expected choices") };
        assert_eq!(options[0].card.command, "sudo dnf install obs-studio");
        assert!(options[0].card.sudo);
        assert_eq!(options[1].card.command, "flatpak install flathub com.obsproject.Studio");
        assert!(matches!(install_reply("zzz", vec![]), Reply::Note { .. }));
    }

    #[test]
    fn processes_are_matched_and_never_ourselves() {
        let ps = parse_ps("  100 vlc 204800 /usr/bin/vlc --started-from-file x\n  101 bash 2000 bash -lc kill vlc\n  200 fedo 9000 /x/fedo\n");
        let m = matching(&ps, "vlc", 999);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].pid, 100);
        assert!(matching(&ps, "vlc", 100).is_empty());
        let fp = parse_flatpak_ps("org.videolan.VLC 4242\n");
        assert_eq!(fp, vec![("org.videolan.VLC".to_string(), 4242)]);
        assert_eq!(mem(204800), "200 MB");
    }

    fn tree(name: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!("fedo-assist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("work/resumes")).unwrap();
        std::fs::write(base.join("work/resumes/DS_Resume_2026.pdf"), b"x").unwrap();
        base
    }

    #[test]
    fn a_file_plan_uses_real_paths_and_builds_its_own_command() {
        let home = tree("plan");
        let src = home.join("work/resumes/DS_Resume_2026.pdf");
        let plan = FilePlan {
            ops: vec![
                PlanOp { op: "mkdir".into(), src: None, dst: home.join("tempres").to_string_lossy().into() },
                PlanOp { op: "cp".into(), src: Some(src.to_string_lossy().into()), dst: home.join("tempres/X.pdf").to_string_lossy().into() },
            ],
            explanation: "Copy the resume".into(),
            ambiguous: None,
            problem: None,
        };
        let c = plan_card(&plan, &home).unwrap();
        assert!(c.command.starts_with("mkdir -p "));
        assert!(c.command.contains("&& cp -- "));
        assert!(c.warnings.iter().any(|w| w.starts_with("Creates the folder")));
        assert!(!c.inline_ok);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_plan_with_a_missing_source_or_folder_is_refused() {
        let home = tree("refuse");
        let missing = FilePlan {
            ops: vec![PlanOp { op: "cp".into(), src: Some(home.join("nope.pdf").to_string_lossy().into()), dst: home.join("a.pdf").to_string_lossy().into() }],
            explanation: String::new(), ambiguous: None, problem: None,
        };
        assert!(plan_card(&missing, &home).unwrap_err().contains("doesn't exist"));
        let src = home.join("work/resumes/DS_Resume_2026.pdf");
        let nofolder = FilePlan {
            ops: vec![PlanOp { op: "cp".into(), src: Some(src.to_string_lossy().into()), dst: home.join("tempres/X.pdf").to_string_lossy().into() }],
            explanation: String::new(), ambiguous: None, problem: None,
        };
        assert!(plan_card(&nofolder, &home).unwrap_err().contains("doesn't exist"));
        let rm = FilePlan {
            ops: vec![PlanOp { op: "rm".into(), src: Some(src.to_string_lossy().into()), dst: "/tmp/x".into() }],
            explanation: String::new(), ambiguous: None, problem: None,
        };
        assert!(plan_card(&rm, &home).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn paths_with_spaces_and_quotes_are_quoted() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("/home/x/a.pdf"), "/home/x/a.pdf");
    }

    #[test]
    fn the_terminal_window_waits_for_enter() {
        let a = terminal_args("sudo dnf install obs-studio");
        assert_eq!(&a[..2], ["--new-window", "--"]);
        assert!(a[4].starts_with("sudo dnf install obs-studio; echo;"));
        assert!(a[4].contains("press Enter to close"));
    }

    // ── Against the real machine and the real `claude` (a few haiku tokens) ──
    // cargo test -p fedo --lib assist::tests::live -- --ignored --nocapture

    #[test]
    #[ignore]
    fn live_install_obs() {
        let t = Instant::now();
        let r = install_reply("obs", find_candidates("obs"));
        println!("{:?}\n({:?})", r, t.elapsed());
        assert!(!matches!(r, Reply::Chat));
    }

    #[test]
    #[ignore]
    fn live_kill_a_real_process() {
        let mut child = Command::new("sleep").arg("987").spawn().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let r = kill_reply("sleep", std::process::id(), false);
        println!("{r:?}");
        let Reply::Card { card } = r else { panic!("no card") };
        assert!(card.command.contains(&child.id().to_string()));
        let _ = child.kill();
        let g = kill_reply("gnome-shell", std::process::id(), false);
        println!("{g:?}");
        let Reply::Card { card } = g else { panic!("no card") };
        assert_eq!(card.risk, Risk::Dangerous);
    }

    #[test]
    #[ignore]
    fn live_router_and_file_plan() {
        let home = std::env::temp_dir().join(format!("fedo-live-{}", std::process::id()));
        std::fs::create_dir_all(home.join("work/resumes")).unwrap();
        std::fs::write(home.join("work/resumes/DS_Resume_2026.pdf"), b"x").unwrap();
        std::fs::write(home.join("work/resumes/Cover_Letter.pdf"), b"x").unwrap();
        let r = plan_files(
            "go to work/resumes, copy the ds resume into a folder called tempres and rename it to X.pdf",
            &home,
        );
        println!("{r:?}");
        let routed = claude_json(ROUTER_SYSTEM, "how much disk space is left on my home partition", ROUTER_SCHEMA, "haiku", None, &home, Duration::from_secs(45)).unwrap();
        println!("{routed}");
        let chat = claude_json(ROUTER_SYSTEM, "why is the sky blue?", ROUTER_SCHEMA, "haiku", None, &home, Duration::from_secs(45)).unwrap();
        println!("{chat}");
        assert_eq!(chat["intent"], "chat");
        let p = write_prompt("no what youre doing is wrong, the save button should be disabled while loading, you keep making it spin forever", &home);
        println!("{p:?}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
