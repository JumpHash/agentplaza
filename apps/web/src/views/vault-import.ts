// The Import .env dialog. The file or pasted text is read here for names only, sent once, then cleared.
// No value is shown, kept after submit, or logged. The raw lines are held only for that one request.
import { badge, button, clip, field } from '../ui/components';
import { h, replace } from '../ui/dom';
import { vaultApi } from '../api';
import type { VaultConsumer, VaultImportBody, VaultImportResult, VaultScope } from '../api';
import { VAULT_NAME_RE, bindingLabel, errorText, nodeName } from './vault-common';
import type { VaultCtx } from './vault-common';
import { check, dialog, dim, errLine, fieldset, nodeHeading, radio } from './vault-ui';

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
  let scopeKind = 'plaza' as 'plaza' | 'nodes';
  const nodeSet = new Set<string>();
  let busy = false;

  const fileInput = h('input', { type: 'file', accept: '.env,text/plain', 'aria-label': 'Choose a .env file', class: 'vlt-file' });
  const ta = h('textarea.textarea.vlt-mask', {
    rows: 6,
    autocomplete: 'off',
    spellcheck: 'false',
    wrap: 'off',
    'aria-label': 'Paste .env contents',
    placeholder: 'NAME=value, one per line',
  });
  const preview = h('div', null);
  const errorBox = errLine();
  const nodeBox = h(
    'div',
    null,
    nodes.map((n) => {
      const row = check(nodeSet.has(n.id), nodeHeading(n.id, nodeName(n.id, nodes)), n.id);
      row.input.addEventListener('change', () => {
        if (row.input.checked) nodeSet.add(n.id);
        else nodeSet.delete(n.id);
      });
      return row.row;
    }),
  );
  nodeBox.hidden = true;
  const submitBtn = button('Import', () => void submit(), { kind: 'primary' });

  const fail = (msg: string) => {
    errorBox.textContent = msg;
  };
  const hideError = () => {
    errorBox.textContent = '';
  };

  function refresh(): void {
    const n = picked.size;
    submitBtn.textContent = n ? `Import ${n} ${plural(n)}` : 'Import';
    submitBtn.disabled = ctx.locked || busy || n === 0;
  }

  const bindRow = (c: VaultConsumer) => {
    const row = check(
      ticks.has(c.binding),
      h('span', null, bindingLabel(c.binding, consumers), ' ', h('span.vlt-sub.mono', null, `wants ${c.wants}`)),
      c.binding,
    );
    row.input.addEventListener('change', () => {
      if (row.input.checked) ticks.add(c.binding);
      else ticks.delete(c.binding);
    });
    return row.row;
  };

  function renderPreview(): void {
    const parts: Node[] = [];
    if (!valid.length && !invalid.length && !problems.length) {
      parts.push(dim('Choose a file or paste the contents to see the names.'));
    } else {
      const from = sourceName ? sourceName : 'the pasted text';
      parts.push(h('p', null, `${valid.length} ${plural(valid.length)} found from ${from}. Values are not shown.`));
      if (valid.length) {
        parts.push(
          h(
            'div.vlt-list',
            null,
            valid.map((n) => {
              const row = check(picked.has(n), h('span.mono', null, n), n);
              row.input.addEventListener('change', () => {
                if (row.input.checked) picked.add(n);
                else picked.delete(n);
                refresh();
              });
              return row.row;
            }),
          ),
        );
      }
      if (invalid.length) {
        parts.push(h('p.vlt-dim', null, 'Not valid names, so skipped: ', ...invalid.map((n) => badge(clip(n, 40), 'muted'))));
      }
      if (problems.length) {
        parts.push(dim(`Skipped ${problems.length} line(s): ${problems.join(', ')}.`));
      }
      if (valid.length) {
        parts.push(
          fieldset('Bindings', [
            h('p.vlt-sub', null, 'A name gets the ticked bindings that want it.'),
            consumers.length ? h('div', null, ...consumers.map(bindRow)) : dim('Nothing asks for a secret yet. The names import without bindings.'),
          ]),
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
      slot,
      h('p', null, n ? `Imported ${n} ${plural(n)}.` : 'Nothing was imported.'),
      n
        ? h(
            'div.vlt-list',
            null,
            res.imported.map((i) => h('div', null, h('span.mono', null, i.name), ' ', h('span.vlt-sub.mono', null, i.id))),
          )
        : null,
      res.skipped.length ? h('p', null, h('strong', null, `Skipped ${res.skipped.length}`)) : null,
      res.skipped.length
        ? h(
            'div.vlt-list',
            null,
            res.skipped.map((s) => h('div', null, h('span.mono', null, s.name), ` ${s.reason}`)),
          )
        : null,
    );
    footer()?.replaceChildren(h('div.vlt-res', null, button('Done', () => close(), { kind: 'primary' })));
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

  const plazaRadio = radio('vlt-imp-scope', 'plaza', scopeKind === 'plaza', 'Plaza (every node)', () => {
    scopeKind = 'plaza';
    nodeBox.hidden = true;
  });
  const nodesRadio = radio('vlt-imp-scope', 'nodes', scopeKind === 'nodes', 'Specific nodes', () => {
    scopeKind = 'nodes';
    nodeBox.hidden = false;
  });

  const slot = h(
    'div.vlt-form',
    null,
    h(
      'div.vlt-banner',
      { role: 'note' },
      h('span', null, '.env files are discouraged. A file can be copied, synced or kept in backups. Delete it after the import.'),
      h('span', null, 'The clean path is ', h('code', null, 'apz vault import FILE.env --shred'), ', which overwrites and deletes the file after a verified import.'),
    ),
    ctx.locked ? dim('Unseal the vault to import.') : null,
    field('.env file', fileInput, 'Read on this device. Nothing is sent until you import.'),
    field('Or paste the contents', ta, 'Typed and pasted values show as dots.'),
    preview,
    fieldset('Scope', [plazaRadio.row, nodesRadio.row, nodeBox]),
    errorBox,
  );

  const footer = () => slot.closest('.sheet')?.querySelector('footer.sheet-f') ?? null;
  const close = dialog('Import .env', [slot], [h('div.vlt-res', null, button('Cancel', () => close(), { kind: 'ghost' }), submitBtn)], () => clearSource());
  renderPreview();
}
