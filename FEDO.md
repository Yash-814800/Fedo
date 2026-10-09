# FEDO — build spec for Claude Code

> Put this file at the root of the forked coucou repo. It is the single source of truth for the build.
> Work through it **one phase at a time**. At the end of every phase: run the acceptance checks, commit, write a short report (what works, what doesn't, what you measured), and **stop and wait for me**.

---

## 0. What we are building

**Fedo** is a personal desktop companion for **my one machine**: Fedora Linux, **GNOME on Wayland**, terminal **Ptyxis** (GNOME Terminal as fallback). It is a fork of [Louis-CFM/coucou](https://github.com/Louis-CFM/coucou), keeping coucou's island UI/UX and Claude Code engine, removing everything that isn't Fedora/GNOME, and adding my own assistant features.

- A small **ghost icon lives in the GNOME top bar** next to my other extensions. Always there.
- **Click it → a floating "island" drops down** from the top-center of the screen (coucou's island look and motion). Click again / Esc / click outside → it tucks away.
- The island is **invisible when idle** and **peeks out by itself** when something needs me (approval, question, task done, timer, reminder, incoming call/notification).
- **All intelligence goes through Claude Code** (the `claude` CLI I'm already logged into). **No API keys anywhere.** No Anthropic/OpenAI/Gemini/Ollama providers.
- **Resource use must be minimal.** See §3 budgets — they are requirements, not wishes.
- **Nothing on my system is changed without my confirmation.** Fedo writes the command, shows it, I press Enter (or say "do it").

### Who I am / my setup (detect and confirm, don't assume)
- Fedora (check `cat /etc/fedora-release`), GNOME (check `gnome-shell --version`), Wayland session.
- Claude Code installed and logged in on a Pro/Max plan.
- KDE Connect installed and paired with my Android phone (check whether it's `kdeconnect` or GSConnect; I believe it's KDE Connect — verify via D-Bus).
- ZapZap (WhatsApp client) — probably Flatpak `com.rtosta.zapzap`; verify.
- Spotify desktop — RPM or Flatpak; verify.
- VS Code with **Antigravity** — verify what is actually installed (Google Antigravity is its own IDE and has an `agy` CLI; it may also be a VS Code extension on my machine). Report what you find.
- Email = **Gmail in the browser**.

---

## 1. Ground rules for you (Claude Code)

1. **Verify before you use.** Before calling any external CLI, D-Bus interface or Claude Code flag, check it on this machine (`--help`, `man`, `busctl --user introspect`, `gdbus introspect`, `claude --help`). Never guess flags. If a doc says one thing and the machine says another, the machine wins.
2. **Be honest about Wayland limits.** GNOME Wayland blocks apps from focusing other windows, injecting keystrokes and drawing over fullscreen apps. When a feature hits such a wall, **don't fake it** — use the approaches in this spec (GNOME Shell extension, URIs, D-Bus, clipboard, notifications + voice) or tell me clearly what's not possible and propose the closest alternative.
3. **I'm the one who runs `sudo`.** If you need packages installed, give me the exact `sudo dnf install …` line and wait.
4. **Small commits, one per logical step.** Build and run after every meaningful change. Don't leave the app broken at the end of a phase.
5. **Licensing (mandatory).** coucou's **code is MIT** — keep the original `LICENSE` with Louis Raillé's copyright notice and add mine. coucou's **name, Mochi character, icon, sounds and media are all rights reserved** (`LICENSE-ASSETS.md`). Remove every one of them. Fedo gets its own name, its own ghost character (drawn in code) and its own sounds. Replace `LICENSE-ASSETS.md` with one covering Fedo's assets.
6. **Read coucou's own `CLAUDE.md`, `windows/README.md` and `docs/AGENTS.md` first** — they describe the existing hook protocol, socket, and Linux specifics. Reuse, don't reinvent.
7. **Keep a running `docs/FEDO-NOTES.md`**: decisions taken, things verified on this machine, known limitations. Rewrite the repo's `CLAUDE.md` for Fedo at the end of Phase 0 so future sessions have context.

---

## 2. Architecture (target)

coucou's Linux build is the **Tauri 2 app in `windows/`** (Rust + TypeScript, Canvas 2D character, Claude Code hooks via `coucou-hook` over a Unix socket in `$XDG_RUNTIME_DIR`). We keep that and split responsibilities so that almost nothing heavy runs while idle:

```
┌───────────────────────────── GNOME Shell ─────────────────────────────┐
│  fedo@<me> extension (GJS, ESM)                                        │
│   • ghost icon in the top bar (state dot: idle / working / needs you)  │
│   • click → D-Bus Toggle() on the daemon                               │
│   • pins the island window: top-center, above all, all workspaces,     │
│     skip taskbar / alt-tab, no focus steal                             │
│   • "jump to terminal": activates the Ptyxis window of a session      │
│   • optional push-to-talk keybinding                                   │
└───────────────┬───────────────────────────────────────────────────────┘
                │ session D-Bus: org.fedo.Fedo
┌───────────────▼───────────────────────────────────────────────────────┐
│  fedo (Tauri 2 / Rust core) — always running, tiny when idle          │
│   • hook socket (Claude Code + Antigravity)  • approvals / answers    │
│   • scheduler: timers, reminders, deadlines  • usage + weekly recap   │
│   • claude -p runner (chat, intents, prompt writer)                   │
│   • KDE Connect / MPRIS (Spotify) / EDS calendar via D-Bus            │
│   • voice pipeline (only when activated)                              │
│   • island WebView: created on demand, destroyed after idle           │
└───────────────────────────────────────────────────────────────────────┘
```

Rename the `windows/` folder to `app/`. Keep `coucou-hook` but rename it `fedo-hook`.

**Island window on GNOME:** GNOME has no layer-shell. Preferred approach: run the Tauri window natively and let the **extension** find it (by WM_CLASS / app id `fedo-island`) and place it (Meta API: position, `make_above`, `stick`, skip taskbar). Fallback: coucou's existing XWayland dock-window path. Pick whichever behaves better, document why.

---

## 3. Resource budgets (measure and report every phase)

| State | RAM (RSS, all fedo processes) | CPU |
|---|---|---|
| Idle, island hidden, no voice | **≤ 40 MB** | **~0 %** (no busy polling) |
| Island open | ≤ 200 MB | < 3 % while animating |
| Wake word enabled | + ≤ 80 MB | ≤ 3 % of one core |
| Whisper transcribing | spikes allowed | returns to idle after |

How:
- **Event-driven only.** D-Bus signals, hook socket, inotify, timers. No polling loops except usage refresh (≥ 60 s, only while island open or a pill needs it).
- **WebView lifecycle:** don't create it at startup. Create on first show; **destroy it after 60 s hidden** (configurable). Animations stop when hidden (no `requestAnimationFrame` while invisible).
- Strip unused crates/npm deps after removing features. Release builds with LTO.
- Add `fedo stats` CLI subcommand that prints RSS/CPU of all fedo processes. Put the numbers in each phase report.

---

## 4. Phases

### Phase 0 — Fork hygiene, rename, strip, build

- Delete: `NotchBuddy/` (macOS + iOS), `relay/` (APNs), `linux/arch/`, Windows-only code paths and packaging, iPhone/macOS/Windows docs and media, the demo GIF/screenshots.
- Delete from the Tauri app: all chat providers that need API keys (Anthropic API, Gemini, OpenAI, Ollama, LM Studio), keyring/Secret Service key storage, service integrations (Stripe, n8n, GitHub, Vercel, Resend, Notion, Cal.com), Apple Music, wardrobe/seasonal outfits, all agents **except Claude Code and Antigravity**, localization except English (keep the i18n mechanism so Hindi can be added later).
- Remove Mochi drawing code, all sounds, icons, the Coucou name everywhere (code, window titles, app id, binary names, socket name, settings paths). New identifiers: app name `Fedo`, binary `fedo`, app id `org.fedo.Fedo`, config `~/.config/fedo/`, data `~/.local/share/fedo/`, socket `$XDG_RUNTIME_DIR/fedo.sock`.
- Temporary placeholder character: a plain circle with two eyes, so the app keeps working until Phase 2.
- Give me the Fedora package list needed to build (verify names with `dnf info`; likely `webkit2gtk4.1-devel gtk3-devel librsvg2-devel openssl-devel dbus-devel gtk-layer-shell-devel` plus GStreamer plugins, Rust, Node 20+).
- **Accept:** `npm run tauri dev` (or equivalent) launches Fedo on my GNOME session; island shows; Claude Code hooks still work for a test session; `grep -ri "coucou\|mochi" .` returns only the LICENSE attribution and CHANGELOG history note. New `CLAUDE.md` written.

### Phase 1 — GNOME top-bar extension + island behavior

- Write the extension in `gnome-extension/` (ESM format for GNOME 45+; set `shell-version` to the version on this machine). Install script symlinks it into `~/.local/share/gnome-shell/extensions/`.
- Note for development: on Wayland the shell can't be restarted in place. Use a nested/devkit shell for testing (`dbus-run-session gnome-shell --devkit` on GNOME 49+, `--nested --wayland` on older) and tell me when I need to log out/in to load the real one.
- Panel button: ghost icon (symbolic SVG so it matches other top-bar icons), small status dot. Left-click toggles island. Right-click menu: Settings, Mute sounds, Pause voice, Quit.
- Daemon exposes `org.fedo.Fedo` on the session bus: `Toggle()`, `Show(view)`, `Hide()`, `State` property + `StateChanged` signal, `FocusSession(id)`.
- If the daemon isn't running, clicking the icon starts it (systemd user service — see Phase 10; for now `fedo` binary on PATH).
- Island: top-center, coucou's spring open/close animation, closes on Esc / outside click, never steals focus when it auto-peeks for an alert.
- **Invisible when idle:** island fully hidden when nothing is pending; auto-peeks (compact pill) for alerts; hovering the top edge **under the icon** opens it.
- **Accept:** icon visible in top bar beside my other extensions; click opens/closes in < 150 ms after first open; idle RSS within budget; island stays above normal windows on all workspaces.

### Phase 2 — The ghost (character) and sounds

- **Ghost character**, original design, drawn in Canvas 2D (no image files): soft rounded ghost body with a wavy hem, big expressive eyes that follow the cursor, gentle float/bob, translucent edges. Ghost-native behavior: **fades in** when appearing, **fades to near-transparent** when idle, wisps away on close.
- States/emotes (port coucou's state machine and timings, new art): idle float, blink, looking at cursor, working (spinning wisp), needs-you (bounce + glow), happy (task done: little loop-de-loop), sleepy (late night/long idle), poked (squish), dizzy (3 fast clicks), listening (ears/aura pulse while mic is open), thinking (while waiting on claude), dancing (Spotify playing).
- **Sounds:** original, short, soft. Generate them yourself — either synthesize with the Web Audio API at runtime or render WAVs with a small script in `scripts/make-sounds.*` (sine/triangle blips, chimes, whooshes). Needed: open, close, done, needs-approval, question, error, timer-end, reminder, poke, dizzy, listening-start, listening-stop, message-received. Play through preloaded buffers. User override: files dropped in `~/.config/fedo/sounds/<name>.wav` replace defaults. Global mute + volume in settings.
- **Accept:** every state reachable from a hidden debug menu; sounds play with no audible lag; nothing from Mochi remains.

### Phase 3 — Claude Code core (the coucou part, made solid on Linux)

- **Hooks:** keep coucou's installer behavior (backup `~/.claude/settings.json`, merge, show diff, write). If Fedo isn't running, the hook exits instantly — **Claude Code is never blocked.**
- **Live sessions:** per-session pill with project name, current step (reading/editing/running), file +N −M, tap for diff.
- **Approve & answer:** permission requests show **Allow / Deny / Always**; `AskUserQuestion` choices (single/multi, up to 4) answerable in the island; "Reply in terminal" fallback.
- **Tell me when done:** when a session stops/finishes → happy ghost + done sound + GNOME notification (only if the island isn't already showing it). Configurable "only notify if the task took > N seconds".
- **Jump to terminal:** from the hook payload's PID, walk up the process tree to the owning terminal window. The **GNOME extension** activates that window (it can do what Wayland apps can't). For Ptyxis, also try to select the right **tab** (investigate: tab titles, Ptyxis D-Bus/GActions); window-level focus is required, tab-level is best-effort — report what works.
- **Usage pill:** show 5-hour and weekly plan usage **without API keys**. Investigate in this order and report what actually works on this machine: (a) how coucou's Windows/Linux build reads plan usage; (b) data Claude Code passes to a `statusLine` command (recent versions include rate-limit info — verify with a test statusline script that dumps its stdin); (c) fallback: parse local transcripts in `~/.claude/projects/**/*.jsonl` for tokens/messages per window (like `ccusage`). Show percentage + reset time; color shifts as limits approach.
- **Weekly recap:** port coucou's recap (time coding, sessions, files, lines changed, commands, approvals, top project, busiest day, longest session). All local in `~/.local/share/fedo/`. Shows Monday morning on first unlock; also on demand. "Save as image" keeps working with the ghost instead of Mochi.
- **Chat routed to Claude Code:** the chat box runs `claude -p` (verify flags: `--output-format stream-json`, `--resume`/`--continue`, `--model`, `--append-system-prompt`, `--allowedTools`). Stream the reply into the island. Keep a Fedo chat session id so follow-ups have context; "New chat" button. Working dir: `~`.
- **Drop a file:** ghost turns into a little sack and swallows it; the file path becomes context for the next chat message (Claude Code can read it). Actions on a dropped file: Ask about it · Send via Gmail · Send via ZapZap · Send to phone (KDE Connect) — the last three come alive in Phase 6.
- **Accept:** run a real Claude Code task in Ptyxis that needs a permission — approve it from the island; get a done notification; jump back to the exact terminal; usage pill shows plausible numbers; chat answers a question; dropped PDF can be asked about.

### Phase 4 — "Make my life easier": command assistant + prompt writer

**Intent router.** Every message (typed or spoken) goes through a router:
1. **Local fast path** (regex/keywords, zero latency, zero usage) for obvious ones: "timer 10 min", "pause/next song", "read my notifications", "mute".
2. Otherwise `claude -p` with a small fast model (e.g. `--model haiku`; verify the alias) and a strict system prompt that returns **JSON only** (use `--json-schema` if this version supports it; else validate and retry once). Schema: `{ intent, args, command?, explanation, risk: "safe"|"careful"|"dangerous" }`.

**Command card (the core UX).** For system actions, Fedo shows a card: the exact command, a one-line explanation, risk color. **Enter = run**, **Esc = cancel**, **E = edit**, or voice "do it" / "cancel". Never auto-run.
- Commands run in a **new Ptyxis window** (`ptyxis --new-window -- bash -lc '<cmd>; echo; read -p "Done — press Enter to close"'` — verify flags) so `sudo` prompts and output are visible. Option to run inline (hidden) for safe, non-sudo commands, with output shown in the island.
- **Install a program:** "install obs" → Fedo checks availability first (`dnf info`/`dnf search`, then `flatpak search` on Flathub) and proposes the best option: `sudo dnf install obs-studio` or `flatpak install flathub com.obsproject.Studio`. If the name is ambiguous, show 2–3 choices.
- **Kill a process:** "kill vlc completely" → show matching processes (name, PID, memory) and the command (`pkill -f vlc`, escalate to `pkill -9` only if needed, Flatpak-aware: `flatpak kill org.videolan.VLC`). Never propose killing gnome-shell, systemd, Xwayland, pipewire, or fedo itself without a red warning.
- **File tasks:** "go to work/resumes, copy the ds resume into tempres and rename it to X.pdf" → resolve fuzzy names against the real filesystem first (`find`/glob relative to `~`), show **a plan with real paths** (`cp ~/work/resumes/DS_Resume_2026.pdf ~/tempres/X.pdf`), create missing folders only if I confirm. If multiple files match "ds resume", ask me to pick.
- **Prompt writer:** a dedicated mode (button + "rewrite this as a prompt"). I paste my raw reaction ("no what you're doing is wrong, the button should…"). Fedo uses `claude -p` with a prompt-engineering system prompt to produce a clear, specific instruction for Claude Code: what's wrong, expected behavior, constraints, acceptance criteria — concise, no fluff. Show it with **Copy** (via `wl-copy`) and, if a Claude Code session is waiting for input, **Send to session** only if a reliable mechanism exists (investigate; otherwise copy + jump to terminal).
- **Accept:** "install obs", "kill vlc", the resume copy task and a prompt rewrite all work end-to-end with confirmation; nothing executes without Enter/voice confirm.

### Phase 5 — Timers, reminders, calendar, deadlines

- **Timers:** "timer 25 min", "remind me in 10 minutes to check the oven". Countdown pill in island, ghost wakes and plays timer sound at the end + GNOME notification. Pause/cancel. Survive Fedo restart (persist to disk).
- **Calendar:** GNOME Calendar uses **Evolution Data Server**. Create events in my local calendar through EDS (e.g. a small Python helper using `gi` ECal/EDataServer bindings, or the D-Bus API — verify what's installed). "Meeting with Riya Friday 4pm" → card showing parsed title/date/time → Enter to create → appears in GNOME Calendar.
- **Reminders for events:** add a VALARM to created events **and** have Fedo's scheduler show its own reminder (ghost + sound) so it works even if Evolution's alarm notifier isn't running. Read today's events on request ("what's on today?").
- **Deadlines:** "I have to apply to Zomato before Oct 20, remind me" → all-day event on the deadline + reminders 3 days before, 1 day before, and the morning of (configurable). Deadlines list view in the island with countdowns.
- **Accept:** events I create show up in GNOME Calendar; reminders fire on time with Fedo restarted in between.

### Phase 6 — Phone & messages (KDE Connect, ZapZap, Gmail)

Use the KDE Connect **D-Bus API** (`org.kde.kdeconnect`) — prefer it over parsing `kdeconnect-cli` output; check `kdeconnect-cli --help` for capabilities. If it turns out to be GSConnect, adapt (`org.gnome.Shell.Extensions.GSConnect`).

- **Contacts:** KDE Connect's contacts plugin syncs vCards (look under `~/.local/share/kpeoplevcard/`). Build a local name → number index; fuzzy match ("pulkit" → Pulkit Sharma); ask when ambiguous.
- **Call:** "call Pulkit" → confirm card → trigger the call through KDE Connect. Investigate the cleanest path (e.g. sharing a `tel:` URI to the phone opens the dialer). If Android still requires one tap on the phone, say so in the card.
- **Read notifications:** list active phone notifications (app, title, text) in the island; read them aloud with TTS when asked by voice (Phase 8). New phone notifications can peek the island (filterable by app; off by default except calls/messages).
- **SMS:** "text Aparna I'll be right there via SMS" → KDE Connect send-SMS, after confirmation.
- **ZapZap (WhatsApp):** "open zapzap and text Aparna I'll be right there" → resolve the number → open the chat with the message **prefilled** (investigate: does ZapZap handle `whatsapp://send?phone=…&text=…` or `https://wa.me/…?text=…`? check `xdg-mime query default x-scheme-handler/whatsapp`). I press Enter in ZapZap to send. "Send it" by voice: only if a reliable method exists (e.g. `ydotool` with `ydotoold` — requires uinput setup; make it opt-in and tell me the setup steps). Don't pretend it sent if it didn't.
- **Send a dropped file via ZapZap:** open the chat and put the file on the clipboard as a file (`wl-copy` with `text/uri-list`), tell me to paste. Investigate whether ZapZap accepts a file via drag/portal more directly.
- **Gmail:** open `https://mail.google.com/mail/?view=cm&to=…&su=…&body=…` in my default browser. Browsers don't allow URL attachments, so for a dropped file: open compose + open the file's folder in Nautilus with the file selected (`nautilus --select`) and copy it to clipboard, so attaching is one drag/paste. Draft the subject/body with claude if I ask ("mail this resume to hr@x.com, say I'm applying for the DS role").
- **Send to phone:** dropped file → KDE Connect share to my phone.
- **Accept:** call, read notifications, SMS, ZapZap prefill, Gmail compose with file ready, send-to-phone all work with confirmation.

### Phase 7 — Spotify pill + "play X"

- Spotify pill via **MPRIS** (`org.mpris.MediaPlayer2.spotify`): album art, title, artist, draggable progress, play/pause, prev/next, shuffle, repeat, volume. Port coucou's Linux 0.3.0 Spotify pill; the ghost dances while music plays. Event-driven (PropertiesChanged), no polling except progress tick while island is open.
- **"Open Spotify and play Earrings by Malcolm Todd"** with no API keys: launch Spotify if not running, find the track's Spotify ID by having `claude -p` run a web search for the `open.spotify.com/track/…` link (verify the WebSearch tool is allowed in headless mode via `--allowedTools`), then `OpenUri("spotify:track:<id>")` over MPRIS. Fallback: `spotify:search:<query>` and tell me to press play.
- **Accept:** pill works; the Earrings example plays the right song.

### Phase 8 — Voice

All speech runs **locally**. Nothing audio-related runs until voice is used.

- **Dictate in chat:** mic button in the chat box → record (PipeWire via `cpal` or GStreamer) with voice-activity detection → **whisper.cpp** (via `whisper-rs` or the `whisper-cli` binary) → text in the box. Default model `base` (multilingual; I may speak Hinglish), configurable to `small`. Download models on first use into `~/.local/share/fedo/models/` with progress shown.
- **Voice commands & confirmations:** spoken text goes through the same intent router; command cards accept "yes / do it / run it / cancel / no" by voice.
- **Spoken replies (TTS):** **Piper** with a natural English voice; short answers spoken, long ones shown. Toggle in settings.
- **Push-to-talk shortcut:** global key (via the GNOME extension keybinding) works even while a fullscreen game has focus. **Minecraft case:** I press the key, ask "what Y level are diamonds most common at?", Fedo answers **by voice** + a GNOME notification. (Wayland won't let the island draw over a fullscreen game — voice + notification is the design.)
- **Wake word "Hey Fedo"** (opt-in, off by default): lightweight always-on detector. Prefer **openWakeWord** with a custom "hey fedo" model or **Vosk** small model in grammar-restricted mode — pick the lighter one that meets the CPU budget, measure it, report false-trigger behavior. After the wake word: listening state, record command, route it. Example to pass: "Hey Fedo, open Spotify and play Earrings by Malcolm Todd."
- **Accept:** dictation accuracy fine for normal speech; push-to-talk works over a fullscreen app; wake word within the §3 budget.

### Phase 9 — Antigravity

- First report what's installed (`which agy antigravity`, `~/.gemini/`, VS Code extensions list `code --list-extensions`).
- **Done detection:** port coucou's Antigravity (`agy`) hook support to Linux (it uses `~/.gemini/config/hooks.json` on macOS — verify the Linux path). Antigravity sessions get their own pill; done → ghost + sound + notification, same as Claude Code. If no hook mechanism exists for my setup, find the most reliable signal available (log file watching via inotify, etc.) and explain the trade-off.
- **Talk to it from the island:** if `agy` has a non-interactive prompt mode, route "ask antigravity …" to it. Otherwise: copy the message to the clipboard and focus the Antigravity/VS Code window via the extension. Say clearly which one you achieved.
- **Accept:** finishing an Antigravity response triggers Fedo; sending a message from the island works by the best available method.

### Phase 10 — Packaging, autostart, polish

- `systemd --user` service `fedo.service` (starts on login, restarts on crash), extension enabled, hooks installed — all via one `scripts/install.sh` (prints the `sudo` lines for me to run) and a matching `uninstall.sh` that restores backups.
- Build an RPM (Tauri bundler) as an alternative.
- Settings window: sounds, mute, auto-hide delay, WebView destroy delay, notification filters, voice (model, TTS on/off, wake word on/off, push-to-talk key), usage pill on/off, reminder offsets, default phone device.
- Keyboard shortcuts: open island, open chat, approve latest, jump to latest session, mute.
- README rewritten for Fedo (Fedora/GNOME only), with credit: "Forked from coucou by Louis Raillé (MIT)".
- Final report: features table (✅ works / ⚠️ works with caveat / ❌ not possible + why), resource measurements, known issues.

---

## 5. Safety rules inside Fedo (enforce in code)

- Every system-changing action (shell command, file op, kill, install, call, message, email, calendar write) shows a confirmation card first. No exceptions, including voice.
- Red "dangerous" styling + a second confirmation for: `rm -r`, anything touching `/etc`, `/usr`, `/boot`, `dnf remove`, `kill -9` of system processes, `chmod/chown -R`, `dd`, `mkfs`.
- `claude -p` calls used for intent parsing run **with no tools allowed** (it only returns JSON); Fedo executes, never Claude headlessly. Chat mode may allow read-only tools (Read, WebSearch).
- Log every executed action to `~/.local/share/fedo/actions.log` (timestamp, command, exit code).

---

## 6. Definition of done

I can live with Fedo all day: the ghost sits in my top bar, stays out of the way, tells me when Claude Code or Antigravity needs me or is done, lets me approve from the island, jump to the right terminal, see my usage and weekly recap, chat and drop files, run "install/kill/copy" tasks with one Enter, rewrite my angry feedback into good prompts, run timers/reminders/calendar/deadlines, call and text people through my phone, control Spotify, and answer my questions by voice while I'm in a game — all while using about as much memory as a terminal tab when idle.
