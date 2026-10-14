// The IDE's two overlays: quick open (a file by name, over the paths the tree has loaded) and search (the hub's bounded
// text search). Each returns its close function; Escape closes an overlay and stops there, so it does not also leave the
// IDE.

import type { IdeMatch } from '../api';
import { rankPaths } from '../ide/model';
import { h } from '../ui/dom';

const LIMIT = 60;

function frame(host: HTMLElement, label: string, body: Node[], close: () => void): () => void {
  const backdrop = h('div.ide-overlay', null, h('div.ide-palette', { role: 'dialog', 'aria-label': label }, body));
  const done = () => {
    backdrop.remove();
    close();
  };
  backdrop.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      e.preventDefault();
      done();
    }
  });
  backdrop.addEventListener('mousedown', (e) => {
    if (e.target === backdrop) done();
  });
  host.appendChild(backdrop);
  backdrop.querySelector<HTMLInputElement>('input')?.focus();
  return done;
}

/** Quick open: type part of a path; Up and Down move, Enter (or a click) picks. `paths` is read on each keystroke. */
export function quickOpen(host: HTMLElement, paths: () => string[], onPick: (path: string) => void, onClose: () => void): () => void {
  const input = h('input.input', { type: 'text', placeholder: 'Open a file by name', 'aria-label': 'File name', autocomplete: 'off', spellcheck: false });
  const list = h('ul.ide-hits', { role: 'listbox' });
  let hits: string[] = [];
  let at = 0;
  let done: () => void = () => undefined;
  const pick = (path: string) => {
    done();
    onPick(path);
  };
  const paint = () => {
    list.replaceChildren(
      ...hits.map((path, i) => {
        const li = h('li', { role: 'option', 'aria-selected': i === at ? 'true' : 'false', class: i === at ? 'on' : '' }, path);
        li.addEventListener('mousedown', (e) => {
          e.preventDefault();
          pick(path);
        });
        return li;
      }),
    );
    list.querySelector('.on')?.scrollIntoView({ block: 'nearest' });
  };
  const search = () => {
    hits = rankPaths(paths(), input.value, LIMIT);
    at = 0;
    paint();
  };
  input.addEventListener('input', search);
  input.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (hits.length) at = (at + (e.key === 'ArrowDown' ? 1 : hits.length - 1)) % hits.length;
      paint();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const path = hits[at];
      if (path) pick(path);
    }
  });
  done = frame(host, 'Quick open', [input, list], onClose);
  search();
  return done;
}

/** Search: a text (and an optional glob). Enter runs it; a click on a match opens the file at that line. */
export function searchPanel(
  host: HTMLElement,
  run: (q: string, glob: string) => Promise<{ matches: IdeMatch[]; truncated: boolean }>,
  onPick: (path: string, line: number) => void,
  onClose: () => void,
): () => void {
  const q = h('input.input', { type: 'search', placeholder: 'Search the workspace', 'aria-label': 'Search text', autocomplete: 'off', spellcheck: false });
  const glob = h('input.input', { type: 'text', placeholder: 'Only files matching, e.g. src/**/*.ts', 'aria-label': 'File glob', autocomplete: 'off', spellcheck: false });
  const status = h('p.ide-note', { 'aria-live': 'polite' }, 'Enter to search.');
  const list = h('ul.ide-hits', { role: 'listbox' });
  let seq = 0;
  let done: () => void = () => undefined;
  const go = async () => {
    const text = q.value.trim();
    if (text === '') return;
    const mine = ++seq;
    status.textContent = 'Searching…';
    list.replaceChildren();
    try {
      const r = await run(text, glob.value.trim());
      if (mine !== seq) return;
      status.textContent = r.matches.length === 0 ? 'No matches.' : `${r.matches.length} matches${r.truncated ? ', more than the search returns: narrow it' : ''}`;
      list.replaceChildren(
        ...r.matches.map((m) => {
          const li = h('li', { role: 'option' }, h('span.ide-hit-where', null, `${m.path}:${m.line}`), h('span.ide-hit-text', null, m.text));
          li.addEventListener('click', () => {
            done();
            onPick(m.path, m.line);
          });
          return li;
        }),
      );
    } catch (err) {
      if (mine === seq) status.textContent = err instanceof Error ? err.message : String(err);
    }
  };
  q.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      void go();
    }
  });
  glob.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      void go();
    }
  });
  done = frame(host, 'Search', [q, glob, status, list], onClose);
  return done;
}
