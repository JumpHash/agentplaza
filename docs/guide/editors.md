# Editors over ACP

Zed, JetBrains IDEs, Neovim and other editors that speak the Agent Client Protocol (ACP)
can drive a plaza agent. You prompt the agent from the editor, watch its work stream in,
and answer its approvals there. The editor starts `agentplaza acp`, which connects to the
hub on your machine.

## Turn it on

1. Set `acp_enabled` to `true` in the hub's settings (`/v1/settings`). It is off by default.
2. Make sure `agentplaza` is on your PATH and the hub is running on this machine. If the
   hub is elsewhere, pass `--hub` (or set `APZ_HUB`) and `--token` (or `APZ_TOKEN`).
3. Run `agentplaza acp sessions`. It lists the editor sessions the hub holds, and is
   empty until you start one.

## Zed

Add the agent to Zed's `settings.json`:

```json
{"agent_servers": {"AgentPlaza": {"command": "agentplaza", "args": ["acp"]}}}
```

The orchestrator answers by default. To drive one agent instead, pass its name:
`"args": ["acp", "--agent", "<name>"]`.

## JetBrains IDEs

In the IDE's AI assistant settings, add a custom ACP agent. Its command is `agentplaza`,
and its argument is `acp` (or `acp --agent <name>` for one agent). The menu names change
between IDE versions, so follow the IDE's own ACP instructions for where the command goes.

## Neovim

With CodeCompanion, add a custom ACP adapter whose command is `agentplaza`, with the
argument `acp`. CodeCompanion's adapter table changes between releases, so check its
documentation for the current field names.

## What the editor sees

- Each session is its own thread on the agent. Close the editor and resume the session
  later; the hub keeps it with the rest of the plaza state.
- The agent's replies as they stream, its tool calls with their status, a coding engine's
  own steps and plan, and the plan the agent keeps.
- Approvals the agent is waiting on. The editor shows them and you allow or deny each one.
  The answer is the owner's, the same as answering it on the hub.
- The agent's approval mode: `autonomous`, `ask` or `read_only`.

Cancelling a prompt in the editor stops the work in progress.

## What agents cannot do through the editor

- Reach your machine. The editor's MCP servers are not connected to the agent, and the
  hub never reads or writes your files or runs a command in your editor.
- Act outside their own VM. Each action is checked by the laws, grants and approvals,
  the same as any other work.
- Use a coding engine's bypass mode. The harness refuses those modes, whatever the engine offers.

For the design, see [ACP for editors](../architecture.md#acp-for-editors) in the architecture notes.
