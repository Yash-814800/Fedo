# Fedo — guide for AI coding agents

Fedo is a personal desktop companion for Fedora Linux, GNOME on Wayland,
one machine only. A ghost character living in the GNOME top bar shows
Claude Code and Antigravity sessions and the Spotify pill, and lets the
user approve, answer and chat from the island. It's a fork of
[coucou](https://github.com/Louis-CFM/coucou), keeping its island UI/UX
and Claude Code hook relay; see [`FEDO.md`](FEDO.md) for the full build
plan (phases, architecture, resource budgets) and
[`docs/FEDO-NOTES.md`](docs/FEDO-NOTES.md) for decisions and known
limitations recorded as each phase lands.

## Where things are

- `app/` — the Tauri 2 app: Rust in `app/src-tauri/`, TypeScript in
  `app/src/`, the `fedo-hook` relay in `app/hook/`. `app/README.md` lists
  what differs from the coucou fork it came from.
- `gnome-extension/` — the top-bar ghost (GJS, ESM, GNOME 50). It owns the
  panel button, the state dot, outside-click/Esc closing and the island
  window's placement under the top bar; everything else is the daemon's.
  `install.sh` symlinks it into `~/.local/share/gnome-shell/extensions/`.
- `app/src-tauri/src/dbus_service.rs` — `org.fedo.Fedo` on the session bus
  (`Toggle`/`Show`/`Hide`/`FocusSession`, `State`/`Visibility`/`Muted`).
  `island.rs` owns the island webview's lifecycle: built on demand, destroyed
  after `webviewIdleSeconds` hidden and idle, events queued while it loads.
  Anything that must reach the island goes through `island::emit`, not
  `emit_to` — `emit_to` silently drops events when the webview is torn down.
- `app/src/core/pills.ts` — single source of truth for every pill
  (workspace tools, agents, AI providers, services). Every pill ID,
  color, category and connection method lives here; IDs are stable
  contract values and are never renamed, even for a pill this build
  doesn't offer (marked `support: "no"` instead of deleted).
- `app/src/ghost/engine.ts` — the ghost: state machine, tweens, particles
  and its Canvas 2D body (wavy hem, translucent edges, float, fade in / idle
  fade / wisp away, loop-de-loop, listening aura, working wisp).
  `app/src/ghost/debug-menu.ts` is the hidden debug menu (Shift+click the
  ghost in the open island, or Ctrl+Alt+Shift+D): every state, emote, move
  and sound.
- `app/src-tauri/src/assist.rs` — the command assistant: router (local fast
  path, then `claude -p --json-schema`), grounded resolvers (dnf/flatpak,
  `ps`, filesystem plans), Fedo's own risk rules, and the terminal/inline
  runners. `app/src/views/command.ts` is the card. **Nothing runs without the
  card's Enter / Run**; model-written command text for file tasks is never run
  (Fedo builds it from a validated plan).
- `app/src-tauri/src/scheduler.rs` (timers/reminders/deadlines, persisted,
  event-driven), `calendar.rs` (+ `helpers/fedo-calendar.py`, Evolution Data
  Server) and `timeplan.rs` (time phrases, reminder plans). Writing to the
  calendar always goes through the confirm card.
- `app/src-tauri/src/contacts.rs`, `phone.rs` (KDE Connect D-Bus + CLI),
  `phoneplan.rs` (sentences) and `actions.rs` (call / SMS / WhatsApp / Gmail /
  send-to-phone cards and their executors). Nothing reaches a person or the
  phone without the card's Enter; `do_action` re-validates every parameter.
- `app/src-tauri/src/spotify.rs` — the Spotify pill (MPRIS, event-driven) and
  `play_uri`/`confirm_title` for "play X"; `assist.rs::play_song` does the
  `claude -p` WebSearch lookup. Only ids matching a 22-char track id are ever
  turned into a URI, and the playing title is checked before Fedo says so.
- `app/src-tauri/src/chat.rs` — the chat: one `claude -p` run per turn
  (stream-json, `--resume` for follow-ups, read-only tools). `notify.rs` is
  the desktop notification; `unlock.rs` the Monday-unlock recap trigger.
  The extension's `org.fedo.Shell.FocusWindow` is how "open terminal" raises a
  window on GNOME (`platform/linux_focus.rs`).
- `app/scripts/make-sounds.mjs` — renders `app/sounds/*.wav` (Fedo's own,
  synthesised; rerun it after editing a sound). Users override any of them
  with `~/.config/fedo/sounds/<name>.wav`.
- `docs/AGENTS.md` — the hook relay's third-party agent protocol (still
  accurate and live, even though only Claude Code and Antigravity get a
  one-click installer in this build).
- `docs/FEDO-NOTES.md` — the running decisions/verification/limitations
  log; read it before assuming something works.

## Build

```
cd app && npm install && npm run tauri dev
```

Ship / test the real artefact with `npx tauri build --no-bundle` (binary) or
`npx tauri build --bundles rpm`. A bare `cargo build` binary loads the dev
server URL and shows nothing without `npm run dev`. `npm test` (295 tests) and
`cargo test --workspace` must stay green.

Needs Rust, Node 20+, and the Fedora packages listed in
[`docs/FEDO-NOTES.md`](docs/FEDO-NOTES.md) (verified against `dnf info`,
not guessed). `xcodegen`/Xcode/Swift do not apply here — there is no
macOS app in this fork.

## Rules

- Rust + TypeScript (Tauri 2), no framework on the frontend. The
  character is drawn in code (Canvas 2D), no Rive/Lottie/image assets.
- **No API keys anywhere.** Fedo's own requests are answered by one of two
  backends, chosen in Settings → AI: the `claude` CLI the user is already
  logged into (default), or a **local Ollama model on this machine**
  (`llm.rs`; changed from "no Ollama" at the owner's request, Phase 6.5).
  No cloud provider code, no OS keychain/Secret Service key storage. Claude
  Code's own sessions and hooks are always Claude Code.
- No telemetry. Network calls only to services the user configured
  (today: none that need configuring — Spotify is read locally over
  MPRIS/D-Bus).
- Never block Claude Code or Antigravity: if the app doesn't answer, the
  hook exits immediately. A cold start of the island (webview torn down) gets
  at most the 5 s `COLD_ACK_TIMEOUT` in `pipe.rs`.
- Never overwrite `~/.claude/settings.json` or an agent's own config:
  dated backup, merge, show the diff, write only after the user confirms
  (`app/src-tauri/src/config_file.rs`, shared by every installer).
- Nothing on the machine changes without the user's confirmation — Fedo
  writes the command, shows it, the user runs it. This applies to every
  phase, including voice.
- Performance: ~0% CPU and ≤40 MB RSS when the island is hidden
  (FEDO.md §3) — event-driven only, no polling loops except where FEDO.md
  explicitly allows one.
- Keep the app id `org.fedo.Fedo` (config, data dir and permissions
  depend on it): config in `~/.config/fedo/`, data in
  `~/.local/share/fedo/`, hook socket at `$XDG_RUNTIME_DIR/fedo.sock`.
- Pill IDs are stable contract values: never rename an existing one, even
  one this build doesn't currently offer.
- Only Claude Code and Antigravity get a one-click hook installer in
  Settings → Agents. The relay's underlying protocol still accepts any
  `--agent <name>` (see `docs/AGENTS.md`) — that part wasn't narrowed,
  only the installer UI.
- English only for now; `app/src/i18n/i18n.ts`'s lookup mechanism stays
  so languages (Hindi first) can be added back without a rewrite.
- Every phase in `FEDO.md`: run its acceptance checks, commit, write a
  short report, and stop — don't start the next phase unasked.
