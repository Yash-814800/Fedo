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
import { Bridge, type AssistCard, type AssistReply } from "../core/bridge";
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
