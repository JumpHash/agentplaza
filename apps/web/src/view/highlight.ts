// The syntax highlighter for code in the web console. The Rust twin in crates/apz-view scans the same way over the same
// rules file (apps/shared/highlight.json), and its golden vectors pin the two scans together.
import rules from '../../../shared/highlight.json';

export type HlClass = 'keyword' | 'string' | 'type' | 'number' | 'comment' | 'function';

export interface HlSpan {
  start: number;
  end: number;
  class: HlClass;
}

interface LanguageRule {
  aliases?: string[];
  keywords?: string[];
  types?: string[];
  constants?: string[];
  line_comments?: string[];
  block_comments?: string[][];
  strings?: string[];
  multiline_strings?: string[];
  ident_extra?: string;
  type_case?: boolean;
}

type MarkerKind = 'line' | 'block' | 'string' | 'multiline';

// A comment or string opener and the closer it runs to (empty for a line comment).
interface Marker {
  kind: MarkerKind;
  open: string[];
  close: string[];
}

// A language ready for the scan: its markers longest first, its word classes, and its flags.
interface Compiled {
  markers: Marker[];
  words: Map<string, HlClass>;
  caseInsensitive: boolean;
  typeCase: boolean;
  identExtra: Set<string>;
}

const LANGUAGES = new Map<string, Compiled>();

function compile(name: string, rule: LanguageRule): Compiled {
  const markers: Marker[] = [];
  for (const m of rule.line_comments ?? []) {
    if (m) markers.push({ kind: 'line', open: Array.from(m), close: [] });
  }
  for (const [open, close] of rule.block_comments ?? []) {
    if (open && close) markers.push({ kind: 'block', open: Array.from(open), close: Array.from(close) });
  }
  for (const m of rule.strings ?? []) {
    if (m) markers.push({ kind: 'string', open: Array.from(m), close: Array.from(m) });
  }
  for (const m of rule.multiline_strings ?? []) {
    if (m) markers.push({ kind: 'multiline', open: Array.from(m), close: Array.from(m) });
  }
  // The sort is stable, so equal-length markers keep the order above. A longer marker wins where two start together.
  markers.sort((a, b) => b.open.length - a.open.length);
  // SQL matches its words without regard to case; every other language matches them exactly.
  const caseInsensitive = name.toLowerCase() === 'sql';
  const words = new Map<string, HlClass>();
  const add = (list: string[] | undefined, cls: HlClass): void => {
    for (const w of list ?? []) {
      const key = caseInsensitive ? w.toLowerCase() : w;
      // The first list to name a word decides its class: keywords, then constants, then types.
      if (!words.has(key)) words.set(key, cls);
    }
  };
  add(rule.keywords, 'keyword');
  add(rule.constants, 'number');
  add(rule.types, 'type');
  return {
    markers,
    words,
    caseInsensitive,
    typeCase: rule.type_case === true,
    identExtra: new Set(Array.from(rule.ident_extra ?? '')),
  };
}

const FILE = rules as unknown as { languages: Record<string, LanguageRule> };
for (const [name, rule] of Object.entries(FILE.languages)) {
  const compiled = compile(name, rule);
  for (const key of [name, ...(rule.aliases ?? [])]) LANGUAGES.set(key.toLowerCase(), compiled);
}

function matchAt(cs: string[], i: number, m: string[]): boolean {
  if (m.length === 0 || i + m.length > cs.length) return false;
  return m.every((c, k) => cs[i + k] === c);
}

function identStart(c: string): boolean {
  return /[A-Za-z_$]/.test(c) || /\p{L}/u.test(c);
}

function identChar(c: string, lang: Compiled): boolean {
  return /[A-Za-z0-9_$]/.test(c) || /\p{L}/u.test(c) || lang.identExtra.has(c);
}

function followedByParen(cs: string[], j: number): boolean {
  let k = j;
  while (cs[k] === ' ' || cs[k] === '\t') k++;
  return cs[k] === '(';
}

// Where a comment or string that opens at i ends: exclusive of a line comment's newline, inclusive of a closer, and at
// the end of the text when it is not closed. A string delimiter that is not multiline also ends at a newline.
function markerEnd(cs: string[], i: number, m: Marker): number {
  const n = cs.length;
  if (m.kind === 'line') {
    let j = i + m.open.length;
    while (j < n && cs[j] !== '\n') j++;
    return j;
  }
  if (m.kind === 'block') {
    let j = i + m.open.length;
    while (j < n && !matchAt(cs, j, m.close)) j++;
    return j < n ? j + m.close.length : n;
  }
  // A closer counts only when the backslashes before it inside the body are an even number.
  let run = 0;
  for (let j = i + m.open.length; j < n; j++) {
    if (matchAt(cs, j, m.close) && run % 2 === 0) return j + m.close.length;
    if (m.kind === 'string' && cs[j] === '\n') return j;
    run = cs[j] === '\\' ? run + 1 : 0;
  }
  return n;
}

// The highlight spans of a text in the named language, as code point indices. An unknown language has none.
export function spans(language: string, text: string): HlSpan[] {
  const lang = LANGUAGES.get(language.toLowerCase());
  if (!lang) return [];
  const cs = Array.from(text);
  const n = cs.length;
  const out: HlSpan[] = [];
  let i = 0;
  while (i < n) {
    const marker = lang.markers.find((m) => matchAt(cs, i, m.open));
    if (marker) {
      const end = markerEnd(cs, i, marker);
      const cls: HlClass = marker.kind === 'line' || marker.kind === 'block' ? 'comment' : 'string';
      out.push({ start: i, end, class: cls });
      i = end;
      continue;
    }
    const c = cs[i] ?? '';
    if (/[0-9]/.test(c) || (c === '.' && /[0-9]/.test(cs[i + 1] ?? ''))) {
      const prev = i > 0 ? (cs[i - 1] ?? '') : '';
      if (!identChar(prev, lang)) {
        let j = i + 1;
        while (j < n) {
          const d = cs[j] ?? '';
          if (/[A-Za-z0-9_.]/.test(d)) j++;
          else if ((d === '+' || d === '-') && (cs[j - 1] === 'e' || cs[j - 1] === 'E')) j++;
          else break;
        }
        out.push({ start: i, end: j, class: 'number' });
        i = j;
        continue;
      }
    }
    if (identStart(c)) {
      let j = i + 1;
      while (j < n && identChar(cs[j] ?? '', lang)) j++;
      const word = cs.slice(i, j).join('');
      const key = lang.caseInsensitive ? word.toLowerCase() : word;
      let cls = lang.words.get(key);
      if (!cls && lang.typeCase && /^\p{Lu}/u.test(word)) cls = 'type';
      if (!cls && followedByParen(cs, j)) cls = 'function';
      if (cls) out.push({ start: i, end: j, class: cls });
      i = j;
      continue;
    }
    i += 1;
  }
  return out;
}

// A fragment of the text from code point `from` to `to`, with the spans that fall in that range wrapped in
// `<span class="syn-...">`. The spans must be in order and must not overlap, as spans() returns them.
export function fragment(cs: string[], marks: HlSpan[], from = 0, to = cs.length): DocumentFragment {
  const frag = document.createDocumentFragment();
  let at = from;
  for (const s of marks) {
    const a = Math.max(s.start, from);
    const b = Math.min(s.end, to);
    if (b <= a) continue;
    if (a > at) frag.append(cs.slice(at, a).join(''));
    const el = document.createElement('span');
    el.className = `syn-${s.class}`;
    el.textContent = cs.slice(a, b).join('');
    frag.append(el);
    at = b;
  }
  if (at < to) frag.append(cs.slice(at, to).join(''));
  return frag;
}

// The whole text highlighted as one fragment: text nodes plus `<span class="syn-keyword">` and the other classes.
export function highlight(language: string, text: string): DocumentFragment {
  return fragment(Array.from(text), spans(language, text));
}
