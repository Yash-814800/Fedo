#!/usr/bin/env bash
# Removes what scripts/install.sh put in your home directory.
#
#   scripts/uninstall.sh              service, desktop entry, extension, binaries, hooks
#   scripts/uninstall.sh --purge      also ~/.config/fedo and ~/.local/share/fedo
#                                     (settings, models, the Piper/Vosk venv, logs)
#   scripts/uninstall.sh --dry-run    print every step, change nothing
#
# Hooks: Fedo's entries are taken out of ~/.claude/settings.json and
# ~/.gemini/config/hooks.json (diff + dated backup, your confirmation), and
# the status line you had before Fedo is put back. Your other settings are
# never rewritten from an old copy; the dated backups stay beside the files.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$HOME/.local/bin"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
cfg="${XDG_CONFIG_HOME:-$HOME/.config}"

dry=0; purge=0
for a in "$@"; do
  case "$a" in
    --dry-run) dry=1 ;;
    --purge) purge=1 ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done
say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
run() { if [ "$dry" = 1 ]; then printf '   [dry-run] %s\n' "$*"; else "$@"; fi; }

say "Hooks"
fedo="$bin/fedo"
[ -x "$fedo" ] || fedo="$root/app/target/release/fedo"
if [ -x "$fedo" ]; then
  if [ "$dry" = 1 ]; then echo "   [dry-run] $fedo hooks uninstall"; else "$fedo" hooks uninstall || true; fi
else
  echo "fedo binary not found: remove Fedo's entries from ~/.claude/settings.json and ~/.gemini/config/hooks.json by hand (see the dated .bak files)."
fi

say "Service"
run systemctl --user disable --now fedo.service 2>/dev/null || true
run rm -f "$cfg/systemd/user/fedo.service"
run systemctl --user daemon-reload

say "Extension"
run "$root/gnome-extension/install.sh" --uninstall

say "Binaries, desktop entry, icon"
if [ "$dry" = 0 ]; then busctl --user call org.fedo.Fedo /org/fedo/Fedo org.fedo.Fedo Quit 2>/dev/null || true; fi
run rm -f "$bin/fedo" "$bin/fedo-hook" "$bin/fedo.new"
run rm -f "$share/applications/org.fedo.Fedo.desktop" "$share/icons/hicolor/128x128/apps/org.fedo.Fedo.png"
run rm -f "$cfg/autostart/org.fedo.Fedo.desktop" "$cfg/autostart/Fedo.desktop"

if [ "$purge" = 1 ]; then
  say "Purging settings and data"
  run rm -rf "$cfg/fedo" "$share/fedo"
else
  echo
  echo "Kept: $cfg/fedo (settings) and $share/fedo (models, venv, logs, actions.log). --purge removes them."
fi
say "Done"
