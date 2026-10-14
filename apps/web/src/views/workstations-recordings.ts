// Recordings of a workstation: the list on its page (paged, deletable), the playback of one recording (its frames on a
// timeline, with the actions it holds marked along it), and the hub-wide recording defaults on the Workstations list.
// A recording's frames are JPEGs the hub keeps per lease; the hub keeps no time per frame, so the timeline spreads them
// evenly over the recording (see frameAt in workstations-model.ts).

import { api, ApiError, type Agent, type RecordingDefaults, type StationRecording, type StationRecordingDetail, type StationRecordingEvent, type StationView } from '../api';
import { go, href } from '../router';
import { ago, attempt, badge, button, card, empty, field, input, modal, select, table, toast, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import {
  activeAction,
  actionText,
  bytesText,
  clockText,
  defaultsForm,
  DEFAULT_FPS,
  frameAt,
  frameTime,
  PLAYBACK_SPEEDS,
  readDefaults,
  RETENTION_DAYS,
  recordingSpan,
  type DefaultsForm,
  type Section,
} from './workstations-model';
import type { Cleanup } from './index';

const PAGE = 20;
const REFRESH_MS = 10_000;
/** The actions the detail holds (the newest); a longer record shows that it is cut. */
const DETAIL_EVENTS = 1000;

const dateText = (ms: number): string => new Date(ms).toLocaleString();

/** A recording's state as a chip: open while its lease runs. */
function stateBadge(rec: StationRecording): HTMLElement {
  return rec.ended_ms === null ? badge('Recording', 'ok', true) : badge('Ended', 'muted');
}

function takeoverBadge(rec: StationRecording): HTMLElement | null {
  return rec.takeover ? badge('Owner took over', 'warn') : null;
}

/** The recordings of one workstation, newest first, with older pages on request. */
export function recordingsCard(stationId: string, nameOf: (cid: number) => string): Section {
  let alive = true;
  // null until a page has loaded; `failed` when the first page could not.
  let rows: StationRecording[] | null = null;
  let failed = false;
  let hasMore = false;
  let loadingMore = false;
  const body = h('div', null, empty('clock', 'Loading…'));
  const more = button('Show older', () => void loadOlder(), { kind: 'ghost' });
  more.hidden = true;

  const open = (rec: StationRecording): void => go('workstations', stationId, 'recordings', rec.id);

  const confirmDelete = (rec: StationRecording): void => {
    const pane = h(
      'div.stack',
      null,
      h('p', null, 'Its frames and actions are deleted. This cannot be undone.'),
      h(
        'div.ws-buttons',
        null,
        button('Cancel', () => close()),
        button(
          'Delete',
          () => {
            close();
            void remove(rec);
          },
          { kind: 'danger' },
        ),
      ),
    );
    const close = modal('Delete this recording?', pane);
  };

  const remove = async (rec: StationRecording): Promise<void> => {
    const done = await attempt(() => api.deleteStationRecording(rec.id), 'Recording deleted');
    if (done === undefined) return;
    if (rows) rows = rows.filter((r) => r.id !== rec.id);
    render();
  };

  const columns = (): Column<StationRecording>[] => [
    { label: 'Started', cell: (r) => h('span', { title: dateText(r.started_ms) }, ago(r.started_ms)) },
    { label: 'Length', cell: (r) => clockText(recordingSpan(r, null)) },
    { label: 'Frames', cell: (r) => String(r.frames), num: true },
    { label: 'Actions', cell: (r) => String(r.actions), num: true },
    { label: 'Size', cell: (r) => bytesText(r.bytes), num: true },
    { label: 'Agent', cell: (r) => nameOf(r.agent) },
    {
      label: 'State',
      cell: (r) => h('span', null, stateBadge(r), takeoverBadge(r)),
    },
    {
      label: '',
      cell: (r) =>
        button('Delete', (ev) => {
          ev.stopPropagation();
          confirmDelete(r);
        }, {
          kind: 'danger',
          size: 'sm',
          disabled: r.ended_ms === null,
          title: r.ended_ms === null ? 'An open recording is deleted once its lease ends.' : 'Delete this recording',
        }),
    },
  ];

  const render = (): void => {
    if (!alive) return;
    if (rows === null) {
      replace(body, failed ? empty('alert', 'Could not load the recordings.') : empty('clock', 'Loading…'));
      return;
    }
    if (!rows.length) {
      replace(body, empty('clock', 'No recordings yet', 'A lease on a workstation that records leaves a recording here.'));
      return;
    }
    replace(body, h('div.ws-scroll', null, table(columns(), rows, (r) => open(r), (r) => r.id)), h('div.ws-buttons', null, more));
    more.hidden = !hasMore;
  };

  async function refresh(): Promise<void> {
    try {
      const page = await api.stationRecordings({ station: stationId, limit: PAGE });
      if (!alive) return;
      // The older pages already loaded stay below the fresh first page.
      const oldest = page.length ? (page[page.length - 1]?.started_ms ?? 0) : 0;
      const kept = rows ? rows.filter((r) => r.started_ms < oldest) : [];
      rows = [...page, ...kept];
      hasMore = kept.length > 0 || page.length === PAGE;
      failed = false;
    } catch {
      // A failed refresh keeps what the page shows; only a first load that fails shows the error.
      if (!alive) return;
      failed = true;
    }
    render();
  }

  async function loadOlder(): Promise<void> {
    if (!rows || !rows.length || loadingMore) return;
    const last = rows[rows.length - 1];
    if (!last) return;
    loadingMore = true;
    more.disabled = true;
    try {
      const page = await api.stationRecordings({ station: stationId, limit: PAGE, before: last.started_ms });
      if (!alive) return;
      const known = new Set(rows.map((r) => r.id));
      rows = [...rows, ...page.filter((r) => !known.has(r.id))];
      hasMore = page.length === PAGE;
    } catch (err) {
      toast(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      loadingMore = false;
      more.disabled = false;
      if (alive) render();
    }
  }

  const el = card('Recordings', body, { flush: true });
  return {
    el,
    refresh,
    dispose(): void {
      alive = false;
    },
  };
}

/** The hub-wide recording defaults: for the workstations that join from now on. */
export function recordingDefaultsCard(): Section {
  let alive = true;
  let baseline = '';
  const record = h('input', { type: 'checkbox' });
  const takeovers = h('input', { type: 'checkbox' });
  const retention = input({ type: 'number', min: RETENTION_DAYS.min, max: RETENTION_DAYS.max, step: 1, inputmode: 'numeric', 'aria-label': 'Keep recordings for, in days' });
  const fps = select(
    Array.from({ length: DEFAULT_FPS.max - DEFAULT_FPS.min + 1 }, (_, i) => ({ value: String(DEFAULT_FPS.min + i), label: i === 0 ? '0 (actions only)' : String(DEFAULT_FPS.min + i) })),
  );
  const problem = h('p.error-text');
  const saveButton = button('Save defaults', () => void save(), { kind: 'primary' });

  const read = (): DefaultsForm => ({
    record: record.checked,
    record_takeovers: takeovers.checked,
    retention_days: retention.value,
    record_fps: fps.value,
  });
  const show = (f: DefaultsForm): void => {
    record.checked = f.record;
    takeovers.checked = f.record_takeovers;
    retention.value = f.retention_days;
    fps.value = f.record_fps;
  };
  const apply = (d: RecordingDefaults): void => {
    const f = defaultsForm(d);
    show(f);
    baseline = JSON.stringify(f);
  };

  const save = async (): Promise<void> => {
    problem.textContent = '';
    const checked = readDefaults(read());
    if (!checked.ok) {
      problem.textContent = checked.error;
      return;
    }
    saveButton.disabled = true;
    const saved = await attempt(() => api.saveStationRecordingDefaults(checked.defaults), 'Recording defaults saved');
    saveButton.disabled = false;
    if (saved && alive) apply(saved);
  };

  const el = card(
    'Recording defaults',
    h(
      'div.stack',
      null,
      h('p.faint', null, 'For the workstations that join from now on. Each workstation keeps its own settings after that.'),
      h(
        'div.ws-form',
        null,
        h('label.ws-check', null, record, h('span', null, 'Record screen sessions')),
        h('label.ws-check', null, takeovers, h('span', null, "Record the owner's takeovers too")),
        field('Keep recordings for (days)', retention, `${RETENTION_DAYS.min} to ${RETENTION_DAYS.max}. Older recordings are deleted.`),
        field('Frames per second', fps, 'What a recording keeps. Zero keeps the actions only.'),
      ),
      h('div.ws-buttons', null, saveButton),
      problem,
    ),
  );

  return {
    el,
    async refresh(): Promise<void> {
      // Loaded once, and again after a save: a refresh must not take what the owner is typing.
      if (baseline && JSON.stringify(read()) !== baseline) return;
      const d = await api.stationRecordingDefaults().catch((): null => null);
      if (d && alive) apply(d);
    },
    dispose(): void {
      alive = false;
    },
  };
}

// ---- Playback ------------------------------------------------------------------------------------------------------

/** One recording's page: its frames on a timeline with the actions marked along it. */
export function playbackScreen(root: HTMLElement, stationId: string, recordingId: string): Cleanup {
  let alive = true;
  let detail: StationRecordingDetail | null = null;
  // null: the recording could not be loaded (or is not there).
  let missing = false;
  let failed = false;
  let station: StationView | null = null;
  let names = new Map<number, string>();
  // The timeline: `pos` ms into the recording, which runs `spanMs` long; `events` in time order.
  let pos = 0;
  let playing = false;
  let speed = 1;
  let lastTick = 0;
  let shownSeq = 0;
  let spanMs = 0;
  let startMs = 0;
  let events: StationRecordingEvent[] = [];
  let tickTimer: ReturnType<typeof setInterval> | undefined;
  let reloadTimer: ReturnType<typeof setInterval> | undefined;

  const back = h('a.ws-back', { href: href('workstations', stationId) }, 'Back to the workstation');
  const title = h('h1');
  const chipBox = h('span.ws-chips');
  const actionsBox = h('div.ws-actions');
  const stageBody = h('div');
  const listBody = h('div');
  const detailsBody = h('div');

  root.append(
    h('div.view-head.ws-head', null, h('div.ws-heading', null, back, h('div.ws-title', null, title, chipBox)), actionsBox),
    h('div.ws-playback', null, card('Playback', stageBody), card('Actions', listBody, { flush: true })),
    card('Details', detailsBody),
  );

  // ---- the stage: the frame, the controls ----
  const frame = h('img.ws-frame', { alt: 'A frame of the recorded screen' });
  const noFrames = h('p.faint');
  const play = button('Play', () => toggle(), { kind: 'primary' });
  const scrub = h('input', { type: 'range', min: 0, max: 0, step: 1, value: 0, 'aria-label': 'Position in the recording' });
  const speedSelect = select(
    PLAYBACK_SPEEDS.map((s) => ({ value: String(s), label: `${s}×` })),
    '1',
  );
  const clock = h('span.ws-clock');
  const frameLabel = h('span.faint');
  replace(
    stageBody,
    h('div.ws-frame-box', null, frame, noFrames),
    h('div.ws-play', null, play, scrub, speedSelect, clock),
    frameLabel,
  );

  const listBox = h('div.ws-actions-list');
  const listItems: HTMLElement[] = [];
  replace(listBody, listBox);

  const paintList = (): void => {
    const current = activeAction(events, startMs, pos);
    listItems.forEach((item, i) => {
      item.classList.toggle('is-current', i === current);
    });
    const row = listItems[current];
    if (row && playing) {
      // Keeps the current action in view while the timeline plays, inside its own box only.
      if (row.offsetTop < listBox.scrollTop || row.offsetTop + row.offsetHeight > listBox.scrollTop + listBox.clientHeight) {
        listBox.scrollTop = Math.max(0, row.offsetTop - listBox.clientHeight / 3);
      }
    }
  };

  /** The timeline's position on screen: the scrubber, the clock, the frame and the action marked. */
  const paint = (): void => {
    scrub.max = String(spanMs);
    scrub.value = String(Math.round(pos));
    const frames = detail?.frames ?? 0;
    const seq = frameAt(pos, spanMs, frames);
    clock.textContent = `${clockText(pos)} / ${clockText(spanMs)}`;
    play.textContent = playing ? 'Pause' : 'Play';
    play.disabled = spanMs === 0 && frames === 0;
    if (frames > 0 && seq !== shownSeq && detail) {
      shownSeq = seq;
      frame.src = api.stationRecordingFrameUrl(detail.id, seq);
    }
    frame.hidden = frames === 0;
    noFrames.hidden = frames > 0;
    noFrames.textContent = detail ? (detail.ended_ms === null && frames === 0 ? 'Waiting for the first frame.' : 'No frames were kept: the actions only.') : '';
    frameLabel.textContent = frames > 0 ? `Frame ${seq} of ${frames}, at ${clockText(frameTime(seq, spanMs, frames))}` : '';
    paintList();
  };

  const tick = (): void => {
    const now = Date.now();
    if (playing) {
      pos = Math.min(spanMs, pos + (now - lastTick) * speed);
      if (pos >= spanMs) stopPlaying();
    }
    lastTick = now;
    paint();
  };

  const startPlaying = (): void => {
    if (playing) return;
    if (pos >= spanMs) pos = 0;
    playing = true;
    lastTick = Date.now();
    clearInterval(tickTimer);
    tickTimer = setInterval(tick, 100);
    paint();
  };

  const stopPlaying = (): void => {
    playing = false;
    clearInterval(tickTimer);
    tickTimer = undefined;
    paint();
  };

  const toggle = (): void => (playing ? stopPlaying() : startPlaying());

  scrub.addEventListener('input', () => {
    stopPlaying();
    pos = Number(scrub.value);
    paint();
  });
  speedSelect.addEventListener('change', () => {
    speed = Number(speedSelect.value);
  });

  /** The actions as rows: their time into the recording, their words; a click jumps to one. */
  const renderList = (): void => {
    listItems.length = 0;
    if (!detail) {
      replace(listBox, empty('alert', failed || missing ? 'Could not load the actions.' : 'Loading…'));
      return;
    }
    if (!events.length) {
      replace(listBox, empty('clock', 'No actions', 'Nothing was asked of the screen during this recording.'));
      return;
    }
    const rows = events.map((ev) => {
      const item = h(
        'button.ws-action',
        { type: 'button', class: ev.ok ? '' : 'is-refused' },
        h('span.ws-action-time', null, clockText(ev.at_ms - startMs)),
        h('span.ws-action-text', null, actionText(ev)),
        ev.ok ? null : badge('refused', 'danger'),
      );
      item.addEventListener('click', () => {
        stopPlaying();
        pos = Math.max(0, Math.min(spanMs, ev.at_ms - startMs));
        paint();
      });
      listItems.push(item);
      return item;
    });
    replace(
      listBox,
      detail.events.length >= DETAIL_EVENTS ? h('p.faint', null, `The newest ${DETAIL_EVENTS} actions are shown.`) : null,
      rows,
    );
  };

  // ---- the details, the header, the delete ----
  const confirmDelete = (): void => {
    const pane = h(
      'div.stack',
      null,
      h('p', null, 'Its frames and actions are deleted. This cannot be undone.'),
      h(
        'div.ws-buttons',
        null,
        button('Cancel', () => close()),
        button(
          'Delete',
          () => {
            close();
            void remove();
          },
          { kind: 'danger' },
        ),
      ),
    );
    const close = modal('Delete this recording?', pane);
  };

  const remove = async (): Promise<void> => {
    const done = await attempt(() => api.deleteStationRecording(recordingId), 'Recording deleted');
    if (done !== undefined) go('workstations', stationId);
  };

  const renderHeader = (): void => {
    if (!detail) {
      replace(title, 'Recording');
      replace(chipBox);
      replace(actionsBox);
      return;
    }
    const stationName = station?.name ?? stationId;
    replace(title, `Recording of ${stationName}`);
    replace(chipBox, stateBadge(detail), takeoverBadge(detail));
    const open = detail.ended_ms === null;
    replace(
      actionsBox,
      button('Delete', () => confirmDelete(), {
        kind: 'danger',
        disabled: open,
        title: open ? 'An open recording cannot be deleted until its lease ends.' : 'Delete this recording',
      }),
    );
  };

  const renderDetails = (): void => {
    if (!detail) {
      replace(detailsBody, failed || missing ? empty('alert', missing ? 'No such recording.' : 'Could not load this recording.') : h('p.faint', null, 'Loading…'));
      return;
    }
    const pairs: [string, Node | string][] = [
      ['Workstation', station ? h('a', { href: href('workstations', stationId) }, station.name) : stationId],
      ['Agent', names.get(detail.agent) ?? `agent ${detail.agent}`],
      ['Task', detail.task ?? '—'],
      ['Lease', detail.lease],
      ['Started', `${dateText(detail.started_ms)} (${ago(detail.started_ms)})`],
      ['Ended', detail.ended_ms === null ? 'Still recording' : dateText(detail.ended_ms)],
      ['Length', clockText(spanMs)],
      ['Frames', String(detail.frames)],
      ['Actions', String(detail.actions)],
      ['Size', bytesText(detail.bytes)],
      ['Picture', detail.width && detail.height ? `${detail.width}×${detail.height}` : '—'],
      ['Owner took over', detail.takeover ? 'Yes, during this recording' : 'No'],
    ];
    replace(detailsBody, h('dl.node-facts', null, pairs.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])));
  };

  /** The record as the hub holds it. An open recording is read again now and then, so its actions and frames grow. */
  const absorb = (d: StationRecordingDetail): void => {
    detail = d;
    startMs = d.started_ms;
    events = [...d.events].sort((a, b) => a.at_ms - b.at_ms);
    const lastAt = events.length ? (events[events.length - 1]?.at_ms ?? null) : null;
    spanMs = recordingSpan(d, lastAt);
    if (pos > spanMs) pos = spanMs;
    if (d.ended_ms !== null && reloadTimer !== undefined) {
      clearInterval(reloadTimer);
      reloadTimer = undefined;
    }
  };

  async function load(): Promise<void> {
    try {
      const d = await api.stationRecording(recordingId);
      if (!alive) return;
      absorb(d);
      failed = false;
      missing = false;
    } catch (err) {
      if (!alive) return;
      missing = err instanceof ApiError && err.status === 404;
      failed = !missing;
      if (missing) {
        detail = null;
      }
    }
    renderHeader();
    renderDetails();
    renderList();
    paint();
  }

  const loadStation = async (): Promise<void> => {
    const [s, ag] = await Promise.all([api.station(stationId).catch((): null => null), api.agents().catch((): Agent[] => [])]);
    if (!alive) return;
    station = s;
    names = new Map((ag ?? []).map((a): [number, string] => [a.cid, a.name]));
    renderHeader();
    renderDetails();
  };

  void loadStation();
  void load();
  reloadTimer = setInterval(() => {
    if (!detail || detail.ended_ms !== null) return;
    void load();
  }, REFRESH_MS);

  return () => {
    alive = false;
    clearInterval(tickTimer);
    clearInterval(reloadTimer);
  };
}
