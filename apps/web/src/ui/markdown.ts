// Markdown for what agents write: a safe renderer that builds DOM nodes (never innerHTML).
// Headings, paragraphs, emphasis, inline code, fenced code, lists (nested, ordered, task
// lists), blockquotes, tables, horizontal rules and http(s) links.

export interface MarkdownOptions {
  /** Called for `[[Wiki links]]` (the memory library); omit to render them as plain text. */
  wikiLink?: (title: string) => HTMLElement;
  /** Clamp to this many top-level blocks (a "Show more" is the caller's). */
  maxBlocks?: number;
}

/** Renders `text` as a `div.md` element. */
export function renderMarkdown(text: string, _opts: MarkdownOptions = {}): HTMLElement {
  // contract: M1
  const el = document.createElement('div');
  el.className = 'md';
  el.textContent = text;
  return el;
}
