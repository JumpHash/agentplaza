// The Import .env dialog. The file or pasted text is read here for names only, sent once, then cleared.
// No value is shown, kept after submit, or logged. The raw lines are held only for that one request.
import '../styles/vault-import.css';
import { button, clip, field, modal } from '../ui/components';
import { h, replace } from '../ui/dom';
import { vaultApi } from '../api';
import type { VaultConsumer, VaultImportBody, VaultImportResult, VaultScope } from '../api';
import { VAULT_NAME_RE, bindingLabel, errorText, nodeName } from './vault-common';
import type { VaultCtx } from './vault-common';

interface Line {
  raw: string;
  /** The env name, when the line has one. */
  name: string | null;
  valid: boolean;
  /** A line with no name or no "=", described by its number only. */
  problem: string | null;
}

/** Splits dotenv text into lines and reads only the names. */
function parseEnv(text: string): Line[] {
  return text.split(/\r?\n/).map((raw, i): Line => {
    const t = raw.trim();
    if (!t || t.startsWith('#')) return { raw, name: null, valid: false, problem: null };
    const body = t.startsWith('export ') ? t.slice(7).trimStart() : t;
    const eq = body.indexOf('=');
    if (eq < 0) return { raw, name: null, valid: false, problem: `line ${i + 1} has no "="` };
    const name = body.slice(0, eq).trim();
    if (!name) return { raw, name: null, valid: false, problem: `line ${i + 1} has no name` };
    return { raw, name, valid: VAULT_NAME_RE.test(name), problem: null };
  });
}

const plural = (n: number) => (n === 1 ? 'name' : 'names');

function checkRow(checked: boolean, label: Node, onChange: (on: boolean) => void): HTMLLabelElement {
  const box = h('input', { type: 'checkbox' });
  box.checked = checked;
  box.addEventListener('change', () => onChange(box.checked));
  return h('label.vimp-check', null, box, label);
}

export function openImportDialog(ctx: VaultCtx): void {
  const consumers: VaultConsumer[] = ctx.data.consumers;
  const nodes = ctx.data.status.nodes;

  let lines: Line[] = [];
  let valid: string[] = [];
  let invalid: string[] = [];
  let problems: string[] = [];
  let picked = new Set<string>();
  let ticks = new Set<string>();
  let sourceName: string | null = null;
  let scopeKind: 'plaza' | 'nodes' = 'plaza';
  const nodeSet = new Set<string>();
  let busy = false;

  const fileInput = h('input', { type: 'file', accept: '.env,text/plain', 'aria-label': 'Choose a .env file' });
  const ta = h('textarea.textarea.vimp-masked', {
    rows: 6,
    autocomplete: 'off',
    spellcheck: 'false',
    wrap: 'off',
    'aria-label': 'Paste .env contents',
    placeholder: 'NAME=value, one per line',
  });
  const preview = h('div.vimp-preview', null);
  const errorBox = h('p.vimp-error', { role: 'alert' });
  errorBox.hidden = true;
  const nodeBox = h(
    'div.vimp-names',
    null,
    nodes.map((n) =>
      checkRow(nodeSet.has(n.id), h('span', null, nodeName(n.id, nodes)), (on) => {
        if (on) nodeSet.add(n.id);
        else nodeSet.delete(n.id);
      }),
    ),
  );
  nodeBox.hidden = true;
  const submitBtn = button('Import', () => void submit(), { kind: 'primary' });

  const fail = (msg: string) => {
    errorBox.textContent = msg;
    errorBox.hidden = false;
  };
  const hideError = () => {
    errorBox.textContent = '';
    errorBox.hidden = true;
  };

  function refresh(): void {
    const n = picked.size;
    submitBtn.textContent = n ? `Import ${n} ${plural(n)}` : 'Import';
    submitBtn.disabled = ctx.locked || busy || n === 0;
  }

  const bindRow = (c: VaultConsumer) =>
    checkRow(
      ticks.has(c.binding),
      h('span', null, bindingLabel(c.binding, consumers), ' ', h('span.mono.vimp-sub', null, `wants ${c.wants}`)),
      (on) => {
        if (on) ticks.add(c.binding);
        else ticks.delete(c.binding);
      },
    );

  function renderPreview(): void {
    const parts: Node[] = [];
    if (!valid.length && !invalid.length && !problems.length) {
      parts.push(h('p.vimp-muted', null, 'Choose a file or paste the contents to see the names.'));
    } else {
      const from = sourceName ? `from ${sourceName}` : 'from the pasted text';
      parts.push(h('p.vimp-count', null, `${valid.length} ${plural(valid.length)} found ${from}. Values are not shown.`));
      if (valid.length) {
        parts.push(
          h(
            'div.vimp-names',
            null,
            valid.map((n) =>
              checkRow(picked.has(n), h('span.mono', null, n), (on) => {
                if (on) picked.add(n);
                else picked.delete(n);
                refresh();
              }),
            ),
          ),
        );
      }
      if (invalid.length) {
        parts.push(
          h(
            'p.vimp-muted',
            null,
            'Not valid names, so skipped: ',
            invalid.map((n) => h('span.mono.vimp-chip', null, clip(n, 40))),
          ),
        );
      }
      if (problems.length) {
        parts.push(h('p.vimp-muted', null, `Skipped ${problems.length} ${problems.length === 1 ? 'line' : 'lines'}: ${problems.join(', ')}.`));
      }
      if (valid.length) {
        parts.push(
          h(
            'div.vimp-section.vimp-bindings',
            null,
            h('h3.vimp-h', null, 'Bindings'),
            h('p.vimp-muted', null, 'A name gets the ticked bindings that want it.'),
            consumers.length
              ? h('div.vimp-names', null, consumers.map(bindRow))
              : h('p.vimp-muted', null, 'Nothing asks for a secret yet. The names import without bindings.'),
          ),
        );
      }
    }
    replace(preview, ...parts);
    refresh();
  }

  function load(raw: string, source: string | null): void {
    hideError();
    lines = parseEnv(raw);
    sourceName = source;
    valid = Array.from(new Set(lines.filter((l) => l.valid).map((l) => l.name ?? '')));
    invalid = Array.from(new Set(lines.filter((l) => l.name && !l.valid).map((l) => l.name ?? '')));
    problems = lines.filter((l) => l.problem).map((l) => l.problem ?? '');
    picked = new Set(valid);
    const wanted = new Set(valid);
    ticks = new Set(consumers.filter((c) => wanted.has(c.wants)).map((c) => c.binding));
    renderPreview();
  }

  /** Drops every parsed line, name and typed or pasted text. */
  function clearSource(): void {
    lines = [];
    valid = [];
    invalid = [];
    problems = [];
    sourceName = null;
    picked = new Set();
    ticks = new Set();
    ta.value = '';
    fileInput.value = '';
    renderPreview();
  }

  function showResult(res: VaultImportResult): void {
    const n = res.imported.length;
    replace(
      body,
      h('p.vimp-count', null, n ? `Imported ${n} ${plural(n)}.` : 'Nothing was imported.'),
      n
        ? h(
            'ul.vimp-list',
            null,
            res.imported.map((i) => h('li', null, h('span.mono', null, i.name), ' ', h('span.mono.vimp-sub', null, i.id))),
          )
        : null,
      res.skipped.length ? h('h3.vimp-h', null, `Skipped ${res.skipped.length}`) : null,
      res.skipped.length
        ? h(
            'ul.vimp-list',
            null,
            res.skipped.map((s) => h('li', null, h('span.mono', null, s.name), ` ${s.reason}`)),
          )
        : null,
      h('div.vimp-foot', null, button('Done', () => close(), { kind: 'primary' })),
    );
  }

  async function submit(): Promise<void> {
    if (busy) return;
    hideError();
    const names = valid.filter((n) => picked.has(n));
    if (!names.length) return fail('Pick at least one name to import.');
    if (scopeKind === 'nodes' && nodeSet.size === 0) return fail('Pick at least one node, or choose plaza.');
    const keep = new Set(names);
    const scope: VaultScope = scopeKind === 'nodes' ? { kind: 'nodes', nodes: Array.from(nodeSet) } : { kind: 'plaza' };
    const bindings: Record<string, string[]> = {};
    for (const n of names) {
      const list = Array.from(new Set(consumers.filter((c) => c.wants === n && ticks.has(c.binding)).map((c) => c.binding)));
      if (list.length) bindings[n] = list;
    }
    const payload: VaultImportBody = {
      format: 'dotenv',
      text: lines
        .filter((l) => l.valid && l.name !== null && keep.has(l.name))
        .map((l) => l.raw)
        .join('\n'),
      scope,
      bindings,
    };
    clearSource();
    busy = true;
    refresh();
    let result: VaultImportResult | undefined;
    try {
      result = await vaultApi.importEnv(payload);
    } catch (err) {
      fail(`${errorText(err)} The text was cleared, so paste it again to retry.`);
    } finally {
      payload.text = '';
      busy = false;
      refresh();
    }
    if (result) {
      void ctx.reload();
      showResult(result);
    }
  }

  ta.addEventListener('input', () => {
    fileInput.value = '';
    load(ta.value, null);
  });
  fileInput.addEventListener('change', () => {
    const file = fileInput.files?.[0];
    if (!file) return;
    ta.value = '';
    void file.text().then(
      (raw) => load(raw, file.name),
      () => fail('The file could not be read.'),
    );
  });

  const radio = (value: 'plaza' | 'nodes', text: string) => {
    const r = h('input', { type: 'radio', name: 'vimp-scope', value });
    r.checked = value === scopeKind;
    r.addEventListener('change', () => {
      scopeKind = value;
      nodeBox.hidden = value !== 'nodes';
    });
    return h('label.vimp-check', null, r, text);
  };

  const body = h(
    'div.vimp-body',
    null,
    h(
      'div.vimp-warn',
      { role: 'note' },
      h('p', null, '.env files are discouraged. A file can be copied, synced or kept in backups. Delete it after the import.'),
      h(
        'p',
        null,
        'The clean path is ',
        h('code', null, 'apz vault import FILE.env --shred'),
        ', which overwrites and deletes the file after a verified import.',
      ),
    ),
    ctx.locked ? h('p.vimp-muted', null, 'Unseal the vault to import.') : null,
    h(
      'div.vimp-section',
      null,
      field('.env file', fileInput, 'Read on this device. Nothing is sent until you import.'),
      field('Or paste the contents', ta, 'Typed and pasted values show as dots.'),
    ),
    preview,
    h(
      'div.vimp-section',
      null,
      h('h3.vimp-h', null, 'Scope'),
      h('div.vimp-names', null, radio('plaza', 'Plaza (every node)'), radio('nodes', 'Specific nodes')),
      nodeBox,
    ),
    errorBox,
    h('div.vimp-foot', null, button('Cancel', () => close(), { kind: 'ghost' }), submitBtn),
  );

  const close = modal('Import .env', body, () => clearSource());
  renderPreview();
}
