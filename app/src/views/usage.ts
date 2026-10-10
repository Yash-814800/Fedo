// Claude plan usage in the island: the pill in the header and the card behind
// it. The numbers and labels come from core/plan.ts.
//
// The pill sits in the header's right side, before the gear, on the overview
// only. Clicking it puts its card in place of the overview's left card
// (clicking it again closes it), and the ghost wears the plan's colour while
// it is open. It closes when the view, the mode or the focused pill changes.
//
// Codex plan usage is gone with Codex (FEDO.md Phase 0: Claude Code and
// Antigravity are the only two agents this fork keeps).

import {
  PLAN_TEXT, claudeSubtitle, dominantPct, effectivePct, pillLabel, planColor, resetLabel,
  type PlanUsage, type PlanWindow,
} from "../core/plan";
import { State } from "../core/state";
import { clear, dot, h, svg } from "./dom";
import { ICONS } from "./icons";
import { language, tl } from "../i18n/i18n";

/** The Claude pill is in the header: overview, turned on, relay in. */
export function claudePillVisible(): boolean {
  const s = State.settings;
  return State.view === "overview" && s.showPlanInNotch && (s.planRelayInstalled || State.planUsage != null);
}

/** A plan card is open and its pill is still there. */
export function planCardOpen(): boolean {
  return State.showingPlanDetail && claudePillVisible();
}

const claudeColor = (now = Date.now()) => planColor(dominantPct(State.planUsage, now));

/** The colour of the open card's plan, which the ghost wears while it is open. */
export function openPlanColor(): string {
  return claudeColor();
}

/** Closes the plan card (view, mode or focus changed). */
export function closePlanCard(): void {
  State.showingPlanDetail = false;
}

// ── Claude numbers ────────────────────────────────────────────────────────────

const STORE_KEY = "fedo.claudePlanUsage";

/** New numbers from the status line; kept so they survive a restart, as on the Mac. */
export function setClaudePlanUsage(usage: PlanUsage): void {
  const prev = State.planUsage;
  const same = JSON.stringify([prev?.fiveHour, prev?.sevenDay]) === JSON.stringify([usage.fiveHour, usage.sevenDay]);
  State.planUsage = usage;
  if (!same) {
    try {
      window.localStorage?.setItem(STORE_KEY, JSON.stringify(usage));
    } catch {
      // Storage off or full: the numbers just will not outlive this run.
    }
  }
  // The relay calls in with every Claude Code update: a hidden island is only
  // woken when the numbers actually moved.
  if (!same || State.mode === "expanded") State.notify();
}

/** The last numbers seen, if any were kept. */
export function storedClaudePlanUsage(): string | null {
  try {
    return window.localStorage?.getItem(STORE_KEY) ?? null;
  } catch {
    return null;
  }
}

// ── Pill ──────────────────────────────────────────────────────────────────────

export interface PlanPill {
  el: HTMLElement;
  sync(): void;
}

/** The Claude plan pill: colour dot and label, lit while hovered or while its card is open. */
export function buildPlanPill(): PlanPill {
  const label = h("span", { class: "plan-pill-label" });
  const dotEl = h("i", { class: "plan-pill-dot" });
  const el = h("button", {
    class: "plan-pill",
    title: tl("Claude plan usage"),
    onclick: () => {
      const open = State.showingPlanDetail;
      State.showingPlanDetail = !open;
      State.notify();
    },
  }, dotEl, label);
  return {
    el,
    sync() {
      const color = claudeColor();
      el.style.setProperty("--plan", color);
      el.classList.toggle("active", State.showingPlanDetail);
      dotEl.style.background = color;
      label.textContent = pillLabel("Claude", State.planUsage);
    },
  };
}

// ── Card ──────────────────────────────────────────────────────────────────────

function gaugeRow(label: string, w: PlanWindow | undefined, weekly: boolean, now: number): HTMLElement {
  const row = h("div", { class: "plan-row" }, h("span", { class: "plan-label", text: label }));
  if (!w) {
    row.append(h("span", { class: "plan-none", text: PLAN_TEXT.none }));
    return row;
  }
  const pct = effectivePct(w, now);
  const fill = h("i", { class: "plan-fill" });
  fill.style.width = `${pct}%`;
  fill.style.background = planColor(pct);
  row.append(
    h("span", { class: "plan-bar" }, fill),
    h("span", { class: "plan-pct", text: `${Math.round(pct)}%` }),
    h("span", { class: "plan-reset-icon" }, svg(ICONS.arrowClockwise, 8, { stroke: 2.6 })),
    h("span", { class: "plan-reset", text: resetLabel(w, weekly, now) }),
  );
  return row;
}

function head(color: string, title: string, subtitle: string): HTMLElement {
  return h("div", { class: "plan-head" },
    dot(color, 7),
    h("span", { class: "plan-title", text: title }),
    h("span", { class: "plan-sub", text: subtitle }),
  );
}

/** The card that stands in for the overview's left card while the pill is open. */
export class PlanCard {
  readonly el = h("div", { class: "plan-card" });
  private key = "";

  /** Re-renders when the numbers change, or every 30 s for the countdowns. */
  sync(now = Date.now()) {
    const u = State.planUsage;
    const key = `${language()}|${JSON.stringify(u)}|${Math.floor(now / 30_000)}`;
    if (key === this.key) return;
    this.key = key;
    clear(this.el);
    this.el.append(
      head(claudeColor(now), PLAN_TEXT.claudeTitle, claudeSubtitle(u, now)),
      h("div", { class: "plan-rows" },
        gaugeRow(PLAN_TEXT.fiveHours, u?.fiveHour, false, now),
        gaugeRow(PLAN_TEXT.week, u?.sevenDay, true, now),
      ),
    );
  }
}
