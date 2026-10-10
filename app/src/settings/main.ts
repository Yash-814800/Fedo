// Settings window — the place where anything that writes to disk is confirmed.
// This covers the Claude Code and Antigravity hooks and the general preferences.
// No API keys or third-party integrations in this fork (FEDO.md §0).

import "./settings.css";
import { Bridge, onEvent, type HookPreview, type HookStatus, type ShortcutsReport } from "../core/bridge";
import {
  ISLAND_SHORTCUTS, SHORTCUTS, SHORTCUT_TEXT, activeKeys, displayKeys, duplicates, effective,
  recordPress, type Binding,
} from "../core/shortcuts";
import { DEFAULT_SETTINGS, type Settings } from "../core/state";
import { SOUND_NAMES } from "../core/sound";
import {
  MAX_DECLARED, PILL_CATEGORIES, availablePills, chooseMainPill, isComingSoon, mainPillChoices,
  sanitizeDeclared, toggleDeclared, type PillDefinition,
} from "../core/pills";
import { h, clear } from "../views/dom";
import { agentsSection } from "./agents";
import { colorDot } from "./colors";
import { renderDiff, statusDot } from "./parts";
import {
  LANGUAGES, isRtl, onLanguageChange, resolveLanguage, setLanguage, systemLanguages, t,
} from "../i18n/i18n";

let settings: Settings = { ...DEFAULT_SETTINGS };
let version = "";

const root = document.getElementById("settings-root")!;

async function save() {
  await Bridge.saveSettings(settings);
}

/** A colour was picked for a pill's ghost (see ./colors.ts): the island follows. */
function pickColor(next: Record<string, string>) {
  settings.pillColors = next;
  void save();
}

// ── Reusable bits ─────────────────────────────────────────────────────────────

function toggle(on: boolean, onChange: (v: boolean) => void): HTMLElement {
  const el = h("button", { class: on ? "switch on" : "switch", "aria-pressed": on });
  el.addEventListener("click", () => {
    const next = !el.classList.contains("on");
    el.classList.toggle("on", next);
    onChange(next);
  });
  return el;
}

// ── Changes to Claude Code's settings.json ────────────────────────────────────

/** One kind of change to ~/.claude/settings.json, with the words that go with it. */
interface Change {
  preview: (install: boolean) => Promise<HookPreview | null>;
  apply: (install: boolean, fingerprint: string) => Promise<string | null>;
  installText: string;
  removeText: string;
  installButton: string;
  removeButton: string;
  /** The note once written; `backup` is "" when there was no file to back up. */
  done: (backup: string) => string;
}

const HOOKS_CHANGE: Change = {
  preview: Bridge.hooksPreview,
  apply: Bridge.hooksApply,
  get installText() { return t("This is exactly what will change in your settings.json. Your own hooks are left untouched."); },
  get removeText() { return t("This removes Fedo's entries only. Your own hooks are left untouched."); },
  get installButton() { return t("Back up and write"); },
  get removeButton() { return t("Back up and remove"); },
  done: (backup) => backup
    ? t("Done. Previous settings saved as {backup}. Open a new Claude Code session to pick the hooks up.", { backup })
    : t("Done. Open a new Claude Code session to pick the hooks up."),
};

const STATUS_LINE_CHANGE: Change = {
  preview: Bridge.statusLinePreview,
  apply: Bridge.statusLineApply,
  get installText() { return t("This is exactly what will change: only the status line. If you already have one it keeps working, Fedo's relay runs it for you."); },
  get removeText() { return t("This puts your previous status line back, or removes the entry if there was none."); },
  get installButton() { return t("Back up and write"); },
  get removeButton() { return t("Back up and remove"); },
  done: (backup) => backup
    ? t("Done. Previous settings saved as {backup}. The numbers appear after the next reply of a Claude Code session.", { backup })
    : t("Done. The numbers appear after the next reply of a Claude Code session."),
};

/**
 * Shows the diff of a change in `body` and writes it only after an explicit
 * click, and only if settings.json still matches the diff that was shown.
 * `back` redraws the section; `applied` runs a moment after a successful write.
 */
async function reviewChange(
  body: HTMLElement,
  change: Change,
  install: boolean,
  back: () => void,
  applied: () => void,
) {
  let preview;
  try {
    preview = await change.preview(install);
  } catch (err) {
    // An unreadable or invalid settings.json stops here rather than being
    // treated as empty and written over.
    clear(body);
    body.append(
      h("div", { class: "notice err", text: String(err).replace(/^Error:\s*/, "") }),
      h("div", { class: "row" }, h("button", { text: t("Back"), onclick: back })),
    );
    return;
  }
  if (!preview) return;
  clear(body);
  body.append(
    h("div", { class: "hint", text: install ? change.installText : change.removeText }),
    renderDiff(preview.diff),
    h("div", { class: "row" },
      h("span", {
        class: "path",
        text: preview.backup
          ? t("Backup → {path}", { path: preview.backup })
          : t("No settings.json yet — nothing to back up."),
      }),
    ),
  );
  const confirm = h("button", {
    class: install ? "primary" : "danger",
    text: install ? change.installButton : change.removeButton,
  });
  confirm.addEventListener("click", async () => {
    confirm.disabled = true;
    try {
      const backup = await change.apply(install, preview.fingerprint);
      clear(body);
      body.append(h("div", { class: "notice ok", text: change.done(backup ?? "") }));
      window.setTimeout(applied, 2600);
    } catch (err) {
      confirm.disabled = false;
      body.append(h("div", { class: "notice err", text: t("Could not write: {error}", { error: String(err) }) }));
    }
  });
  body.append(h("div", { class: "row" }, confirm, h("button", { text: t("Cancel"), onclick: back })));
}

// ── Claude Code section ───────────────────────────────────────────────────────

function claudeSection(status: HookStatus): HTMLElement {
  const body = h("div", { style: "display:flex;flex-direction:column;gap:12px" });
  const section = h(
    "section",
    {},
    h("h2", {}, statusDot(status.installed), h("span", { text: "Claude Code" })),
    body,
  );

  const redraw = () => {
    clear(body);
    draw();
  };
  const rebuild = async () => {
    const fresh = await Bridge.hooksStatus();
    if (fresh) Object.assign(status, fresh);
    redraw();
    const head = section.querySelector("h2")!;
    clear(head);
    head.append(statusDot(status.installed), h("span", { text: "Claude Code" }));
  };

  function draw() {
    body.append(
      h("div", {
        class: "hint",
        text: status.installed
          ? t("Fedo is hooked into your Claude Code sessions. Tool calls, questions and permission requests show up in the island, and you can answer them there.")
          : t("Install the hooks to see your Claude Code sessions in the island and approve permissions without leaving what you are doing."),
      }),
      h("div", { class: "row" },
        h("label", { text: "settings.json" }),
        h("span", { class: "path", text: status.settingsPath }),
      ),
      h("div", { class: "row" },
        h("label", { text: t("Relay") }),
        h("span", { class: "path", text: status.hookPath }),
        statusDot(status.hookReady),
      ),
    );

    if (!status.hookReady) {
      body.append(h("div", {
        class: "notice warn",
        text: t("fedo-hook is not in place yet. Restart Fedo; if it still fails, build it with `cargo build -p fedo-hook`."),
      }));
    }

    const actions = h("div", { class: "row" });
    const install = h("button", {
      class: "primary",
      text: status.installed ? t("Reinstall hooks…") : t("Install hooks…"),
      onclick: () => void reviewChange(body, HOOKS_CHANGE, true, redraw, () => void rebuild()),
    });
    // Writing hook commands that point at a relay which isn't there would give
    // every Claude Code session a broken hook and nothing to show for it.
    if (!status.hookReady) {
      install.disabled = true;
      install.title = t("The relay isn't installed yet.");
    }
    actions.append(install);
    if (status.installed) {
      actions.append(h("button", {
        class: "danger",
        text: t("Uninstall hooks…"),
        onclick: () => void reviewChange(body, HOOKS_CHANGE, false, redraw, () => void rebuild()),
      }));
    }
    body.append(actions);
  }

  draw();
  return section;
}

// ── Plan usage section ────────────────────────────────────────────────────────

/**
 * The 5-hour and weekly limits in the island's header. They come from Claude
 * Code's status line, so the relay has to be the status line first: turning the
 * switch on without it starts the install, and the switch only stays on once
 * that has been confirmed. A status line the user had keeps working.
 */
const PLAN_SETTINGS_TEXT = {
  get claude() { return t("Shows your Claude plan usage (5-hour and weekly limits) in the island's header. Fedo adds a status line relay in ~/.claude/settings.json. If you already have a status line, it keeps working as before. Pro and Max plans only."); },
  get showClaude() { return t("Show in notch"); },
};

function planSection(status: HookStatus): HTMLElement {
  const body = h("div", { style: "display:flex;flex-direction:column;gap:12px" });
  const section = h("section", {}, h("h2", {}, h("span", { text: t("Plan usage") })), body);

  const redraw = () => {
    clear(body);
    draw();
  };
  const rebuild = async () => {
    const fresh = await Bridge.hooksStatus();
    if (fresh) Object.assign(status, fresh);
    settings.planRelayInstalled = status.planRelayInstalled;
    // Cancelled or failed: a switch that was waiting for the install falls back.
    if (!status.planRelayInstalled) settings.showPlanInNotch = false;
    redraw();
  };

  function draw() {
    // The switch shows "on" while the install it asked for is being reviewed.
    const sw = toggle(settings.showPlanInNotch, (on) => {
      if (!on) {
        settings.showPlanInNotch = false;
        void save();
      } else if (status.planRelayInstalled) {
        settings.showPlanInNotch = true;
        void save();
      } else {
        // Turned on before the relay is in: install it first; it stays on once confirmed.
        void reviewChange(body, STATUS_LINE_CHANGE, true, () => void rebuild(), () => {
          settings.planRelayInstalled = true;
          settings.showPlanInNotch = true;
          void save().then(rebuild);
        });
      }
    });
    body.append(
      h("div", {
        class: "hint",
        text: PLAN_SETTINGS_TEXT.claude,
      }),
      h("div", { class: "row" }, h("label", { text: PLAN_SETTINGS_TEXT.showClaude }), sw),
      h("div", { class: "row" },
        h("label", { text: t("Relay") }),
        statusDot(status.planRelayInstalled),
        h("span", { class: "hint", text: status.planRelayInstalled ? t("installed") : t("not installed") }),
        status.planRelayInstalled
          ? h("button", {
              class: "danger",
              text: t("Uninstall relay…"),
              onclick: () => void reviewChange(body, STATUS_LINE_CHANGE, false, redraw, () => void rebuild()),
            })
          : h("button", {
              class: "primary",
              text: t("Install relay…"),
              onclick: () => void reviewChange(body, STATUS_LINE_CHANGE, true, redraw, () => void rebuild()),
            }),
      ),
    );
  }

  draw();
  return section;
}

// ── Active pills section ──────────────────────────────────────────────────────

/**
 * The tools you use (Mac 0.1.1–0.1.2): pick the main workspace tool, which is
 * always on and takes no slot, and declare the agents and chat providers you
 * want as pills. Services are declared in Integrations below, next to their keys.
 */
function activePillsSection(connected: Record<string, boolean>): HTMLElement {
  const slots = h("div", { class: "hint" });
  const main = h("select", {}) as HTMLSelectElement;
  for (const def of mainPillChoices()) main.append(h("option", { value: def.id, text: def.name }));
  main.addEventListener("change", () => {
    const next = chooseMainPill(settings, main.value);
    if (!next) return;
    settings.mainPill = next.mainPill;
    settings.activeIntegrations = next.activeIntegrations;
    declaredChanged();
  });
  const groups = h("div", { style: "display:flex;flex-direction:column;gap:12px" });

  /** Why a pill would show nothing yet, as on the Mac's row. */
  function hint(def: PillDefinition): string | null {
    if (isComingSoon(def.id)) return t("Coming soon");
    if (def.connect.kind === "hooks" && !connected[def.id]) return t("Hooks not installed");
    if (def.connect.kind === "key" && !connected[def.id]) return t("Key not configured");
    return null;
  }

  function row(def: PillDefinition): HTMLElement {
    const isMain = def.id === settings.mainPill;
    const on = settings.activeIntegrations.includes(def.id);
    const full = !isMain && !on && settings.activeIntegrations.length >= MAX_ACTIVE;
    const el = h("div", { class: full ? "pill-row full" : "pill-row" },
      colorDot(def, "width:10px;height:10px", () => settings.pillColors, pickColor),
      h("span", { class: "name", text: def.name }),
    );
    if (isMain) {
      el.append(h("span", { class: "state", text: t("Main") }));
      return el;
    }
    const why = hint(def);
    el.append(h("span", { class: "state", text: why ?? "" }));
    const sw = h("button", { class: on ? "switch on" : "switch" }) as HTMLButtonElement;
    sw.disabled = full;
    sw.addEventListener("click", () => {
      const next = toggleDeclared(settings, def.id);
      if (!next) return;
      settings.activeIntegrations = next;
      declaredChanged();
    });
    el.append(sw);
    return el;
  }

  function draw() {
    const used = settings.activeIntegrations.length;
    slots.textContent = t("{used}/{max} slots in use — the main tool doesn't take one.", { used, max: MAX_ACTIVE });
    slots.classList.toggle("full", used >= MAX_ACTIVE);
    main.value = settings.mainPill;
    clear(groups);
    for (const cat of PILL_CATEGORIES) {
      if (cat.id === "service") continue;
      const pills = availablePills().filter((p) => p.category === cat.id);
      if (pills.length === 0) continue;
      groups.append(h("div", { class: "pill-group" }, h("h3", { text: t(cat.title) }), ...pills.map(row)));
    }
  }
  declaredViews.push(draw);
  draw();

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: t("Active pills") })),
    h("div", { class: "hint", text: t("Choose the tools you use. Fedo only shows what you declare here.") }),
    slots,
    h("div", { class: "row" }, h("label", { text: t("Main tool") }), main),
    groups,
  );
}

// ── Services section ──────────────────────────────────────────────────────────
//
// Only Spotify is left (FEDO.md Phase 0 removed Stripe, n8n, Vercel, GitHub,
// Notion and Cal.com, all key-based) — read over D-Bus (core/pills.ts), with
// nothing to enter here.

const MAX_ACTIVE = MAX_DECLARED;

/** Everything that shows the declared pills, redrawn when any of them changes. */
const declaredViews: (() => void)[] = [];

function declaredChanged() {
  for (const redraw of declaredViews) redraw();
  void save();
}

function servicesSection(): HTMLElement {
  const note = h("div", { class: "hint" });
  const list = h("div", { style: "display:flex;flex-direction:column;gap:14px" });

  function updateNote() {
    const used = settings.activeIntegrations.length;
    note.textContent = t("Pick up to {max} pills to show next to the ghost — {used}/{max} in use.", { max: MAX_ACTIVE, used });
  }
  declaredViews.push(updateNote);

  const def = { id: "integration_spotify", name: "Spotify", color: "#1DB954" };
  if (availablePills().some((p) => p.id === def.id)) {
    const active = settings.activeIntegrations.includes(def.id);
    const sw = h("button", { class: active ? "switch on" : "switch" });
    sw.addEventListener("click", () => {
      const on = settings.activeIntegrations.includes(def.id);
      if (on) {
        settings.activeIntegrations = settings.activeIntegrations.filter((x) => x !== def.id);
      } else {
        if (settings.activeIntegrations.length >= MAX_ACTIVE) return;
        settings.activeIntegrations = [...settings.activeIntegrations, def.id];
      }
      sw.classList.toggle("on", !on);
      declaredChanged();
    });

    const hint = h("div", { style: "padding-top:5px" });
    void Bridge.spotifyInstalled().then((ok) => {
      hint.textContent = ok === false ? t("Not installed") : "";
    });

    list.append(
      h("div", { style: "display:flex;gap:12px;align-items:flex-start" },
        h("div", { style: "display:flex;align-items:center;gap:8px;min-width:132px;padding-top:4px" },
          sw,
          colorDot(def, "", () => settings.pillColors, pickColor),
          h("span", { style: "font-size:12.5px", text: def.name }),
        ),
        hint,
      ),
    );
  }

  updateNote();
  return h("section", {}, h("h2", {}, h("span", { text: t("Services") })), note, list);
}

// ── General section ───────────────────────────────────────────────────────────

function generalSection(): HTMLElement {
  const volume = h("input", {
    type: "range", min: "0", max: "0.2", step: "0.005",
    value: String(settings.soundVolume),
  }) as HTMLInputElement;
  volume.addEventListener("input", () => {
    settings.soundVolume = Number(volume.value);
    void save();
  });

  // Your own sounds: the folder, a reload, and how many are replaced.
  const customCount = h("span", { class: "hint" });
  const countCustom = () =>
    void Bridge.customSounds(SOUND_NAMES).then((own) => {
      const n = own?.length ?? 0;
      customCount.textContent = n > 0 ? t("{0} custom", { 0: n }) : "";
    });
  countCustom();
  const soundsFolder = h("button", { text: t("Open sounds folder"), onclick: () => void Bridge.revealSoundsFolder() });
  const reloadSounds = h("button", {
    text: t("Reload sounds"),
    onclick: () => void Bridge.reloadSounds().then(() => window.setTimeout(countCustom, 300)),
  });

  const autoClose = h("input", {
    type: "number", min: "5", max: "120", step: "1",
    value: String(Math.round(settings.autoCloseInterval)),
    style: "width:72px",
  }) as HTMLInputElement;
  autoClose.addEventListener("change", () => {
    settings.autoCloseInterval = Math.max(5, Math.min(120, Number(autoClose.value) || 15));
    autoClose.value = String(settings.autoCloseInterval);
    void save();
  });

  const idleFree = h("input", {
    type: "number", min: "0", max: "3600", step: "10",
    value: String(Math.round(settings.webviewIdleSeconds)),
    style: "width:72px",
  }) as HTMLInputElement;
  idleFree.addEventListener("change", () => {
    settings.webviewIdleSeconds = Math.max(0, Math.min(3600, Number(idleFree.value) || 0));
    idleFree.value = String(settings.webviewIdleSeconds);
    void save();
  });

  const screen = h("select", {}) as HTMLSelectElement;
  screen.append(
    h("option", { value: "primary", text: t("Main display") }),
    h("option", { value: "cursor", text: t("Display under the cursor") }),
  );
  screen.value = settings.screen;
  void Bridge.listMonitors().then((list) => {
    for (const m of list ?? []) screen.append(h("option", { value: m.key, text: m.label }));
    // Set again now the option exists. A display saved under an older key (moved,
    // resized, or saved before names were kept) is shown by its place or its
    // name; one that is gone shows as the main one.
    const saved = settings.screen;
    const [place, name] = saved.split("|");
    const keys = (list ?? []).map((m) => m.key);
    screen.value =
      keys.find((k) => k === saved) ??
      (saved.startsWith("at:") ? keys.find((k) => k.split("|")[0] === place) : undefined) ??
      (name ? keys.find((k) => k.split("|")[1] === name) : undefined) ??
      saved;
    if (!screen.value) screen.value = "primary";
  });
  screen.addEventListener("change", () => {
    settings.screen = screen.value;
    void save();
  });

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: t("General") })),
    h("div", { class: "row" },
      h("label", { text: t("Sound") }),
      toggle(settings.soundEnabled, (v) => { settings.soundEnabled = v; void save(); }),
      volume,
    ),
    h("div", { class: "row" }, soundsFolder, reloadSounds, customCount),
    h("div", { class: "hint", text: t("Drop a file named like one of the ghost's sounds (finish.wav, approval.mp3, greet.m4a…) in the sounds folder to replace it, then Reload.") }),
    h("div", { class: "row" },
      h("label", { text: t("Open on hover") }),
      toggle(settings.openOnHover, (v) => { settings.openOnHover = v; void save(); }),
    ),
    h("div", { class: "hint", text: t("Hovering the island opens it; it folds again shortly after the pointer leaves. Click inside to keep it open.") }),
    h("div", { class: "row" },
      h("label", { text: t("Auto-close") }),
      autoClose,
      h("span", { class: "hint", text: t("seconds after you leave the island") }),
    ),
    h("div", { class: "row" },
      h("label", { text: t("Free memory after") }),
      idleFree,
      h("span", { class: "hint", text: t("seconds hidden and idle (0 keeps the island loaded)") }),
    ),
    h("div", { class: "row" },
      h("label", { text: t("Island lives on") }),
      screen,
    ),
    h("div", { class: "row" },
      h("label", { text: t("Launch at startup") }),
      toggle(settings.autostart, (v) => { settings.autostart = v; void save(); }),
    ),
    ...recapRows(),
    languageRow(),
  );
}

/**
 * Settings → General → Language, as on the Mac: "System" follows the
 * system's language when Fedo has it (else English), or one of the ten.
 * Both windows and the tray switch in place, without a restart.
 */
function languageRow(): HTMLElement {
  const select = h("select", {}) as HTMLSelectElement;
  select.append(h("option", { value: "", text: t("System") }));
  for (const { code, name } of LANGUAGES) select.append(h("option", { value: code, text: name, lang: code }));
  select.value = LANGUAGES.some((l) => l.code === settings.language) ? settings.language : "";
  select.addEventListener("change", () => {
    settings.language = select.value;
    void save();
    applyLanguage();
  });
  return h("div", { class: "row" }, h("label", { text: t("Language") }), select);
}

// ── Shortcuts section ─────────────────────────────────────────────────────────

const SHORTCUTS_UI = {
  get title() { return t("Shortcuts"); },
  get hint() { return t("Work from any app. Click a shortcut to change it, then press the new keys — Esc cancels, Backspace removes it."); },
  get global() { return t("From anywhere"); },
  get island() { return t("In the open island"); },
  get recording() { return t("Press keys…"); },
  get none() { return t("None"); },
  get reset() { return t("Reset to defaults"); },
  get inUse() { return t("In use by another app"); },
  get duplicate() { return t("Used twice"); },
  get invalid() { return t("Not a valid shortcut"); },
  get unavailable() { return t("Not available"); },
  types: (ch: string) => t("Types “{char}”", { char: ch }),
  typesNote: (keys: string, ch: string) =>
    t("{keys} types “{char}” on your keyboard, so it can't be a shortcut. Pick another key.", { keys, char: ch }),
  get needsModifier() { return t("Hold Ctrl, Alt or the Windows key with it."); },
  get unsupportedKey() { return t("That key can't be used in a shortcut."); },
  get wayland() {
    return t("Your Wayland desktop doesn't let apps listen for keys outside their own windows. Add the shortcuts in your system's keyboard settings instead, with these commands:");
  },
  get noDisplay() { return t("No display server was found, so global shortcuts are off."); },
  get portalPending() {
    return t("Asking your desktop to register the shortcuts. It may show its own window to confirm them.");
  },
  get portalActive() {
    return t("These shortcuts are registered with your desktop. It may ask you to confirm them or to pick other keys; when it says which keys it uses, they show next to each shortcut.");
  },
  get portalFallback() {
    return t("If one doesn't work, you can also add it in your system's keyboard settings with these commands:");
  },
  desktopKeys: (keys: string) => t("Desktop: {keys}", { keys }),
  get refused() { return t("Not set by your desktop"); },
};

function shortcutsSection(initial: ShortcutsReport | null): HTMLElement {
  let report = initial;
  const list = h("div", { class: "shortcut-list" });
  const feedback = h("div", {});
  const blockedNote = h("div", {});

  let stopRecording: (() => void) | null = null;

  function store(id: string, binding: Binding) {
    settings.shortcuts = { ...settings.shortcuts, [id]: binding };
    void save();
  }

  function tagFor(id: string, dups: Set<string>): HTMLElement | null {
    if (dups.has(id)) return h("span", { class: "tag err", text: SHORTCUTS_UI.duplicate });
    const st = report?.actions.find((a) => a.id === id);
    switch (st?.status) {
      case "inUse": return h("span", { class: "tag warn", text: SHORTCUTS_UI.inUse });
      case "duplicate": return h("span", { class: "tag err", text: SHORTCUTS_UI.duplicate });
      case "invalid": return h("span", { class: "tag err", text: SHORTCUTS_UI.invalid });
      case "typesCharacter": return h("span", { class: "tag warn", text: SHORTCUTS_UI.types(st.typed ?? "?") });
      case "unsupported": return h("span", { class: "tag", text: SHORTCUTS_UI.unavailable });
      case "refused": return h("span", { class: "tag warn", text: SHORTCUTS_UI.refused });
      // Wayland: the desktop may run it on other keys than the ones asked for.
      case "active": return st.trigger ? h("span", { class: "tag", text: SHORTCUTS_UI.desktopKeys(st.trigger) }) : null;
      default: return null;
    }
  }

  function record(id: string, binding: Binding, button: HTMLButtonElement) {
    stopRecording?.();
    clear(feedback);
    button.classList.add("recording");
    button.textContent = SHORTCUTS_UI.recording;
    void Bridge.shortcutsSuspend(true);

    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const result = recordPress(e);
      switch (result.kind) {
        case "pending":
          return;
        case "keys":
          finish();
          store(id, { keys: result.keys, enabled: true });
          return;
        case "clear":
          finish();
          store(id, { keys: "", enabled: binding.enabled });
          return;
        case "typesCharacter":
          finish();
          feedback.append(h("div", {
            class: "notice warn",
            text: SHORTCUTS_UI.typesNote(displayKeys(result.keys), result.typed),
          }));
          return;
        case "needsModifier":
          feedback.replaceChildren(h("div", { class: "notice warn", text: SHORTCUTS_UI.needsModifier }));
          return;
        case "unsupported":
          feedback.replaceChildren(h("div", { class: "notice warn", text: SHORTCUTS_UI.unsupportedKey }));
          return;
        case "cancel":
          finish();
          return;
      }
    };
    const onBlur = () => finish();

    function finish() {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("blur", onBlur);
      stopRecording = null;
      // Takes the global shortcuts back, from what is saved by now.
      void Bridge.shortcutsSuspend(false);
      draw();
    }
    stopRecording = finish;
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("blur", onBlur);
  }

  function draw() {
    clear(list);
    const dups = duplicates(activeKeys(settings.shortcuts));
    for (const d of SHORTCUTS) {
      if (!d.ported) continue;
      const binding = effective(d, settings.shortcuts);
      const keycap = h("button", {
        class: "keycap",
        text: binding.keys ? displayKeys(binding.keys) : SHORTCUTS_UI.none,
      }) as HTMLButtonElement;
      keycap.disabled = !binding.enabled;
      keycap.addEventListener("click", () => record(d.id, binding, keycap));
      const sw = toggle(binding.enabled, (on) => store(d.id, { keys: binding.keys, enabled: on }));
      const tag = binding.enabled ? tagFor(d.id, dups) : null;
      list.append(h("div", { class: binding.enabled ? "row shortcut" : "row shortcut off" },
        sw,
        h("span", { class: "shortcut-name", text: t(SHORTCUT_TEXT[d.id]) }),
        ...(tag ? [tag] : []),
        keycap,
      ));
    }

    clear(blockedNote);
    const commands = (command: string) => {
      const list = h("div", { class: "diff" });
      for (const d of SHORTCUTS) {
        if (d.ported) list.append(h("div", { class: "ctx", text: `${command} ${d.id}` }));
      }
      return list;
    };
    if (report?.portal) {
      // Wayland, through the desktop's GlobalShortcuts portal; the commands
      // stay as a way around a shortcut the desktop didn't take.
      const active = report.portal === "active";
      blockedNote.append(
        h("div", {
          class: active ? "notice ok" : "notice",
          text: active ? SHORTCUTS_UI.portalActive : SHORTCUTS_UI.portalPending,
        }),
        h("div", { class: "hint", text: SHORTCUTS_UI.portalFallback }),
        commands(report.command),
      );
    } else if (report?.blocked === "wayland") {
      blockedNote.append(h("div", { class: "notice warn", text: SHORTCUTS_UI.wayland }), commands(report.command));
    } else if (report?.blocked) {
      blockedNote.append(h("div", { class: "notice warn", text: SHORTCUTS_UI.noDisplay }));
    }
  }

  const islandList = h("div", { class: "shortcut-list" });
  for (const row of ISLAND_SHORTCUTS) {
    islandList.append(h("div", { class: "row shortcut" },
      h("span", { class: "shortcut-name", text: t(row.description) }),
      h("span", { class: "keycap static", text: row.keys }),
    ));
  }

  const reset = h("button", {
    text: SHORTCUTS_UI.reset,
    onclick: () => {
      stopRecording?.();
      settings.shortcuts = {};
      clear(feedback);
      void save();
      draw();
    },
  });

  // The events are listened to once (see main); only the section on screen redraws.
  shortcutsListener = {
    report(fresh) {
      report = fresh;
      if (!stopRecording) draw();
    },
    settingsChanged() {
      if (!stopRecording) draw();
    },
  };

  draw();
  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: SHORTCUTS_UI.title })),
    h("div", { class: "hint", text: SHORTCUTS_UI.hint }),
    h("div", { class: "subhead", text: SHORTCUTS_UI.global }),
    list,
    blockedNote,
    feedback,
    h("div", { class: "row" }, reset),
    h("div", { class: "subhead", text: SHORTCUTS_UI.island }),
    islandList,
  );
}

/** Settings → General → Weekly recap. The prefs live with the history in Rust. */
function recapRows(): HTMLElement[] {
  const T = {
    label: t("Weekly recap"),
    keep: t("Keep a history of my coding sessions"),
    clear: t("Clear history"),
    cleared: t("History cleared."),
    about: t("Counts and project names only — never commands, files or prompts. Kept on this computer for 12 weeks."),
  };
  const feedback = h("span", { class: "hint" });
  const sw = toggle(true, (v) => { void Bridge.recapSetEnabled(v); });
  void Bridge.recapPrefs().then((prefs) => {
    if (prefs) sw.classList.toggle("on", prefs.enabled);
  });
  const clearBtn = h("button", {
    class: "danger",
    text: T.clear,
    onclick: async () => {
      clearBtn.disabled = true;
      await Bridge.recapClear();
      feedback.textContent = T.cleared;
      window.setTimeout(() => {
        clearBtn.disabled = false;
        feedback.textContent = "";
      }, 2400);
    },
  }) as HTMLButtonElement;
  return [
    h("div", { class: "row" },
      h("label", { text: T.label }),
      sw,
      h("span", { class: "hint", text: T.keep }),
    ),
    h("div", { class: "row" },
      h("label", {}),
      clearBtn,
      feedback,
    ),
    h("div", { class: "row" },
      h("label", {}),
      h("span", { class: "hint", style: "flex:1 1 0;min-width:0", text: T.about }),
    ),
  ];
}

// ── Language ──────────────────────────────────────────────────────────────────

/** The shortcuts section on screen, told about the events listened to once in main. */
let shortcutsListener: { report(fresh: ShortcutsReport): void; settingsChanged(): void } | null = null;

/**
 * Shows the language Settings asks for. A change redraws the window in place,
 * where it was scrolled to: nothing reloads, nothing is written.
 */
function applyLanguage() {
  setLanguage(resolveLanguage(settings.language, systemLanguages()));
}

function applyDirection() {
  document.documentElement.dir = isRtl() ? "rtl" : "ltr";
  document.title = t("Settings — Fedo");
}

let rendering: Promise<void> | null = null;
let renderAgain = false;

/** Redraws every section from fresh state, keeping the scroll position. */
async function rerender() {
  if (rendering) {
    renderAgain = true;
    return;
  }
  const scroll = document.scrollingElement?.scrollTop ?? 0;
  rendering = render();
  try {
    await rendering;
  } finally {
    rendering = null;
  }
  if (document.scrollingElement) document.scrollingElement.scrollTop = scroll;
  if (renderAgain) {
    renderAgain = false;
    await rerender();
  }
}

// ── Boot ──────────────────────────────────────────────────────────────────────

async function main() {
  const boot = await Bridge.boot();
  if (boot) {
    settings = { ...settings, ...boot.settings };
    version = boot.version;
  }
  setLanguage(resolveLanguage(settings.language, systemLanguages()));
  applyDirection();
  onLanguageChange(() => {
    applyDirection();
    void rerender();
  });
  await render();

  void onEvent<ShortcutsReport>("shortcuts-status", (fresh) => shortcutsListener?.report(fresh));
  void onEvent<Settings>("settings-changed", (s) => {
    settings = { ...settings, ...s };
    shortcutsListener?.settingsChanged();
    for (const redraw of declaredViews) redraw();
    applyLanguage();
  });
}

/** Reads what the sections show and draws them all. */
async function render() {
  const status = (await Bridge.hooksStatus()) ?? {
    installed: false, planRelayInstalled: false, settingsPath: "", hookPath: "", hookReady: false,
  };
  const agents = await Bridge.agentHooksList();
  const shortcutReport = await Bridge.shortcutsStatus();

  // A main pill this build can run, and no pill declared twice.
  settings = { ...settings, ...sanitizeDeclared(settings) };
  const connected: Record<string, boolean> = { ...((await Bridge.agentHooksStatus()) ?? {}) };

  declaredViews.length = 0;
  shortcutsListener = null;
  clear(root);
  root.append(
    h("h1", {}, h("span", { text: "Fedo" }), h("span", { class: "version", text: version })),
    claudeSection(status),
    agentsSection(agents),
    planSection(status),
    activePillsSection(connected),
    servicesSection(),
    generalSection(),
    shortcutsSection(shortcutReport),
    h("div", {
      class: "hint",
      text: t("No telemetry. Network requests only go to the services you configure yourself."),
    }),
  );
}

void main();
