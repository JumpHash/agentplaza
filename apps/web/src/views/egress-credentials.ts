// Settings > Egress credentials: the rules by which the hub adds a header to an agent's HTTPS requests to a host. The
// header's value is a vault secret's name, filled into `format`; the secret never enters the VM. Only hosts with a rule
// are intercepted, every other host stays an opaque tunnel. A table of the rules (the switch and delete per row) and a
// form to add one. The pure checks and the form's body are exported and unit-tested (test/tests.ts).

import '../styles/proto/settings-a.css';
import { egressApi, type EgressRule, type EgressRuleBody } from '../api';
import { button, empty, field, input, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { sheet } from './models-providers';
import { errorText } from './vault-common';

export const DEFAULT_PORT = 443;
export const DEFAULT_PATH = '/';
export const DEFAULT_HEADER = 'Authorization';
export const DEFAULT_FORMAT = 'Bearer ${SECRET}';
/** The placeholder the format must carry: the secret's value goes there. */
export const SECRET_TOKEN = '${SECRET}';

/** An exact host, or `*.suffix` for every host under it. Lower case; labels of letters, digits and hyphens. */
const HOST_RE = /^(\*\.)?[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$/;
/** An HTTP header name: an RFC 7230 token, so no spaces, colons or separators. */
const HEADER_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

/** The form's fields as the owner types them; `port` stays text until it is checked. */
export interface RuleForm {
  host: string; port: string; methods: string; path_prefix: string; roles: string;
  header: string; format: string; secret: string; enabled: boolean;
}

/** The form before anything is typed: the defaults the server would fill, and an enabled rule. */
export function defaultRuleForm(): RuleForm {
  return { host: '', port: String(DEFAULT_PORT), methods: '', path_prefix: DEFAULT_PATH, roles: '', header: DEFAULT_HEADER, format: DEFAULT_FORMAT, secret: '', enabled: true };
}

/** A comma-separated list: trimmed, blanks and repeats dropped, order kept. */
export function listOf(text: string): string[] {
  return Array.from(new Set(text.split(',').map((s) => s.trim()).filter(Boolean)));
}

/**
 * Why the form cannot be saved, in the owner's words, or null when it can. Checked here before the request so the
 * owner sees the problem beside the field; the hub checks the same rules and answers 400 for anything it refuses.
 */
export function ruleProblem(f: RuleForm): string | null {
  const host = f.host.trim().toLowerCase();
  if (!host) return 'Enter the host the header goes to.';
  if (host.length > 253 || !HOST_RE.test(host)) return 'The host is a name such as api.example.com, or *.example.com for every host under it.';
  const port = f.port.trim();
  const n = Number(port);
  if (!/^\d+$/.test(port) || n < 1 || n > 65535) return 'The port is a number from 1 to 65535.';
  if (n === 80) return 'Port 80 is refused: the hub intercepts HTTPS only, so use 443 or another TLS port.';
  if (!f.path_prefix.trim().startsWith('/')) return 'The path prefix starts with a slash, such as /v1.';
  if (listOf(f.methods).some((m) => !/^[A-Za-z]+$/.test(m))) return 'Methods are names such as GET or POST, separated by commas.';
  if (!HEADER_RE.test(f.header.trim())) return 'The header is a name such as Authorization or X-Api-Key, with no spaces or colons.';
  if (!f.format.includes(SECRET_TOKEN)) return 'The format must contain ${SECRET}, where the secret goes, such as Bearer ${SECRET}.';
  if (!f.secret.trim()) return 'Enter the name of the vault secret to use.';
  return null;
}

/** The request body for a form that `ruleProblem` accepts: the host and methods normalised, the lists split. */
export function ruleBody(f: RuleForm): EgressRuleBody {
  return {
    roles: listOf(f.roles),
    host: f.host.trim().toLowerCase(),
    port: Number(f.port.trim()),
    methods: listOf(f.methods).map((m) => m.toUpperCase()),
    path_prefix: f.path_prefix.trim(),
    header: f.header.trim(),
    format: f.format,
    secret: f.secret.trim(),
    enabled: f.enabled,
  };
}

/** The methods a rule matches, as the table reads them: an empty list is every method. */
export function methodsText(methods: string[]): string {
  return methods.length ? methods.join(', ') : 'Any';
}

/** The roles a rule matches, as the table reads them: an empty list is every role. */
export function rolesText(roles: string[]): string {
  return roles.length ? roles.join(', ') : 'Every role';
}

const COLUMNS = ['Host', 'Port', 'Methods', 'Path', 'Roles', 'Header', 'Secret', 'Enabled', 'Actions'];

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let loadSeq = 0;
  let rules: EgressRule[] | null = null;
  let loadError: unknown = null;

  const tableSlot = h('div', null, h('p.mdl-faint', null, 'Loading the rules…'));
  const listError = h('p.eg-err', { role: 'alert' });
  const formError = h('p.eg-err', { role: 'alert' });
  listError.hidden = true;
  formError.hidden = true;

  /** Shows a message in an error line, or hides the line when there is none. */
  const showError = (line: HTMLElement, text: string | null): void => {
    line.textContent = text ?? '';
    line.hidden = !text;
  };

  const reload = async (): Promise<void> => {
    const seq = ++loadSeq;
    try {
      const list = await egressApi.rules();
      if (!alive || seq !== loadSeq) return;
      rules = list;
      loadError = null;
    } catch (err) {
      if (!alive || seq !== loadSeq) return;
      rules = null;
      loadError = err;
    }
    drawRules();
  };

  // ---- Rules ----------------------------------------------------------------------------------------------------

  /** The switch turns the rule on or off; the list is read again either way, so the switch shows what the hub holds. */
  const setEnabled = async (rule: EgressRule, on: boolean): Promise<void> => {
    showError(listError, null);
    try {
      await egressApi.patchRule(rule.id, { enabled: on });
    } catch (err) {
      if (alive) showError(listError, errorText(err));
    }
    if (alive) void reload();
  };

  const removeRule = (rule: EgressRule): void => {
    let close = (): void => {};
    close = sheet(
      `Delete the rule for ${rule.host}`,
      h('p', null, `The hub stops adding ${rule.header} to requests to ${rule.host}:${rule.port}. The vault secret ${rule.secret} stays.`),
      [
        button('Cancel', () => close(), { kind: 'ghost' }),
        button(
          'Delete',
          () => {
            close();
            void deleteRule(rule);
          },
          { kind: 'danger' },
        ),
      ],
    );
  };

  const deleteRule = async (rule: EgressRule): Promise<void> => {
    showError(listError, null);
    try {
      await egressApi.deleteRule(rule.id);
      if (!alive) return;
      toast(`Deleted the rule for ${rule.host}`);
    } catch (err) {
      if (alive) showError(listError, errorText(err));
    }
    if (alive) void reload();
  };

  const ruleRow = (r: EgressRule): HTMLElement => {
    const box = input({ type: 'checkbox', 'aria-label': `Enabled: ${r.host}` });
    box.checked = r.enabled;
    box.addEventListener('change', () => void setEnabled(r, box.checked));
    return h(
      'tr',
      { 'data-key': `rule-${r.id}` },
      h('td', { 'data-label': 'Host', class: 'mono' }, r.host),
      h('td', { 'data-label': 'Port' }, String(r.port)),
      h('td', { 'data-label': 'Methods' }, methodsText(r.methods)),
      h('td', { 'data-label': 'Path', class: 'mono' }, r.path_prefix),
      h('td', { 'data-label': 'Roles' }, rolesText(r.roles)),
      h('td', { 'data-label': 'Header' }, h('span.mono', null, r.header), ' ', h('span.mdl-faint', { title: 'The value the header carries' }, r.format)),
      h('td', { 'data-label': 'Secret', class: 'mono' }, r.secret),
      h('td', { 'data-label': 'Enabled' }, h('label.check', null, box, h('span', null, r.enabled ? 'On' : 'Off'))),
      h('td', { 'data-label': 'Actions' }, button('Delete', () => removeRule(r), { kind: 'danger', size: 'sm', icon: 'trash' })),
    );
  };

  const drawRules = (): void => {
    if (loadError) {
      replace(tableSlot, empty('alert', 'Could not load the rules', errorText(loadError)));
      return;
    }
    if (!rules) return;
    replace(
      tableSlot,
      rules.length
        ? h(
            'div.eg-wrap',
            null,
            h(
              'table.mdl-table',
              null,
              h('thead', null, h('tr', null, COLUMNS.map((c) => h('th', null, c)))),
              h('tbody', null, rules.map(ruleRow)),
            ),
          )
        : empty('agents', 'No rules', 'No host gets a header yet. Until a rule is added, every host is an opaque tunnel.'),
    );
  };

  // ---- Add a rule -----------------------------------------------------------------------------------------------

  const fields = {
    host: input({ maxlength: 253, autocomplete: 'off', placeholder: 'api.example.com or *.example.com' }),
    port: input({ type: 'number', min: 1, max: 65535, step: 1 }),
    methods: input({ placeholder: 'GET, POST (blank: any method)', autocomplete: 'off' }),
    path_prefix: input({ autocomplete: 'off' }),
    roles: input({ placeholder: 'coder, reviewer (blank: every role)', autocomplete: 'off' }),
    header: input({ autocomplete: 'off' }),
    format: input({ autocomplete: 'off' }),
    secret: input({ placeholder: 'the vault secret name', autocomplete: 'off' }),
  };
  const enabledBox = input({ type: 'checkbox' });

  const fillForm = (f: RuleForm): void => {
    fields.host.value = f.host;
    fields.port.value = f.port;
    fields.methods.value = f.methods;
    fields.path_prefix.value = f.path_prefix;
    fields.roles.value = f.roles;
    fields.header.value = f.header;
    fields.format.value = f.format;
    fields.secret.value = f.secret;
    enabledBox.checked = f.enabled;
  };
  const readForm = (): RuleForm => ({
    host: fields.host.value,
    port: fields.port.value,
    methods: fields.methods.value,
    path_prefix: fields.path_prefix.value,
    roles: fields.roles.value,
    header: fields.header.value,
    format: fields.format.value,
    secret: fields.secret.value,
    enabled: enabledBox.checked,
  });

  const addBtn = button('Add rule', () => void addRule(), { kind: 'primary', icon: 'plus' });

  const addRule = async (): Promise<void> => {
    const form = readForm();
    const problem = ruleProblem(form);
    showError(formError, problem);
    if (problem) return;
    addBtn.disabled = true;
    let made: EgressRule;
    try {
      made = await egressApi.addRule(ruleBody(form));
    } catch (err) {
      // The hub's own reason (400 for a rule it refuses, 503 when the vault is not ready) is shown beside the form.
      if (alive) showError(formError, errorText(err));
      return;
    } finally {
      addBtn.disabled = false;
    }
    if (!alive) return;
    // The host and the secret are the rule's own; the rest stays, so the next rule starts from the same defaults.
    fillForm({ ...readForm(), host: '', secret: '' });
    toast(`Added the rule for ${made.host}`);
    void reload();
  };

  const egForm = h(
    'div.mdl-card.mdl-stack',
    null,
    h('h3', { style: 'margin:0' }, 'Add a rule'),
    h(
      'div.eg-form',
      null,
      h('div.wide', null, field('Host', fields.host, 'An exact host, or *.suffix for every host under it.')),
      field('Port', fields.port, 'Default 443. Port 80 is refused.'),
      field('Path prefix', fields.path_prefix, 'Requests whose path starts with this. Default /.'),
      field('Methods', fields.methods, 'Comma-separated. Blank means any method.'),
      field('Roles', fields.roles, 'Comma-separated role names. Blank means every role.'),
      field('Header', fields.header, 'The header the hub sets. Default Authorization.'),
      field('Format', fields.format, 'The header value; ${SECRET} is replaced by the secret. Default Bearer ${SECRET}.'),
      h('div.wide', null, field('Secret', fields.secret, 'The name of a vault secret bound to the egress consumer.')),
      h('div.wide', null, h('label.check', null, enabledBox, h('span', null, 'Enabled'))),
    ),
    formError,
    h('div.mdl-row-a', null, addBtn),
  );

  fillForm(defaultRuleForm());
  replace(
    root,
    h(
      'div.mdl-page',
      null,
      h(
        'header.page-head',
        null,
        h('div.page-head-row', null, h('h1', null, 'Egress credentials'), h('span.sp'), h('div.page-actions')),
        h(
          'p.lede',
          null,
          "The hub adds this header to the agent's HTTPS requests to the host; the secret never enters the VM. Only hosts with a rule are intercepted; every other host stays an opaque tunnel.",
        ),
      ),
      h('div.mdl-card', null, h('h3', { style: 'margin:0 0 8px' }, 'Rules'), listError, tableSlot),
      egForm,
    ),
  );
  drawRules();
  void reload();

  return () => {
    alive = false;
  };
}
