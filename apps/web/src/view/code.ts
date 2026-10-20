// The code block the console's views show for a file or an artifact: a head with the file name and a Copy button, then
// the text with line numbers in a gutter and the highlighting from highlight.ts. Long code folds after 40 lines. The
// markup is the prototype's code block: a file head (dfile-h) over a .code block of .cl lines.
import '../styles/view-code.css';
import { fragment, spans } from './highlight';

const COLLAPSED_LINES = 40;

export interface CodeBody {
  language: string;
  text: string;
  filename?: string;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

// The [start, end) code point range of each line, without the newline. A text that ends in a newline has no empty last line.
function lineRanges(chars: string[]): [number, number][] {
  const out: [number, number][] = [];
  let start = 0;
  for (let i = 0; i < chars.length; i++) {
    if (chars[i] === '\n') {
      out.push([start, i]);
      start = i + 1;
    }
  }
  if (start < chars.length || out.length === 0) out.push([start, chars.length]);
  return out;
}

export function renderCode(body: CodeBody): HTMLElement {
  const root = el('div', 'view-code');

  const head = el('div', 'dfile-h');
  head.append(el('span', 'mono', body.filename || body.language || 'text'), el('span', 'sp'));
  const copy = el('button', 'btn sm ghost', 'Copy');
  copy.type = 'button';
  copy.addEventListener('click', () => {
    // A refused clipboard (no permission, or no secure context) leaves the button as it was.
    navigator.clipboard?.writeText(body.text).then(
      () => {
        copy.textContent = 'Copied';
      },
      () => {},
    );
  });
  head.append(copy);

  const chars = Array.from(body.text);
  const marks = spans(body.language, body.text);
  const lines = lineRanges(chars);
  const pre = el('div', 'code view-code-body');

  const draw = (count: number): void => {
    pre.replaceChildren();
    for (let k = 0; k < count; k++) {
      const [a, b] = lines[k] ?? [0, 0];
      const row = el('div', 'cl');
      row.append(el('span', 'ln', String(k + 1)));
      const src = el('code', '');
      src.append(fragment(chars, marks, a, b));
      row.append(src);
      pre.append(row);
    }
  };

  const folded = lines.length > COLLAPSED_LINES;
  draw(folded ? COLLAPSED_LINES : lines.length);
  root.append(head, pre);
  if (folded) {
    const more = el('button', 'btn sm ghost view-code-more', `Show all ${lines.length} lines`);
    more.type = 'button';
    more.addEventListener('click', () => {
      draw(lines.length);
      more.remove();
    });
    root.append(more);
  }
  return root;
}
