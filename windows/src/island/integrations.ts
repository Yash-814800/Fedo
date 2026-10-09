// Whether each pill is connected, for its idle card. No third-party pollers
// remain in this build (FEDO.md Phase 0) — this only tracks the hook-driven
// pills (Claude Code, Antigravity).

import { Bridge } from "../core/bridge";
import { availablePills } from "../core/pills";
import { State } from "../core/state";

/**
 * Whether each pill is connected, so its idle card can say so: the hooks in
 * place for a pill fed by hook events, nothing at all for Claude Desktop.
 */
export async function refreshConfigured() {
  const hooks = (await Bridge.agentHooksStatus()) ?? null;
  for (const def of availablePills(State.os)) {
    const configured = def.connect.kind === "hooks"
      ? hooks?.[def.id] ?? (def.id === "integration_claude" ? State.settings.hooksInstalled : false)
      : true;
    const info = State.integrations[def.id] ?? { data: {}, error: null, loaded: false, configured: false };
    State.integrations[def.id] = { ...info, configured };
  }
  State.notify();
}

/** Only the hook-driven pills, for when the island opens: a few small file reads. */
export async function refreshHookPills() {
  const hooks = await Bridge.agentHooksStatus();
  if (!hooks) return;
  for (const [id, present] of Object.entries(hooks)) {
    const info = State.integrations[id] ?? { data: {}, error: null, loaded: false, configured: false };
    State.integrations[id] = { ...info, configured: present };
  }
  State.notify();
}
