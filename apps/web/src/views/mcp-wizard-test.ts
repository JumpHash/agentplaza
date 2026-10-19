// MCP wizard steps 3 (Test) and 4 (Tools). The shell (mcp-wizard.ts) owns navigation and persistence:
// step 3 saves the draft itself when it has not been saved yet, step 4 leaves persisting to Next.
import '../styles/proto/settings-b.css';
import { api, mcpApi } from '../api';
import type { McpProbeTool, McpRisk, McpTestResult, McpToolRow } from '../api';
import { attempt, badge, clip, field, select } from '../ui/components';
import type { Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { RISKS, btn, tbl } from './mcp-common';
import type { WizardCtx } from './mcp-wizard';

const RISK_TONE: Record<McpRisk, Tone> = { read: 'ok', write: 'warn', irreversible: 'danger' };

/** This hub first, then the nodes that have not been revoked. */
async function nodeChoices(): Promise<{ value: string; label: string }[]> {
  const nodes = (await attempt(() => api.nodes())) ?? [];
  return [
    { value: 'n0', label: 'This hub' },
    ...nodes.filter((n) => !n.revoked && n.id !== 'n0').map((n) => ({ value: n.id, label: n.name })),
  ];
}

/** Several nodes in a row, separated by a space. */
function joined(nodes: Node[]): DocumentFragment {
  const f = document.createDocumentFragment();
  nodes.forEach((n, i) => {
    if (i) f.append(' ');
    f.append(n);
  });
  return f;
}

/** Hint chips for the annotations a tool advertises. Only boolean hints are shown. */
function hintChips(annotations: unknown): Node[] {
  const a = (annotations ?? {}) as Record<string, unknown>;
  return (['readOnlyHint', 'destructiveHint'] as const)
    .filter((k) => typeof a[k] === 'boolean')
    .map((k) => h('span.mcp-chip', { title: k }, `${k}: ${a[k] ? 'true' : 'false'}`));
}

function toolsTable(tools: McpProbeTool[]): HTMLElement {
  return tbl<McpProbeTool>(
    [
      {
        label: 'Tool',
        cell: (t) => joined([h('strong', null, t.name), h('span.mono.muted', null, t.agent_name)]),
      },
      { label: 'Description', cell: (t) => clip(t.description, 140) },
      { label: 'Annotations', cell: (t) => joined(hintChips(t.annotations)) },
      { label: 'Risk', cell: (t) => badge(t.risk, RISK_TONE[t.risk]) },
    ],
    tools,
    (t) => t.name,
  );
}

/** The result under the Run button: a failure with its error and stderr tail, or the tools the server reports. */
function resultView(res: McpTestResult): (Node | null)[] {
  if (!res.ok) {
    const stderr = res.stderr ?? [];
    return [
      badge('Test failed', 'danger'),
      h('p.mcpwz-err', null, res.error || 'The server did not start.'),
      stderr.length ? h('pre.mcp-pre.mono', null, stderr.map((l) => l.line).join('\n')) : null,
    ];
  }
  const info = res.server_info;
  const tools = res.tools ?? [];
  return [
    badge('Test passed', 'ok'),
    info ? h('p.muted', null, `${info.name} ${info.version} · protocol ${info.protocol}`) : null,
    tools.length ? toolsTable(tools) : h('p.muted', null, 'The server reports no tools.'),
  ];
}

/** Step 3: run the server once on a node and show what it reports. */
export function step3Test(ctx: WizardCtx): HTMLElement {
  let node = 'n0';
  let running = false;
  let ran = false;

  const sel = select([{ value: 'n0', label: 'This hub' }], 'n0');
  sel.setAttribute('aria-label', 'Node');
  sel.addEventListener('change', () => {
    node = sel.value;
  });
  const resultBox = h('div.mcpwz-res');
  const runBtn = btn('Run test', () => void run(), { kind: 'primary' });
  const setRunLabel = (text: string): void => {
    const label = runBtn.querySelector('span');
    if (label) label.textContent = text;
  };

  async function run(): Promise<void> {
    if (running) return;
    running = true;
    runBtn.disabled = true;
    setRunLabel('Testing');
    replace(resultBox, h('p.muted', null, 'Testing the server. This can take a minute.'));
    try {
      if (!ctx.state.saved && !(await ctx.persist())) {
        replace(resultBox, h('p.mcpwz-err', null, 'The server is not saved yet. Fix the field named above, then run the test again.'));
        return;
      }
      const id = ctx.state.draft.id;
      const res = await attempt(() => mcpApi.testServer(id, node));
      if (!res) {
        replace(resultBox, h('p.mcpwz-err', null, 'The test did not finish. Try again.'));
        return;
      }
      // The probe carries each tool's agent name; step 4 reads the same list when the test passed.
      ctx.state.probe = res.ok ? (res.tools ?? []) : null;
      ran = true;
      replace(resultBox, resultView(res));
    } finally {
      running = false;
      runBtn.disabled = false;
      setRunLabel(ran ? 'Run test again' : 'Run test');
    }
  }

  void nodeChoices().then((choices) => {
    replace(
      sel,
      choices.map((c) => h('option', { value: c.value }, c.label)),
    );
    sel.value = node;
  });

  // The body is the shell's `.mcpwz-body` column, so its children are spaced the same way.
  return h(
    'div.mcpwz-body',
    null,
    h('p.muted', null, 'Run the server once to check that it starts and lists its tools. Unsaved changes are saved first.'),
    field('Node', sel, 'The node that runs the test.'),
    h('div.mcpwz-row', null, runBtn),
    resultBox,
  );
}

/** The risk the hub derives from a tool's hints. Only used when the hub's own auto value is not known. */
function autoRiskOf(row: McpToolRow): McpRisk {
  const a = (row.annotations ?? {}) as { readOnlyHint?: unknown; destructiveHint?: unknown };
  if (a.destructiveHint === true) return 'irreversible';
  if (a.readOnlyHint === true) return 'read';
  return 'write';
}

/** Step 4: pick the tools agents may call, and each tool's risk. The shell persists on Next. */
export function step4Tools(ctx: WizardCtx): HTMLElement {
  const id = ctx.state.draft.id;
  const body = h('div.mcpwz-body', null, h('p.muted', null, 'Loading tools'));
  // The hub's own risk for each tool that had no override when this step opened.
  const autoRisk = new Map<string, McpRisk>();

  const draw = (rows: McpToolRow[]): void => {
    const tools = ctx.state.draft.tools;
    const allow = tools.allow;
    const allAll = allow === 'all';
    const picked = new Set<string>(allow === 'all' ? [] : allow);

    const allowRadio = (value: 'all' | 'pick', text: string): HTMLLabelElement => {
      const r = h('input', { type: 'radio', name: 'mcpwz-allow', value, checked: value === 'all' ? allAll : !allAll });
      r.addEventListener('change', () => {
        if (value === 'all') {
          ctx.state.draft.tools.allow = 'all';
        } else if (ctx.state.draft.tools.allow === 'all') {
          ctx.state.draft.tools.allow = rows.map((row) => row.name);
        }
        draw(rows);
      });
      return h('label.check', null, r, h('span', null, text));
    };
    const radios = h('div.mcpwz-row', null, allowRadio('all', 'Allow all tools'), allowRadio('pick', 'Pick tools'));

    if (!rows.length) {
      replace(
        body,
        radios,
        h('p.muted', null, ctx.state.probe ? 'The server reports no tools.' : 'Run the test in step 3 first. Tools appear once the server has run.'),
      );
      return;
    }

    const toolRow = (row: McpToolRow): HTMLElement => {
      const override = tools.risk[row.name];
      const auto = autoRisk.get(row.name) ?? autoRiskOf(row);

      const pickBox = h('input', { type: 'checkbox', disabled: allAll });
      pickBox.checked = allAll || picked.has(row.name);
      pickBox.addEventListener('change', () => {
        const cur = ctx.state.draft.tools.allow;
        if (cur === 'all') return;
        const next = new Set(cur);
        if (pickBox.checked) next.add(row.name);
        else next.delete(row.name);
        ctx.state.draft.tools.allow = [...next];
      });

      const risk = select(
        [{ value: '', label: `auto (${auto})` }, ...RISKS.map((r) => ({ value: r, label: r }))],
        override ?? '',
      );
      risk.setAttribute('aria-label', `Risk for ${row.name}`);
      risk.addEventListener('change', () => {
        const map = ctx.state.draft.tools.risk;
        if (risk.value === '') delete map[row.name];
        else map[row.name] = risk.value as McpRisk;
      });

      return h(
        'div.mcpwz-tool',
        null,
        h('label.check', null, pickBox, ' ', h('strong', null, row.name), ' ', h('span.mono.muted', null, row.tool)),
        h('p.muted', null, clip(row.description || '', 140)),
        risk,
      );
    };

    replace(
      body,
      radios,
      h('p.muted', null, "Risk sets how a call is treated. Auto uses the risk the hub derives from each tool's hints."),
      rows.map(toolRow),
    );
  };

  const load = async (): Promise<void> => {
    const probe = ctx.state.probe;
    if (probe) {
      // A passing test in this session: its tools are the source, and their auto risk is the hub's own.
      for (const t of probe) autoRisk.set(t.name, t.auto_risk);
      draw(
        probe.map((t) => ({
          name: t.name,
          tool: t.agent_name,
          description: t.description,
          input_schema: null,
          annotations: t.annotations,
          risk: t.risk,
          allowed: true,
        })),
      );
      return;
    }
    // Editing a saved server with no test in this session: the hub's own list.
    const rows = ctx.state.mode === 'edit' && ctx.state.saved ? ((await attempt(() => mcpApi.serverTools(id))) ?? []) : [];
    for (const row of rows) {
      if (ctx.state.draft.tools.risk[row.name] === undefined) autoRisk.set(row.name, row.risk);
    }
    draw(rows);
  };
  void load();

  return body;
}
