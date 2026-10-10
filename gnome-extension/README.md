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

## Push-to-talk

`schemas/org.gnome.shell.extensions.fedo.gschema.xml` defines the
`push-to-talk` key (default `Ctrl+Alt+Space`). It calls `PushToTalk()` on the
daemon: press to start listening, press again to finish. It is a window-manager
keybinding, so it works over a fullscreen app. `install.sh` compiles the schema
(`glib-compile-schemas`); log out and in once for the shell to load the change.
