# Fedo — third-party agent integration

Any tool that can write to a Unix domain socket can send events to Fedo and
have its own pill next to Claude Code. This is the relay's generic protocol;
it works for any agent that follows it, even though Settings → Agents only
ships a one-click installer for Antigravity today (FEDO.md Phase 0 narrows
the agents this fork actively installs hooks for to Claude Code and
Antigravity — the protocol itself isn't narrowed).

## The `fedo_agent` field

Add the optional field `fedo_agent` to any hook JSON payload. Fedo will
create a pill labelled with the agent name and route all events to it.

**Validation:** the name must match `^[a-z0-9-]{1,24}$` (lowercase letters,
digits and hyphens, 1–24 characters). An absent or invalid name routes the
event to the Claude Code pill instead.

## Hook command

Configure your tool to call the Fedo relay with `--agent <your-name>` after
the hook executable:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "type": "command", "command": "/path/to/fedo-hook --agent my-tool" }
    ]
  }
}
```

Fedo copies the relay to `~/.local/share/fedo/bin/fedo-hook` at startup.

## Payload format

The relay adds `fedo_agent` to the JSON it forwards. You can also add it
yourself if you talk to the socket directly:

```json
{
  "hook_event_name": "UserPromptSubmit",
  "session_id": "my-session-1",
  "fedo_agent": "my-tool",
  "prompt": "Running task…"
}
```

Send newline-terminated JSON to the socket at
`$XDG_RUNTIME_DIR/fedo.sock` (usually `/run/user/<uid>/fedo.sock`). Only
your own user account can connect.

## Supported events

All standard Claude Code hook events are supported, **except
`PermissionRequest`**: approval cards are not yet implemented for
third-party agents (only Claude Code gets one). A `PermissionRequest` from
an external agent is answered immediately with no decision, so the relay
writes nothing and the agent re-asks in its terminal.

The pill lifecycle:

| Event | Effect |
|---|---|
| `SessionStart` | Creates the pill (if absent), sets state to idle |
| `UserPromptSubmit` | State → thinking; prompt shown in ticker |
| `PreToolUse` | State → working; tool label shown in ticker |
| `PostToolUse` / `PostToolUseFailure` | State → working |
| `Notification` | Rate-limit or question state if applicable |
| `Stop` | State → finished for 5 s; active declared pills (catalog + checked in Settings) reset to idle — all others are removed |
| `StopFailure` | State → error |
| `SessionEnd` | Active declared pills (catalog + checked in Settings) reset to idle — all others are removed |
| `SubagentStart` / `SubagentStop` | Step added to ticker |

## Declared pills

A **declared pill** is a catalog entry (`src/core/pills.ts`) that has been
enabled in **Settings → Active pills**. When a session ends for a declared
pill, the pill stays visible and resets to idle instead of disappearing.

A catalog pill that is not checked in Settings behaves like any other
agent: it gets an automatic pill when a session starts, and that pill is
removed when the session ends.

## Antigravity

Fedo supports Antigravity out of the box via **Settings → Agents →
Antigravity → Install hooks**. The installer writes to
`~/.gemini/config/hooks.json` (timeouts in seconds) and uses `--agent
antigravity`. The relay translates `toolCall.name` / `conversationId` to
the island's `tool_name` / `session_id`.

| Antigravity event | Canonical event |
|---|---|
| `PreInvocation` | `UserPromptSubmit` |
| `PreToolUse` | `PreToolUse` |
| `PostToolUse` | `PostToolUse` |
| `PostInvocation` | `PostToolUse` |
| `Stop` | `Stop` |

## Any other tool

Follow the generic pattern: call
`~/.local/share/fedo/bin/fedo-hook --agent <your-name> <EventName>` and let
the relay forward the event.

## Quick test

With Fedo running:

```sh
echo '{"hook_event_name":"UserPromptSubmit","session_id":"t1","prompt":"hello","fedo_agent":"demo"}' \
  | ~/.local/share/fedo/bin/fedo-hook --agent demo
```

A "demo" pill should appear in the island.
