// The IDE's editor: one CodeMirror view for every open tab. Each tab keeps its own EditorState (text, undo history and
// selection), and switching tabs swaps that state into the one view. The theme reads the console's tokens, so dark mode
// follows the [data-theme] blocks with no code here.

import { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { css } from '@codemirror/lang-css';
import { html } from '@codemirror/lang-html';
import { javascript } from '@codemirror/lang-javascript';
import { json } from '@codemirror/lang-json';
import { markdown } from '@codemirror/lang-markdown';
import { python } from '@codemirror/lang-python';
import { rust } from '@codemirror/lang-rust';
import { bracketMatching, HighlightStyle, indentOnInput, syntaxHighlighting } from '@codemirror/language';
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';
import { Annotation, EditorState, type Extension } from '@codemirror/state';
import {
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
} from '@codemirror/view';
import { tags } from '@lezer/highlight';
import { languageFor, type Lang } from '../ide/model';

/** Marks a transaction the IDE makes itself (a reload), so it does not count as an edit. */
const external = Annotation.define<boolean>();

const theme = EditorView.theme(
  {
    '&': { height: '100%', fontSize: '13px', color: 'var(--ink)', backgroundColor: 'var(--sheet)' },
    '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.55' },
    '.cm-content': { caretColor: 'var(--signal)' },
    '.cm-gutters': { backgroundColor: 'var(--sheet)', color: 'var(--ink-3)', borderRight: '1px solid var(--rule)' },
    '.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--signal) 6%, transparent)' },
    '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--ink)' },
    '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': { backgroundColor: 'color-mix(in srgb, var(--info) 22%, transparent)' },
    '.cm-matchingBracket': { color: 'var(--ink)', backgroundColor: 'var(--signal-soft)' },
    '.cm-searchMatch': { backgroundColor: 'var(--warn-soft)' },
    '.cm-panels': { backgroundColor: 'var(--sheet)', color: 'var(--ink)', borderColor: 'var(--rule)' },
    '.cm-tooltip': { backgroundColor: 'var(--sheet)', color: 'var(--ink)', borderColor: 'var(--rule)', borderRadius: '0' },
  },
  { dark: false },
);

const highlight = HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier, tags.operatorKeyword], color: 'var(--signal-ink)' },
  { tag: [tags.string, tags.regexp], color: 'var(--ok)' },
  { tag: [tags.number, tags.bool, tags.null, tags.atom], color: 'var(--warn)' },
  { tag: [tags.typeName, tags.className, tags.namespace], color: 'var(--info)' },
  { tag: [tags.comment, tags.lineComment, tags.blockComment], color: 'var(--ink-3)', fontStyle: 'italic' },
  { tag: [tags.heading, tags.strong], fontWeight: '600' },
  { tag: [tags.link, tags.url], color: 'var(--info)', textDecoration: 'underline' },
  { tag: tags.invalid, color: 'var(--crit)' },
]);

function languageExtension(lang: Lang): Extension {
  switch (lang) {
    case 'typescript':
      return javascript({ typescript: true });
    case 'tsx':
      return javascript({ typescript: true, jsx: true });
    case 'javascript':
      return javascript();
    case 'jsx':
      return javascript({ jsx: true });
    case 'rust':
      return rust();
    case 'python':
      return python();
    case 'markdown':
      return markdown();
    case 'json':
      return json();
    case 'css':
      return css();
    case 'html':
      return html();
    case 'plain':
      return [];
  }
}

const base: Extension = [
  lineNumbers(),
  highlightActiveLineGutter(),
  highlightSpecialChars(),
  history(),
  drawSelection(),
  dropCursor(),
  EditorState.allowMultipleSelections.of(true),
  EditorState.tabSize.of(2),
  indentOnInput(),
  bracketMatching(),
  closeBrackets(),
  highlightActiveLine(),
  highlightSelectionMatches(),
  syntaxHighlighting(highlight),
  theme,
  keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, indentWithTab]),
];

export interface StateOpts {
  readOnly: boolean;
  /** Runs on each edit the user makes (not on a reload). */
  onEdit: () => void;
  /** Runs when the cursor or selection moves, or the text changes (the status bar reads the position). */
  onSelect: () => void;
}

/** The state for one file: its text, its language, and whether it may be edited. */
export function stateFor(path: string, text: string, opts: StateOpts): EditorState {
  return EditorState.create({
    doc: text,
    extensions: [
      base,
      languageExtension(languageFor(path)),
      EditorState.readOnly.of(opts.readOnly),
      EditorView.editable.of(!opts.readOnly),
      EditorView.updateListener.of((u) => {
        if (u.docChanged && !u.transactions.some((t) => t.annotation(external))) opts.onEdit();
        if (u.selectionSet || u.docChanged) opts.onSelect();
      }),
    ],
  });
}

export class Editor {
  private readonly view: EditorView;

  constructor(parent: HTMLElement) {
    this.view = new EditorView({ parent, state: EditorState.create({ doc: '' }) });
  }

  /** The state the view holds now: the active tab's, with its edits. */
  current(): EditorState {
    return this.view.state;
  }

  show(state: EditorState): void {
    this.view.setState(state);
  }

  /** Replaces the whole text of the shown file (a reload). The undo history keeps the old text as one step. */
  setText(text: string): void {
    const doc = this.view.state.doc;
    this.view.dispatch({ changes: { from: 0, to: doc.length, insert: text }, annotations: external.of(true) });
  }

  /** Moves the cursor to a line and scrolls it into view. */
  reveal(line: number): void {
    const doc = this.view.state.doc;
    const l = doc.line(Math.min(Math.max(line, 1), doc.lines));
    this.view.dispatch({ selection: { anchor: l.from }, scrollIntoView: true });
    this.view.focus();
  }

  focus(): void {
    this.view.focus();
  }

  destroy(): void {
    this.view.destroy();
  }
}
