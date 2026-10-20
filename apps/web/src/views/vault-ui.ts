// Markup helpers for the Vault and Nodes screens, emitting the prototype's own markup (the vlt-* rules in
// styles/proto/settings-c.css and the shared base.css classes). Pure builders: no state lives here.
import '../styles/proto/settings-c.css';
import { modal } from '../ui/components';
import { h } from '../ui/dom';

type Kid = Node | null | false | undefined;

/** The page head: title on the left, actions on the right (the prototype's h.head). */
export function pageHead(title: string, actions: Node[] = []): HTMLElement {
  return h('header.page-head', null, h('div.page-head-row', null, h('h1', null, title), h('span.sp'), h('div.page-actions', null, ...actions)));
}

/** A dialog over the scrim. `body` sits in the sheet's body; `foot` in its footer (omitted when empty). */
export function dialog(title: string, body: Kid[], foot: Kid[] = [], onClose?: () => void): () => void {
  const host = h('div.vlt-dlg', null, ...body.filter((n): n is Node => !!n));
  const close = modal(title, host, onClose);
  const kids = foot.filter((n): n is Node => !!n);
  if (kids.length) host.closest('.sheet')?.append(h('footer.sheet-f', null, ...kids));
  return close;
}

/** The error line in a dialog or card; empty means no error. */
export function errLine(): HTMLDivElement {
  return h('div.vlt-err', { role: 'alert' });
}

export function dim(text: string): HTMLParagraphElement {
  return h('p.vlt-dim', null, text);
}

export function fieldset(legend: string, kids: Kid[]): HTMLFieldSetElement {
  return h('fieldset.vlt-fs', null, h('legend', null, legend), ...kids.filter((n): n is Node => !!n));
}

/** A radio row. `onChange` runs when the radio is picked. */
export function radio(name: string, value: string, checked: boolean, text: Kid | string, onChange?: () => void): { input: HTMLInputElement; row: HTMLLabelElement } {
  const input = h('input', { type: 'radio', name, value });
  input.checked = checked;
  if (onChange) input.addEventListener('change', onChange);
  return { input, row: h('label.vlt-radio', null, input, h('span', null, text ?? '')) };
}

/** A checkbox row; `content` is the label (text or nodes). */
export function check(checked: boolean, content: Kid | string, value?: string): { input: HTMLInputElement; row: HTMLLabelElement } {
  const input = h('input', { type: 'checkbox', value: value ?? null });
  input.checked = checked;
  return { input, row: h('label.vlt-chk', null, input, h('span', null, content ?? '')) };
}

/** A definition list of label and value pairs (the prototype's h.kv). */
export function kv(pairs: [string, Node | string][]): HTMLElement {
  return h('dl.kv', null, ...pairs.map(([k, v]) => h('div', null, h('dt', null, k), h('dd', null, v))));
}

/** A table in its scroll wrapper (the prototype's h.table). */
export function tableWrap(table: HTMLElement): HTMLElement {
  return h('div.tbl-wrap', null, table);
}

/** A node's name in the Per node table and the lists: "This hub" for n0. */
export function nodeHeading(id: string, name: string): string {
  return id === 'n0' ? 'This hub' : name;
}

/** Relative time as the prototype words it: "just now", "4 min ago", "3 h ago", "60 d ago". */
export function agoText(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return '—';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}
