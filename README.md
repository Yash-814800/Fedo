# Fedo

**A personal desktop companion for Fedora Linux, GNOME on Wayland.**

A ghost in the GNOME top bar shows your Claude Code and Antigravity
sessions, lets you approve permissions and answer questions without
leaving what you're doing, and will grow into a small assistant over the
phases described in [`FEDO.md`](FEDO.md).

Fedo is a fork of [coucou](https://github.com/Louis-CFM/coucou) by Louis
Raillé, keeping its island UI/UX and Claude Code hook relay, with
everything that isn't Fedora/GNOME removed: no macOS or Windows build, no
API keys anywhere, no third-party service integrations, and agent support
narrowed to Claude Code and Antigravity.

![Fedora](https://img.shields.io/badge/Fedora-Linux-294172?logo=fedora&logoColor=white)
![GNOME](https://img.shields.io/badge/GNOME-Wayland-4A86CF?logo=gnome&logoColor=white)
![Tauri 2](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=black)
![Rust](https://img.shields.io/badge/Rust-backend-000?logo=rust)
![Code: MIT](https://img.shields.io/badge/code-MIT-green)

## Status

This repo is in **Phase 0** (fork hygiene, rename, strip, build) of
`FEDO.md`'s build plan. What exists today:

- The Tauri app builds and the island runs on this machine's GNOME/Wayland
  session, with a plain placeholder ghost (a circle with two eyes) in
  place of the original character — a real design comes in Phase 2.
- Claude Code hooks work: approve or deny a permission request, answer an
  `AskUserQuestion` prompt, see a session's live diff.
- Antigravity has a one-click hook installer in Settings → Agents.
- The Spotify pill works over MPRIS/D-Bus.
- Chat, the weekly recap, timers, voice and everything else `FEDO.md`
  describes for later phases is **not built yet**.

See [`docs/FEDO-NOTES.md`](docs/FEDO-NOTES.md) for decisions and known
limitations as each phase lands.

## Build

```
cd app
npm install
npm run tauri dev
```

Needs Rust, Node 20+, and the Fedora packages listed in
[`docs/FEDO-NOTES.md`](docs/FEDO-NOTES.md). See
[`app/README.md`](app/README.md) for the Tauri app's own layout and what
differs from the coucou fork it came from.

## Credits

Forked from [coucou](https://github.com/Louis-CFM/coucou) by
[Louis Raillé](https://louisraille.fr), built with Claude Code.

## License

- **Code:** [MIT](LICENSE) — the original coucou copyright notice stays;
  Fedo's own changes are © 2026 the Fedo maintainer.
- **Fedo's own name, ghost character and sounds:** see
  [LICENSE-ASSETS.md](LICENSE-ASSETS.md). coucou's own name, Mochi
  character, icon, sounds and media are not part of this fork.
