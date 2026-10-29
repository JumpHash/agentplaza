# Providers, engines and models

The plaza calls models through providers: an API account (OpenAI, Anthropic, Gemini, a local server and so on), or
a coding engine the agents run inside their VMs (Claude Code, Codex, OpenCode, Gemini CLI, or any agent from the ACP
registry). This page covers adding and checking them, the favourites that lead the model pickers, the usage hubs that
show pooled subscription quotas, and the commands for each.

Everything here is in the console under **Models › Providers**, in the desktop app's Models screen, in the terminal
app's Providers tab (press `m`), and in the CLI as `agentplaza models`.

## 1. API providers

A provider is one account on one API. The plaza keeps each one as an **instance**: its label, its accent colour, its
endpoint, its models, whether it is on, and its key. You can have several instances of one kind, for example two
OpenAI accounts, each with its own key and label.

### Adding one

1. Click **Add provider**. The dialog has three steps: **Provider** (a kind, such as OpenAI or an OpenAI-compatible
   server), **Identity** (the id and the label) and **Config** (the endpoint, the models and the key).
2. Type the key. It goes into the plaza's vault, and the console never shows it again. The field is write-only.
3. Check the models. The list comes from the provider itself; a provider that cannot list its models still works if
   you name the models by hand.

On the command line, `agentplaza models presets` lists the kinds, and `agentplaza models add openai` adds one with the
preset's endpoint and models. A custom provider takes `--id`, `--kind` and `--base-url`. Its key can come from:

- `--env NAME`: an environment variable of the hub's process;
- `--secret NAME`: a named entry in the hub's secret store;
- `--command CMD`: a shell command whose output is the bearer token;
- `--key-stdin`: the key is read from stdin and stored with the provider.

To store or change a key later, use `agentplaza models key <id> --value -` and type the key on stdin. The key is
never echoed and never passed as an argument.

### Accents, on and off

Each instance can take one of eight accent colours, or none; the swatch in its detail sets it. **Enabled** turns the
instance on or off. A disabled instance keeps its key and its settings, and nothing uses it. The same switch is `agentplaza models enable <id>` and `disable <id>`.

Use `agentplaza models providers` to list every instance with its state, health, key, endpoint and models. `--family
api` shows only the API providers, `--family engine` only the engines.

## 2. Coding engines

An engine is the program an agent runs to do its work. Agents run inside their own VM, and the engine reaches models
only through the plaza's gateway.

### The built-in engines

Claude Code, Codex, OpenCode and Gemini CLI come with the plaza. Their on and off switch is the same as before; their
versions, arguments and default model route can be changed like any other instance, and a built-in engine cannot be
removed, only turned off.

### Adding an engine

Two kinds of extra engine can be added:

- **From the ACP registry**: an agent from the registry at `cdn.agentclientprotocol.com`. Search it in the console's
  Add provider dialog, or with `agentplaza models registry search [query]` (`--refresh` fetches the registry again).
  Then `agentplaza models engines add my-agent --registry claude-code`.
- **A local ACP command**: a program on the hub's machine that speaks ACP, given as one program with its arguments:
  `agentplaza models engines add my-tool --command ./my-tool --arg acp`.

An engine can also take plain variables (`--env NAME=VALUE`), a home folder, and a default model route
(`--route provider/model`; empty means the plaza's default). Variables are visible to the agent, so a key never goes
there: the hub refuses names that look like secrets. Engines use the plaza's model gateway, so a key for a model is
never needed in an engine at all.

### Versions and pins

An npm engine can follow `latest`, a version you type, or `pinned`, the version the plaza's catalogue has checked.
When the plaza saves an engine, it pins the package: the npm integrity hash, the PyPI sha256 for a `uvx` package, or
the sha256 of a release archive. The VM installs only a package that matches its pin. If the registry later serves
something different, the install is refused rather than run.

When an engine's version is not the pinned one, the engine shows it, and **Revert to the pinned version** puts it back.

### Update all

**Update all** moves the npm engines to their newest versions. It runs a dry run first and lists each change; you
confirm before anything is installed. From the command line, `agentplaza models update --dry-run` lists the changes
and `agentplaza models update` installs them.

## 3. Health checks

The hub checks every provider and engine in the background, every `health_interval_s` seconds. The default is five
minutes. The setting is in the console's providers card (the **Check less often** and **Check more often** buttons),
and on the command line with `agentplaza models settings health-interval [seconds]`. A value of `0` turns the
background checks off; the owner then checks by hand. The most it may be is a day.

A check reads the provider's key and lists its models, and it notes how long the listing took and how many models it
offered. A check never makes a paid call: listing models is free. An engine's check also reads the provider behind
its model route.

Each check has a status. **ok** means the instance is ready. **warn** means it works with a note: an engine has a
newer version, has no default model route, or its key is held only by another node. **crit** means it cannot be
used: no key, a sealed vault, a failed listing, or an engine that routes through a provider that is missing or
disabled. The model picker lists none of a **crit** instance's models, only the reason and a link to its setup.
**off** means the instance is disabled.

To check now, click **Check every provider now** in the card, or run `agentplaza models check` for all of them or
`agentplaza models check <id>` for one. The checks publish their results, so the consoles update without a reload.

## 4. Subscription accounts

### Local Codex and OpenCode

The **Codex (local login)** preset runs the installed Codex app-server on the hub's machine. It uses that
installation's existing ChatGPT login; AgentPlaza never reads or copies its tokens. Configure the provider's
`options.executable` with the Codex executable to use, especially when the desktop app and the CLI on `PATH`
have different versions. `options.working_directory` may name an empty local directory. Use `auth: "none"`;
HTTP endpoint, header and body overrides are not supported for this local provider.

The preset's provider id is `codex-local` and its kind is `codex-app-server`. A model selection such as
`codex-local/gpt-6-astra@ultra` keeps both the model and the effort explicit. The installed Codex catalog must
offer them, and the adapter checks the model and effort returned when it starts the thread. Listing a model
does not prove account access: a completed inference turn is the check that it can actually run.

Codex receives the AgentPlaza tools as dynamic tools. It returns calls to the normal guest harness for policy
checks and execution. Its own shell, plugins, integrations, subagents and environment access are disabled.
Consequently, an Astra orchestrator delegates through AgentPlaza's configured worker models. Codex's native
conversation remains authoritative when the guest compacts its wire transcript; completed tool calls are
never replayed as new calls.

The **OpenCode (local)** preset uses an installed OpenCode executable with a private temporary configuration.
Use a separate provider instance and scoped key for each upstream: for example, models
`["deepseek/deepseek-flash"]` with a DeepSeek key, or `["opencode/glm-5.3"]` with an OpenCode Zen key. With instance
ids `local-deepseek` and `local-glm`, the complete model routes are
`local-deepseek/deepseek/deepseek-flash` and `local-glm/opencode/glm-5.3`. A key cannot be sent to a different
upstream by changing the requested model. OpenCode's native tools are denied; its structured response supplies
tool calls that AgentPlaza validates and executes. This transport returns complete responses, without native
tool-call streaming.

Both local providers keep execution in AgentPlaza and stop their child processes during normal hub shutdown.
Codex continuations are bound to their original caller, model, tools and transcript. Losing the child process,
restarting the hub, or changing those settings requires a fresh conversation; the adapter reports the loss
instead of replaying earlier actions. Model health listings check the local installation and configuration;
they do not spend tokens to test inference.

### CLIProxyAPI

An existing **CLIProxyAPI** hub is another option for subscription accounts:

1. Run a CLIProxyAPI hub on a machine the plaza can reach, with your subscription accounts added to it.
2. Add the hub's models as an ordinary **OpenAI-compatible** provider, with the hub's client key in the vault.

Agents then use those models through the plaza's gateway like any other provider. No subscription login and no key
ever goes into an agent's VM.

## 5. Usage providers

A usage provider is a CLIProxyAPI hub that the plaza reads for its usage, so you can see the pooled quotas of your
accounts beside the providers. Add one in the **Usage providers** card, or with
`agentplaza models usage-providers add --url URL --key-stdin --label NAME`. The management key is read from stdin and
kept in the vault; it never leaves the hub.

Each hub shows its totals, the models it has served, and its pooled accounts. **Refresh** reads the hub now;
`agentplaza models usage-providers usage <id>` prints the same. Remove a hub with its remove button, or
`agentplaza models usage-providers remove <id>`, which also deletes its key. `agentplaza models usage-providers list`
shows every hub with its last reading.

## 6. Favourites and the model picker

Star a model in a provider's model list, or run `agentplaza models favourite add <provider>/<model>`, to make it a
favourite. Favourites are saved on the hub, so every console and device sees the same list. Up to 200 are kept.
`agentplaza models favourite remove <provider>/<model>` takes one off.

The model picker, which every model choice uses (agent pages, the spawn dialog, the model policy and the decision
model), lists the favourites first. A rail filters the list to the favourites or to one provider's models. The picker
offers only the choices its caller gives, so everything it lists can be used.

## 7. Model policy and prices

The model policy says which models the orchestrator and the workers use, and the effort each one spends. It is set on the
Models screens beside the providers, and with `agentplaza models policy show` and `policy set`. Prices come from the price sources
and can be set by hand on a model's line in its provider's list; `agentplaza models price <provider>/<model>` shows one, and
`agentplaza models prices refresh` fetches the sources again. Prices are what the usage screens use for their costs.

## 8. The terminal app

The terminal app's Providers tab (press `m`) shows the same list, with the selected instance's setup beside it.

| Key | What it does |
|---|---|
| `Space` | Turn the selected provider or engine on or off |
| `c` / `C` | Check the selected one / check every provider |
| `f` | Favourite the model under the cursor |
| `a` | Add a provider or an engine |
| `k` | Set the selected API provider's key (masked; never shown) |
| `e` | Change the selected engine's runtime: version, arguments, variables and route |
| `d` | Remove the selected instance, after a yes |
| `u` | Update the engines: the dry run, then a yes |
| `h` | Add a usage hub |
| `U` | Move to the usage hubs; `d` removes the chosen one |
| `/` | Search the list |
| `r` | Refresh |

## 9. Command reference

| Command | What it does |
|---|---|
| `models providers [--group chat\|decisions] [--family api\|engine]` | List the instances |
| `models presets` | The provider kinds you can add |
| `models add <preset>` / `models add --id X --kind Y` | Add a provider |
| `models edit <id>` | Change an endpoint, its auth or its models; flags not given keep what they are |
| `models remove <id> [--yes]` | Remove a provider |
| `models key <id> --value -` | Store or change a key from stdin |
| `models test <id> [--model M]` | One short call to check the provider |
| `models list [--group chat\|decisions]` | Every model the providers list |
| `models enable <id>` / `models disable <id>` | Turn a provider or an engine on or off |
| `models check [id]` | Check one instance or all of them now |
| `models update [--dry-run]` | Move the npm engines to the pinned or latest versions |
| `models engines add\|set\|remove` | Add, change or remove an engine (`--registry`, `--command`, `--arg`, `--env`, `--route`, `--version`, `--label`) |
| `models registry search [query] [--refresh]` | Search the ACP registry |
| `models favourite add\|remove <provider>/<model>` | Favourite a model |
| `models usage-providers add\|list\|usage\|remove` | The CLIProxyAPI hubs |
| `models settings health-interval [seconds]` | Show or set the background check interval |
| `models policy`, `models decisions`, `models price`, `models prices` | The model policy, the decision model and the prices |

Removing an instance, a hub or an engine asks first unless you give `--yes`. The hub's HTTP routes are in the API
reference (`docs/api/openapi.json`, and the route families in `docs/architecture.md`).
