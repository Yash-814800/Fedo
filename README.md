# Fedo

**A ghost in your GNOME top bar — for Claude Code, Antigravity, your phone and your day.**

Fedo is a personal desktop companion for **Fedora Linux on GNOME / Wayland**,
built for one machine. The ghost sits in the top bar; click it (or let it peek
on its own) and a small island drops down:

- shows your **Claude Code** and **Antigravity** sessions — working, done, or waiting for you;
- lets you **approve permissions, answer questions** and jump to the right terminal;
- **chat** (through the `claude` CLI you're logged into — no API keys), drop files, rewrite an angry note into a good prompt;
- runs **timers, reminders, calendar events and deadlines**;
- **calls and texts through your phone** (KDE Connect), opens WhatsApp / Gmail drafts;
- controls **Spotify** ("open Spotify and play Earrings by Malcolm Todd");
- listens: **dictation, voice commands, spoken answers, push-to-talk, "Hey Fedo"** — all local.

Nothing on your machine changes without your confirmation: Fedo writes the
command, shows it, and you press Enter.

![Fedora](https://img.shields.io/badge/Fedora-Linux-294172?logo=fedora&logoColor=white)
![GNOME](https://img.shields.io/badge/GNOME-Wayland-4A86CF?logo=gnome&logoColor=white)
![Tauri 2](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=black)
![Code: MIT](https://img.shields.io/badge/code-MIT-green)

## Install

```sh
git clone https://github.com/Yash-814800/fedo && cd fedo
scripts/install.sh            # build, install, systemd --user service, GNOME extension, hooks
```

The script runs no `sudo`: if a package is missing it prints the exact
`sudo dnf install …` line. It installs into your home directory:

| What | Where |
|---|---|
| `fedo`, `fedo-hook` | `~/.local/bin/` |
| `fedo-stt` (speech-to-text helper) | `~/.local/share/fedo/bin/` |
| `fedo.service` (starts at login, restarts on a crash) | `~/.config/systemd/user/` |
| desktop entry + icon (the shortcuts portal needs the app id) | `~/.local/share/applications/`, `…/icons/` |
| GNOME extension `fedo@yash` | `~/.local/share/gnome-shell/extensions/` (symlink) |
| hooks | `~/.claude/settings.json`, `~/.gemini/config/hooks.json` — **diff shown, dated backup, written only after you type y** |

Log out and in once so GNOME loads the extension. `scripts/install.sh --dry-run`
prints every step without doing it. For an RPM instead: `scripts/install.sh --rpm`
(then `sudo dnf install` the file it names; the extension and hooks are still
`gnome-extension/install.sh --enable` and `fedo hooks install`).

`scripts/uninstall.sh` removes all of it and takes Fedo's entries out of the
hook files (with a diff and a backup; your previous status line is put back).
`--purge` also deletes settings, models and logs.

### Voice (optional, fetched on first use)

Nothing audio-related runs until you use voice. The first use downloads, into
`~/.local/share/fedo/`: the whisper model (`base`, ~140 MB), the Piper voice
(~60 MB) and a small Python venv with `piper-tts` and `vosk`. Settings → Voice
picks the model (`small` is better for Hinglish), turns spoken replies and the
wake word on or off.

## Keys

The GNOME extension binds these (they work over a fullscreen app); change them
in Settings or see [`gnome-extension/README.md`](gnome-extension/README.md).

| Key | Does |
|---|---|
| `Super+Alt+V` | push-to-talk: speak a command, press again to finish |
| `Super+Alt+F` | open / close the island |
| `Super+Alt+C` | open the chat |
| `Super+Alt+Y` | approve the permission card that is on screen (brings it up if it isn't) |
| `Super+Alt+J` | jump to the latest session's window |
| `Super+Alt+M` | mute / unmute |

## What works and what doesn't

| | |
|---|---|
| Ghost in the top bar, island on demand, destroyed when idle | ✅ |
| Claude Code approvals, questions, done, usage pill, weekly recap | ✅ |
| Antigravity working/done detection (hooks) | ✅ |
| Approving an Antigravity permission from the island | ❌ its hooks can't answer |
| "Ask Antigravity …" (always behind a confirm card — `agy -p` edits files unprompted) | ✅ |
| Chat, file drop, prompt writer, command assistant (install/kill/files with one Enter) | ✅ |
| Timers, reminders, EDS calendar, deadlines | ✅ |
| Calls / SMS / files via KDE Connect; WhatsApp (ZapZap) and Gmail drafts | ⚠️ phone must be reachable; WhatsApp/Gmail need your Enter there |
| Spotify pill, "play X" (verified against what Spotify actually started) | ✅ |
| Voice: dictation, commands, spoken answers (Piper) | ✅ |
| Push-to-talk over a fullscreen game | ⚠️ the key is a shell keybinding; not tested over a real game |
| "Hey Fedo" wake word | ⚠️ opt-in; ~180 MB while on, ~1.3 % CPU |
| Local Ollama model instead of Claude (Settings → AI) | ⚠️ small models route and plan worse |
| Drawing the island over a fullscreen game | ❌ Wayland forbids it — the answer is spoken and sent as a notification |
| Resource budget: idle ≤ 40 MB RSS | ⚠️ 66 MB RSS / 30 MB PSS, 0 % CPU (GTK+WebKit libraries count in RSS) |
| Resource budget: island open ≤ 200 MB | ❌ ~500 MB RSS (WebKitGTK) |

Details, measurements and every known limitation are in
[`docs/FEDO-NOTES.md`](docs/FEDO-NOTES.md); the build plan is
[`FEDO.md`](FEDO.md).

## Safety

- No API keys anywhere; no telemetry; network only for what you asked for
  (model downloads on first use, a web search for "play X").
- Every system-changing action shows a card first — voice included — and is
  appended to `~/.local/share/fedo/actions.log`.
- Fedo never overwrites an agent's config: dated backup, diff, then your y.

## Build from source

See [`app/README.md`](app/README.md). Tests: `cd app && npm test && cargo test --workspace`.

## Credits and licence

Forked from **[coucou](https://github.com/Louis-CFM/coucou) by Louis Raillé**
(MIT), keeping its island UI/UX and Claude Code hook relay. Fedo's code is MIT
(see [`LICENSE`](LICENSE)); the ghost, sounds and other assets are Fedo's own
and are covered by [`LICENSE-ASSETS.md`](LICENSE-ASSETS.md). coucou's name,
character and media are not used.
