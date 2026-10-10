// Timers, reminders, deadlines and events (FEDO.md Phase 5): the list view and
// the header's countdown pill. The list itself lives in Rust (scheduler.rs,
// persisted); State.schedule is its mirror, refreshed on `schedule-changed`.
// Countdowns tick once a second and only while something is on screen.

import { h, clear } from "./dom";
import { Bridge, type ScheduleItem } from "../core/bridge";
import { State } from "../core/state";
import type { ViewActions, ViewHost } from "./views";

const nowS = () => Math.floor(Date.now() / 1000);

/** "24:31", "1:02:03". */
export function mmss(total: number): string {
  const s = Math.max(0, Math.round(total));
  const hh = Math.floor(s / 3600);
  const mm = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  const p = (n: number) => String(n).padStart(2, "0");
  return hh > 0 ? `${hh}:${p(mm)}:${p(ss)}` : `${p(mm)}:${p(ss)}`;
}

/** "in 10 d", "in 3 h", "today", "overdue". */
export function countdown(item: ScheduleItem, now = nowS()): string {
  const d = item.fireAt - now;
  if (item.allDay) {
    // Calendar days, not 24 h blocks: due Tuesday is "in 10 d" from Saturday.
    const midnight = (t: number) => {
      const x = new Date(t * 1000);
      return new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    };
    const days = Math.round((midnight(item.fireAt) - midnight(now)) / 86_400_000);
    if (days === 0) return "today";
    if (days < 0) return "overdue";
    return days === 1 ? "tomorrow" : `in ${days} d`;
  }
  if (d < 0) return "now";
  if (d < 3600) return `in ${Math.max(1, Math.round(d / 60))} min`;
  if (d < 86_400) return `in ${Math.round(d / 3600)} h`;
  return `in ${Math.round(d / 86_400)} d`;
}

function remaining(t: ScheduleItem, now = nowS()): number {
  return t.pausedLeft ?? Math.max(0, t.fireAt - now);
}

export function refreshSchedule(): void {
  void Bridge.scheduleList().then((items) => {
    if (!items) return;
    State.schedule = items;
    State.notify();
  });
}

/** The pill in the header: nearest timer, counting down. Hidden with none. */
export function buildTimePill(open: () => void) {
  const label = h("span", {});
  const el = h("button", { class: "plan-pill time-pill", title: "Timers & deadlines", onclick: open }, "⏱ ", label);
  el.style.display = "none";
  let timer: number | null = null;

  const update = () => {
    const t = State.schedule.filter((i) => i.kind === "timer").sort((a, b) => remaining(a) - remaining(b))[0];
    if (!t) {
      el.style.display = "none";
      return;
    }
    el.style.display = "";
    label.textContent = (t.pausedLeft != null ? "⏸ " : "") + mmss(remaining(t));
  };
  const stop = () => {
    if (timer != null) window.clearInterval(timer);
    timer = null;
  };
  return {
    el,
    sync() {
      update();
      const running = State.schedule.some((i) => i.kind === "timer" && i.pausedLeft == null);
      if (running && State.mode !== "hidden" && timer == null) {
        timer = window.setInterval(() => {
          if (State.mode === "hidden") return stop();
          update();
        }, 1000);
      } else if ((!running || State.mode === "hidden") && timer != null) {
        stop();
      }
    },
  };
}

export function buildSchedule(_actions: ViewActions): ViewHost {
  const body = h("div", { class: "sched-body" });
  const el = h("div", { class: "view" }, h("div", { class: "card sched-card" }, body));
  let key = "";
  let lastSecond = -1;
  const live: { id: string; node: HTMLElement }[] = [];

  const sectionTitle = (t: string) => h("div", { class: "sched-title", text: t });
  const when = (i: ScheduleItem) =>
    new Date(i.fireAt * 1000).toLocaleString(undefined, {
      weekday: "short", day: "numeric", month: "short",
      ...(i.allDay ? {} : { hour: "numeric", minute: "2-digit" }),
    });
  const iconBtn = (label: string, title: string, on: () => void) =>
    h("button", { class: "sched-btn", title, onclick: on }, label);

  function render() {
    clear(body);
    live.length = 0;
    const items = State.schedule;
    const timers = items.filter((i) => i.kind === "timer");
    const deadlines = items.filter((i) => i.kind === "deadline").sort((a, b) => a.fireAt - b.fireAt);
    const events = items.filter((i) => i.kind === "event").sort((a, b) => a.fireAt - b.fireAt);
    const reminders = items
      .filter((i) => i.kind === "reminder" && !i.parent)
      .sort((a, b) => a.fireAt - b.fireAt);

    if (timers.length) {
      body.append(sectionTitle("Timers"));
      for (const t of timers) {
        const t$ = h("b", { class: "sched-time" });
        live.push({ id: t.id, node: t$ });
        body.append(
          h(
            "div",
            { class: "sched-row" },
            t$,
            h("span", { class: "grow", text: t.title }),
            iconBtn(t.pausedLeft != null ? "▶" : "⏸", t.pausedLeft != null ? "Resume" : "Pause", () => {
              void (t.pausedLeft != null ? Bridge.scheduleResume(t.id) : Bridge.schedulePause(t.id)).then(refreshSchedule);
            }),
            iconBtn("✕", "Cancel", () => void Bridge.scheduleCancel(t.id).then(refreshSchedule)),
          ),
        );
      }
    }
    const list = (title: string, rows: ScheduleItem[]) => {
      if (!rows.length) return;
      body.append(sectionTitle(title));
      for (const i of rows) {
        body.append(
          h(
            "div",
            { class: "sched-row" },
            h("b", { class: "sched-count", text: countdown(i) }),
            h("span", { class: "grow", text: i.title }),
            h("span", { class: "sched-when", text: when(i) }),
            iconBtn("✕", "Remove from Fedo's list (the calendar entry stays)", () =>
              void Bridge.scheduleCancel(i.id).then(refreshSchedule)),
          ),
        );
      }
    };
    list("Deadlines", deadlines);
    list("Reminders", reminders);
    list("Coming up", events);
    if (!body.children.length) {
      body.append(h("div", { class: "sched-empty", text: "Nothing scheduled. Try “timer 25 min” or “apply to Zomato before Oct 20”." }));
    }
    tickText();
  }

  function tickText() {
    const n = nowS();
    for (const { id, node } of live) {
      const t = State.schedule.find((i) => i.id === id);
      if (t) node.textContent = (t.pausedLeft != null ? "⏸ " : "") + mmss(remaining(t, n));
    }
  }

  return {
    el,
    sync() {
      const k = JSON.stringify(State.schedule);
      if (k !== key) {
        key = k;
        render();
      }
    },
    tick(nowMs) {
      const sec = Math.floor(nowMs / 1000);
      if (sec !== lastSecond) {
        lastSecond = sec;
        tickText();
      }
      return live.length > 0;
    },
  };
}
