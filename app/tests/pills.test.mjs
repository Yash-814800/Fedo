// The pill catalog (src/core/pills.ts) and the declared pills in the app state.
// Fedo offers Claude Code, Antigravity and Spotify; every other pill the fork
// came with stays in the catalog, marked unsupported, so its ID is never reused.

import { beforeEach, test } from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_MAIN_PILL, MAX_DECLARED, PILL_CATALOG, availablePills, chooseMainPill, isHookPill,
  mainPillChoices, orderPills, pillDefinition, sanitizeDeclared, sessionSubtitle, toggleDeclared,
} from "../src/core/pills.ts";
import { DEFAULT_SETTINGS, State } from "../src/core/state.ts";

const CLAUDE = "integration_claude";
const AGY = "agent_antigravity";
const SPOTIFY = "integration_spotify";

test("IDs are unique and never renamed, even for pills this build doesn't offer", () => {
  const ids = PILL_CATALOG.map((p) => p.id);
  assert.equal(new Set(ids).size, ids.length);
  for (const id of [
    CLAUDE, "agent_cursor", AGY, "agent_codex", "agent_gemini", "agent_copilot", "agent_muse",
    "agent_opencode", "agent_amp", "agent_hermes", "agent_claude-desktop", "ai_anthropic", "ai_google",
    "ai_openai", "ai_ollama", "ai_lmstudio", "integration_resend", "integration_n8n", "integration_vercel",
    "integration_github", "integration_notion", "integration_calcom", "integration_stripe",
    "integration_music", SPOTIFY,
  ]) {
    assert.ok(pillDefinition(id), `${id} is gone from the catalog`);
  }
});

test("this build offers Claude Code, Antigravity and Spotify, and nothing else", () => {
  assert.deepEqual(availablePills("linux").map((p) => p.id), [CLAUDE, AGY, SPOTIFY]);
  for (const p of PILL_CATALOG) {
    if (![CLAUDE, AGY, SPOTIFY, "agent_claude-desktop"].includes(p.id)) {
      assert.equal(p.support, "no", `${p.id} should be marked unsupported`);
    }
  }
  // Fedo has no API-key chat providers (FEDO.md §0).
  assert.ok(PILL_CATALOG.filter((p) => p.category === "ai").every((p) => p.support === "no"));
});

test("the Claude Code pill is named for the tool, not for an editor", () => {
  assert.equal(pillDefinition(CLAUDE).name, "Claude Code");
  assert.equal(DEFAULT_MAIN_PILL, CLAUDE);
});

test("only Claude Code and Antigravity can be the main pill", () => {
  assert.deepEqual(mainPillChoices("linux").map((p) => p.id), [CLAUDE, AGY]);
  assert.equal(chooseMainPill({ mainPill: CLAUDE, activeIntegrations: [AGY] }, SPOTIFY), null);
  assert.deepEqual(
    chooseMainPill({ mainPill: CLAUDE, activeIntegrations: [AGY, SPOTIFY] }, AGY),
    { mainPill: AGY, activeIntegrations: [SPOTIFY] },
  );
});

test("hook-driven pills are the ones that connect through the relay", () => {
  assert.equal(isHookPill(CLAUDE), true);
  assert.equal(isHookPill(AGY), true);
  assert.equal(isHookPill(SPOTIFY), false);
  assert.equal(sessionSubtitle(CLAUDE), "Claude Code");
});

test("a declaration from an older settings file is made usable", () => {
  assert.deepEqual(
    sanitizeDeclared({ mainPill: "agent_cursor", activeIntegrations: ["integration_n8n", AGY, AGY, "nonsense", SPOTIFY] }, "linux"),
    { mainPill: CLAUDE, activeIntegrations: [AGY, SPOTIFY] },
  );
  // Never the main pill itself.
  assert.deepEqual(
    sanitizeDeclared({ mainPill: AGY, activeIntegrations: [AGY, SPOTIFY] }, "linux"),
    { mainPill: AGY, activeIntegrations: [SPOTIFY] },
  );
});

test("toggling declares and undeclares, never the main pill or an unoffered one", () => {
  const d = { mainPill: CLAUDE, activeIntegrations: [] };
  assert.deepEqual(toggleDeclared(d, AGY, "linux"), [AGY]);
  assert.equal(toggleDeclared(d, CLAUDE, "linux"), null);
  assert.equal(toggleDeclared(d, "integration_github", "linux"), null);
  assert.deepEqual(toggleDeclared({ ...d, activeIntegrations: [AGY, SPOTIFY] }, AGY, "linux"), [SPOTIFY]);
  assert.equal(MAX_DECLARED, 4);
});

test("pills are ordered main first, then the rest in catalog order", () => {
  const tasks = [{ id: SPOTIFY }, { id: AGY }, { id: CLAUDE }];
  assert.deepEqual(orderPills(tasks, AGY).map((t) => t.id), [AGY, CLAUDE, SPOTIFY]);
  assert.deepEqual(orderPills(tasks, CLAUDE).map((t) => t.id), [CLAUDE, AGY, SPOTIFY]);
});

// ── In the app state ──────────────────────────────────────────────────────────

beforeEach(() => {
  State.tasks = [];
  State.focusId = null;
  State.settings = { ...DEFAULT_SETTINGS };
});

test("the main pill always loads, the declared ones join it, and focus starts on it", () => {
  State.settings = { ...DEFAULT_SETTINGS, mainPill: AGY, activeIntegrations: [SPOTIFY] };
  State.loadIntegrationTasks();
  assert.deepEqual(State.tasks.map((t) => t.id), [AGY, SPOTIFY]);
  assert.equal(State.focusId, AGY);
});

test("an old settings file gets Claude Code as its main pill and keeps what is still offered", () => {
  State.settings = {
    ...DEFAULT_SETTINGS, mainPill: "agent_codex",
    activeIntegrations: ["integration_resend", "integration_github", SPOTIFY],
  };
  State.loadIntegrationTasks();
  assert.deepEqual(State.tasks.map((t) => t.id), [CLAUDE, SPOTIFY]);
});

test("switching the main pill frees its slot, and the old main pill goes when idle", () => {
  State.settings = { ...DEFAULT_SETTINGS, mainPill: CLAUDE, activeIntegrations: [AGY] };
  State.loadIntegrationTasks();
  State.settings = { ...State.settings, mainPill: AGY, activeIntegrations: [] };
  State.loadIntegrationTasks();
  assert.deepEqual(State.tasks.map((t) => t.id), [AGY]);
});

test("a pill in the middle of a session stays until the session ends", () => {
  State.settings = { ...DEFAULT_SETTINGS, mainPill: CLAUDE, activeIntegrations: [AGY] };
  State.loadIntegrationTasks();
  State.updateTask(AGY, "working");
  State.settings = { ...State.settings, activeIntegrations: [] };
  State.loadIntegrationTasks();
  assert.ok(State.tasks.some((t) => t.id === AGY), "kept while it is working");
  State.updateTask(AGY, "idle");
  State.loadIntegrationTasks();
  assert.ok(!State.tasks.some((t) => t.id === AGY), "gone once idle and no longer declared");
});
