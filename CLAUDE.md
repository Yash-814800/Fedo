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
- `app/src/core/pills.ts` — single source of truth for every pill
  (workspace tools, agents, AI providers, services). Every pill ID,
  color, category and connection method lives here; IDs are stable
  contract values and are never renamed, even for a pill this build
  doesn't offer (marked `support: "no"` instead of deleted).
- `app/src/ghost/engine.ts` — the ghost's animation engine: state
  machine, tweens, particles. Its body is a plain circle for now
  (Phase 0 placeholder) — Phase 2 redraws it, in code, without touching
  the state machine underneath.
- `docs/AGENTS.md` — the hook relay's third-party agent protocol (still
  accurate and live, even though only Claude Code and Antigravity get a
  one-click installer in this build).
- `docs/FEDO-NOTES.md` — the running decisions/verification/limitations
  log; read it before assuming something works.

## Build

```
cd app && npm install && npm run tauri dev
```

Needs Rust, Node 20+, and the Fedora packages listed in
[`docs/FEDO-NOTES.md`](docs/FEDO-NOTES.md) (verified against `dnf info`,
not guessed). `xcodegen`/Xcode/Swift do not apply here — there is no
macOS app in this fork.

## Rules

- Rust + TypeScript (Tauri 2), no framework on the frontend. The
  character is drawn in code (Canvas 2D), no Rive/Lottie/image assets.
- **No API keys anywhere.** All intelligence goes through the `claude`
  CLI the user is already logged into. No Anthropic/OpenAI/Gemini/Ollama
  provider code, no OS keychain/Secret Service key storage.
- No telemetry. Network calls only to services the user configured
  (today: none that need configuring — Spotify is read locally over
  MPRIS/D-Bus).
- Never block Claude Code or Antigravity: if the app doesn't answer, the
  hook exits immediately.
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
