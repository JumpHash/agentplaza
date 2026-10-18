// Files a composer carries: a button that picks them, a chip for each (its name and size, "sending…" while it uploads,
// "failed" when it did not), and drops or pastes on the composer or the pane around it. Each file is uploaded as it is
// taken (file-drop.ts attachFiles); the ids go with the next message, and clear() empties the list after a send. The
// rules a send goes by are plain functions here, so the tests need no DOM.

import { button, toast } from './components';
import { h, icon, replace } from './dom';
import { attachFiles, dropZone, keepFiles, pickFiles, sizeText, type PickedFile } from './file-drop';

export type ChipState = 'sending' | 'ready' | 'failed';

/** A chip's text after its name: the size, and what the file waits for or why it failed. */
export function chipLabel(size: number, state: ChipState): string {
  const s = sizeText(size);
  if (state === 'sending') return `${s} · sending…`;
  if (state === 'failed') return `${s} · failed`;
  return s;
}

/** A file's name in a message: the last part of its path (`src/main.rs` is `main.rs`). */
export function fileName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

/** A message may go when nothing is busy and it has some text or some files. */
export function maySend(text: string, ids: string[], busy: boolean): boolean {
  return !busy && (text.trim() !== '' || ids.length > 0);
}

interface Chip {
  name: string;
  size: number;
  state: ChipState;
  file: PickedFile;
  /** The uploaded file's id, once it is ready. */
  id: string;
}

export interface AttachBar {
  /** The chips, shown above the composer's text; hidden while there are none. */
  el: HTMLElement;
  /** The button that opens the file dialog. */
  button: HTMLButtonElement;
  /** Adds files: each one uploads as it is taken, one at a time. */
  take(files: PickedFile[]): void;
  /** The ids of the files that are ready, in the order they were taken. */
  ids(): string[];
  /** True while an upload is in flight. */
  busy(): boolean;
  /** True while a file that failed is still on the list. */
  failed(): boolean;
  /** Empties the list, after a send. */
  clear(): void;
  /** Makes `zone` take dropped files and pasted files. Returns the cleanup. */
  bind(zone: HTMLElement): () => void;
}

export function attachBar(opts: { onChange?: () => void } = {}): AttachBar {
  let chips: Chip[] = [];
  // Uploads run one after another, in the order the files were taken.
  let queue: Promise<void> = Promise.resolve();

  const el = h('div.attach-chips');
  el.style.display = 'none';

  function chipEl(c: Chip): HTMLElement {
    return h(
      'span.attach-chip',
      { 'data-state': c.state, title: `${c.name}, ${chipLabel(c.size, c.state)}` },
      h('span.name', null, c.name),
      h('span', null, chipLabel(c.size, c.state)),
      h('button', { type: 'button', title: `Remove ${c.name}`, 'aria-label': `Remove ${c.name}`, onclick: () => remove(c) }, icon('x')),
    );
  }

  function changed(): void {
    replace(el, chips.map((c) => chipEl(c)));
    el.style.display = chips.length ? '' : 'none';
    opts.onChange?.();
  }

  function remove(chip: Chip): void {
    chips = chips.filter((c) => c !== chip);
    changed();
  }

  async function upload(chip: Chip): Promise<void> {
    // Removed before its turn: nothing to send.
    if (!chips.includes(chip)) return;
    try {
      const [att] = await attachFiles([chip.file]);
      if (!att) throw new Error('the hub sent no file back');
      chip.id = att.id;
      chip.state = 'ready';
    } catch (err) {
      chip.state = 'failed';
      toast(`${chip.name} did not upload: ${err instanceof Error ? err.message : String(err)}`, 'error');
    }
    if (chips.includes(chip)) changed();
  }

  function take(files: PickedFile[]): void {
    const { kept } = keepFiles(files);
    if (kept.length === 0) return;
    const added = kept.map(
      (file): Chip => ({ name: fileName(file.path), size: file.file.size, state: 'sending', file, id: '' }),
    );
    chips = [...chips, ...added];
    changed();
    // upload() reports its own failures; the catch keeps one failure from stopping the rest of the queue.
    for (const chip of added) queue = queue.then(() => upload(chip)).catch(() => undefined);
  }

  const attachButton = button('', () => void pickFiles().then(take), { icon: 'paperclip', title: 'Attach files' });

  return {
    el,
    button: attachButton,
    take,
    ids: () => chips.filter((c) => c.state === 'ready').map((c) => c.id),
    busy: () => chips.some((c) => c.state === 'sending'),
    failed: () => chips.some((c) => c.state === 'failed'),
    clear: () => {
      chips = [];
      changed();
    },
    bind: (zone: HTMLElement) => {
      const off = dropZone(zone, take);
      const onPaste = (ev: ClipboardEvent): void => {
        const files = [...(ev.clipboardData?.files ?? [])];
        if (files.length === 0) return;
        ev.preventDefault();
        take(files.map((file) => ({ path: file.name, file })));
      };
      zone.addEventListener('paste', onPaste);
      return () => {
        off();
        zone.removeEventListener('paste', onPaste);
      };
    },
  };
}
