// Voice (FEDO.md Phase 8). Everything is local and nothing audio-related runs
// until voice is used:
//
//   mic  ──pw-record──▶ energy VAD ──PCM──▶ fedo-stt (whisper.cpp, one run per
//   utterance, its own process so the model's memory goes back) ──▶ text
//   text ──▶ the island's chat box (dictation), or the same router the chat
//            uses (a command: push-to-talk / wake word)
//   reply ──▶ Piper (a venv under ~/.local/share/fedo) ──▶ pw-play
//
// The island may be torn down or hidden when a command comes in (a fullscreen
// game has focus: Wayland won't let it draw there). Then the whole turn runs
// here in Rust and the answer is spoken and sent as a desktop notification.

use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub const SAMPLE_RATE: usize = 16_000;
/// 20 ms of audio.
const FRAME: usize = 320;
/// Whisper's own limit per window is 30 s; an utterance is never longer.
const MAX_SECONDS: u64 = 30;
/// Nobody spoke: give the microphone back.
const NO_SPEECH_SECONDS: u64 = 8;
/// 200 ms of room tone before the VAD judges anything.
const CALIBRATION_FRAMES: usize = 10;

pub const WHISPER_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
const VOICE_BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium";
pub const VOICE_NAME: &str = "en_US-lessac-medium";
pub const VOSK_MODEL: &str = "vosk-model-small-en-us-0.15";
const VOSK_URL: &str = "https://alphacephei.com/vosk/models/vosk-model-small-en-us-0.15.zip";

// ── Voice-activity detection (pure) ───────────────────────────────────────────

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Vad {
    Quiet,
    Started,
    Ended,
}

/// Energy VAD with a tracked noise floor. Speech starts after 100 ms above the
/// threshold and ends after `end_quiet_ms` below it.
pub struct VadState {
    floor: f32,
    speaking: bool,
    loud: u32,
    quiet: u32,
    end_frames: u32,
    calibrated: u32,
}

impl VadState {
    pub fn new(end_quiet_ms: u32) -> Self {
        Self { floor: 0.004, speaking: false, loud: 0, quiet: 0, end_frames: end_quiet_ms / 20, calibrated: 0 }
    }

    /// The first frames of a recording are the room, not the user: they set the
    /// noise floor outright (a fan above the minimum threshold is not speech).
    pub fn calibrate(&mut self, rms: f32) {
        self.floor = if self.calibrated == 0 { rms } else { self.floor * 0.7 + rms * 0.3 };
        self.calibrated += 1;
    }

    pub fn floor(&self) -> f32 {
        self.floor
    }

    pub fn threshold(&self) -> f32 {
        (self.floor * 3.0).max(0.012)
    }

    /// One 20 ms frame's RMS (0..1).
    pub fn push(&mut self, rms: f32) -> Vad {
        let loud = rms > self.threshold();
        if !self.speaking {
            if !loud {
                // Only quiet frames teach the floor, and slowly.
                self.floor = 0.97 * self.floor + 0.03 * rms;
                self.loud = 0;
                return Vad::Quiet;
            }
            self.loud += 1;
            if self.loud >= 5 {
                self.speaking = true;
                self.quiet = 0;
                return Vad::Started;
            }
            return Vad::Quiet;
        }
        if loud {
            self.quiet = 0;
            Vad::Quiet
        } else {
            self.quiet += 1;
            if self.quiet >= self.end_frames {
                self.speaking = false;
                self.loud = 0;
                Vad::Ended
            } else {
                Vad::Quiet
            }
        }
    }
}

pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

// ── What was said (pure) ──────────────────────────────────────────────────────

/// The words of an utterance, lowercase, without punctuation.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Answer {
    Yes,
    No,
}

/// "yes / do it / run it / go ahead" and "cancel / no / never mind". Only a
/// short, unambiguous utterance counts: "no, play it again" is a command.
pub fn confirmation(text: &str) -> Option<Answer> {
    let w = words(text);
    let j = w.join(" ");
    if w.is_empty() || w.len() > 4 {
        return None;
    }
    const YES: &[&str] = &[
        "yes", "yeah", "yep", "yup", "sure", "ok", "okay", "do it", "run it", "go ahead", "send it",
        "confirm", "yes please", "do it please", "run it please", "go for it", "approve",
    ];
    const NO: &[&str] = &[
        "no", "nope", "cancel", "stop", "never mind", "nevermind", "don't", "dont", "don't do it",
        "no thanks", "cancel that", "forget it", "deny", "abort",
    ];
    if YES.contains(&j.as_str()) {
        Some(Answer::Yes)
    } else if NO.contains(&j.as_str()) {
        Some(Answer::No)
    } else {
        None
    }
}

/// The command after "hey fedo", when the utterance starts with the wake
/// phrase (None when it doesn't; empty when it is only the phrase). Whisper
/// hears "Fedo" as Fido, Feto, Federal… so a few spellings count.
pub fn after_wake_word(text: &str) -> Option<String> {
    const HEY: &[&str] = &["hey", "hay", "hi", "hello", "okay", "ok", "a"];
    const FEDO: &[&str] = &["fedo", "fido", "feto", "fedoh", "fedor", "fado", "feedo", "pedo", "federal", "fedora", "video"];
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        let word_char = c.is_alphanumeric() || c == '\'';
        match (word_char, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                spans.push((s, i));
                start = None;
            }
            _ => {}
        }
        if spans.len() == 2 {
            break;
        }
    }
    if let (Some(s), true) = (start, spans.len() < 2) {
        spans.push((s, text.len()));
    }
    if spans.len() < 2 {
        return None;
    }
    let w = |(a, b): (usize, usize)| text[a..b].to_lowercase();
    if !HEY.contains(&w(spans[0]).as_str()) || !FEDO.contains(&w(spans[1]).as_str()) {
        return None;
    }
    Some(text[spans[1].1..].trim_matches(|c: char| matches!(c, ',' | '.' | '!' | '?' | ':' | ';') || c.is_whitespace()).to_string())
}

/// The part of a reply worth saying aloud: no markdown, no code, short.
/// Returns (spoken, was_cut).
pub fn speakable(text: &str, limit: usize) -> (String, bool) {
    let mut out = String::new();
    let mut in_code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let line = line.trim().trim_start_matches(['#', '>', '-', '*', ' ']).trim();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    let out: String = out.chars().filter(|c| !matches!(c, '*' | '`' | '_' | '#')).collect();
    // Links read badly: keep the words.
    let mut clean = String::new();
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ']' && chars.peek() == Some(&'(') {
            for d in chars.by_ref() {
                if d == ')' {
                    break;
                }
            }
        } else if c != '[' {
            clean.push(c);
        }
    }
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= limit {
        return (clean, false);
    }
    // Whole sentences up to the limit.
    let mut spoken = String::new();
    for sentence in clean.split_inclusive(['.', '!', '?']) {
        if spoken.chars().count() + sentence.chars().count() > limit {
            break;
        }
        spoken.push_str(sentence);
    }
    if spoken.trim().is_empty() {
        spoken = clean.chars().take(limit).collect();
    }
    (spoken.trim().to_string(), true)
}

// ── Where things are ──────────────────────────────────────────────────────────

pub fn models_dir() -> PathBuf {
    crate::platform::local_dir().join("models")
}

fn venv_python() -> PathBuf {
    crate::platform::local_dir().join("venv/bin/python")
}

pub fn whisper_model_path(size: &str) -> PathBuf {
    let size = if size == "small" { "small" } else { "base" };
    models_dir().join(format!("ggml-{size}.bin"))
}

fn stt_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.join("fedo-stt"),
        crate::platform::local_dir().join("bin/fedo-stt"),
        // The RPM puts resources in /usr/lib/Fedo.
        PathBuf::from("/usr/lib/Fedo/fedo-stt"),
        dir.join("../release/fedo-stt"),
        dir.join("../debug/fedo-stt"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

fn wake_script() -> PathBuf {
    crate::platform::local_dir().join("wake.py")
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whisper model + the fedo-stt helper are in place.
    pub stt: bool,
    /// Piper and its voice are in place.
    pub tts: bool,
    /// The wake-word model and engine are in place.
    pub wake: bool,
    pub recording: bool,
    pub mic: bool,
}

pub fn status(model: &str) -> Status {
    let m = models_dir();
    Status {
        stt: whisper_model_path(model).is_file() && stt_binary().is_some(),
        tts: venv_python().is_file() && m.join(format!("{VOICE_NAME}.onnx")).is_file(),
        wake: venv_python().is_file() && m.join(VOSK_MODEL).is_dir(),
        recording: RECORDING.load(Ordering::Relaxed),
        mic: crate::platform::find_on_path("pw-record").is_some(),
    }
}

// ── Events to the island ──────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct VoiceEvent<'a> {
    /// listening | transcribing | idle | error
    state: &'a str,
    mode: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn emit_state(app: &AppHandle, state: &str, mode: Mode, level: Option<f32>, message: Option<String>) {
    // A hidden island that isn't loaded has nobody to tell: don't build it.
    if crate::island::is_loaded(app) {
        let _ = app.emit_to(
            crate::island::WINDOW_LABEL,
            "voice",
            VoiceEvent { state, mode: mode.name(), level, message },
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// The text goes into the chat box.
    Dictate,
    /// The text is a command: routed and answered.
    Command,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Dictate => "dictate",
            Mode::Command => "command",
        }
    }
    pub fn parse(s: &str) -> Mode {
        if s == "dictate" {
            Mode::Dictate
        } else {
            Mode::Command
        }
    }
}

// ── Recording ─────────────────────────────────────────────────────────────────

static RECORDING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// Bumped on every recording; a stale one's result is dropped.
static EPOCH: AtomicU32 = AtomicU32::new(0);

/// Starts listening, or — if already listening — ends it now and transcribes
/// what was heard (push-to-talk pressed again).
pub fn toggle(app: &AppHandle, mode: Mode) {
    if RECORDING.load(Ordering::Acquire) {
        STOP.store(true, Ordering::Release);
        return;
    }
    start(app, mode);
}

pub fn cancel() {
    CANCEL.store(true, Ordering::Release);
    STOP.store(true, Ordering::Release);
}

pub fn start(app: &AppHandle, mode: Mode) {
    if RECORDING.swap(true, Ordering::AcqRel) {
        return;
    }
    STOP.store(false, Ordering::Release);
    CANCEL.store(false, Ordering::Release);
    let epoch = EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    stop_speaking();
    let app = app.clone();
    let _ = std::thread::Builder::new().name("fedo-voice".into()).spawn(move || {
        let outcome = record(&app, mode);
        RECORDING.store(false, Ordering::Release);
        match outcome {
            Ok(Some(pcm)) if !CANCEL.load(Ordering::Acquire) => {
                emit_state(&app, "transcribing", mode, None, None);
                let model = model_setting(&app);
                crate::log::line(format!("voice: transcribing {:.1}s", pcm.len() as f32 / SAMPLE_RATE as f32));
                let heard = transcribe(&pcm, &model, &lang_setting(&app));
                crate::log::line(format!("voice: heard {heard:?}"));
                match heard {
                    Ok(text) if !text.is_empty() => {
                        if EPOCH.load(Ordering::Acquire) == epoch {
                            emit_state(&app, "idle", mode, None, None);
                            deliver(&app, mode, text);
                        }
                    }
                    Ok(_) => emit_state(&app, "idle", mode, None, Some("I didn't catch that.".into())),
                    Err(e) => emit_state(&app, "error", mode, None, Some(e)),
                }
            }
            Ok(_) => emit_state(&app, "idle", mode, None, None),
            Err(e) => {
                crate::log::line(format!("voice: {e}"));
                emit_state(&app, "error", mode, None, Some(e));
            }
        }
    });
}

/// A child that is sent SIGTERM if Fedo dies (a Quit or a crash must not leave
/// a microphone open).
fn die_with_parent(cmd: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    cmd
}

fn spawn_recorder() -> std::io::Result<Child> {
    let mut cmd = Command::new("pw-record");
    die_with_parent(&mut cmd)
        .args(["--rate", "16000", "--channels", "1", "--format", "s16", "--latency", "20ms", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

/// Records one utterance: from the first speech to 1.1 s of quiet. None when
/// nobody spoke or it was cancelled.
fn record(app: &AppHandle, mode: Mode) -> Result<Option<Vec<i16>>, String> {
    let mut child = spawn_recorder().map_err(|e| format!("Can't open the microphone (pw-record): {e}"))?;
    let mut out = child.stdout.take().ok_or("no audio stream")?;
    emit_state(app, "listening", mode, None, None);
    crate::log::line(format!("voice: listening ({})", mode.name()));

    let started = Instant::now();
    let mut vad = VadState::new(1100);
    let mut pcm: Vec<i16> = Vec::new();
    // A little before the first loud frame, so the first syllable isn't clipped.
    let mut pre: std::collections::VecDeque<Vec<i16>> = Default::default();
    let mut speaking = false;
    let mut bytes = vec![0u8; FRAME * 2];
    let mut last_level = Instant::now();
    let mut heard_audio = false;
    let mut frames_seen = 0;
    let result = loop {
        if STOP.load(Ordering::Acquire) {
            break Ok(speaking.then_some(pcm).filter(|p| p.len() > SAMPLE_RATE / 3));
        }
        if out.read_exact(&mut bytes).is_err() {
            break Err("The microphone stopped delivering audio.".to_string());
        }
        heard_audio = true;
        let frame: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let level = rms(&frame);
        if last_level.elapsed() > Duration::from_millis(80) {
            emit_state(app, "listening", mode, Some((level * 6.0).min(1.0)), None);
            last_level = Instant::now();
        }
        if frames_seen < CALIBRATION_FRAMES {
            frames_seen += 1;
            vad.calibrate(level);
            pre.push_back(frame);
            continue;
        }
        match vad.push(level) {
            Vad::Started => {
                crate::log::line(format!("voice: speech started (floor {:.4}, threshold {:.4})", vad.floor(), vad.threshold()));
                speaking = true;
                for f in pre.drain(..) {
                    pcm.extend(f);
                }
                pcm.extend(frame);
            }
            Vad::Ended => {
                pcm.extend(frame);
                break Ok(Some(pcm));
            }
            Vad::Quiet => {
                if speaking {
                    pcm.extend(frame);
                } else {
                    pre.push_back(frame);
                    if pre.len() > 15 {
                        pre.pop_front();
                    }
                }
            }
        }
        let secs = started.elapsed().as_secs();
        if !speaking && secs >= NO_SPEECH_SECONDS {
            break Ok(None);
        }
        if secs >= MAX_SECONDS {
            break Ok(speaking.then_some(pcm));
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    if result.is_ok() && !heard_audio {
        return Err("No microphone input.".into());
    }
    result
}

/// Whisper, in its own process.
pub fn transcribe(pcm: &[i16], model: &str, lang: &str) -> Result<String, String> {
    let bin = stt_binary().ok_or("Speech-to-text isn't set up (fedo-stt is missing).")?;
    let path = whisper_model_path(model);
    if !path.is_file() {
        return Err(format!("The speech model {} isn't downloaded yet.", path.display()));
    }
    let mut child = Command::new(bin)
        .arg("--model")
        .arg(&path)
        .args(["--lang", if lang.is_empty() { "auto" } else { lang }])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start fedo-stt: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
    });
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !out.status.success() {
        return Err("Speech-to-text failed.".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ── Settings ──────────────────────────────────────────────────────────────────

fn model_setting(app: &AppHandle) -> String {
    app.state::<crate::Shared>().settings.lock().unwrap().voice_model.clone()
}

fn lang_setting(app: &AppHandle) -> String {
    app.state::<crate::Shared>().settings.lock().unwrap().voice_language.clone()
}

fn speak_setting(app: &AppHandle) -> bool {
    app.state::<crate::Shared>().settings.lock().unwrap().voice_speak
}

// ── What happens with the words ───────────────────────────────────────────────

#[derive(Serialize, Clone)]
struct Said<'a> {
    text: &'a str,
    mode: &'a str,
    /// Spoken replies are expected for this one.
    voice: bool,
}

fn deliver(app: &AppHandle, mode: Mode, text: String) {
    // A command: when the island is on screen the page runs it, so cards,
    // "yes / do it" and the chat all behave as they do for typing.
    let visibility = crate::dbus_service::visibility(app);
    if mode == Mode::Dictate || visibility != "hidden" {
        crate::island::emit(app, "voice-text", Said { text: &text, mode: mode.name(), voice: true });
        return;
    }
    crate::log::line("voice: island hidden, answering headless");
    let app = app.clone();
    tauri::async_runtime::spawn(async move { headless(&app, text).await });
}

/// A command with no island on screen: route it, answer it, say it and notify.
async fn headless(app: &AppHandle, text: String) {
    use crate::assist::Reply;
    let say = |app: &AppHandle, body: String| {
        crate::log::line(format!("voice: answer {:?}", body.chars().take(80).collect::<String>()));
        // The notification is the long form, without markdown or code.
        let _ = crate::notify::send("Fedo", &speakable(&body, 600).0);
        if speak_setting(app) {
            speak(app, &body, true);
        }
    };
    match crate::assist::route(app, text.clone(), false, false, None).await {
        Ok(Reply::Done { text }) | Ok(Reply::Note { text }) => say(app, text),
        Ok(Reply::Chat) => {
            let chat = app.state::<crate::chat::Chat>();
            let settings = app.state::<crate::Shared>().settings.lock().unwrap().clone();
            match crate::chat::send(app, &chat, &settings, text, None).await {
                Ok(reply) => say(app, reply.text),
                Err(e) => say(app, format!("Sorry, that failed: {e}")),
            }
        }
        Ok(other) => {
            // A card needs the user's eyes and Enter: open the island on it.
            crate::island::request(app, "show", None);
            crate::island::emit(app, "voice-reply", other);
            say(app, "That needs your confirmation — it's on the card.".into());
        }
        Err(e) => say(app, format!("Sorry: {e}")),
    }
}

// ── Speaking ──────────────────────────────────────────────────────────────────

static SPEAKING: Mutex<Option<u32>> = Mutex::new(None);

pub fn stop_speaking() {
    if let Some(pid) = SPEAKING.lock().unwrap().take() {
        unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
    }
}

/// Says `text` with Piper. Long text is cut at a sentence; with `pointer` the
/// cut says where the rest is. Best effort and silent when Piper isn't set up.
pub fn speak(app: &AppHandle, text: &str, pointer: bool) {
    let (mut spoken, cut) = speakable(text, 280);
    if spoken.is_empty() {
        return;
    }
    if cut && pointer {
        spoken.push_str(" The rest is in the notification.");
    } else if cut {
        spoken.push_str(" The rest is on screen.");
    }
    if !status(&model_setting(app)).tts {
        return;
    }
    stop_speaking();
    let _ = std::thread::Builder::new().name("fedo-speak".into()).spawn(move || {
        let _ = say(&spoken);
    });
}

fn say(text: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let wav = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("fedo-say-{}.wav", std::process::id()));
    let voice = models_dir().join(format!("{VOICE_NAME}.onnx"));
    let mut piper = Command::new(venv_python())
        .args(["-m", "piper", "-m"])
        .arg(&voice)
        .arg("-f")
        .arg(&wav)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = piper.id();
    *SPEAKING.lock().unwrap() = Some(pid);
    if let Some(mut stdin) = piper.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let ok = piper.wait().map(|s| s.success()).unwrap_or(false);
    // Stopped (a new utterance began) while synthesising.
    if SPEAKING.lock().unwrap().take() != Some(pid) || !ok {
        let _ = std::fs::remove_file(&wav);
        return Ok(());
    }
    let play = Command::new("pw-play").arg(&wav).stdin(Stdio::null()).process_group(0).spawn();
    if let Ok(mut play) = play {
        *SPEAKING.lock().unwrap() = Some(play.id());
        let _ = play.wait();
        let mut slot = SPEAKING.lock().unwrap();
        if *slot == Some(play.id()) {
            *slot = None;
        }
    }
    let _ = std::fs::remove_file(&wav);
    Ok(())
}

// ── Setting up: models and the Piper/Vosk venv ────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    what: String,
    /// 0..1, or -1 when unknown.
    fraction: f64,
    done: bool,
    error: Option<String>,
}

fn progress(app: &AppHandle, what: &str, fraction: f64, done: bool, error: Option<String>) {
    let p = Progress { what: what.into(), fraction, done, error };
    let _ = app.emit("voice-download", p.clone());
    if crate::island::is_loaded(app) {
        let _ = app.emit_to(crate::island::WINDOW_LABEL, "voice-download", p);
    }
}

async fn download(app: &AppHandle, what: &str, url: &str, dest: &std::path::Path) -> Result<(), String> {
    if dest.is_file() {
        return Ok(());
    }
    if crate::spotify::PAUSED.load(Ordering::Relaxed) {
        return Err("Network is paused (tray → Pause).".into());
    }
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    let part = dest.with_extension("part");
    let client = reqwest::Client::builder().build().map_err(|e| e.to_string())?;
    let mut resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{what}: HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0) as f64;
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut got = 0f64;
    let mut last = Instant::now();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        got += chunk.len() as f64;
        if last.elapsed() > Duration::from_millis(250) {
            progress(app, what, if total > 0.0 { got / total } else { -1.0 }, false, None);
            last = Instant::now();
        }
    }
    drop(file);
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

/// The wake listener's script, shipped inside the binary.
const WAKE_PY: &str = include_str!("../../voice/wake.py");

pub async fn prepare(app: &AppHandle, what: &str, model: &str) -> Result<(), String> {
    let result = prepare_inner(app, what, model).await;
    match &result {
        Ok(()) => progress(app, what, 1.0, true, None),
        Err(e) => progress(app, what, -1.0, true, Some(e.clone())),
    }
    result
}

async fn prepare_inner(app: &AppHandle, what: &str, model: &str) -> Result<(), String> {
    let m = models_dir();
    match what {
        "stt" => {
            if stt_binary().is_none() {
                return Err("fedo-stt isn't built (run: cargo build --release -p fedo-stt).".into());
            }
            let size = if model == "small" { "small" } else { "base" };
            let url = format!("{WHISPER_URL}/ggml-{size}.bin");
            download(app, &format!("Speech model ({size})"), &url, &whisper_model_path(size)).await
        }
        "tts" => {
            ensure_venv(app).await?;
            for ext in ["onnx", "onnx.json"] {
                let name = format!("{VOICE_NAME}.{ext}");
                download(app, "Voice", &format!("{VOICE_BASE}/{name}"), &m.join(&name)).await?;
            }
            Ok(())
        }
        "wake" => {
            ensure_venv(app).await?;
            let dir = m.join(VOSK_MODEL);
            if dir.is_dir() {
                return Ok(());
            }
            let zip = m.join("vosk.zip");
            download(app, "Wake-word model", VOSK_URL, &zip).await?;
            let py = venv_python();
            let (zip2, m2) = (zip.clone(), m.clone());
            let ok = tauri::async_runtime::spawn_blocking(move || {
                Command::new(py)
                    .args(["-c", "import sys,zipfile;zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])"])
                    .arg(&zip2)
                    .arg(&m2)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            })
            .await
            .unwrap_or(false);
            let _ = std::fs::remove_file(&zip);
            if ok { Ok(()) } else { Err("Couldn't unpack the wake-word model.".into()) }
        }
        _ => Err(format!("unknown voice component {what}")),
    }
}

/// `~/.local/share/fedo/venv` with piper-tts and vosk: user space only.
async fn ensure_venv(app: &AppHandle) -> Result<(), String> {
    let py = venv_python();
    let probe = |py: PathBuf| {
        Command::new(py)
            .args(["-c", "import piper, vosk"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if py.is_file() && probe(py.clone()) {
        return Ok(());
    }
    progress(app, "Speech engine (Piper, Vosk)", -1.0, false, None);
    let dir = crate::platform::local_dir().join("venv");
    let ok = tauri::async_runtime::spawn_blocking(move || {
        let made = py_path_exists(&dir) || Command::new("python3").args(["-m", "venv"]).arg(&dir).status().map(|s| s.success()).unwrap_or(false);
        made && Command::new(dir.join("bin/pip"))
            .args(["install", "--quiet", "piper-tts", "vosk"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    if ok { Ok(()) } else { Err("Couldn't install Piper and Vosk with pip.".into()) }
}

fn py_path_exists(dir: &std::path::Path) -> bool {
    dir.join("bin/python").is_file()
}

// ── Wake word supervisor ──────────────────────────────────────────────────────

static WAKE_GEN: AtomicU32 = AtomicU32::new(0);
static WAKE_ON: AtomicBool = AtomicBool::new(false);
static WAKE_PID: Mutex<Option<u32>> = Mutex::new(None);

/// Starts or stops the "Hey Fedo" listener to match the setting. Off by
/// default; while off, no process exists.
pub fn sync_wake(app: &AppHandle, enabled: bool) {
    // Called after every settings save: act only on a change.
    if WAKE_ON.swap(enabled, Ordering::AcqRel) == enabled {
        return;
    }
    let generation = WAKE_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    if let Some(pid) = WAKE_PID.lock().unwrap().take() {
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
    if !enabled {
        return;
    }
    let model = model_setting(app);
    if !status(&model).wake {
        crate::log::line("voice: wake word is on but its engine isn't set up");
        return;
    }
    let script = wake_script();
    if std::fs::write(&script, WAKE_PY).is_err() {
        return;
    }
    let app = app.clone();
    let _ = std::thread::Builder::new().name("fedo-wake".into()).spawn(move || wake_loop(app, script, generation));
}

fn wake_loop(app: AppHandle, script: PathBuf, generation: u32) {
    let model_dir = models_dir().join(VOSK_MODEL);
    let mut cmd = Command::new(venv_python());
    die_with_parent(&mut cmd);
    cmd.arg(&script).arg("--model").arg(&model_dir);
    // Testing: listen to a 16 kHz mono wav instead of the microphone.
    if let Some(wav) = std::env::var_os("FEDO_WAKE_WAV") {
        cmd.arg("--wav").arg(wav);
    }
    let mut child = match cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            crate::log::line(format!("voice: wake listener failed to start: {e}"));
            return;
        }
    };
    *WAKE_PID.lock().unwrap() = Some(child.id());
    let Some(out) = child.stdout.take() else { return };
    for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
        if WAKE_GEN.load(Ordering::Acquire) != generation {
            break;
        }
        let Some(path) = line.strip_prefix("WAKE ") else { continue };
        let path = PathBuf::from(path.trim());
        let heard = std::fs::read(&path).ok();
        let _ = std::fs::remove_file(&path);
        // Not while Fedo is already listening or talking.
        if RECORDING.load(Ordering::Acquire) {
            continue;
        }
        let Some(bytes) = heard else { continue };
        let pcm: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let model = model_setting(&app);
        let heard = transcribe(&pcm, &model, "en");
        crate::log::line(format!("voice: wake check heard {:.1}s: {heard:?}", pcm.len() as f32 / SAMPLE_RATE as f32));
        match heard.map(|t| after_wake_word(&t)) {
            Ok(Some(command)) if command.is_empty() => start(&app, Mode::Command),
            Ok(Some(command)) => {
                stop_speaking();
                deliver(&app, Mode::Command, command);
            }
            Ok(None) => crate::log::line("voice: wake word check failed (false trigger)"),
            Err(e) => crate::log::line(format!("voice: wake check: {e}")),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn voice_status(app: AppHandle) -> Status {
    status(&model_setting(&app))
}

/// Mic button / push-to-talk: start listening, or finish now.
#[tauri::command]
pub fn voice_toggle(app: AppHandle, mode: String) {
    toggle(&app, Mode::parse(&mode));
}

#[tauri::command]
pub fn voice_cancel() {
    cancel();
}

#[tauri::command]
pub async fn voice_prepare(app: AppHandle, what: String) -> Result<(), String> {
    let model = model_setting(&app);
    prepare(&app, &what, &model).await
}

#[tauri::command]
pub fn voice_speak(app: AppHandle, text: String) {
    speak(&app, &text, false);
}

#[tauri::command]
pub fn voice_stop_speaking() {
    stop_speaking();
}

/// A typed-in confirmation check, for the island ("yes", "do it"…).
#[tauri::command]
pub fn voice_confirmation(text: String) -> Option<&'static str> {
    confirmation(&text).map(|a| match a {
        Answer::Yes => "yes",
        Answer::No => "no",
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(level: f32, n: usize) -> Vec<f32> {
        vec![level; n]
    }

    #[test]
    fn vad_starts_after_100ms_and_ends_after_the_quiet() {
        let mut v = VadState::new(1100);
        let mut events = Vec::new();
        for r in frames(0.003, 50).into_iter().chain(frames(0.1, 40)).chain(frames(0.003, 70)) {
            events.push(v.push(r));
        }
        assert_eq!(events.iter().filter(|e| **e == Vad::Started).count(), 1);
        assert_eq!(events.iter().filter(|e| **e == Vad::Ended).count(), 1);
        let started = events.iter().position(|e| *e == Vad::Started).unwrap();
        let ended = events.iter().position(|e| *e == Vad::Ended).unwrap();
        assert_eq!(started, 54, "five loud frames after 50 quiet ones");
        assert_eq!(ended, 90 + 54, "55 quiet frames after the speech ends at frame 90");
    }

    #[test]
    fn a_click_and_a_pause_inside_speech_do_not_end_or_start_it() {
        let mut v = VadState::new(1100);
        // A two-frame click is not speech.
        let click: Vec<_> = frames(0.003, 20).into_iter().chain(frames(0.2, 2)).chain(frames(0.003, 100)).collect();
        assert!(click.iter().all(|r| v.push(*r) == Vad::Quiet));
        // A 0.5 s pause between words does not end the utterance.
        let mut v = VadState::new(1100);
        let talk: Vec<_> = frames(0.1, 20).into_iter().chain(frames(0.003, 25)).chain(frames(0.1, 20)).collect();
        assert!(talk.iter().all(|r| v.push(*r) != Vad::Ended));
    }

    #[test]
    fn the_noise_floor_raises_the_bar() {
        let mut v = VadState::new(1100);
        for _ in 0..10 {
            v.calibrate(0.02);
        }
        // A fan at 0.02 RMS isn't speech, though it is above the 0.012 minimum…
        assert!(v.threshold() > 0.02);
        assert!((0..100).all(|_| v.push(0.02) == Vad::Quiet));
        // …but a voice over it still is.
        assert!((0..10).map(|_| v.push(0.15)).any(|e| e == Vad::Started));
    }

    #[test]
    fn rms_of_samples() {
        assert_eq!(rms(&[]), 0.0);
        assert!((rms(&[16384, -16384]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn yes_and_no_by_voice() {
        for y in ["Yes.", "yeah", "Do it", "run it!", "Go ahead", "ok", "Yes, please"] {
            assert_eq!(confirmation(y), Some(Answer::Yes), "{y}");
        }
        for n in ["No", "Cancel.", "never mind", "Stop", "no thanks"] {
            assert_eq!(confirmation(n), Some(Answer::No), "{n}");
        }
        for other in ["no, play it again", "yes I need that file copied", "what time is it", ""] {
            assert_eq!(confirmation(other), None, "{other}");
        }
    }

    #[test]
    fn the_wake_phrase_and_the_command_after_it() {
        let c = |s| after_wake_word(s);
        assert_eq!(c("Hey Fedo, open Spotify and play Earrings by Malcolm Todd."), Some("open Spotify and play Earrings by Malcolm Todd".into()));
        assert_eq!(c("Hey Fido."), Some(String::new()));
        assert_eq!(c("hey fedo what's the time"), Some("what's the time".into()));
        assert_eq!(c("Hey Federal, mute."), Some("mute".into()));
        assert_eq!(c("I said hey fedo earlier"), None);
        assert_eq!(c("Hey there"), None);
        assert_eq!(c("fedo"), None);
        assert_eq!(c(""), None);
    }

    #[test]
    fn replies_are_cleaned_and_cut_for_speech() {
        assert_eq!(speakable("**Diamonds** are most common at Y -58.", 280), ("Diamonds are most common at Y -58.".into(), false));
        assert_eq!(speakable("Run:\n```bash\nrm -rf x\n```\nThen done.", 280).0, "Run: Then done.");
        assert_eq!(speakable("See [the docs](https://x.y/z) now.", 280).0, "See the docs now.");
        let long = "One sentence here. ".repeat(40);
        let (spoken, cut) = speakable(&long, 100);
        assert!(cut && spoken.chars().count() <= 100 && spoken.ends_with('.'));
    }

    #[test]
    fn whisper_model_paths_only_take_known_sizes() {
        assert!(whisper_model_path("small").ends_with("ggml-small.bin"));
        assert!(whisper_model_path("../../etc/passwd").ends_with("ggml-base.bin"));
    }
}
