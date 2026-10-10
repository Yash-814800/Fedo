// The hidden debug menu (FEDO.md Phase 2 acceptance): every ghost state, emote
// and move, and every sound, reachable by hand. Open it with Shift+click on the
// ghost in the open island, or Ctrl+Alt+Shift+D while the island has the
// keyboard; either again closes it.
//
// It is plain DOM built on demand and removed on close, so it costs nothing
// while shut. Plain English, deliberately outside the i18n tables.

import type { BotEmoteName, BotStateName } from "../core/layout";
import { Sound, SOUND_NAMES } from "../core/sound";
import { BOT_STATES, STATE_SOUND, type BotEngine } from "./engine";
import { h } from "../views/dom";

export interface DebugHost {
  engine: BotEngine;
  /** Forces a state (null = back to what the sessions say). */
  forceState(state: BotStateName | null): void;
  /** Forces dancing on/off (null = back to the Spotify rule). */
  forceDance(on: boolean | null): void;
  /** Keeps the island from folding away while the menu is up. */
  pin(on: boolean): void;
  /** The island's own three-pokes-make-it-dizzy emote path. */
  emote(name: BotEmoteName): void;
}

const EMOTES: BotEmoteName[] = ["love", "surprised", "proud", "wink", "yawn", "happy", "annoyed"];

export class DebugMenu {
  private el: HTMLElement | null = null;
  private dancing: boolean | null = null;

  constructor(private host: DebugHost, private parent: HTMLElement) {}

  get isOpen(): boolean {
    return this.el != null;
  }

  toggle() {
    if (this.el) this.close();
    else this.open();
  }

  close() {
    this.el?.remove();
    this.el = null;
    this.host.forceState(null);
    this.host.forceDance(null);
    this.dancing = null;
    this.host.pin(false);
  }

  private open() {
    const e = this.host.engine;
    const btn = (label: string, on: () => void, title = "") =>
      h("button", { class: "dbg-btn", title, onClick: (ev: Event) => { ev.stopPropagation(); Sound.resume(); on(); } }, label);
    const group = (title: string, ...kids: HTMLElement[]) =>
      h("div", { class: "dbg-group" }, h("div", { class: "dbg-title" }, title), h("div", { class: "dbg-row" }, ...kids));

    const states = (Object.keys(BOT_STATES) as BotStateName[]).map((s) =>
      btn(s, () => {
        this.host.forceState(s === "idle" ? null : s);
        const snd = STATE_SOUND[s];
        if (snd) Sound.play(snd);
      }),
    );
    const emotes = EMOTES.map((n) => btn(n, () => this.host.emote(n)));
    const moves = [
      btn("poke", () => e.slap(), "squish; three quick ones make it dizzy"),
      btn("loop-de-loop", () => e.loopDeLoop()),
      btn("fade in", () => e.appear()),
      btn("wisp away", () => e.vanish()),
      btn("go idle-faint", () => e.restIdle(), "skips the 6 s wait before it fades"),
      btn("dance", () => {
        this.dancing = this.dancing ? null : true;
        this.host.forceDance(this.dancing);
      }, "toggle"),
    ];
    const sounds = SOUND_NAMES.map((n) => btn(n, () => Sound.play(n)));

    this.el = h(
      "div",
      { id: "debug-menu" },
      h(
        "div",
        { class: "dbg-head" },
        h("span", {}, "ghost debug"),
        btn("close", () => this.close()),
      ),
      group("states", ...states),
      group("emotes", ...emotes),
      group("moves", ...moves),
      group("sounds", ...sounds),
    );
    // Keys typed here are not the island's (Esc would fold it away).
    this.el.addEventListener("mousedown", (ev) => ev.stopPropagation());
    this.parent.append(this.el);
    this.host.pin(true);
  }
}
