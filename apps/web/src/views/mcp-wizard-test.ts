// MCP wizard steps 3 (Test) and 4 (Tools). The shell (mcp-wizard.ts) owns navigation and persistence:
// step 3 saves the draft itself when it has not been saved yet, step 4 leaves persisting to Next.
import '../styles/mcp-wizard-test.css';
import { api, mcpApi } from '../api';
import type { McpLogLine, McpProbeTool, McpRisk, McpTestResult, McpToolRow } from '../api';
import { attempt, badge, button, clip, field, select, table } from '../ui/components';
import type { Column, Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { RISKS } from './mcp-common';
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

/** Hint chips for the annotations a tool advertises. Only boolean hints are shown. */
function hintChips(annotations: unknown): HTMLElement[] {
  const a = (annotations ?? {}) as Record<string, unknown>;
  return (['readOnlyHint', 'destructiveHint'] as const)
    .filter((k) => typeof a[k] === 'boolean')
    .map((k) => h('span.chip', { title: k }, `${k}: ${a[k] ? 'true' : 'false'}`));
}

/** The stderr tail as mono lines. Lines are already redacted by the hub. */
function stderrBlock(lines: McpLogLine[]): HTMLElement | null {
  if (!lines.length) return null;
  return h(
    'pre.wz-log',
    {},
    lines.map((l) => h('div', { class: l.stream === 'stderr' ? 'err' : '' }, l.line)),
  );
}

function toolsTable(tools: McpProbeTool[], names: Map<string, string>): HTMLElement {
  const columns: Column<McpProbeTool>[] = [
    {
      label: 'Tool',
      cell: (t) =>
        h('div.wz-tool-name', {}, h('strong', {}, t.name.replace(/_/g, '_\u200b')), names.has(t.name) ? h('span.mono.muted', {}, names.get(t.name) ?? '') : null),
    },
    { label: 'Description', cell: (t) => clip(t.description, 140) },
    { label: 'Annotations', cell: (t) => h('div.wz-chips', {}, hintChips(t.annotations)) },
    { label: 'Risk', cell: (t) => badge(t.risk, RISK_TONE[t.risk]) },
  ];
  return h('div.wz-scroll', {}, table(columns, tools));
}

function resultView(res: McpTestResult, names: Map<string, string>): HTMLElement {
  if (!res.ok) {
    return h(
      'div.wz-test-result',
      {},
      badge('Test failed', 'danger', true),
      h('p.wz-test-error', {}, res.error ?? 'The server did not start.'),
      stderrBlock(res.stderr ?? []),
    );
  }
  const info = res.server_info;
  const tools = res.tools ?? [];
  return h(
    'div.wz-test-result',
    {},
    h(
      'div.wz-test-head',
      {},
      badge('Test passed', 'ok', true),
      info ? h('span.muted', {}, `${info.name} ${info.version} · protocol ${info.protocol}`) : null,
    ),
    tools.length ? toolsTable(tools, names) : h('p.muted', {}, 'The server reports no tools.'),
  );
}

/** Step 3: run the server once on a node and show what it reports. */
export function step3Test(ctx: WizardCtx): HTMLElement {
  let node = 'n0';
  let running = false;
  let ran = false;

  const nodeSlot = h('div.wz-node-slot', {}, select([{ value: 'n0', label: 'This hub' }], 'n0'));
  const resultBox = h('div.wz-test-box', {});

  const runBtn = button('Run test', () => void run(), { kind: 'primary' });

  async function run(): Promise<void> {
    if (running) return;
    running = true;
    runBtn.disabled = true;
    runBtn.textContent = 'Testing';
    replace(resultBox, h('p.muted', {}, 'Testing the server. This can take a minute.'));
    try {
      if (!ctx.state.saved && !(await ctx.persist())) {
        replace(resultBox, h('p.muted', {}, 'The server is not saved yet. Fix the field named above, then run the test again.'));
        return;
      }
      const id = ctx.state.draft.id;
      const res = await attempt(() => mcpApi.testServer(id, node));
      if (!res) {
        replace(resultBox, h('p.muted', {}, 'The test did not finish. Try again.'));
        return;
      }
      // The probe carries each tool's agent name; step 4 reads the same list when the test passed.
      ctx.state.probe = res.ok ? (res.tools ?? []) : null;
      const names = new Map((res.tools ?? []).map((t) => [t.name, t.agent_name] as const));
      ran = true;
      replace(resultBox, resultView(res, names));
    } finally {
      running = false;
      runBtn.disabled = false;
      runBtn.textContent = ran ? 'Run test again' : 'Run test';
    }
  }

  void nodeChoices().then((choices) => {
    const sel = select(choices, node);
    sel.addEventListener('change', () => {
      node = sel.value;
    });
    replace(nodeSlot, sel);
  });

  return h(
    'div.wz-test',
    {},
    h('p.muted', {}, 'Run the server once to check that it starts and lists its tools. Unsaved changes are saved first.'),
    field('Node', nodeSlot, 'The node that runs the test.'),
    h('div.wz-test-actions', {}, runBtn),
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
  const body = h('div.wz-tools-body', {}, h('p.muted', {}, 'Loading tools'));
  // The hub's own risk for each tool that had no override when this step opened.
  const autoRisk = new Map<string, McpRisk>();

  const draw = (rows: McpToolRow[]): void => {
    if (!rows.length) {
      replace(
        body,
        h(
          'p.wz-note',
          {},
          ctx.state.probe ? 'The server reports no tools.' : 'Run the test in step 3 first. Tools appear once the server has run.',
        ),
      );
      return;
    }
    const tools = ctx.state.draft.tools;
    const allow = tools.allow;
    const allAll = allow === 'all';
    const picked = new Set<string>(allow === 'all' ? [] : allow);

    const radio = (value: 'all' | 'pick', text: string): HTMLElement => {
      const r = h('input', { type: 'radio', name: 'mcp-tools-allow', value });
      r.checked = (value === 'all') === allAll;
      r.addEventListener('change', () => {
        if (value === 'all') {
          ctx.state.draft.tools.allow = 'all';
        } else if (ctx.state.draft.tools.allow === 'all') {
          ctx.state.draft.tools.allow = rows.map((row) => row.name);
        }
        draw(rows);
      });
      return h('label.wz-radio', {}, r, text);
    };

    const list = rows.map((row) => {
      const override = tools.risk[row.name];
      const auto = autoRisk.get(row.name) ?? autoRiskOf(row);

      const check = h('input', { type: 'checkbox', disabled: allAll });
      check.checked = allAll || picked.has(row.name);
      check.addEventListener('change', () => {
        const cur = ctx.state.draft.tools.allow;
        if (cur === 'all') return;
        const next = new Set(cur);
        if (check.checked) next.add(row.name);
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
        'div.wz-tool-row',
        {},
        h(
          'label.wz-tool-pick',
          {},
          check,
          h(
            'span.wz-tool-text',
            {},
            h('strong', {}, row.name),
            h('span.mono.muted', {}, row.tool),
            row.description ? h('span.muted.wz-tool-desc', {}, clip(row.description, 140)) : null,
          ),
        ),
        risk,
      );
    });

    replace(
      body,
      h(
        'div.wz-radios',
        {},
        radio('all', 'Allow all tools'),
        radio('pick', 'Pick tools'),
      ),
      h('p.wz-note', {}, 'Risk sets how a call is treated. Auto uses the risk the hub derives from each tool\'s hints.'),
      h('div.wz-tool-list', {}, list),
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
