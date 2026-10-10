// The chat view (src/views/chat.ts) on a fake DOM, through the real bridge:
// every message goes to the assistant's router first; a system action becomes
// the command card instead of a chat turn, anything else is a `claude -p`
// turn whose answer grows in place as it streams.

import { beforeEach, test } from "node:test";
import assert from "node:assert/strict";
import { installFakeDom } from "./fakedom.mjs";
import { calls, emit, internals, sent } from "./tauri.mjs";

installFakeDom();
const { buildPrompt } = await import("../src/views/chat.ts");
const { DEFAULT_SETTINGS, State } = await import("../src/core/state.ts");

/** What the mocked Rust side answers, by command. */
let answers;
const plainInvoke = internals.invoke;
internals.invoke = async (cmd, args) => {
  const result = await plainInvoke(cmd, args);
  if (cmd in answers) return typeof answers[cmd] === "function" ? answers[cmd](args) : answers[cmd];
  return result;
};

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

let view;
beforeEach(() => {
  calls.length = 0;
  answers = { assist_route: { kind: "chat" } };
  State.settings = { ...DEFAULT_SETTINGS };
  State.assist = null;
  State.promptMode = false;
  State.chatHistory = [];
  State.stateOverride = null;
  State.view = "prompt";
  State.droppedFile = null;
  view = buildPrompt(() => {});
  view.sync();
});

const $ = (cls) => view.el.querySelector(cls);

const type = async (text) => {
  $(".chat-input").value = text;
  $(".send-btn").fire("click");
  await flush();
  view.sync();
};

test("a chat answer streams into one reply, then the finished text replaces it", async () => {
  let finish;
  answers.chat_send = () => new Promise((resolve) => (finish = resolve));
  await type("hello");
  assert.deepEqual(sent("assist_route"), [{ query: "hello", promptMode: false, fastOnly: false, file: null }]);
  assert.ok($(".typing"), "dots until the first visible text");

  emit("chat-delta", ""); // still thinking
  assert.ok($(".typing"));
  emit("chat-delta", "Hel");
  emit("chat-delta", "Hello **there**");
  view.sync(); // another view update mid-stream must not wipe the live answer
  assert.equal($(".typing"), null);
  const replies = view.el.find(".reply");
  assert.equal(replies.length, 1);
  assert.equal(replies[0].find("STRONG")[0].textContent, "there");

  finish({ text: "Hello **there**!" });
  await flush();
  view.sync();
  assert.equal(view.el.find(".reply").length, 1);
  assert.equal(view.el.find(".reply")[0].textContent, "Hello there!");
  assert.deepEqual(State.chatHistory.map((m) => m.role), ["user", "assistant"]);
});

test("a system action becomes the command card and never a chat turn", async () => {
  const card = { command: "df -h ~", explanation: "Disk space", risk: "safe", warnings: [], details: [], sudo: false, inlineOk: true };
  answers.assist_route = { kind: "card", card };
  await type("how much space is left");
  assert.deepEqual(sent("chat_send"), []);
  assert.equal(State.view, "command");
  assert.deepEqual(State.assist, { kind: "card", card });
  assert.deepEqual(State.chatHistory, []);
  assert.equal(State.stateOverride, null);
});

test("handled on the spot: nothing opens and nothing is asked of the chat", async () => {
  answers.assist_route = { kind: "done", text: "Sounds muted." };
  await type("mute");
  assert.deepEqual(sent("chat_send"), []);
  assert.deepEqual(State.assist, { kind: "done", text: "Sounds muted." });
});

test("the deadlines list opens the schedule view", async () => {
  answers.assist_route = { kind: "open", view: "schedule" };
  await type("deadlines");
  assert.equal(State.view, "schedule");
  assert.equal(State.assist, null);
});

test("the Prompt button sends the text to the prompt writer", async () => {
  answers.assist_route = { kind: "prompt", text: "Fix the button." };
  $(".prompt-toggle").fire("click");
  assert.equal(State.promptMode, true);
  await type("no the button is wrong");
  assert.deepEqual(sent("assist_route"), [{ query: "no the button is wrong", promptMode: true, fastOnly: false, file: null }]);
  assert.deepEqual(State.assist, { kind: "prompt", text: "Fix the button." });
  assert.equal(State.promptMode, false);
});

test("follow-ups in a conversation only get the local fast path", async () => {
  answers.chat_send = { text: "first answer" };
  await type("hello");
  await flush();
  answers.chat_send = { text: "second answer" };
  await type("and then?");
  await flush();
  const asks = sent("assist_route");
  assert.equal(asks[0].fastOnly, false);
  assert.equal(asks[1].fastOnly, true);
});

test("a router that fails must not take the chat down with it", async () => {
  answers.assist_route = () => { throw new Error("no claude"); };
  answers.chat_send = { text: "still here" };
  await type("hello");
  await flush();
  assert.deepEqual(sent("chat_send").length, 1);
  assert.deepEqual(State.chatHistory.map((m) => m.content), ["hello", "still here"]);
});

test("the dropped file goes to the router, so \"send it to my phone\" knows which", async () => {
  State.droppedFile = { name: "cv.pdf", path: "/inbox/cv.pdf", size: 10 };
  answers.assist_route = { kind: "chat" };
  answers.chat_send = { text: "ok" };
  await type("send this to my phone");
  assert.deepEqual(sent("assist_route")[0].file, { name: "cv.pdf", path: "/inbox/cv.pdf" });
});

test("a call, SMS or WhatsApp card is a confirmation, never an action taken", async () => {
  const action = {
    kind: "sms", title: "Text Aparna", lines: ["To Aparna · +91…", "“On my way”"],
    note: "Sent as an SMS from your phone, right away.", warning: null, params: { number: "+91…" },
  };
  answers.assist_route = { kind: "action", action };
  await type("text Aparna On my way");
  assert.equal(State.view, "command");
  assert.deepEqual(State.assist, { kind: "action", action });
  assert.deepEqual(sent("assist_do"), [], "nothing is sent until Enter on the card");
  assert.deepEqual(sent("chat_send"), []);
});

test("a card that asked 'to whom?' fills the input", () => {
  State.chatPrefill = "Send it via ZapZap to ";
  view.sync();
  assert.equal($(".chat-input").value, "Send it via ZapZap to ");
  assert.equal(State.chatPrefill, null);
});
