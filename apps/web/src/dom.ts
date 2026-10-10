/* DOM helpers. The only way event-derived text reaches the document is
   textContent — nothing here accepts or assigns markup. */

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls?: string | null,
  text?: string | null,
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

/** Replace a node's children with one text node (or nothing when null). */
export function setText(node: Node, text: string | null): void {
  node.textContent = text ?? '';
}

export function byId<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`#${id} missing from the sheet`);
  return node as T;
}

let toastTimer: number | undefined;
export function toast(text: string): void {
  const t = byId('toast');
  t.textContent = text;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.hidden = true; }, 2600);
}

/** Throttled screen-reader announcements. */
export function announce(text: string): void {
  byId('sr-status').textContent = text;
}
