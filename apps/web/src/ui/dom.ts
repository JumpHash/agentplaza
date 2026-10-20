// DOM helpers. Text only ever reaches the document as textContent or as an
// attribute value — nothing here accepts markup.

import { ICONS, LUCIDE, type IconName, type LucideName } from './icons.gen';

type Child = Node | string | number | null | undefined | false;
type Attrs = Record<string, string | number | boolean | null | undefined | EventListener>;

/** `h('div.card.wide', {onclick: f, title: 'x'}, child, 'text')`. */
export function h<K extends keyof HTMLElementTagNameMap>(
  spec: K | `${K}.${string}`,
  attrs?: Attrs | null,
  ...children: (Child | Child[])[]
): HTMLElementTagNameMap[K] {
  const [tag, ...classes] = spec.split('.') as [K, ...string[]];
  const el = document.createElement(tag);
  if (classes.length) el.className = classes.join(' ');
  if (attrs) {
    for (const [key, value] of Object.entries(attrs)) {
      if (value == null || value === false) continue;
      if (key.startsWith('on') && typeof value === 'function') {
        el.addEventListener(key.slice(2), value);
      } else if (key === 'class') {
        el.className = [el.className, String(value)].filter(Boolean).join(' ');
      } else if (value === true) {
        el.setAttribute(key, '');
      } else {
        el.setAttribute(key, String(value));
      }
    }
  }
  append(el, children);
  return el;
}

export function append(parent: Node, children: (Child | Child[])[]): void {
  for (const child of children.flat()) {
    if (child == null || child === false) continue;
    parent.appendChild(typeof child === 'object' ? child : document.createTextNode(String(child)));
  }
}

/** Replaces `node`'s children, keeping the state a reader gave the old ones (see `keepState`). */
export function replace(node: Node, ...children: (Child | Child[])[]): void {
  const restore = keepState(node);
  while (node.firstChild) node.removeChild(node.firstChild);
  append(node, children);
  restore();
}

/** What a redraw must not take from the reader: open <details>, scroll offsets (a log
    scrolled to its end stays at its end), `[data-open]` toggles, and the focused field. */
interface Kept {
  open?: boolean;
  top?: number;
  left?: number;
  bottom?: boolean;
  toggled?: string;
}

/**
 * Records the UI state under `root` and returns a function that puts it back on the
 * elements that replace them. Elements are matched by a path from `root`: an ancestor
 * with `data-key` (or an id) anchors it, so keyed rows survive inserts and reorders;
 * the rest is tag, class and position among like siblings.
 */
export function keepState(root: Node): () => void {
  if (!(root instanceof Element) || !root.firstElementChild) return () => {};
  const kept = new Map<string, Kept>();
  const active = document.activeElement;
  let focus: string | null = null;
  walk(root, (el, path) => {
    if (el === active) focus = path;
    const k: Kept = {};
    if (el instanceof HTMLDetailsElement && el.open) k.open = true;
    if (el instanceof HTMLElement && el.dataset.open !== undefined) k.toggled = el.dataset.open;
    if (el.scrollTop > 0 || el.scrollLeft > 0) {
      k.top = el.scrollTop;
      k.left = el.scrollLeft;
      k.bottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 4;
    }
    if (k.open || k.toggled !== undefined || k.top !== undefined) kept.set(path, k);
  });
  const selection =
    focus && (active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement)
      ? ([active.selectionStart, active.selectionEnd, active.selectionDirection] as const)
      : null;
  if (!kept.size && !focus) return () => {};
  return () =>
    walk(root, (el, path) => {
      const k = kept.get(path);
      if (k) {
        if (k.open && el instanceof HTMLDetailsElement) el.open = true;
        if (k.toggled !== undefined && el instanceof HTMLElement && el.dataset.open !== k.toggled) {
          // A toggle owns its look; ask it to switch rather than setting the attribute.
          el.dispatchEvent(new CustomEvent('keep:toggle', { detail: k.toggled }));
        }
        if (k.top !== undefined) {
          el.scrollTop = k.bottom ? el.scrollHeight : k.top;
          el.scrollLeft = k.left ?? 0;
        }
      }
      if (path === focus && el instanceof HTMLElement && document.activeElement !== el) {
        el.focus({ preventScroll: true });
        if (selection && (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) {
          try {
            el.setSelectionRange(selection[0], selection[1], selection[2] ?? undefined);
          } catch {
            // Inputs without a selection (number, email) refuse it.
          }
        }
      }
    });
}

/** Visits every element under `root` with its path: an ancestor's `data-key` (or id)
    anchors it; otherwise tag, first class and position among like siblings. */
function walk(root: Element, visit: (el: Element, path: string) => void): void {
  const go = (parent: Element, prefix: string): void => {
    const seen = new Map<string, number>();
    for (let el = parent.firstElementChild; el; el = el.nextElementSibling) {
      const key = el instanceof HTMLElement ? el.dataset.key || el.id : '';
      let path: string;
      if (key) path = `#${key}`;
      else {
        const sig = `${el.tagName}.${el.classList[0] ?? ''}`;
        const i = seen.get(sig) ?? 0;
        seen.set(sig, i + 1);
        path = `${prefix}/${sig}:${i}`;
      }
      visit(el, path);
      go(el, path);
    }
  };
  go(root, '');
}

const SVG = 'http://www.w3.org/2000/svg';

/** Console icon names that the Loom and Graphite consoles draw as another lucide icon (the prototype's own aliases). */
const ALIAS: Partial<Record<IconName, LucideName>> = {
  agents: 'bot', alert: 'triangle-alert', approval: 'shield-check', board: 'pin', commit: 'git-commit-horizontal',
  conversations: 'messages-square', git: 'git-branch', harness: 'cpu', loop: 'repeat', machine: 'server', models: 'cpu',
  orchestrator: 'bot', projects: 'folder', refresh: 'refresh-cw', society: 'network', stop: 'square', tasks: 'list-checks',
  file: 'file', settings: 'settings', library: 'library', overview: 'house',
};

function drawn(nodes: readonly (readonly [string, Record<string, string>])[], size: number, cls: string, stroke: number): SVGSVGElement {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('class', `ic icon ${cls}`.trim());
  svg.setAttribute('width', String(size));
  svg.setAttribute('height', String(size));
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', String(stroke));
  svg.setAttribute('stroke-linecap', 'round');
  svg.setAttribute('stroke-linejoin', 'round');
  svg.setAttribute('aria-hidden', 'true');
  for (const [tag, attrs] of nodes) {
    const node = document.createElementNS(SVG, tag);
    for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
    svg.appendChild(node);
  }
  return svg;
}

/** A UI icon (lucide), drawn as the Loom and Graphite consoles draw it: `svg.ic`, 16 px, a 1.75 stroke. */
export function icon(name: IconName, cls = '', size = 16): SVGSVGElement {
  const alias = ALIAS[name];
  return drawn(alias ? LUCIDE[alias] : ICONS[name], size, cls, 1.75);
}

/** A lucide icon by its own name, as the prototype names them (`shield-check`, `git-branch`). */
export function lu(name: LucideName, size = 16, cls = '', stroke = 1.75): SVGSVGElement {
  return drawn(LUCIDE[name], size, cls, stroke);
}

/** A filled single-path SVG (brand logos). */
export function pathSvg(d: string, viewBox = '0 0 24 24'): SVGSVGElement {
  return markSvg([{ d }], viewBox);
}

/** A multi-colour mark: each path keeps its own fill, so the parent's colour never applies. */
export function markSvg(paths: { d: string; fill?: string }[], viewBox = '0 0 24 24'): SVGSVGElement {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', viewBox);
  svg.setAttribute('aria-hidden', 'true');
  for (const { d, fill } of paths) {
    const path = document.createElementNS(SVG, 'path');
    path.setAttribute('d', d);
    if (fill) path.setAttribute('fill', fill);
    svg.appendChild(path);
  }
  return svg;
}
