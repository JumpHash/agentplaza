// A task's seats as a card: one row per seat, with its role, its state, the session filling it, the tries and rounds it
// took, the model it last ran on and what the step that sent the task back said. The Expert switch adds the session ids,
// the recipe, the step and the model policy. The card reloads when the task or one of its sessions changes; its dispose
// stops that.

import { workApi, type Seat, type SeatState } from '../api-work';
import { href } from '../router';
import { roleLabel } from '../roles';
import { badge, card, clip, empty, table, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { refreshOn, type LiveCard } from './budget';
import { isTaskFrame, isTaskSession, policyText, seatLabel, seatNotes } from './budget-model';
import { readExpert, switchBox, writeExpert } from './work-expert';

/** Where the seats card's Expert switch is kept. */
const EXPERT_KEY = 'apz.seats.expert';
/** How many characters of a seat's returned findings the table shows. */
const RETURNED_MAX = 160;

const TONE: Record<SeatState, Tone> = { waiting: 'muted', ready: 'info', filled: 'accent', done: 'ok', failed: 'danger' };

/** A task's seats, one row each, in the order the hub gives them. */
export function seatsCard(taskId: string): LiveCard {
  let expert = readExpert(EXPERT_KEY);
  let seats: Seat[] | undefined;
  // Why the latest load failed, or null. A failed reload keeps the rows already on screen.
  let failure: string | null = null;
  let seq = 0;
  const acts = h('span');
  const body = h('div');
  const el = card('Seats', body, { actions: [acts] });

  async function load(): Promise<void> {
    // A reload that a newer one overtakes is dropped: only the latest answer is drawn.
    const mine = ++seq;
    try {
      const list = await workApi.seats(taskId);
      if (mine !== seq) return;
      seats = list;
      failure = null;
    } catch (err) {
      if (mine !== seq) return;
      failure = err instanceof Error ? err.message : String(err);
    }
    draw();
  }

  function setExpert(checked: boolean): void {
    expert = checked;
    writeExpert(EXPERT_KEY, checked);
    draw();
  }

  function draw(): void {
    replace(acts, switchBox('Expert', expert, setExpert));
    replace(body, content());
  }

  function content(): Node[] {
    if (!seats) return [failure ? empty('alert', 'The seats could not be loaded', failure) : h('p.muted', null, 'Loading the seats…')];
    // A failed reload keeps the rows below and says so in one line above them.
    const alert: Node[] = failure ? [h('p.hint.err', { role: 'alert' }, 'Could not refresh the seats. Showing the last ones.')] : [];
    return [...alert, seats.length ? table(columns(), seats) : empty('users', 'No seats yet')];
  }

  function columns(): Column<Seat>[] {
    return [
      { label: 'Seat', cell: (s) => h('div', null, h('strong', null, s.name), h('div.muted', null, roleLabel(s.role))) },
      { label: 'State', cell: (s) => badge(seatLabel(s.state), TONE[s.state]) },
      { label: 'Session', cell: (s) => (s.session ? h('a', { href: href('sessions', s.session) }, expert ? s.session : 'Open') : 'Nobody yet') },
      { label: 'Details', cell: details },
    ];
  }

  /** The seat's tries and rounds, the model it last ran on, the expert line, and the findings that sent the task back. */
  function details(s: Seat): Node | string {
    const lines = seatNotes(s);
    if (s.last_model) lines.push(s.last_model);
    if (expert) lines.push(`${s.recipe} · step ${s.step} · ${policyText(s.model_policy)}`);
    if (s.returned_with) lines.push(`Returned with: ${clip(s.returned_with, RETURNED_MAX)}`);
    return lines.length ? h('div', null, lines.map((line) => h('div', null, line))) : '—';
  }

  const dispose = refreshOn(el, { task: (data) => isTaskFrame(data, taskId), session: (data) => isTaskSession(data, taskId) }, () => void load());
  draw();
  void load();
  return { el, dispose };
}
