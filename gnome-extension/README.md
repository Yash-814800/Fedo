# Fedo — GNOME Shell extension

The ghost in the top bar. Talks to the `fedo` daemon over `org.fedo.Fedo`
on the session bus; see `docs/FEDO-NOTES.md` (Phase 1) for what it does and
what has and has not been verified.

```
./install.sh                  # symlink into ~/.local/share/gnome-shell/extensions/
./install.sh --link-bin ../app/target/release/fedo   # also put fedo on PATH
./install.sh --enable         # also gnome-extensions enable
```

On Wayland the shell cannot be restarted in place: log out and in once
after the first install. To try it without touching your session
(`sudo dnf install mutter-devkit`):

```
dbus-run-session -- gnome-shell --devkit
```

Check it against the daemon without the shell:

```
busctl --user introspect org.fedo.Fedo /org/fedo/Fedo
busctl --user call org.fedo.Fedo /org/fedo/Fedo org.fedo.Fedo Toggle
```

## Keys

`schemas/org.gnome.shell.extensions.fedo.gschema.xml` defines the extension's
keys. They are window-manager keybindings, so they work over a fullscreen app,
and each one calls the daemon (`PushToTalk()` or `Action(name)`).

| Key (GSettings) | Default | Does |
|---|---|---|
| `push-to-talk` | `Super+Alt+V` | listen for a spoken command; press again to finish |
| `toggle-island` | `Super+Alt+F` | open or close the island |
| `open-chat` | `Super+Alt+C` | open the chat |
| `approve-latest` | `Super+Alt+Y` | allow the permission card **that is on screen**; if it isn't, bring it up |
| `jump-to-latest` | `Super+Alt+J` | raise the window of the session that is asking or working |
| `mute-toggle` | `Super+Alt+M` | mute / unmute |

Change them in Fedo's Settings, or with `gsettings --schemadir
~/.local/share/gnome-shell/extensions/fedo@yash/schemas set
org.gnome.shell.extensions.fedo push-to-talk "['<Super><Alt>v']"`.
`install.sh` compiles the schema; log out and in once for the shell to load a
changed extension.
