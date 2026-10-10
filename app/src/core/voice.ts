// The microphone, as the island sees it. The daemon (voice.rs) records, runs
// whisper and speaks; this file turns its events into island state and decides
// what a spoken sentence means *here*: an answer to the card on screen, or a
// question for the chat.

import { Bridge, onEvent } from "./bridge";
import { State } from "./state";
import type { AssistReply } from "./bridge";

interface Heard { text: string; mode: "dictate" | "command"; voice: boolean }
interface Download { what: string; fraction: number; done: boolean; error: string | null }

/** A key press the command card's own handler takes, as if it were typed. */
function press(key: "Enter" | "Escape") {
  window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

/** What a sentence does when a command card is waiting: "yes / do it / run it" or "no / cancel". */
async function answersCard(text: string): Promise<boolean> {
  if (State.view !== "command" || !State.assist || State.mode !== "expanded") return false;
  const a = State.assist;
  // Only cards that wait for Enter. A finished answer just closes.
  if (a.kind === "note" || a.kind === "done") return false;
  const answer = await Bridge.voiceConfirmation(text);
  if (!answer) return false;
  press(answer === "yes" ? "Enter" : "Escape");
  return true;
}

export function registerVoiceHandlers(island: { alert(view: "prompt" | "command"): void }) {
  void onEvent<{ state: string; mode: string; level?: number; message?: string }>("voice", (e) => {
    const v = State.voice;
    v.state = e.state as typeof v.state;
    v.mode = e.mode === "dictate" ? "dictate" : "command";
    v.level = e.level ?? 0;
    // The message sticks until the next recording starts.
    if (e.state === "listening" && e.level === undefined) v.message = "";
    if (e.message) v.message = e.message;
    State.notify();
  });

  void onEvent<Download>("voice-download", (d) => {
    State.voice.download = d.done ? null : { what: d.what, fraction: d.fraction };
    if (d.error) {
      State.voice.state = "error";
      State.voice.message = d.error;
    }
    State.notify();
  });

  void onEvent<Heard>("voice-text", async (said) => {
    State.voice.state = "idle";
    if (said.mode === "command" && (await answersCard(said.text))) {
      State.notify();
      return;
    }
    State.pendingVoice = { text: said.text, mode: said.mode };
    State.speakNext = said.mode === "command" && State.settings.voiceSpeak;
    if (State.view !== "prompt" || State.mode !== "expanded") island.alert("prompt");
    State.notify();
  });

  // A command heard while the island was hidden turned out to need a card.
  void onEvent<AssistReply>("voice-reply", (reply) => {
    State.assist = reply;
    State.view = "command";
    island.alert("command");
    State.notify();
  });
}
