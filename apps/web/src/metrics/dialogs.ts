// The Metrics dialogs: a new (or edited) alert rule, and the metrics settings (collection level, retention, budgets,
// forecast basis, the opt-in outside endpoints).

import type { MetricDef, MetricsSettings, Rule, ScopeNode } from './model';
import { dialog, type DialogAction } from './overlays';
import { button, input, select } from '../ui/components';
import { h } from '../ui/dom';

interface Option {
  value: string;
  label: string;
}

/** The scope kinds a rule or a budget can name, as the scope tree lists them. */
const NAMEABLE_KINDS = ['global', 'node', 'run', 'role', 'agent', 'provider', 'model'];

/** The "every" scopes a rule can name besides a tree node. */
const EVERY_SCOPES: Option[] = [
  { value: 'every:node', label: 'every node' },
  { value: 'every:agent', label: 'every agent' },
  { value: 'every:run', label: 'every run' },
  { value: 'every:volume', label: 'every volume' },
];

const AGGREGATIONS: Option[] = ['p95', 'p50', 'max', 'avg', 'sum', 'last'].map((a) => ({ value: a, label: a }));

/** The "for" choices of a rule; the values are milliseconds. */
const FOR_CHOICES: Option[] = [
  { value: '0', label: '0 s' },
  { value: '60000', label: '1 min' },
  { value: '300000', label: '5 min' },
  { value: '900000', label: '15 min' },
  { value: '3600000', label: '1 h' },
];

/** The collection levels, each with the one-line description the plan gives it. */
const LEVELS: { value: MetricsSettings['level']; label: string; text: string }[] = [
  { value: 'off', label: 'Off', text: 'only what billing needs' },
  { value: 'basic', label: 'Basic', text: 'host and VM CPU and memory at 60 s, lifecycle, latency' },
  { value: 'standard', label: 'Standard', text: 'plus disk, IO, network at 10 s, context split, traces (default)' },
  { value: 'full', label: 'Full', text: 'plus 2 s everywhere and every trace; reverts to Standard after 24 h' },
];

/** The retention fields, each with its unit. */
const RETENTION: { key: keyof MetricsSettings['retention']; label: string; unit: string }[] = [
  { key: 'chunks_hours', label: 'Chunks', unit: 'hours' },
  { key: 'minute_days', label: 'Minute rollups', unit: 'days' },
  { key: 'hour_days', label: 'Hour rollups', unit: 'days' },
  { key: 'calls_days', label: 'Model calls', unit: 'days' },
  { key: 'traces_days', label: 'Kept traces', unit: 'days' },
  { key: 'traces_cap_mb', label: 'Kept traces, size cap', unit: 'MB' },
  { key: 'events_days', label: 'Events', unit: 'days' },
];

let uid = 0;
const nextId = (): string => `mx-dlg-${++uid}`;

/** The scope tree nodes a rule or a budget can name, as select options. */
function scopeOptions(scopes: ScopeNode[]): Option[] {
  return scopes.filter((s) => NAMEABLE_KINDS.includes(s.kind)).map((s) => ({ value: s.id, label: s.label }));
}

/** `options`, plus `value` when it is not there, so a preselected id is never silently dropped. */
function withValue(options: Option[], value: string | undefined): Option[] {
  if (value === undefined || options.some((o) => o.value === value)) return options;
  return [...options, { value, label: value }];
}

/** A grid row: the label (for `labelled`, else for `control`) and the control. */
function row(text: string, control: HTMLElement, labelled: HTMLElement = control): HTMLElement {
  if (!labelled.id) labelled.id = nextId();
  return h('div.mx-row', null, h('label', { for: labelled.id }, text), control);
}

/** A checkbox with its text; `box` is the control to read and set. */
function checkbox(text: string, on: boolean): { label: HTMLLabelElement; box: HTMLInputElement } {
  const box = h('input', { type: 'checkbox' });
  box.checked = on;
  return { label: h('label.mx-check', null, box, h('span', null, text)), box };
}

/** The error line of a dialog; hidden until a check or a save fails. */
function errorLine(): { el: HTMLElement; show(text: string): void; clear(): void } {
  const el = h('div.mx-flag.mx-crit', { role: 'alert', hidden: true });
  return {
    el,
    show(text: string): void {
      el.textContent = text;
      el.hidden = false;
    },
    clear(): void {
      el.textContent = '';
      el.hidden = true;
    },
  };
}

/** The text of a failed save, for the dialog. */
function errorText(e: unknown): string {
  const text = e instanceof Error ? e.message : String(e);
  return text || 'Could not save.';
}

/** A number from a number field, or NaN when the field is empty. */
function numberOf(el: HTMLInputElement): number {
  return el.value.trim() === '' ? NaN : Number(el.value);
}

function isHttpUrl(text: string): boolean {
  try {
    const url = new URL(text);
    return url.protocol === 'https:' || url.protocol === 'http:';
  } catch {
    return false;
  }
}

/** Copies `text`; `done` gets whether the copy worked. */
function copyText(text: string, done: (ok: boolean) => void): void {
  try {
    navigator.clipboard.writeText(text).then(
      () => done(true),
      () => done(false),
    );
  } catch {
    done(false);
  }
}

/** The alert rule dialog; `save` is called with the rule when the owner confirms. */
export function ruleDialog(catalog: MetricDef[], scopes: ScopeNode[], scope: string, save: (r: Rule) => Promise<void>, rule?: Rule): void {
  const titleOf = (id: string): string => catalog.find((m) => m.id === id)?.title ?? id;
  const signOf = (cmp: string): string => (cmp === 'lt' ? '<' : '>');

  const name = input({ type: 'text', placeholder: 'e.g. Builder spend over $25 a day' });
  const metric = select(withValue(catalog.map((m) => ({ value: m.id, label: m.title })), rule?.metric), rule?.metric);
  const agg = select(withValue(AGGREGATIONS, rule?.agg), rule?.agg ?? 'p95');
  const scopeChoices = withValue([...EVERY_SCOPES, ...scopeOptions(scopes)], rule?.scope ?? scope);
  const scopeBox = select(scopeChoices, rule?.scope ?? scope);
  const cmp = select(
    [
      { value: 'gt', label: '>' },
      { value: 'lt', label: '<' },
    ],
    rule?.cmp ?? 'gt',
  );
  const threshold = input({ type: 'number', step: 'any', value: rule ? String(rule.threshold) : '90' });
  const forBox = select(withValue(FOR_CHOICES, rule ? String(rule.for_ms) : undefined), rule ? String(rule.for_ms) : '300000');
  const severity = select(
    [
      { value: 'warning', label: 'warning' },
      { value: 'critical', label: 'critical' },
    ],
    rule?.severity ?? 'warning',
  );
  const notifyConsole = checkbox('console', rule?.notify.console ?? true);
  const notifyDesktop = checkbox('desktop notification', rule?.notify.desktop ?? true);
  const notifyBoard = checkbox('board message', rule?.notify.board ?? false);

  // The name follows the metric, the sign and the threshold until the owner types their own.
  const defaultName = (): string => `${titleOf(metric.value)} ${signOf(cmp.value)} ${threshold.value.trim()}`;
  let nameTouched = rule !== undefined;
  name.value = rule?.name ?? defaultName();
  name.addEventListener('input', () => {
    nameTouched = true;
  });
  const refreshName = (): void => {
    if (!nameTouched) name.value = defaultName();
  };
  metric.addEventListener('change', refreshName);
  cmp.addEventListener('change', refreshName);
  threshold.addEventListener('input', refreshName);

  const err = errorLine();
  let saving = false;
  let close: () => void = () => undefined;

  const submit = (): boolean => {
    if (saving) return false;
    err.clear();
    if (!metric.value) {
      err.show('Pick a metric.');
      return false;
    }
    const limit = numberOf(threshold);
    if (!Number.isFinite(limit)) {
      err.show('Enter the threshold as a number.');
      return false;
    }
    const built: Rule = {
      id: rule?.id ?? '',
      name: name.value.trim() || defaultName(),
      metric: metric.value,
      agg: agg.value,
      scope: scopeBox.value,
      cmp: cmp.value as Rule['cmp'],
      threshold: limit,
      for_ms: Number(forBox.value),
      severity: severity.value as Rule['severity'],
      on: true,
      notify: { console: notifyConsole.box.checked, desktop: notifyDesktop.box.checked, board: notifyBoard.box.checked },
      starter: false,
    };
    saving = true;
    void (async () => {
      try {
        await save(built);
        close();
      } catch (e) {
        saving = false;
        err.show(errorText(e));
      }
    })();
    return false;
  };

  const body = h(
    'div.mx-form',
    null,
    row('Name', name),
    row('Metric', metric),
    row('Aggregation', agg),
    row('Scope', scopeBox),
    row('Condition', h('div.mx-inline', null, cmp, threshold), cmp),
    row('For', forBox),
    row('Severity', severity),
    h('div.mx-row', null, h('label', null, 'Notify'), h('div.mx-checks', null, notifyConsole.label, notifyDesktop.label, notifyBoard.label)),
    h('p.mx-sub', null, "Rules run on the hub's in-memory aggregates as points arrive, so they cost no database queries."),
    err.el,
  );

  const actions: DialogAction[] = [
    { label: 'Cancel' },
    { label: rule ? 'Save rule' : 'Create rule', primary: true, run: submit },
  ];
  close = dialog(rule ? 'Edit alert rule' : 'New alert rule', body, actions);
}

/** The settings dialog; `save` returns the saved settings (and the OpenMetrics token once, when just made). */
export function settingsDialog(current: MetricsSettings, scopes: ScopeNode[], save: (s: MetricsSettings) => Promise<{ openmetrics_token?: string }>): void {
  const err = errorLine();
  let saving = false;
  let tokenShown = false;
  let close: () => void = () => undefined;

  // Collection level.
  const radioGroup = nextId();
  const levels = LEVELS.map((l) => {
    const box = h('input', { type: 'radio', name: radioGroup, value: l.value });
    box.checked = current.level === l.value;
    return { value: l.value, box, label: h('label.mx-radio', null, box, h('span', null, l.label), h('span.mx-hint', null, l.text)) };
  });

  // Retention.
  const retention = RETENTION.map((r) => {
    const box = input({ type: 'number', min: 1, step: 1, value: current.retention[r.key] });
    return { key: r.key, label: r.label, box, row: row(r.label, h('div.mx-inline', null, box, h('span.mx-unit', null, r.unit)), box) };
  });

  // Budgets: the monthly budget, and one daily cap per scope.
  const choices = scopeOptions(scopes);
  const monthly = input({ type: 'number', min: 0, step: 'any', value: current.monthly_budget_usd ?? '' });
  const caps: { scope: HTMLSelectElement; usd: HTMLInputElement }[] = [];
  const capList = h('div.mx-caps', null);
  const addCap = (scopeValue: string, usd: number | string): void => {
    const scopeBox = select(withValue(choices, scopeValue), scopeValue);
    const usdBox = input({ type: 'number', min: 0, step: 'any', value: usd, 'aria-label': 'Daily cap in USD' });
    const cap = { scope: scopeBox, usd: usdBox };
    const line = h(
      'div.mx-row',
      null,
      scopeBox,
      h(
        'div.mx-inline',
        null,
        usdBox,
        h('span.mx-unit', null, 'USD'),
        button(
          'Remove',
          () => {
            const at = caps.indexOf(cap);
            if (at >= 0) caps.splice(at, 1);
            line.remove();
          },
          { kind: 'ghost', size: 'sm' },
        ),
      ),
    );
    caps.push(cap);
    capList.appendChild(line);
  };
  for (const cap of current.budgets) addCap(cap.scope, cap.cap_usd);
  const addButton = button(
    'Add daily cap',
    () => {
      const used = new Set(caps.map((c) => c.scope.value));
      const free = choices.find((o) => !used.has(o.value));
      addCap(free?.value ?? choices[0]?.value ?? 'global', '');
    },
    { kind: 'ghost', size: 'sm' },
  );

  // Forecast basis.
  const basis = select(
    [
      { value: '14', label: '14 days' },
      { value: '30', label: '30 days' },
    ],
    current.basis_days === 14 ? '14' : '30',
  );

  // Outside endpoints: both off until the owner turns them on.
  const openmetrics = checkbox('/metrics for an outside scraper, with its own token', current.outside.openmetrics);
  const otlp = checkbox('OTLP export', current.outside.otlp);
  const otlpUrl = input({ type: 'text', placeholder: 'https://collector.example.com:4318', value: current.outside.otlp_url });
  const otlpConfirm = checkbox('send kept traces to this collector', current.outside.otlp_confirmed);
  const syncOtlp = (): void => {
    otlpUrl.disabled = !otlp.box.checked;
    otlpConfirm.box.disabled = !otlp.box.checked;
  };
  otlp.box.addEventListener('change', syncOtlp);
  syncOtlp();

  const section = (title: string, ...children: Node[]): HTMLElement => h('section.mx-sec', null, h('h3', null, title), ...children);

  const body = h(
    'div.mx-form',
    null,
    section('Collection level', h('div.mx-radios', null, levels.map((l) => l.label))),
    section('Retention', ...retention.map((r) => r.row)),
    section(
      'Budgets',
      row('Monthly budget', h('div.mx-inline', null, monthly, h('span.mx-unit', null, 'USD')), monthly),
      h(
        'div.mx-row',
        null,
        h('label', null, 'Daily caps'),
        h('div.mx-stack', null, capList, h('div', null, addButton), h('p.mx-sub', null, 'Laws enforce caps; Metrics only reports them.')),
      ),
    ),
    section('Forecast basis', row('Basis', basis)),
    section(
      'Outside endpoints',
      openmetrics.label,
      otlp.label,
      row('Collector URL', otlpUrl),
      otlpConfirm.label,
    ),
    err.el,
  );

  /** Replaces the form with the token, shown once; the Save button becomes Done. */
  const showToken = (d: HTMLElement, token: string): void => {
    tokenShown = true;
    const copy = button(
      'Copy',
      () => {
        copyText(token, (ok) => {
          copy.textContent = ok ? 'Copied' : 'Copy failed; select the token';
        });
      },
      { kind: 'ghost', size: 'sm' },
    );
    body.replaceChildren(
      section('OpenMetrics token', h('p.mx-sub', null, 'Copy it now; it is not shown again.'), h('div.mx-token', null, token), copy),
    );
    const saveBtn = [...d.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Save settings');
    if (saveBtn) saveBtn.textContent = 'Done';
  };

  const submit = (d: HTMLElement): boolean | void => {
    if (tokenShown) return;
    if (saving) return false;
    err.clear();

    const retentionValues = {} as MetricsSettings['retention'];
    for (const r of retention) {
      const days = numberOf(r.box);
      if (!Number.isInteger(days) || days < 1) {
        err.show(`${r.label}: enter a whole number of at least 1.`);
        r.box.focus();
        return false;
      }
      retentionValues[r.key] = days;
    }

    const monthlyText = monthly.value.trim();
    const monthlyUsd = monthlyText === '' ? null : Number(monthlyText);
    if (monthlyUsd !== null && !(Number.isFinite(monthlyUsd) && monthlyUsd >= 0)) {
      err.show('Monthly budget: enter a USD amount of 0 or more, or leave it empty.');
      return false;
    }

    const budgets: MetricsSettings['budgets'] = [];
    for (const cap of caps) {
      const usd = numberOf(cap.usd);
      if (!(Number.isFinite(usd) && usd >= 0)) {
        err.show('Daily caps: each cap needs a USD amount of 0 or more.');
        return false;
      }
      if (budgets.some((b) => b.scope === cap.scope.value)) {
        err.show('Daily caps: each scope can have one cap.');
        return false;
      }
      budgets.push({ scope: cap.scope.value, cap_usd: usd });
    }

    if (otlp.box.checked) {
      if (!isHttpUrl(otlpUrl.value.trim())) {
        err.show('OTLP export: enter the collector URL, starting with http:// or https://.');
        return false;
      }
      if (!otlpConfirm.box.checked) {
        err.show('OTLP export: confirm that kept traces may be sent to this collector.');
        return false;
      }
    }

    const picked = levels.find((l) => l.box.checked);
    const next: MetricsSettings = {
      ...current,
      level: (picked?.value ?? current.level) as MetricsSettings['level'],
      retention: retentionValues,
      budgets,
      monthly_budget_usd: monthlyUsd,
      basis_days: Number(basis.value),
      outside: {
        openmetrics: openmetrics.box.checked,
        otlp: otlp.box.checked,
        otlp_url: otlpUrl.value.trim(),
        otlp_confirmed: otlpConfirm.box.checked,
      },
    };

    saving = true;
    void (async () => {
      try {
        const saved = await save(next);
        if (saved.openmetrics_token) {
          saving = false;
          showToken(d, saved.openmetrics_token);
          return;
        }
        close();
      } catch (e) {
        saving = false;
        err.show(errorText(e));
      }
    })();
    return false;
  };

  const actions: DialogAction[] = [{ label: 'Cancel' }, { label: 'Save settings', primary: true, run: submit }];
  close = dialog('Metrics settings', body, actions);
}
