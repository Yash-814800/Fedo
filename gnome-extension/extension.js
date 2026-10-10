// Fedo — the ghost in the GNOME top bar.
//
// Everything heavy lives in the `fedo` daemon (org.fedo.Fedo on the session
// bus). This file only: draws the icon and its state dot, forwards clicks,
// starts the daemon when it is not running, and keeps the island's window
// where GNOME will not put it by itself (below the top bar, centred, on every
// workspace, out of the taskbar).

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

const BUS_NAME = 'org.fedo.Fedo';
const OBJECT_PATH = '/org/fedo/Fedo';
const IFACE = 'org.fedo.Fedo';

// How long the pointer must rest on the icon before the island opens.
const HOVER_MS = 350;
// The island window is the Tauri app's DOCK window; its Settings window shares
// the WM_CLASS but is a normal window.
const ISLAND_WM_CLASS = 'Fedo';

const FedoButton = GObject.registerClass(
class FedoButton extends PanelMenu.Button {
    _init(onPrimary) {
        super._init(0.5, 'Fedo');
        this._onPrimary = onPrimary;
    }

    vfunc_event(event) {
        const type = event.type();
        if (type === Clutter.EventType.BUTTON_PRESS || type === Clutter.EventType.TOUCH_BEGIN) {
            const secondary = type === Clutter.EventType.BUTTON_PRESS &&
                event.get_button() === Clutter.BUTTON_SECONDARY;
            if (secondary) {
                this.menu.toggle();
            } else {
                this.menu.close();
                this._onPrimary();
            }
            return Clutter.EVENT_STOP;
        }
        return Clutter.EVENT_PROPAGATE;
    }
});

export default class FedoExtension extends Extension {
    enable() {
        this._proxy = null;
        this._owner = false;
        this._state = 'idle';
        this._visibility = 'hidden';
        this._hoverId = 0;
        this._captureId = 0;
        this._windowSignals = new Map();
        this._placing = false;
        this._cancellable = new Gio.Cancellable();

        this._buildButton();
        Main.panel.addToStatusArea(this.uuid, this._button, 0, 'right');

        this._watchId = Gio.bus_watch_name(Gio.BusType.SESSION, BUS_NAME,
            Gio.BusNameWatcherFlags.NONE,
            () => this._connect(),
            () => this._disconnect());

        // The island window can show up (or be re-created) at any time.
        this._createdId = global.display.connect('window-created', (_d, w) => this._maybeAdopt(w));
        for (const actor of global.get_window_actors())
            this._maybeAdopt(actor.meta_window);
        this._monitorsId = Main.layoutManager.connect('monitors-changed', () => this._placeAll());
        this._panelSizeId = Main.panel.connect('notify::height', () => this._placeAll());
    }

    disable() {
        if (this._watchId) Gio.bus_unwatch_name(this._watchId);
        this._watchId = 0;
        this._cancellable.cancel();
        this._cancellable = null;
        this._stopHover();
        this._setCapture(false);
        if (this._createdId) global.display.disconnect(this._createdId);
        if (this._monitorsId) Main.layoutManager.disconnect(this._monitorsId);
        if (this._panelSizeId) Main.panel.disconnect(this._panelSizeId);
        this._createdId = this._monitorsId = this._panelSizeId = 0;
        for (const [w, ids] of this._windowSignals) {
            for (const id of ids) {
                try { w.disconnect(id); } catch (_e) { /* window already gone */ }
            }
        }
        this._windowSignals.clear();
        this._proxy = null;
        this._button?.destroy();
        this._button = null;
    }

    // ── Panel button ──────────────────────────────────────────────────────────

    _buildButton() {
        this._button = new FedoButton(() => this._primaryClick());

        const box = new St.Widget({layout_manager: new Clutter.BinLayout(), style_class: 'panel-status-indicators-box'});
        const iconFile = this.dir.get_child('icons').get_child('fedo-symbolic.svg');
        this._icon = new St.Icon({
            gicon: new Gio.FileIcon({file: iconFile}),
            style_class: 'system-status-icon',
        });
        this._dot = new St.Widget({
            style_class: 'fedo-dot fedo-dot-offline',
            x_align: Clutter.ActorAlign.END,
            y_align: Clutter.ActorAlign.START,
            x_expand: true,
            y_expand: true,
        });
        box.add_child(this._icon);
        box.add_child(this._dot);
        this._button.add_child(box);

        const menu = this._button.menu;
        menu.addAction('Settings', () => this._call('Show', '(s)', ['settings']));
        this._muteItem = new PopupMenu.PopupSwitchMenuItem('Mute sounds', false);
        this._muteItem.connect('toggled', (_i, on) => this._setMuted(on));
        menu.addMenuItem(this._muteItem);
        // The voice pipeline arrives in Phase 8; the entry is here so the menu
        // does not change shape later, and says why it does nothing yet.
        this._voiceItem = menu.addAction('Pause voice (not available yet)', () => {});
        this._voiceItem.setSensitive(false);
        menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        menu.addAction('Quit Fedo', () => this._quit());

        this._button.connect('enter-event', () => this._startHover());
        this._button.connect('leave-event', () => this._stopHover());
        this._button.menu.connect('open-state-changed', (_m, open) => {
            if (open) this._refreshMute();
        });
        this._renderDot();
    }

    _renderDot() {
        const kind = this._owner ? this._state : 'offline';
        this._dot.style_class = `fedo-dot fedo-dot-${kind}`;
        this._icon.opacity = this._owner ? 255 : 130;
    }

    // ── Clicks ────────────────────────────────────────────────────────────────

    _primaryClick() {
        if (!this._owner) {
            this._startDaemon();
            return;
        }
        this._call('Toggle');
    }

    _startHover() {
        this._stopHover();
        if (!this._owner || this._visibility === 'open') return;
        this._hoverId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, HOVER_MS, () => {
            this._hoverId = 0;
            if (this._visibility !== 'open' && !this._button.menu.isOpen)
                this._call('Show', '(s)', ['']);
            return GLib.SOURCE_REMOVE;
        });
    }

    _stopHover() {
        if (this._hoverId) GLib.source_remove(this._hoverId);
        this._hoverId = 0;
    }

    _quit() {
        this._call('Quit');
    }

    _startDaemon() {
        const bin = GLib.find_program_in_path('fedo') ??
            GLib.build_filenamev([GLib.get_home_dir(), '.local', 'bin', 'fedo']);
        try {
            Gio.Subprocess.new([bin], Gio.SubprocessFlags.NONE);
        } catch (e) {
            Main.notify('Fedo', `Could not start the fedo daemon (${bin}): ${e.message}`);
        }
    }

    // ── D-Bus ─────────────────────────────────────────────────────────────────

    _connect() {
        this._owner = true;
        Gio.DBusProxy.new_for_bus(Gio.BusType.SESSION, Gio.DBusProxyFlags.NONE, null,
            BUS_NAME, OBJECT_PATH, IFACE, this._cancellable, (_s, res) => {
                try {
                    this._proxy = Gio.DBusProxy.new_for_bus_finish(res);
                } catch (e) {
                    if (!e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                        console.error(`fedo: proxy: ${e.message}`);
                    return;
                }
                this._proxy.connect('g-properties-changed', () => this._sync());
                this._sync();
            });
        this._renderDot();
    }

    _disconnect() {
        this._owner = false;
        this._proxy = null;
        this._visibility = 'hidden';
        this._setCapture(false);
        this._renderDot();
    }

    _sync() {
        if (!this._proxy) return;
        const get = n => this._proxy.get_cached_property(n)?.unpack();
        this._state = get('State') ?? 'idle';
        this._visibility = get('Visibility') ?? 'hidden';
        this._muteItem.setToggleState(!!get('Muted'));
        this._setCapture(this._visibility === 'open');
        this._renderDot();
        // The island may have moved or resized with the state.
        this._placeAll();
    }

    _refreshMute() {
        this._muteItem.setToggleState(!!this._proxy?.get_cached_property('Muted')?.unpack());
    }

    _setMuted(on) {
        if (!this._owner) return;
        Gio.DBus.session.call(BUS_NAME, OBJECT_PATH, 'org.freedesktop.DBus.Properties', 'Set',
            new GLib.Variant('(ssv)', [IFACE, 'Muted', new GLib.Variant('b', on)]),
            null, Gio.DBusCallFlags.NONE, -1, this._cancellable, null);
    }

    _call(method, sig = null, args = null) {
        if (!this._proxy) return;
        this._proxy.call(method, sig ? new GLib.Variant(sig, args) : null,
            Gio.DBusCallFlags.NO_AUTO_START, 2000, this._cancellable, (p, res) => {
                try {
                    p.call_finish(res);
                } catch (e) {
                    if (!e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                        console.error(`fedo: ${method}: ${e.message}`);
                }
            });
    }

    // ── Outside click / Escape closes the open island ─────────────────────────
    //
    // The island never takes keyboard focus, so it cannot hear either itself.
    // The stage can, but only while the island is open; the event is always
    // passed on, so Escape in a terminal still reaches the terminal.

    _setCapture(on) {
        if (on && !this._captureId) {
            this._captureId = global.stage.connect('captured-event', (_s, event) => this._onStageEvent(event));
        } else if (!on && this._captureId) {
            global.stage.disconnect(this._captureId);
            this._captureId = 0;
        }
    }

    _onStageEvent(event) {
        const type = event.type();
        if (type === Clutter.EventType.KEY_PRESS) {
            if (event.get_key_symbol() === Clutter.KEY_Escape && !this._button.menu.isOpen)
                this._call('Hide');
        } else if (type === Clutter.EventType.BUTTON_PRESS || type === Clutter.EventType.TOUCH_BEGIN) {
            const source = event.get_source();
            if (source && this._button.contains(source)) return Clutter.EVENT_PROPAGATE;
            if (this._button.menu.isOpen) return Clutter.EVENT_PROPAGATE;
            const [x, y] = event.get_coords();
            const rect = this._islandRect();
            if (!rect || x < rect.x || x >= rect.x + rect.width || y < rect.y || y >= rect.y + rect.height)
                this._call('Hide');
        }
        return Clutter.EVENT_PROPAGATE;
    }

    // ── The island window ─────────────────────────────────────────────────────

    _isIsland(w) {
        return w.get_wm_class() === ISLAND_WM_CLASS &&
            w.get_window_type() === Meta.WindowType.DOCK;
    }

    _islands() {
        return global.get_window_actors()
            .map(a => a.meta_window)
            .filter(w => w && this._isIsland(w));
    }

    _islandRect() {
        const w = this._islands()[0];
        return w ? w.get_frame_rect() : null;
    }

    _maybeAdopt(w) {
        if (!w || this._windowSignals.has(w)) return;
        // On X11 the class arrives after the window is created: wait for it.
        if (!w.get_wm_class()) {
            const waitId = w.connect('notify::wm-class', () => {
                w.disconnect(waitId);
                this._maybeAdopt(w);
            });
            return;
        }
        if (!this._isIsland(w)) return;
        this._windowSignals.set(w, [
            w.connect('position-changed', () => this._place(w)),
            w.connect('size-changed', () => this._place(w)),
            w.connect('unmanaged', () => this._windowSignals.delete(w)),
        ]);
        w.stick();
        this._place(w);
    }

    _placeAll() {
        for (const w of this._islands()) this._place(w);
    }

    // Centre on the monitor under the top bar, just below it. The Tauri side
    // sizes the window and asks for y=0; GNOME's panel would cover that.
    _place(w) {
        if (this._placing) return;
        const monitor = w.get_monitor();
        if (monitor < 0) return;
        const area = Main.layoutManager.monitors[monitor];
        if (!area) return;
        const rect = w.get_frame_rect();
        const panel = monitor === Main.layoutManager.primaryIndex ? Main.panel.height : 0;
        const x = area.x + Math.round((area.width - rect.width) / 2);
        const y = area.y + panel;
        if (rect.x === x && rect.y === y) return;
        this._placing = true;
        try {
            w.move_frame(true, x, y);
        } finally {
            this._placing = false;
        }
    }
}
