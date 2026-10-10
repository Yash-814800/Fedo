#!/usr/bin/env bash
# Installs the Fedo GNOME Shell extension by symlinking this directory into
# ~/.local/share/gnome-shell/extensions/, so edits here are live on next login.
#
#   ./install.sh                  symlink only (changes nothing else)
#   ./install.sh --enable         also `gnome-extensions enable` it
#   ./install.sh --link-bin PATH  also symlink PATH as ~/.local/bin/fedo, so
#                                 the icon can start the daemon
#   ./install.sh --uninstall      remove the symlink (and disable it)
#
# On Wayland the shell cannot be restarted in place: a newly linked extension
# is only seen after you log out and back in.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
uuid="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['uuid'])" "$here/metadata.json")"
target="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$uuid"

enable=0; uninstall=0; bin=""
while [ $# -gt 0 ]; do
  case "$1" in
    --enable) enable=1 ;;
    --uninstall) uninstall=1 ;;
    --link-bin) shift; bin="${1:?--link-bin needs a path}" ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ "$uninstall" = 1 ]; then
  gnome-extensions disable "$uuid" 2>/dev/null || true
  if [ -L "$target" ]; then rm "$target"; echo "removed $target"; else echo "nothing at $target"; fi
  exit 0
fi

mkdir -p "$(dirname "$target")"
if [ -e "$target" ] && [ ! -L "$target" ]; then
  echo "$target exists and is not a symlink; refusing to touch it" >&2
  exit 1
fi
# The push-to-talk key lives in a GSettings schema, which must be compiled.
if command -v glib-compile-schemas >/dev/null; then
  glib-compile-schemas "$here/schemas"
else
  echo "glib-compile-schemas not found: push-to-talk needs it (sudo dnf install glib2-devel)" >&2
fi
ln -sfn "$here" "$target"
echo "linked $target -> $here"

if [ -n "$bin" ]; then
  [ -x "$bin" ] || { echo "$bin is not an executable file" >&2; exit 1; }
  mkdir -p "$HOME/.local/bin"
  ln -sfn "$(readlink -f "$bin")" "$HOME/.local/bin/fedo"
  echo "linked ~/.local/bin/fedo -> $(readlink -f "$bin")"
fi

if [ "$enable" = 1 ]; then
  if gnome-extensions enable "$uuid"; then
    echo "enabled $uuid"
  else
    echo "the shell does not know $uuid yet — log out and in, then: gnome-extensions enable $uuid"
  fi
else
  echo "next: log out and in, then: gnome-extensions enable $uuid"
fi
