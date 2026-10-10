// The command card (FEDO.md Phase 4): what Fedo's assistant wants to run, in
// full, before it does anything. Enter = run, Esc = cancel, E = edit. Nothing
// here runs by itself; every path out of this view is a button or a key.
//
// One view, several shapes, chosen by State.assist (set by the chat's router):
//   card     the command, why, risk colour, warnings, details (processes, paths)
//   choices  two or three alternatives (dnf or Flathub; which file)
//   prompt   a rewritten prompt, with Copy and Jump to terminal
//   output   what an inline run printed
// Plain English, deliberately outside the i18n tables like the debug menu.

import { h, clear } from "./dom";
import { Bridge, type AssistAction, type AssistCard, type AssistEvent, type AssistReply } from "../core/bridge";
import { State } from "../core/state";
import type { ViewActions, ViewHost } from "./views";

const RISK_COLOR: Record<AssistCard["risk"], string> = {
  safe: "34,197,94",
  careful: "245,158,11",
  dangerous: "244,80,94",
};

type Phase =
  | { kind: "ask" }
  | { kind: "edit"; text: string }
  | { kind: "editEvent"; title: string; when: string }
  | { kind: "ran"; text: string };

export function buildCommand(actions: ViewActions): ViewHost {
  const who = h("div", { class: "who-row" });
  const body = h("div", { class: "cmd-body" });
  const row = h("div", { class: "actions" });
  const el = h(
    "div",
    { class: "view" },
    h("div", { class: "card wash cmd-card" }, h("div", { class: "stack cmd-stack" }, who, body, row)),
  );
  const cardEl = el.querySelector(".card") as HTMLElement;

  let phase: Phase = { kind: "ask" };
  /** The card chosen from a `choices` reply, if any. */
  let chosen: AssistCard | null = null;
  /** The action chosen from an `actionChoices` reply (or the lone `action`). */
  let chosenAction: AssistAction | null = null;
  let shown: AssistReply | null = null;
  let pinned = false;
  let busy = false;

  const pin = (on: boolean) => {
    if (pinned === on) return;
    pinned = on;
    actions.pin(on);
  };

  const finish = (back: "prompt" | "overview" = "overview") => {
    State.assist = null;
    chosen = null;
    chosenAction = null;
    phase = { kind: "ask" };
    pin(false);
    actions.setView(back);
  };

  const current = (): AssistCard | null => {
    const a = State.assist;
    if (chosen) return chosen;
    return a && a.kind === "card" ? a.card : null;
  };

  async function run(inline: boolean) {
    const c = current();
    if (!c || busy) return;
    const command = phase.kind === "edit" ? phase.text.trim() : c.command;
    if (!command) return;
    busy = true;
    try {
      const r = await Bridge.assistRun(command, inline && phase.kind !== "edit");
      if (!r) {
        phase = { kind: "ran", text: "Running commands needs the Fedo app." };
      } else if (inline && r.output) {
        phase = { kind: "ran", text: r.output };
      } else if (inline && r.ok) {
        phase = { kind: "ran", text: "(no output)" };
      } else if (!r.ok) {
        phase = { kind: "ran", text: r.note || "It didn't run." };
      } else {
        // A terminal window has it now.
        finish();
        return;
      }
      State.notify();
    } catch (err) {
      phase = { kind: "ran", text: String(err).replace(/^Error:\s*/, "") };
      State.notify();
    } finally {
      busy = false;
    }
  }

  const button = (label: string, kind: "primary" | "secondary", on: () => void, kbd?: string) =>
    h("button", { class: `btn ${kind}`, onclick: on }, h("span", { text: label }), kbd ? h("span", { class: "kbd", text: kbd }) : null);

  function renderCard(c: AssistCard) {
    cardEl.style.setProperty("--wash", `rgba(${RISK_COLOR[c.risk]},0.5)`);
    who.replaceChildren(
      h("span", { class: `risk-chip ${c.risk}`, text: c.risk }),
      h("span", { text: c.sudo ? "needs administrator rights" : "ready to run" }),
    );
    if (phase.kind === "edit") {
      const input = h("input", { class: "cmd-edit", type: "text", spellcheck: "false", value: phase.text }) as HTMLInputElement;
      input.addEventListener("input", () => { if (phase.kind === "edit") phase = { kind: "edit", text: input.value }; });
      input.addEventListener("keydown", (e) => {
        e.stopPropagation();
        if (e.key === "Enter") { e.preventDefault(); void run(false); }
        if (e.key === "Escape") { e.preventDefault(); phase = { kind: "ask" }; State.notify(); }
      });
      body.append(input, h("div", { class: "cmd-why", text: "Edited commands open in a terminal window." }));
      row.append(
        button("Back", "secondary", () => { phase = { kind: "ask" }; State.notify(); }, "Esc"),
        button("Run in terminal", "primary", () => void run(false), "⏎"),
      );
      queueMicrotask(() => input.focus());
      return;
    }
    body.append(h("div", { class: "cmd-code", text: c.command }));
    if (c.explanation) body.append(h("div", { class: "cmd-why", text: c.explanation }));
    for (const w of c.warnings) body.append(h("div", { class: `cmd-warn ${c.risk}`, text: w }));
    if (c.details.length) body.append(h("pre", { class: "cmd-details", text: c.details.join("\n") }));
    row.append(
      button("Cancel", "secondary", () => finish(), "Esc"),
      button("Edit", "secondary", () => { phase = { kind: "edit", text: c.command }; State.notify(); }, "E"),
    );
    if (c.inlineOk) row.append(button("Run here", "secondary", () => void run(true), "H"));
    row.append(button("Run in terminal", "primary", () => void run(false), "⏎"));
  }

  /** datetime-local / date input value from epoch seconds (local time). */
  const inputValue = (epoch: number, allDay: boolean) => {
    const d = new Date(epoch * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    const day = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
    return allDay ? day : `${day}T${p(d.getHours())}:${p(d.getMinutes())}`;
  };

  async function createEvent(ev: AssistEvent, title: string, when: string) {
    if (busy) return;
    busy = true;
    try {
      let start = ev.start;
      let end = ev.end;
      if (when) {
        // The edited time: the browser parses a local datetime-local / date value.
        const parsed = new Date(ev.allDay ? `${when}T12:00` : when).getTime() / 1000;
        if (Number.isFinite(parsed)) {
          end = ev.allDay ? parsed : parsed + (ev.end - ev.start);
          start = parsed;
        }
      }
      const text = await Bridge.assistCreateEvent(ev.kind, title, start, end, ev.allDay);
      phase = { kind: "ran", text: text ?? "Needs the Fedo app." };
    } catch (err) {
      phase = { kind: "ran", text: String(err).replace(/^Error:\s*/, "") };
    } finally {
      busy = false;
    }
    State.notify();
  }

  function renderEvent(ev: AssistEvent) {
    cardEl.style.setProperty("--wash", "rgba(99,102,241,0.5)");
    who.replaceChildren(
      h("span", { class: "risk-chip safe", text: ev.kind === "deadline" ? "deadline" : "event" }),
      h("span", { text: `goes to ${ev.calendar}` }),
    );
    if (phase.kind === "editEvent") {
      const title = h("input", { class: "cmd-edit", type: "text", value: phase.title }) as HTMLInputElement;
      const when = h("input", {
        class: "cmd-edit", type: ev.allDay ? "date" : "datetime-local", value: phase.when,
      }) as HTMLInputElement;
      for (const i of [title, when]) {
        i.addEventListener("keydown", (e) => {
          e.stopPropagation();
          if (e.key === "Enter") { e.preventDefault(); void createEvent(ev, title.value.trim() || ev.title, when.value); }
          if (e.key === "Escape") { e.preventDefault(); phase = { kind: "ask" }; State.notify(); }
        });
      }
      body.append(title, when);
      row.append(
        button("Back", "secondary", () => { phase = { kind: "ask" }; State.notify(); }, "Esc"),
        button("Add to calendar", "primary", () => void createEvent(ev, title.value.trim() || ev.title, when.value), "⏎"),
      );
      queueMicrotask(() => title.focus());
      return;
    }
    body.append(
      h("div", { class: "cmd-code", text: ev.title }),
      h("div", { class: "cmd-why big", text: ev.when }),
    );
    if (ev.reminders.length) {
      const f = (t: number) =>
        new Date(t * 1000).toLocaleString(undefined, { weekday: "short", day: "numeric", month: "short", hour: "numeric", minute: "2-digit" });
      body.append(h("pre", { class: "cmd-details", text: ev.reminders.map((r) => `Reminder ${f(r.at)} — ${r.label}`).join("\n") }));
    } else {
      body.append(h("div", { class: "cmd-why", text: "No reminders: it's too close or already past." }));
    }
    row.append(
      button("Cancel", "secondary", () => finish(), "Esc"),
      button("Edit", "secondary", () => {
        phase = { kind: "editEvent", title: ev.title, when: inputValue(ev.start, ev.allDay) };
        State.notify();
      }, "E"),
      button("Add to calendar", "primary", () => void createEvent(ev, ev.title, ""), "⏎"),
    );
  }

  async function doAction(a: AssistAction) {
    if (busy) return;
    busy = true;
    try {
      const text = await Bridge.assistDo(a);
      phase = { kind: "ran", text: text ?? "Needs the Fedo app." };
    } catch (err) {
      phase = { kind: "ran", text: String(err).replace(/^Error:\s*/, "") };
    } finally {
      busy = false;
    }
    State.notify();
  }

  function renderAction(a: AssistAction) {
    cardEl.style.setProperty("--wash", a.warning ? "rgba(245,158,11,0.5)" : "rgba(99,102,241,0.5)");
    who.replaceChildren(h("span", { class: "risk-chip safe", text: a.kind.replace("-", " ") }), h("span", { text: "confirm first" }));
    body.append(h("div", { class: "cmd-code", text: a.title }));
    if (a.lines.length) body.append(h("pre", { class: "cmd-details", text: a.lines.join("\n") }));
    if (a.warning) body.append(h("div", { class: "cmd-warn careful", text: a.warning }));
    if (a.note) body.append(h("div", { class: "cmd-why", text: a.note }));
    row.append(button("Cancel", "secondary", () => finish(), "Esc"));
    // A card that cannot work (no country code) has nothing to confirm.
    if (!(a.warning && a.params["digits"] === "")) row.append(button("Do it", "primary", () => void doAction(a), "⏎"));
  }

  function renderActionChoices(title: string, options: { label: string; detail: string; action: AssistAction }[]) {
    cardEl.style.setProperty("--wash", "rgba(99,102,241,0.5)");
    who.replaceChildren(h("span", { text: title }));
    for (const [i, o] of options.entries()) {
      body.append(
        h(
          "button",
          { class: "cmd-choice", onclick: () => { chosenAction = o.action; State.notify(); } },
          h("b", { text: `${i + 1}  ${o.label}` }),
          h("span", { text: o.detail }),
        ),
      );
    }
    row.append(button("Cancel", "secondary", () => finish(), "Esc"));
  }

  function renderChoices(title: string, options: AssistReply & { kind: "choices" }) {
    cardEl.style.setProperty("--wash", "rgba(99,102,241,0.5)");
    who.replaceChildren(h("span", { text: title }));
    for (const [i, o] of options.options.entries()) {
      body.append(
        h(
          "button",
          { class: "cmd-choice", onclick: () => { chosen = o.card; State.notify(); } },
          h("b", { text: `${i + 1}  ${o.label}` }),
          h("span", { text: o.detail }),
        ),
      );
    }
    row.append(button("Cancel", "secondary", () => finish(), "Esc"));
  }

  function renderPrompt(text: string) {
    cardEl.style.setProperty("--wash", "rgba(99,102,241,0.5)");
    who.replaceChildren(h("span", { text: "Prompt for Claude Code" }));
    body.append(h("pre", { class: "cmd-prompt", text }));
    const copy = button("Copy", "primary", () => {
      void Bridge.assistCopy(text).then((ok) => {
        copy.querySelector("span")!.textContent = ok ? "Copied" : "wl-copy missing";
      });
    }, "C");
    row.append(button("Close", "secondary", () => finish(), "Esc"));
    const task = State.focusTask;
    if (task?.sessionId) {
      row.append(
        button("Jump to terminal", "secondary", () => {
          void Bridge.assistCopy(text);
          void Bridge.openSession(task.sessionId ?? null, task.sessionCwd ?? null);
          finish();
        }, "J"),
      );
    }
    row.append(copy);
  }

  // Keys, while the card is the view on screen. Captured so Esc cancels the
  // card rather than folding the whole island.
  window.addEventListener(
    "keydown",
    (e) => {
      if (State.view !== "command" || !State.assist || State.mode !== "expanded") return;
      const typing = (e.target as HTMLElement | null)?.tagName === "INPUT";
      if (typing) return;
      const a = State.assist;
      const c = current();
      const swallow = () => { e.preventDefault(); e.stopImmediatePropagation(); };
      if (phase.kind === "ran" || a.kind === "note" || a.kind === "done") {
        if (e.key === "Escape" || e.key === "Enter") { swallow(); finish(); }
        return;
      }
      if (e.key === "Escape") { swallow(); finish(); return; }
      if (a.kind === "choices" && !chosen) {
        const n = Number(e.key);
        if (n >= 1 && n <= a.options.length) { swallow(); chosen = a.options[n - 1].card; State.notify(); }
        return;
      }
      if (a.kind === "action" || (a.kind === "actionChoices" && chosenAction)) {
        const act = a.kind === "action" ? a.action : chosenAction!;
        if (e.key === "Enter" && phase.kind === "ask" && !(act.warning && act.params["digits"] === "")) {
          swallow();
          void doAction(act);
        }
        return;
      }
      if (a.kind === "actionChoices") {
        const n = Number(e.key);
        if (n >= 1 && n <= a.options.length) { swallow(); chosenAction = a.options[n - 1].action; State.notify(); }
        return;
      }
      if (a.kind === "event" && phase.kind === "ask") {
        if (e.key === "Enter") { swallow(); void createEvent(a.event, a.event.title, ""); }
        else if (e.key.toLowerCase() === "e") {
          swallow();
          phase = { kind: "editEvent", title: a.event.title, when: inputValue(a.event.start, a.event.allDay) };
          State.notify();
        }
        return;
      }
      if (c && phase.kind === "ask") {
        if (e.key === "Enter") { swallow(); void run(false); }
        else if (e.key.toLowerCase() === "e") { swallow(); phase = { kind: "edit", text: c.command }; State.notify(); }
        else if (e.key.toLowerCase() === "h" && c.inlineOk) { swallow(); void run(true); }
      }
      if (a.kind === "prompt" && e.key.toLowerCase() === "c") { swallow(); void Bridge.assistCopy(a.text); }
    },
    true,
  );

  return {
    el,
    sync() {
      const a = State.assist;
      if (a !== shown) {
        shown = a;
        chosen = null;
        chosenAction = null;
        phase = { kind: "ask" };
      }
      clear(body);
      clear(row);
      if (!a) {
        pin(false);
        return;
      }
      pin(true);
      if (phase.kind === "ran") {
        cardEl.style.setProperty("--wash", "rgba(34,197,94,0.4)");
        who.replaceChildren(h("span", { text: "Output" }));
        body.append(h("pre", { class: "cmd-details tall", text: phase.text }));
        row.append(button("Done", "primary", () => finish(), "⏎"));
        return;
      }
      switch (a.kind) {
        case "card":
          renderCard(chosen ?? a.card);
          break;
        case "choices":
          if (chosen) renderCard(chosen);
          else renderChoices(a.title, a);
          break;
        case "prompt":
          renderPrompt(a.text);
          break;
        case "event":
          renderEvent(a.event);
          break;
        case "action":
          renderAction(a.action);
          break;
        case "actionChoices":
          if (chosenAction) renderAction(chosenAction);
          else renderActionChoices(a.title, a.options);
          break;
        case "open":
          break;
        case "note":
        case "done":
          cardEl.style.setProperty("--wash", "rgba(99,102,241,0.4)");
          who.replaceChildren(h("span", { text: "Fedo" }));
          body.append(h("div", { class: "cmd-why big", text: a.text }));
          row.append(button("OK", "primary", () => finish(), "⏎"));
          break;
        case "chat":
          break;
      }
    },
  };
}
