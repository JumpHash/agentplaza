# Editors over ACP

Zed, JetBrains IDEs, Neovim and other editors that speak the Agent Client Protocol (ACP)
can drive a plaza agent. You prompt the agent from the editor, watch its work stream in,
and answer its approvals there. The editor starts `agentplaza acp`, which connects to the
hub on your machine.

## Turn it on

1. In the console, open Setup › MCP › This hub as a server and turn on **Editors (ACP)** (the
   desktop app has the same switch on its hub MCP screen). It is off by default; the setting is
   `acp_enabled` in `/v1/settings`.
2. Make sure `agentplaza` is on your PATH and the hub is running on this machine. If the
   hub is elsewhere, pass `--hub` (or set `APZ_HUB`) and `--token` (or `APZ_TOKEN`).
3. Run `agentplaza acp sessions`. It lists the editor sessions the hub holds, and is
   empty until you start one.

## Using a token

An editor can connect with a hub MCP token instead of the owner token. On Setup › MCP › This
hub as a server, mint a token with the **operate** or **govern** scope:

- **operate** drives the agents: it starts and resumes sessions, prompts, and cancels or steers
  the work. The approvals stay with you in the console, and the approval mode and the model do
  not change from the editor.
- **govern** does the same and also answers the agent's approvals from the editor and changes
  the approval mode and the model.

An observe token cannot connect. Connect with the token:

```
agentplaza acp --hub https://<your hub> --token apz_mcp_…
```

To limit which agents the token drives, add `acp:<agent>` entries to its allowlist, naming the
agent by its name or its CID. With no `acp:` entry the token may drive every agent. The same
allowlist limits the token's MCP tools, so a token meant only for editors has `acp:` entries and
nothing else. Each message the token sends counts against its rate limit (20 a minute, shared
with its MCP calls), and each one is written to the audit log as `acp/<method>`.

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
  The answer is the owner's, the same as answering it on the hub. A token with the
  `operate` scope cannot answer them; `govern` can.
- The agent's approval mode (`autonomous`, `ask` or `read_only`) and its model. The model
  picker lists the models within the agent's tier.
- The session's title, taken from its first prompt, and the agent's own model usage.
- The plaza's slash commands, offered in the editor's command list. They run on the agent
  at once instead of queuing as chat:
  - `/compress` summarises the conversation so far.
  - `/steer <note>` steers the work in progress.
  - `/extend-turns <n>` allows more model calls for the work.
  - `/stop` stops the work.

Cancelling a prompt in the editor stops the work in progress and any prompts queued behind it.

## What agents cannot do through the editor

- Reach your machine. The editor's MCP servers are not connected to the agent, and the
  hub never reads or writes your files or runs a command in your editor.
- Act outside their own VM. Each action is checked by the laws, grants and approvals,
  the same as any other work.
- Use a coding engine's bypass mode. The harness refuses those modes, whatever the engine offers.

For the design, see [ACP for editors](../architecture.md#acp-for-editors) in the architecture notes.
