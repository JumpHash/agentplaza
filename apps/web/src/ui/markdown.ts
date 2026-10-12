// Markdown for what agents write: a safe renderer that builds DOM nodes (never innerHTML).
// Headings, paragraphs, emphasis, inline code, fenced code, lists (nested, ordered, task
// lists), blockquotes, tables, horizontal rules, http(s) links and [[wiki links]].

import '../styles/markdown.css';
import { h } from './dom';

export interface MarkdownOptions {
  /** Called for `[[Wiki links]]` (the memory library); omit to render them as plain text. */
  wikiLink?: (title: string) => HTMLElement;
  /** Clamp to this many top-level blocks (a "Show more" is the caller's). */
  maxBlocks?: number;
}

interface Ctx {
  wikiLink?: (title: string) => HTMLElement;
}

interface Parsed {
  nodes: Node[];
  truncated: boolean;
}

type Inline = Node | string;

// A `#` heading renders as h3 so it sits inside cards; deeper levels step down to h6.
const HEADING_TAGS = ['h3', 'h4', 'h5', 'h6'] as const;
const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/;
const FENCE_OPEN = /^\s*```\s*([^\s`]*)/;
const FENCE_CLOSE = /^\s*```\s*$/;
const HR = /^\s{0,3}([-*_])(?:\s*\1){2,}\s*$/;
const QUOTE = /^\s{0,3}>/;
const ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TABLE_SEP = /^\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*$/;
const HTTP = /^https?:\/\//i;
const TRAILING = /[.,;:!?'"*_)\]]+$/;
const INLINE =
  /`([^`\n]+)`|\[\[([^\]\n]+)\]\]|\[([^\]\n]+)\]\(((?:[^()\s]|\([^()\s]*\))*)\)|\*\*([^*\n]+)\*\*|\*([^*\s][^*\n]*?)\*|_([^_\s][^_\n]*?)_(?!\w)|https?:\/\/[^\s<>]+/g;

/** Renders `text` as a `div.md` element. Only DOM APIs build it; markup in `text` stays text. */
export function renderMarkdown(text: string, opts: MarkdownOptions = {}): HTMLElement {
  const ctx: Ctx = opts.wikiLink ? { wikiLink: opts.wikiLink } : {};
  const { nodes, truncated } = parseBlocks(text.replace(/\r\n?/g, '\n').split('\n'), ctx, opts.maxBlocks ?? Infinity);
  const root = h('div.md', null, nodes);
  if (truncated) root.dataset.truncated = 'true';
  return root;
}

function parseBlocks(lines: string[], ctx: Ctx, maxBlocks = Infinity): Parsed {
  const at = (j: number): string => lines[j] ?? '';
  const nodes: Node[] = [];
  let i = 0;
  while (i < lines.length) {
    if (!at(i).trim()) {
      i++;
      continue;
    }
    if (nodes.length >= maxBlocks) return { nodes, truncated: true };
    const line = at(i);

    const fence = FENCE_OPEN.exec(line);
    if (fence) {
      const code: string[] = [];
      for (i++; i < lines.length && !FENCE_CLOSE.test(at(i)); i++) code.push(at(i));
      i++;
      nodes.push(h('pre', fence[1] ? { 'data-lang': fence[1] } : null, h('code', null, code.join('\n'))));
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      const tag = HEADING_TAGS[Math.min((heading[1] ?? '#').length, 4) - 1] ?? 'h6';
      nodes.push(h(tag, null, inline(heading[2] ?? '', ctx)));
      i++;
      continue;
    }

    if (HR.test(line)) {
      nodes.push(h('hr'));
      i++;
      continue;
    }

    if (QUOTE.test(line)) {
      const inner: string[] = [];
      for (; i < lines.length && QUOTE.test(at(i)); i++) inner.push(at(i).replace(/^\s{0,3}> ?/, ''));
      nodes.push(h('blockquote', null, parseBlocks(inner, ctx).nodes));
      continue;
    }

    if (ITEM.test(line)) {
      const [list, next] = parseList(lines, i, indentOf(line), ctx);
      nodes.push(list);
      i = next;
      continue;
    }

    if (isTableStart(lines, i)) {
      const head = splitRow(line);
      const aligns = splitRow(at(i + 1)).map(alignOf);
      const cell = (tag: 'th' | 'td', text: string, k: number): HTMLElement =>
        h(tag, aligns[k] ? { style: `text-align:${aligns[k]}` } : null, inline(text, ctx));
      i += 2;
      const rows: Node[] = [];
      for (; i < lines.length && at(i).trim() && !startsBlock(lines, i); i++) {
        const cells = splitRow(at(i));
        rows.push(h('tr', null, head.map((_, k) => cell('td', cells[k] ?? '', k))));
      }
      nodes.push(
        h('div.md-table', null, h('table', null, h('thead', null, h('tr', null, head.map((c, k) => cell('th', c, k)))), h('tbody', null, rows))),
      );
      continue;
    }

    // A paragraph: consecutive lines up to a blank line or the start of another block.
    const para: string[] = [line];
    for (i++; i < lines.length && at(i).trim() && !startsBlock(lines, i); i++) para.push(at(i));
    nodes.push(h('p', null, paragraphInline(para, ctx)));
  }
  return { nodes, truncated: false };
}

/** A list at `base` indent; deeper items nest under the item before them. Returns the list and the next line. */
function parseList(lines: string[], start: number, base: number, ctx: Ctx): [HTMLElement, number] {
  const at = (j: number): string => lines[j] ?? '';
  const marker = ITEM.exec(at(start))?.[2] ?? '-';
  const ordered = /^\d/.test(marker);
  const items: Node[] = [];
  let i = start;
  while (i < lines.length) {
    const line = at(i);
    if (!line.trim()) {
      // A blank line only separates items when another item of this list follows.
      let j = i;
      while (j < lines.length && !at(j).trim()) j++;
      const next = ITEM.exec(at(j));
      if (!next || indentOf(at(j)) !== base || /^\d/.test(next[2] ?? '') !== ordered) break;
      i = j;
      continue;
    }
    const item = ITEM.exec(line);
    if (!item || indentOf(line) !== base || /^\d/.test(item[2] ?? '') !== ordered) break;
    let content = item[3] ?? '';
    i++;

    const nested: Node[] = [];
    while (i < lines.length) {
      const cur = at(i);
      if (!cur.trim()) {
        let j = i;
        while (j < lines.length && !at(j).trim()) j++;
        if (ITEM.test(at(j)) && indentOf(at(j)) > base) {
          i = j;
          continue;
        }
        break;
      }
      if (ITEM.test(cur)) {
        if (indentOf(cur) <= base) break;
        const [sub, next] = parseList(lines, i, indentOf(cur), ctx);
        nested.push(sub);
        i = next;
        continue;
      }
      if (indentOf(cur) <= base) break;
      content += ` ${cur.trim()}`;
      i++;
    }

    const task = /^\[([ xX])\](?:\s+|$)/.exec(content);
    const children: Node[] = [];
    if (task) {
      children.push(h('input', { type: 'checkbox', disabled: true, checked: /[xX]/.test(task[1] ?? '') }));
      content = content.slice(task[0].length);
    }
    items.push(h('li', task ? { class: 'task' } : null, children, inline(content, ctx), nested));
  }
  const number = ordered ? Number.parseInt(marker, 10) : 1;
  const list: HTMLElement = h(ordered ? 'ol' : 'ul', ordered && number !== 1 ? { start: number } : null, items);
  return [list, i];
}

function startsBlock(lines: string[], i: number): boolean {
  const line = lines[i] ?? '';
  return FENCE_OPEN.test(line) || HEADING.test(line) || HR.test(line) || QUOTE.test(line) || ITEM.test(line) || isTableStart(lines, i);
}

function isTableStart(lines: string[], i: number): boolean {
  const head = lines[i] ?? '';
  const sep = lines[i + 1] ?? '';
  return head.includes('|') && sep.includes('|') && TABLE_SEP.test(sep) && splitRow(head).length === splitRow(sep).length;
}

function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|') && !s.endsWith('\\|')) s = s.slice(0, -1);
  return s
    .replace(/\\\|/g, '\u0000')
    .split('|')
    .map((c) => c.replace(/\u0000/g, '|').trim());
}

function alignOf(cell: string): string | null {
  const left = cell.startsWith(':');
  const right = cell.endsWith(':');
  return left && right ? 'center' : right ? 'right' : left ? 'left' : null;
}

function indentOf(line: string): number {
  return (line.match(/^\s*/)?.[0] ?? '').replace(/\t/g, '  ').length;
}

/** Paragraph lines: a single newline is a space; two trailing spaces or a backslash is a hard break. */
function paragraphInline(lines: string[], ctx: Ctx): Inline[] {
  let text = '';
  lines.forEach((raw, k) => {
    if (k === lines.length - 1) text += raw.trim();
    else if (/\\$/.test(raw)) text += `${raw.trim().slice(0, -1)}\n`;
    else if (/ {2,}$/.test(raw)) text += `${raw.trim()}\n`;
    else text += `${raw.trim()} `;
  });
  return inline(text, ctx);
}

/** Inline spans: `code`, [[wiki]], [text](url), **bold**, *italic*, _italic_, bare http(s) URLs. */
function inline(text: string, ctx: Ctx): Inline[] {
  const out: Inline[] = [];
  let last = 0;
  for (const m of text.matchAll(INLINE)) {
    const start = m.index ?? 0;
    const whole = m[0] ?? '';
    const [code, wiki, label, url, bold, italic, under] = m.slice(1);
    pushText(out, text.slice(last, start));
    last = start + whole.length;
    if (code !== undefined) out.push(h('code', null, code));
    else if (wiki !== undefined) out.push(ctx.wikiLink ? ctx.wikiLink(wiki.trim()) : `[[${wiki}]]`);
    else if (label !== undefined) out.push(url !== undefined && HTTP.test(url) ? link(url, label) : label);
    else if (bold !== undefined) out.push(h('strong', null, bold));
    else if (italic !== undefined) out.push(h('em', null, italic));
    else if (under !== undefined) out.push(/\w/.test(text.charAt(start - 1)) ? whole : h('em', null, under));
    else {
      // A bare URL; trailing punctuation stays as text.
      const bare = whole.replace(TRAILING, '');
      out.push(link(bare, bare));
      last = start + bare.length;
    }
  }
  pushText(out, text.slice(last));
  return out;
}

function pushText(out: Inline[], s: string): void {
  s.split('\n').forEach((part, k) => {
    if (k > 0) out.push(h('br'));
    if (part) out.push(part);
  });
}

function link(href: string, text: string): HTMLAnchorElement {
  return h('a', { href, target: '_blank', rel: 'noopener' }, text);
}
