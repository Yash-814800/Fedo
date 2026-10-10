# Fedo — the Tauri app

The Rust + TypeScript app: Canvas 2D ghost character, Claude Code and
Antigravity hooks via `fedo-hook` over a Unix socket in
`$XDG_RUNTIME_DIR`. Forked from [coucou](https://github.com/Louis-CFM/coucou)'s
Windows/Linux Tauri app — see [`../FEDO.md`](../FEDO.md) for the fork's
full build plan and [`../docs/FEDO-NOTES.md`](../docs/FEDO-NOTES.md) for
decisions and limitations recorded as each phase lands.

## Build

Needs [Rust](https://rustup.rs), [Node 20+](https://nodejs.org), and on
Fedora:

```sh
sudo dnf install -y gcc gcc-c++ make pkgconf-pkg-config rust cargo \
  webkit2gtk4.1-devel gtk3-devel librsvg2-devel openssl-devel dbus-devel \
  gtk-layer-shell-devel libayatana-appindicator-gtk3-devel \
  patchelf gstreamer1-plugins-base gstreamer1-plugins-good
```

```sh
cd app
npm install
npm run tauri dev      # live-reloading development build
npm run pack           # AppImage, .deb and .rpm in app/release/
```

`npm run dev` alone serves the front end in an ordinary browser, enough to
work on the island's looks.

## Layout

```
app/
  src/                 island front end (TypeScript, no framework)
    ghost/             the ghost engine, greeting, debug menu and desktop logic
    desktop/           the ghost's own little window, when it lives on the desktop
    island/            state machine, hooks, agent handling
    views/             every island view
    settings/          the settings window
  src-tauri/           Rust backend: window, Unix socket, pollers
  hook/                fedo-hook, the Claude Code / Antigravity relay
  scripts/             icon generator
```

## What's different from upstream coucou

- **No macOS, no Windows.** `NotchBuddy/` (the Mac app) and every
  Windows-only code path (`platform/windows.rs`, the NSIS installer, the
  named-pipe hook transport) are gone.
- **No API keys anywhere.** The Anthropic/Google AI/OpenAI chat providers,
  Ollama/LM Studio local-model chat, and the OS keychain/Secret Service
  key storage are all removed. The chat runs every turn through the `claude`
  CLI you are logged into (`claude -p`).
- **No third-party service integrations.** Stripe, n8n, GitHub, Vercel,
  Resend, Notion and Cal.com are gone. Spotify (read over MPRIS/D-Bus,
  Linux-only even in the coucou fork) is the one integration kept.
- **Agents narrowed to Claude Code and Antigravity.** The one-click
  installers for Gemini CLI, Cursor, Codex, GitHub Copilot CLI, Muse Code,
  OpenCode, Amp and Hermes are removed from Settings → Agents. The relay's
  underlying protocol is unchanged and still accepts any `--agent <name>`
  tag (see [`../docs/AGENTS.md`](../docs/AGENTS.md)) — only the
  one-click installer UI was narrowed.
- **No wardrobe.** Seasonal outfits and the right-click wardrobe view are
  removed along with the upstream character art; the ghost is drawn from
  scratch in code (`src/ghost/engine.ts`).
- **English only**, for now — `src/i18n/i18n.ts`'s lookup mechanism is
  kept so languages (Hindi first) can be added back later, but
  `strings.json`/`extra.json`'s translation tables are empty.
- **Renamed:** the app id is `org.fedo.Fedo`, the binary is `fedo`, the
  hook relay is `fedo-hook`, config lives in `~/.config/fedo/`, data in
  `~/.local/share/fedo/`, and the hook socket is
  `$XDG_RUNTIME_DIR/fedo.sock`.

## GNOME on Wayland

GNOME has no layer-shell and ignores where a Wayland window asks to go.
The XWayland dock-window fallback — top centre, on every workspace — is
what Fedo uses; the GNOME Shell extension in `../gnome-extension/` places it
under the top bar;
`FEDO_X11=0` keeps the native Wayland window instead,
`FEDO_DOCK=0` makes it a utility window, `FEDO_LAYER_SHELL=0` forces a
regular window anywhere.

## Log

`~/.local/share/fedo/fedo.log` — hook events, permission decisions. It
stays on your machine.
