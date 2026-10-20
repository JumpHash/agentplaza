// The agent page's Harness card: the agent's limits (turns per work item, when its
// conversation is compressed), controls for its running work (compress, steer, more
// turns) and its loops.

import { api, type HarnessLimits, type HarnessLoop } from '../api';
import { on } from '../store';
import { ago, attempt, badge, button, clip, field, input, modal, textarea, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';

export interface HarnessCard {
  el: HTMLElement;
  /** Called when the agent's record changes (status, current work). */
  update(working: boolean): void;
  destroy(): void;
}

const KB = 1024;
const STATE_TONE: Record<HarnessLoop['state'], Tone> = { running: 'info', done: 'ok', stopped: 'muted' };

type ControlBody = Parameters<typeof api.agentControl>[1];

const message = (err: unknown): string => (err instanceof Error ? err.message : String(err));

/** The text a control call's result shows, or the fallback when the result says nothing. */
function resultText(result: unknown, fallback: string): string {
  if (typeof result === 'string' && result.trim()) return result.trim();
  if (result && typeof result === 'object') {
    const record = result as Record<string, unknown>;
    const note = record['message'] ?? record['note'];
    if (typeof note === 'string' && note.trim()) return note.trim();
  }
  return fallback;
}

/** A limit's field value: undefined when empty, NaN when it is not a number of 0 or more. */
function numberOf(control: HTMLInputElement): number | undefined {
  const raw = control.value.trim();
  if (raw === '') return undefined;
  const n = Number(raw);
  return Number.isFinite(n) && n >= 0 ? n : Number.NaN;
}

export function harnessCard(cid: number): HarnessCard {
  let alive = true;
  let working = false;
  let loopSeq = 0;

  // ---- Limits: blank means the plaza default ----
  const turnsIn = input({ type: 'number', min: 1, placeholder: 'default', 'aria-label': 'Turns per work item' });
  const compressIn = input({ type: 'number', min: 0, step: 'any', placeholder: 'default', 'aria-label': 'Compress at (KB)' });
  const keepIn = input({ type: 'number', min: 0, placeholder: 'default', 'aria-label': 'Keep recent turns' });

  const loadLimits = async (): Promise<void> => {
    const limits = await attempt<HarnessLimits>(() => api.agentLimits(cid));
    if (!alive || !limits) return;
    turnsIn.value = limits.max_turns == null ? '' : String(limits.max_turns);
    compressIn.value = limits.compress_at == null ? '' : String(Number((limits.compress_at / KB).toFixed(1)));
    keepIn.value = limits.keep_recent == null ? '' : String(limits.keep_recent);
  };

  const saveLimits = async (): Promise<void> => {
    const turns = numberOf(turnsIn);
    const kb = numberOf(compressIn);
    const keep = numberOf(keepIn);
    if ([turns, kb, keep].some((n) => n !== undefined && Number.isNaN(n))) {
      toast('Limits must be numbers of 0 or more', 'error');
      return;
    }
    if (turns === undefined && kb === undefined && keep === undefined) {
      toast('Fill in a limit to save it', 'error');
      return;
    }
    const body: ControlBody = { action: 'set_limits' };
    if (turns !== undefined) body.max_turns = turns;
    if (kb !== undefined) body.compress_at = Math.round(kb * KB);
    if (keep !== undefined) body.keep_recent = keep;
    await attempt(() => api.agentControl(cid, body), 'Limits saved');
    void loadLimits();
  };

  const saveLimitsBtn = button('Save limits', () => void saveLimits(), { kind: 'primary', size: 'sm', title: 'Save the filled limits' });

  // ---- Running work: enabled only while the agent is working ----
  const idle = h('p.hint', null, 'Available while the agent is working.');

  const control = async (body: ControlBody, fallback: string): Promise<boolean> => {
    try {
      toast(resultText(await api.agentControl(cid, body), fallback));
      return true;
    } catch (err) {
      toast(message(err), 'error');
      return false;
    }
  };

  const compressBtn = button('Compress now', () => void control({ action: 'compress' }, 'Compression requested'), { title: 'Compress this agent’s conversation now' });
  const extendBtn = button('+20 turns', () => void control({ action: 'extend_turns', turns: 20 }, 'Turns extended'), { title: 'Allow 20 more turns for the current work' });
  const steerIn = input({ type: 'text', placeholder: 'Steer the current work…', 'aria-label': 'Steer note' });
  const steer = async (): Promise<void> => {
    const text = steerIn.value.trim();
    if (!text || !working) return;
    if (await control({ action: 'steer', text }, 'Steer sent')) steerIn.value = '';
  };
  const steerBtn = button('Steer', () => void steer(), { kind: 'primary', size: 'sm', title: 'Send a note into the current work' });
  steerIn.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      void steer();
    }
  });
  const busy: (HTMLButtonElement | HTMLInputElement)[] = [compressBtn, extendBtn, steerIn, steerBtn];

  // ---- Loops: this agent's, newest first ----
  const loopHost = h('div', null, h('p.muted', null, 'Loading…'));

  const stopLoop = async (loop: HarnessLoop): Promise<void> => {
    await attempt(() => api.stopLoop(loop.id), 'Loop stopped');
    void loadLoops();
  };

  const loopRow = (loop: HarnessLoop): HTMLElement => {
    const runs = `${loop.runs}/${loop.max_runs > 0 ? loop.max_runs : '∞'}`;
    return h(
      'div.ag-row',
      null,
      h(
        'div',
        { style: 'min-width:0' },
        h('div.ag-clip', { title: loop.prompt }, clip(loop.prompt, 80)),
        h(
          'div.ag-row-sub',
          null,
          badge(loop.state, STATE_TONE[loop.state]),
          ` every ${Math.round(loop.every_secs / 60)} min · `,
          h('span', { title: 'Runs so far / maximum (∞ until the agent says LOOP DONE)' }, `runs ${runs}`),
          loop.last_work != null ? ` · last ${ago(loop.last_work)}` : null,
        ),
        loop.last_summary ? h('div.ag-row-sub', { title: loop.last_summary }, clip(loop.last_summary, 120)) : null,
      ),
      loop.state === 'running' ? button('Stop', () => void stopLoop(loop), { kind: 'danger', size: 'sm', title: 'Stop this loop' }) : null,
    );
  };

  const loadLoops = async (): Promise<void> => {
    const seq = ++loopSeq;
    try {
      const all = await api.loops();
      if (!alive || seq !== loopSeq) return;
      const mine = all.filter((loop) => loop.cid === cid).sort((a, b) => b.created_ms - a.created_ms);
      replace(loopHost, mine.length ? mine.map(loopRow) : h('p.muted', null, 'No loops for this agent.'));
    } catch (err) {
      if (!alive || seq !== loopSeq) return;
      replace(loopHost, h('p.danger', null, message(err)));
    }
  };

  const openNewLoop = (): void => {
    const prompt = textarea({ rows: 4, placeholder: 'What the agent should do on each run…', 'aria-label': 'Loop prompt' });
    const every = input({ type: 'number', min: 1, step: 1, value: 30, 'aria-label': 'Every (minutes)' });
    const maxRuns = input({ type: 'number', min: 0, step: 1, value: 0, 'aria-label': 'Max runs' });
    const start = async (): Promise<void> => {
      const text = prompt.value.trim();
      const minutes = Number(every.value);
      const runs = Number(maxRuns.value);
      if (!text) return toast('Write the loop prompt first', 'error');
      if (!Number.isInteger(minutes) || minutes < 1) return toast('Every must be at least 1 minute', 'error');
      if (!Number.isInteger(runs) || runs < 0) return toast('Max runs must be 0 or more', 'error');
      const started = await attempt(() => api.startLoop(cid, text, minutes, runs), 'Loop started');
      if (started) {
        close();
        void loadLoops();
      }
    };
    const body = h(
      'div.ag-stack',
      null,
      field('Prompt', prompt),
      field('Every (minutes)', every),
      field('Max runs', maxRuns, '0 = until the agent says LOOP DONE'),
      h('div.cluster', { style: 'justify-content:flex-end' }, button('Start loop', () => void start(), { kind: 'primary', size: 'sm' })),
    );
    const close = modal('New loop', body);
  };

  const newLoopBtn = button('New loop', () => openNewLoop(), { title: 'Start a recurring prompt for this agent' });

  const el = h(
    'div.ag-stack',
    null,
    h('h4', null, 'Limits'),
    h('div.ag-grid3', null, field('Turns per work item', turnsIn), field('Compress at (KB)', compressIn), field('Keep recent turns', keepIn)),
    h('div.cluster', { style: 'justify-content:flex-end' }, saveLimitsBtn),
    h('h4', null, 'Running work'),
    idle,
    h('div.cluster', null, compressBtn, extendBtn),
    h('div.ag-compose', null, steerIn, steerBtn),
    h('h4', null, 'Loops'),
    h('div', null, newLoopBtn),
    loopHost,
  );

  const off = on('loop', () => void loadLoops());
  void loadLimits();
  void loadLoops();

  return {
    el,
    update(next: boolean) {
      working = next;
      for (const control of busy) control.disabled = !working;
      idle.hidden = working;
    },
    destroy() {
      alive = false;
      off();
    },
  };
}
