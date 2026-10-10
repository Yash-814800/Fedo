#!/usr/bin/env bash
# Fedo — one-shot install for the Fedora/GNOME machine this was built for.
#
#   scripts/install.sh              build, install, enable the service and the extension
#   scripts/install.sh --rpm        build the RPM instead and print the dnf line
#   scripts/install.sh --dry-run    print every step, change nothing
#   scripts/install.sh --no-build   reuse app/target/release (already built)
#   scripts/install.sh --no-hooks   skip the Claude Code / Antigravity hook step
#   scripts/install.sh --no-service --no-extension
#
# Everything lands in your home directory. Nothing here runs sudo: where a
# package is needed it prints the exact `sudo dnf install …` line for you.
# The hook step shows a diff of ~/.claude/settings.json and
# ~/.gemini/config/hooks.json, takes a dated backup, and writes only after a "y".
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="$root/app"
bin="$HOME/.local/bin"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
cfg="${XDG_CONFIG_HOME:-$HOME/.config}"
data="$share/fedo"

dry=0; build=1; rpm=0; hooks=1; service=1; extension=1
for a in "$@"; do
  case "$a" in
    --dry-run) dry=1 ;;
    --no-build) build=0 ;;
    --rpm) rpm=1 ;;
    --no-hooks) hooks=0 ;;
    --no-service) service=0 ;;
    --no-extension) extension=0 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
run() { if [ "$dry" = 1 ]; then printf '   [dry-run] %s\n' "$*"; else "$@"; fi; }

[ "$(uname -s)" = Linux ] || { echo "Fedo is for Linux (Fedora, GNOME on Wayland)." >&2; exit 1; }

# ── 1. What the build needs ──────────────────────────────────────────────────
say "Checking build requirements"
missing=()
need_cmd() { command -v "$1" >/dev/null 2>&1 || missing+=("$2"); }
need_pc()  { pkg-config --exists "$1" 2>/dev/null || missing+=("$2"); }
if [ "$build" = 1 ] || [ "$rpm" = 1 ]; then
  need_cmd cargo cargo; need_cmd rustc rust; need_cmd node nodejs; need_cmd npm npm
  need_cmd cmake cmake; need_cmd pkg-config pkgconf-pkg-config; need_cmd gcc gcc-c++
  need_pc webkit2gtk-4.1 webkit2gtk4.1-devel; need_pc gtk+-3.0 gtk3-devel
  need_pc dbus-1 dbus-devel; need_pc gtk-layer-shell-0 gtk-layer-shell-devel
  need_pc openssl openssl-devel; need_pc librsvg-2.0 librsvg2-devel
  need_pc ayatana-appindicator3-0.1 libayatana-appindicator-gtk3-devel
  [ -e /usr/lib64/libclang.so ] || ls /usr/lib64/libclang.so.* >/dev/null 2>&1 || missing+=(clang-libs)
  [ "$rpm" = 1 ] && need_cmd rpmbuild rpm-build
fi
# Runtime tools Fedo shells out to; voice and the phone are optional features.
opt=()
command -v pw-record >/dev/null || opt+=(pipewire-utils)
command -v gsettings >/dev/null || opt+=(glib2)
command -v glib-compile-schemas >/dev/null || opt+=(glib2-devel)
if [ "${#missing[@]}" -gt 0 ]; then
  echo "Missing packages. Run this, then run the script again:"
  echo "  sudo dnf install -y ${missing[*]}"
  exit 1
fi
[ "${#opt[@]}" -gt 0 ] && echo "Optional (voice / extension keys):  sudo dnf install -y ${opt[*]}"
echo "ok"

# ── 2. Build ─────────────────────────────────────────────────────────────────
if [ "$build" = 1 ]; then
  say "Building (Rust + frontend; the first build compiles whisper.cpp, a few minutes)"
  run bash -c "cd '$app' && npm install --no-audit --no-fund"
  if [ "$rpm" = 1 ]; then
    run bash -c "cd '$app' && cargo build --release -p fedo-hook -p fedo-stt && npx tauri build --bundles rpm"
  else
    run bash -c "cd '$app' && cargo build --release -p fedo-hook -p fedo-stt && npx tauri build --no-bundle"
  fi
fi

if [ "$rpm" = 1 ]; then
  rpmfile="$(ls -t "$app"/target/release/bundle/rpm/*.rpm 2>/dev/null | head -1 || true)"
  say "RPM"
  echo "Built: ${rpmfile:-<see app/target/release/bundle/rpm>}"
  echo "Install it (you run sudo):"
  echo "  sudo dnf install -y ${rpmfile:-app/target/release/bundle/rpm/*.rpm}"
  echo "Then, for the top-bar ghost and hooks:  $root/gnome-extension/install.sh --enable  &&  fedo hooks install"
  exit 0
fi

rel="$app/target/release"
if [ "$dry" = 0 ]; then
  for f in fedo fedo-hook fedo-stt; do
    [ -x "$rel/$f" ] || { echo "$rel/$f is missing — run without --no-build." >&2; exit 1; }
  done
fi

# ── 3. Binaries ──────────────────────────────────────────────────────────────
say "Installing binaries to $bin and $data/bin"
run mkdir -p "$bin" "$data/bin"
# A copy, not a symlink: rebuilding the repo must not change a running daemon.
run install -m 0755 "$rel/fedo" "$bin/fedo.new"
run mv -f "$bin/fedo.new" "$bin/fedo"
run install -m 0755 "$rel/fedo-hook" "$bin/fedo-hook"
run install -m 0755 "$rel/fedo-stt" "$data/bin/fedo-stt"
case ":$PATH:" in *":$bin:"*) ;; *) echo "note: $bin is not on your PATH" ;; esac

# ── 4. Desktop entry + icon (the desktop portal needs the app id) ────────────
say "Desktop entry and icon"
run mkdir -p "$share/applications" "$share/icons/hicolor/128x128/apps"
run install -m 0644 "$app/src-tauri/icons/128x128.png" "$share/icons/hicolor/128x128/apps/org.fedo.Fedo.png"
if [ "$dry" = 1 ]; then
  echo "   [dry-run] write $share/applications/org.fedo.Fedo.desktop"
else
  cat > "$share/applications/org.fedo.Fedo.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=Fedo
Comment=A ghost in the top bar for Claude Code, Antigravity, your phone and your day
Exec=$bin/fedo
Icon=org.fedo.Fedo
Terminal=false
Categories=Utility;
StartupWMClass=Fedo
X-GNOME-UsesNotifications=true
DESK
  command -v update-desktop-database >/dev/null && update-desktop-database "$share/applications" 2>/dev/null || true
fi

# ── 5. systemd --user service ────────────────────────────────────────────────
if [ "$service" = 1 ]; then
  say "systemd --user service (starts on login, restarts on a crash)"
  unit="$cfg/systemd/user/fedo.service"
  run mkdir -p "$(dirname "$unit")"
  if [ "$dry" = 1 ]; then
    echo "   [dry-run] write $unit"
  else
    cat > "$unit" <<UNIT
[Unit]
Description=Fedo — a ghost in the GNOME top bar
After=graphical-session.target
PartOf=graphical-session.target
ConditionEnvironment=WAYLAND_DISPLAY

[Service]
Type=simple
ExecStart=$bin/fedo
# A clean exit (Quit from the ghost's menu) stays quit; a crash comes back.
Restart=on-failure
RestartSec=3

[Install]
WantedBy=graphical-session.target
UNIT
  fi
  run systemctl --user daemon-reload
  run systemctl --user enable fedo.service
  if systemctl --user is-active --quiet fedo.service 2>/dev/null || pgrep -x fedo >/dev/null; then
    echo "Fedo is already running; the service takes over at your next login."
  else
    run systemctl --user start fedo.service
  fi
  if [ -f "$cfg/autostart/org.fedo.Fedo.desktop" ] || [ -f "$cfg/autostart/Fedo.desktop" ]; then
    echo "note: Settings → 'Launch at startup' also autostarts Fedo; with the service you can turn it off."
  fi
fi

# ── 6. GNOME extension ───────────────────────────────────────────────────────
if [ "$extension" = 1 ]; then
  say "GNOME Shell extension"
  run "$root/gnome-extension/install.sh" --enable
  echo "On Wayland, log out and in once for GNOME to load a new or changed extension."
fi

# ── 7. Hooks ─────────────────────────────────────────────────────────────────
if [ "$hooks" = 1 ]; then
  say "Claude Code and Antigravity hooks (diff + backup, then your confirmation)"
  if [ "$dry" = 1 ]; then echo "   [dry-run] $bin/fedo hooks install"; else "$bin/fedo" hooks install; fi
fi

say "Done"
echo "Settings: click the ghost, then the gear.  Remove everything: scripts/uninstall.sh"
