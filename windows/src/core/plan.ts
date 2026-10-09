// Claude plan usage gauge logic, with no DOM so it can be tested on its own.
// The pill and card that show it are in views/usage.ts.
//
// The numbers come from Claude Code's own status line (`rate_limits` in what
// it hands its status line command), through the relay. Pro and Max only.
// Nothing is read from any credentials, nothing is fetched by Fedo itself.
// Codex plan usage is gone with Codex (FEDO.md Phase 0).

export interface PlanWindow {
  /** 0–100, clamped. */
  usedPct: number;
  /** Epoch milliseconds. */
  resetsAt: number;
}

export interface PlanUsage {
  fiveHour?: PlanWindow;
  sevenDay?: PlanWindow;
  /** When the numbers arrived (epoch ms). */
  updatedAt: number;
}

import { t, weekdayShort } from "../i18n/i18n";

const DAY_MS = 86_400_000;

/** Every user-visible string of the gauge, in the current language (src/i18n). */
export const PLAN_TEXT = {
  get claudeTitle() { return t("Claude plan"); },
  get claudePillTitle() { return t("Claude plan usage"); },
  get waiting() { return t("Waiting for a response from Claude Code"); },
  get justNow() { return t("just now"); },
  minAgo: (n: number) => t("{n} min ago", { n }),
  hAgo: (n: number) => t("{n} h ago", { n }),
  get fiveHours() { return t("5 hours"); },
  get week() { return t("Week"); },
  get resets() { return t("Resets"); },
  get resetting() { return t("Resetting…"); },
  inHM: (h: number, m: number) => t("in {h} h {m}", { h, m }),
  inM: (m: number) => t("in {m} min", { m }),
  none: "—",
};

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const asObj = (v: unknown): Record<string, unknown> | null =>
  v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;

// ── Claude ────────────────────────────────────────────────────────────────────

function parseClaudeWindow(raw: unknown, now: number): PlanWindow | undefined {
  const w = asObj(raw);
  const pct = w?.used_percentage;
  const epoch = w?.resets_at;
  if (!isNum(pct) || !isNum(epoch)) return undefined;
  // Anything outside 0–200 is not a percentage; 100–200 is a plan over its limit, shown full.
  if (pct < 0 || pct > 200) return undefined;
  // resets_at is epoch seconds. A date more than 400 days away is milliseconds in disguise.
  if (epoch <= 0 || epoch * 1000 > now + 400 * DAY_MS) return undefined;
  return { usedPct: Math.min(100, pct), resetsAt: epoch * 1000 };
}

/** ClaudePlanGauge.parse: `rate_limits` of a status line call, or null when it holds neither window. */
export function parseClaudePlan(rateLimits: unknown, now = Date.now()): PlanUsage | null {
  const rl = asObj(rateLimits);
  const fiveHour = parseClaudeWindow(rl?.five_hour, now);
  const sevenDay = parseClaudeWindow(rl?.seven_day, now);
  if (!fiveHour && !sevenDay) return null;
  const usage: PlanUsage = { updatedAt: now };
  if (fiveHour) usage.fiveHour = fiveHour;
  if (sevenDay) usage.sevenDay = sevenDay;
  return usage;
}

// ── Shared ────────────────────────────────────────────────────────────────────

/** What to show for a window: 0 once its reset time has passed. */
export const effectivePct = (w: PlanWindow, now = Date.now()): number => (w.resetsAt <= now ? 0 : w.usedPct);

/** The higher of the two effective percentages; null when there are no windows. */
export function dominantPct(u: PlanUsage | null | undefined, now = Date.now()): number | null {
  const pcts = [u?.fiveHour, u?.sevenDay].filter((w): w is PlanWindow => !!w).map((w) => effectivePct(w, now));
  return pcts.length ? Math.max(...pcts) : null;
}

/** Green below 50 %, orange up to 80 %, red above, grey without data. */
export function planColor(pct: number | null): string {
  if (pct == null) return "#6B7079";
  if (pct < 50) return "#22C55E";
  if (pct < 80) return "#F59E0B";
  return "#F4505E";
}

/** "Claude 73%" on the pill; "Claude —" while there are no numbers. */
export function pillLabel(name: "Claude", u: PlanUsage | null | undefined, now = Date.now()): string {
  const pct = dominantPct(u, now);
  return pct == null ? `${name} ${PLAN_TEXT.none}` : `${name} ${Math.round(pct)}%`;
}

/** "just now", "12 min ago", "3 h ago". */
export function ageLabel(updatedAt: number, now = Date.now()): string {
  const secs = (now - updatedAt) / 1000;
  if (secs < 60) return PLAN_TEXT.justNow;
  const mins = Math.floor(secs / 60);
  return mins < 60 ? PLAN_TEXT.minAgo(mins) : PLAN_TEXT.hAgo(Math.floor(mins / 60));
}

/** "in 1 h 20" / "in 5 min" for the 5-hour window, "Mon 9:00" for the week. */
export function resetLabel(w: PlanWindow, weekly: boolean, now = Date.now()): string {
  const secs = (w.resetsAt - now) / 1000;
  if (secs <= 0) return PLAN_TEXT.resetting;
  if (weekly) {
    const d = new Date(w.resetsAt);
    return `${weekdayShort(d.getDay())} ${d.getHours()}:${String(d.getMinutes()).padStart(2, "0")}`;
  }
  const hours = Math.floor(secs / 3600);
  const mins = Math.floor((secs % 3600) / 60);
  return hours > 0 ? PLAN_TEXT.inHM(hours, mins) : PLAN_TEXT.inM(mins);
}

/** The Claude card's subtitle. */
export function claudeSubtitle(u: PlanUsage | null, now = Date.now()): string {
  return u ? ageLabel(u.updatedAt, now) : PLAN_TEXT.waiting;
}

/** A stored Claude usage, if it still looks like one (the last numbers survive a restart). */
export function restorePlanUsage(raw: string | null): PlanUsage | null {
  if (!raw) return null;
  try {
    const v = asObj(JSON.parse(raw));
    if (!v || !isNum(v.updatedAt)) return null;
    const win = (w: unknown): PlanWindow | undefined => {
      const o = asObj(w);
      return o && isNum(o.usedPct) && isNum(o.resetsAt) && o.usedPct >= 0 && o.usedPct <= 100
        ? { usedPct: o.usedPct, resetsAt: o.resetsAt }
        : undefined;
    };
    const usage: PlanUsage = { updatedAt: v.updatedAt };
    const fh = win(v.fiveHour);
    const sd = win(v.sevenDay);
    if (fh) usage.fiveHour = fh;
    if (sd) usage.sevenDay = sd;
    return fh || sd ? usage : null;
  } catch {
    return null;
  }
}
