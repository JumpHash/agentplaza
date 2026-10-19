// Recordings of a workstation: the list on its page (paged, deletable), the playback of one recording (its frames on a
// timeline, with the actions it holds marked along it), and the hub-wide recording defaults on the Workstations list.
// A recording's frames are JPEGs the hub keeps per lease, each with the time it was captured, so the timeline runs on
// those times (a takeover's gap shows); a recording kept before frames had times spreads them evenly (frameTimes in
// workstations-model.ts).

import { api, ApiError, type Agent, type RecordingDefaults, type StationRecording, type StationRecordingDetail, type StationRecordingEvent, type StationView } from '../api';
import { go, href } from '../router';
import { attempt, badge, card, field, input, select, toast } from '../ui/components';
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
  frameTimes,
  PLAYBACK_SPEEDS,
  readDefaults,
  RETENTION_DAYS,
  recordingSpan,
  type DefaultsForm,
  type Section,
} from './workstations-model';
import type { Cleanup } from './index';
import { ago, btn, check, dialog, empty, muted, kv, ptable, type PCol } from './workstations-ui';

const PAGE = 20;
const REFRESH_MS = 10_000;
/** The actions the detail holds (the newest); a longer record shows that it is cut. */
const DETAIL_EVENTS = 1000;

/** A recording's state as a chip: open while its lease runs. */
function stateBadge(rec: { ended_ms: number | null }): HTMLElement {
  return rec.ended_ms === null ? badge('Recording', 'ok', true) : badge('Ended', 'muted', true);
}

function takeoverBadge(rec: { takeover: boolean }): HTMLElement | null {
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
  const more = btn('Show older', () => void loadOlder(), { kind: 'ghost' });
  more.hidden = true;

  const confirmDelete = (rec: StationRecording): void => {
    const close = dialog(
      'Delete this recording?',
      h('p', null, 'Its frames and actions are deleted. This cannot be undone.'),
      [
        btn('Cancel', () => close(), { kind: 'ghost' }),
        btn(
          'Delete',
          () => {
            close();
            void remove(rec);
          },
          { kind: 'danger' },
        ),
      ],
    );
  };

  const remove = async (rec: StationRecording): Promise<void> => {
    const done = await attempt(() => api.deleteStationRecording(rec.id), 'Recording deleted');
    if (done === undefined) return;
    if (rows) rows = rows.filter((r) => r.id !== rec.id);
    render();
  };

  const columns: PCol<StationRecording>[] = [
    {
      label: 'Started',
      cell: (r) => h('a', { href: href('workstations', stationId, 'recordings', r.id), title: new Date(r.started_ms).toLocaleString() }, ago(r.started_ms)),
    },
    { label: 'Length', cell: (r) => clockText(recordingSpan(r, null)) },
    { label: 'Frames', cls: 'num', cell: (r) => String(r.frames) },
    { label: 'Actions', cls: 'num', cell: (r) => String(r.actions) },
    { label: 'Size', cls: 'num', cell: (r) => bytesText(r.bytes) },
    { label: 'Agent', cell: (r) => nameOf(r.agent) },
    {
      label: 'State',
      cell: (r) => [stateBadge(r), ...(takeoverBadge(r) ? [' ', takeoverBadge(r) as HTMLElement] : [])],
    },
    {
      label: '',
      cls: 'num',
      cell: (r) =>
        btn('Delete', () => confirmDelete(r), {
          kind: 'danger',
          sm: true,
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
    replace(body, ptable(columns, rows), h('div.ws-cluster', null, more));
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
  const retention = input({ type: 'number', min: RETENTION_DAYS.min, max: RETENTION_DAYS.max, step: 1, inputmode: 'numeric', id: 'ws-rd-days', 'aria-label': 'Keep recordings for, in days' });
  const fps = select(
    Array.from({ length: DEFAULT_FPS.max - DEFAULT_FPS.min + 1 }, (_, i) => ({ value: String(DEFAULT_FPS.min + i), label: String(DEFAULT_FPS.min + i) })),
  );
  const problem = h('div.ws-problem');
  const saveButton = btn('Save defaults', () => void save(), { kind: 'primary' });

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
      'div.ws-stack',
      null,
      muted('For the workstations that join from now on. Each workstation keeps its own settings after that.'),
      h(
        'div.ws-form',
        null,
        check(record, 'Record screen sessions'),
        check(takeovers, "Record the owner's takeovers too"),
        field('Keep recordings for (days)', retention, `${RETENTION_DAYS.min} to ${RETENTION_DAYS.max}. Older recordings are deleted.`),
        field('Frames per second', fps, `${DEFAULT_FPS.min} to ${DEFAULT_FPS.max}. What a recording keeps.`),
      ),
      h('div.ws-cluster', null, saveButton),
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
  // When each frame was kept, in ms into the recording (index seq - 1).
  let times: number[] = [];
  let events: StationRecordingEvent[] = [];
  let tickTimer: ReturnType<typeof setInterval> | undefined;
  let reloadTimer: ReturnType<typeof setInterval> | undefined;

  const back = h('a.ws-back', { href: href('workstations', stationId) }, 'All workstations');
  const title = h('h1');
  const chipRow = h('div.ws-chips-l');
  const actionsBox = h('div.page-actions');
  const detailsBody = h('div');
  const listBody = h('div');

  // ---- the stage: the frame, the controls ----
  const frame = h('img', { alt: 'A frame of the recorded screen' });
  const screen = h('div.ws-rec-screen', null, frame);
  const noFrames = h('div.ws-stage');
  const frameLine = h('p.muted');
  const play = btn('Play', () => toggle(), { kind: 'primary', sm: true });
  const speedBox = h('div.seg', { role: 'group', 'aria-label': 'Speed' });
  const scrub = h('input.ws-rec-range', { type: 'range', min: 0, max: 0, step: 100, value: 0, 'aria-label': 'Playback position', id: 'ws-rp-range' });
  const clock = h('span.ws-rec-clock');
  const playerBody = h('div.ws-rec-player', null, screen, noFrames, frameLine, h('div.ws-rec-bar', null, play, speedBox, scrub, clock));

  const paintSpeed = (): void => {
    replace(
      speedBox,
      PLAYBACK_SPEEDS.map((x) => {
        const on = x === speed;
        const b = btn(`${x}x`, () => {
          speed = x;
          paintSpeed();
        }, { kind: on ? 'primary' : 'ghost', sm: true });
        b.setAttribute('aria-pressed', String(on));
        return b;
      }),
    );
  };
  paintSpeed();

  const listItems: HTMLElement[] = [];
  const listBox = h('div.ws-rec-acts');

  root.append(
    back,
    h('header.page-head', null, h('div.page-head-row', null, title, h('span.sp'), actionsBox)),
    chipRow,
    h(
      'div.ws-stack',
      null,
      card('Playback', playerBody),
      card('Actions', listBody, { flush: true }),
      card('Details', detailsBody),
    ),
  );
  replace(listBody, listBox);

  const paintList = (): void => {
    const current = activeAction(events, startMs, pos);
    listItems.forEach((item, i) => {
      item.classList.toggle('on', i === current);
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
    const seq = frameAt(pos, times);
    clock.textContent = `${clockText(pos)} / ${clockText(spanMs)}`;
    play.textContent = playing ? 'Pause' : 'Play';
    play.disabled = spanMs === 0 && frames === 0;
    if (frames > 0 && seq !== shownSeq && detail) {
      shownSeq = seq;
      frame.src = api.stationRecordingFrameUrl(detail.id, seq);
    }
    screen.hidden = frames === 0;
    noFrames.hidden = frames > 0;
    replace(noFrames, detail ? (detail.ended_ms === null && frames === 0 ? 'Waiting for the first frame.' : 'No frames were kept: the actions only.') : '');
    frameLine.hidden = frames === 0;
    frameLine.textContent = frames > 0 ? `Frame ${seq} of ${frames}, at ${clockText(frameTime(seq, times))}` : '';
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

  /** The actions as rows: their time into the recording, their words; a click jumps to one. */
  const renderList = (): void => {
    listItems.length = 0;
    if (!detail) {
      replace(listBox, empty('alert', failed || missing ? 'Could not load the actions.' : 'Loading…'));
      return;
    }
    if (!events.length) {
      replace(listBox, empty('clock', 'No actions yet'));
      return;
    }
    const rows = events.map((ev) => {
      const item = h(
        'div.ws-rec-act',
        { role: 'button', tabindex: 0 },
        h('span.ws-mono.muted', null, clockText(ev.at_ms - startMs)),
        h('span.clip', null, actionText(ev)),
        ev.ok ? null : badge('refused', 'danger'),
      );
      const jump = (): void => {
        stopPlaying();
        pos = Math.max(0, Math.min(spanMs, ev.at_ms - startMs));
        paint();
      };
      item.addEventListener('click', jump);
      item.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          jump();
        }
      });
      listItems.push(item);
      return item;
    });
    replace(listBox, rows, detail.events.length >= DETAIL_EVENTS ? h('p.hint', null, `The newest ${DETAIL_EVENTS} actions are shown.`) : null);
  };

  // ---- the details, the header, the delete ----
  const confirmDelete = (): void => {
    const close = dialog(
      'Delete this recording?',
      h('p', null, 'Its frames and actions are deleted. This cannot be undone.'),
      [
        btn('Cancel', () => close(), { kind: 'ghost' }),
        btn(
          'Delete',
          () => {
            close();
            void remove();
          },
          { kind: 'danger' },
        ),
      ],
    );
  };

  const remove = async (): Promise<void> => {
    const done = await attempt(() => api.deleteStationRecording(recordingId), 'Recording deleted');
    if (done !== undefined) go('workstations', stationId);
  };

  const renderHeader = (): void => {
    if (!detail) {
      replace(title, 'Recording');
      replace(chipRow);
      replace(actionsBox);
      return;
    }
    const stationName = station?.name ?? stationId;
    replace(title, `Recording of ${stationName}`);
    replace(chipRow, stateBadge(detail), takeoverBadge(detail));
    const open = detail.ended_ms === null;
    replace(
      actionsBox,
      btn('Delete', () => confirmDelete(), {
        kind: 'danger',
        disabled: open,
        title: open ? 'An open recording cannot be deleted until its lease ends.' : 'Delete this recording',
      }),
    );
  };

  const renderDetails = (): void => {
    if (!detail) {
      replace(detailsBody, failed || missing ? empty('alert', missing ? 'No such recording.' : 'Could not load this recording.') : muted('Loading…'));
      return;
    }
    replace(
      detailsBody,
      kv([
        ['Workstation', station ? h('a', { href: href('workstations', stationId) }, station.name) : stationId],
        ['Agent', names.get(detail.agent) ?? `agent ${detail.agent}`],
        ['Task', detail.task ?? '—'],
        ['Lease', detail.lease],
        ['Started', ago(detail.started_ms)],
        ['Ended', detail.ended_ms === null ? 'Still recording' : ago(detail.ended_ms)],
        ['Length', clockText(spanMs)],
        ['Frames', String(detail.frames)],
        ['Actions', String(detail.actions)],
        ['Size', bytesText(detail.bytes)],
        ['Picture', detail.width && detail.height ? `${detail.width}×${detail.height}` : '—'],
        ['Owner took over', detail.takeover ? 'Yes' : 'No'],
      ]),
    );
  };

  /** The record as the hub holds it. An open recording is read again now and then, so its actions and frames grow. */
  const absorb = (d: StationRecordingDetail): void => {
    detail = d;
    startMs = d.started_ms;
    events = [...d.events].sort((a, b) => a.at_ms - b.at_ms);
    // An open recording runs to its newest action or frame, whichever is later.
    const lastFrame = d.frame_times.length ? (d.frame_times[d.frame_times.length - 1]?.ts_ms ?? null) : null;
    const lastEvent = events.length ? (events[events.length - 1]?.at_ms ?? null) : null;
    const lastAt = lastFrame === null ? lastEvent : Math.max(lastFrame, lastEvent ?? lastFrame);
    spanMs = recordingSpan(d, lastAt);
    times = frameTimes(d.frames, spanMs, d.started_ms, d.frame_times);
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
