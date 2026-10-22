// The Workstations screen: the machines agents operate through the hub. The list enrolls new ones
// with a join code (its token shown once, with the command to run on the machine), shows each one's
// state and a picture of its screen, and holds the hub-wide recording defaults. `#/workstations/<id>` is one
// workstation: its facts, the live view (and takeover), its leases, its work folder, its recordings, its
// settings (the takeover policy, recording and rate) and its audit. `#/workstations/<id>/recordings/<rid>` plays
// one recording. Refreshes on the station, lease and sync frames, and every few seconds.

import { agentLabel, agentName } from '../roles';
import '../styles/proto/settings-d.css';
import { api, ApiError, type Agent, type StationAuditRow, type StationCode, type StationLease, type StationView, type Takeover } from '../api';
import { href } from '../router';
import { on } from '../store';
import { attempt, badge, card, field, input, select, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import {
  accountBadge,
  auditLabel,
  codeChip,
  countdown,
  displaysText,
  formLabel,
  gpuLine,
  joinCommand,
  leaseCount,
  leaseKindLabel,
  leaseStateChip,
  MAX_NAME_CHARS,
  nameProblem,
  osText,
  pictureNote,
  RATE_MAX,
  RECORD_FPS,
  rateText,
  readRate,
  readSettings,
  RETENTION_DAYS,
  type Section,
  settingsForm,
  sortStations,
  stationSummary,
  statusChip,
  TAKEOVER_OPTIONS,
  takeoverText,
  TTL_OPTIONS,
  WORK_FOLDER_MAX,
  type SettingsForm,
} from './workstations-model';
import { liveView, type LiveHandle } from './workstations-live';
import { playbackScreen, recordingDefaultsCard, recordingsCard } from './workstations-recordings';
import { syncCard } from './workstations-sync';
import { vmsSection } from './workstations-vms';
import { ago, btn, check, dialog, empty, head, kv, muted, ptable, type PCol } from './workstations-ui';
import type { Cleanup } from './index';

const POLL_MS = 10_000;
/** How often the list's pictures are asked for again, while the page is in view. */
const THUMB_MS = 10_000;
/** The frames that change a workstation, its leases, its work folder or its recordings. */
const EVENTS = [
  'station',
  'lease_waiting',
  'lease_granted',
  'lease_released',
  'lease_ended',
  'lease_failed',
  'lease_owner_input',
  'station_takeover',
  'station_give_back',
  'station_sync_conflict',
  'station_sync_paused',
];

/** The OS's name alone, as a picture box shows it. */
const OS_NAME: Record<string, string> = { macos: 'macOS', windows: 'Windows', linux: 'Linux' };

/** Calls `fn` a moment after the last change frame, so a burst of them is one refresh. Returns the unsubscribe. */
function onChanges(fn: () => void): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const later = (): void => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      fn();
    }, 300);
  };
  const offs = EVENTS.map((kind) => on(kind, later));
  return () => {
    offs.forEach((off) => off());
    clearTimeout(timer);
  };
}

/** A copy button for one piece of text. */
function copyButton(value: string): HTMLButtonElement {
  return btn('Copy', () => void attempt(() => navigator.clipboard.writeText(value), 'Copied'), { kind: 'ghost', sm: true });
}

/** A value to copy: a mono box with a Copy button. */
function copyBox(value: string): HTMLElement {
  return h('div.ws-cluster', null, h('div.ws-code', null, value), copyButton(value));
}

/** Puts a hint under a field (the hint's words change as the owner types). */
function withHint(f: HTMLLabelElement, hint: HTMLElement): HTMLLabelElement {
  f.append(hint);
  return f;
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const [id, part, recording] = params;
  if (id && part === 'recordings' && recording) return playbackScreen(root, id, recording);
  return id ? detailScreen(root, id) : listScreen(root);
}

// ---- The list: the workstations and the join codes -------------------------

function listScreen(root: HTMLElement): Cleanup {
  let alive = true;
  let ready = false;
  // null: the last request failed.
  let stations: StationView[] | null = null;
  let codes: StationCode[] | null = null;
  // The open dialog, closed on leaving the screen.
  let closeDialog: (() => void) | null = null;
  // The last picture of each online workstation, as an object URL, and when the pictures were last asked for.
  const pictures = new Map<string, string>();
  let picturesAt = 0;

  const listBody = h('div');
  const codesBody = h('div');
  const defaults: Section = recordingDefaultsCard();
  const vms = vmsSection();

  root.append(
    head('Workstations', [btn('New VM', () => vms.openNew()), btn('Add workstation', () => openAdd(), { kind: 'primary' })]),
    h('p.ws-lede', null, 'The machines agents work on through this hub. A workstation gives its screen to agents one lease at a time, and you can take it over at any moment.'),
    h(
      'div.ws-stack',
      null,
      card('Workstations', listBody, { flush: true }),
      vms.el,
      card('Join codes', codesBody, { flush: true }),
      defaults.el,
    ),
  );

  /**
   * A row's picture box, always there so the rows line up: the screen as it was last asked for, or the OS and why there
   * is no picture (no session open, offline, revoked).
   */
  const picture = (s: StationView): HTMLElement => {
    const url = s.online && !s.revoked ? pictures.get(s.id) : undefined;
    return h(
      'div',
      { class: url ? 'ws-pic on' : 'ws-pic' },
      url ? h('img', { 'data-thumb': s.id, alt: `Screen of ${s.name}`, src: url, role: 'img' }) : null,
      url ? null : h('span', null, OS_NAME[s.facts.os] ?? '', h('br'), pictureNote(s)),
    );
  };

  /** Each online workstation's picture: asked for again every few seconds while the page is in view. */
  const refreshPictures = async (): Promise<void> => {
    picturesAt = Date.now();
    if (!stations) return;
    await Promise.all(
      stations
        .filter((s) => s.online && !s.revoked)
        .map(async (s) => {
          const shot = await api.stationThumbnail(s.id);
          if (!alive) return;
          const previous = pictures.get(s.id);
          if (shot) pictures.set(s.id, URL.createObjectURL(shot));
          else pictures.delete(s.id);
          if (previous) URL.revokeObjectURL(previous);
        }),
    );
    if (alive) renderList();
  };

  const row = (s: StationView): HTMLElement => {
    const chip = statusChip(s.status);
    const owner = accountBadge(s.facts.account);
    const leases = leaseCount(s.leases);
    return h(
      'div.ws-row',
      null,
      picture(s),
      h(
        'div.ws-info',
        null,
        h('a.ws-name', { href: href('workstations', s.id) }, s.name),
        h('div.ws-sum', null, stationSummary(s.facts)),
        h('div.ws-meta', null, `${leases.holding} active · ${leases.waiting} queued · seen ${ago(s.last_seen_ms)}`),
      ),
      h('div.ws-chips', null, badge(chip.label, chip.tone, true), owner ? badge(owner.text, owner.tone) : null),
    );
  };

  const renderList = (): void => {
    if (!ready) return;
    if (stations === null) return replace(listBody, empty('alert', 'Could not load the workstations.'));
    if (!stations.length) {
      return replace(listBody, empty('machine', 'No workstations yet', 'Add one here, then run the join command on the machine.'));
    }
    replace(listBody, sortStations(stations).map(row));
  };

  const revokeCode = async (c: StationCode): Promise<void> => {
    await attempt(() => api.revokeStationCode(c.id), 'Join code revoked');
    void load();
  };

  const codeColumns = (): PCol<StationCode>[] => [
    { label: 'Label', cell: (c) => (c.label ? h('b', null, c.label) : 'no label') },
    { label: 'ID', cell: (c) => h('span.ws-mono.muted', null, c.id) },
    { label: 'Created', cell: (c) => ago(c.created_ms) },
    {
      label: 'Expires',
      cell: (c) => (c.state === 'active' ? ['in ', h('span', { 'data-cd': c.expires_ms }, countdown(c.expires_ms - Date.now()))] : ago(c.expires_ms)),
    },
    {
      label: 'State',
      cell: (c) => {
        const chip = codeChip(c.state);
        return [badge(chip.label, chip.tone, true), ...(c.used_by ? [h('span.muted', null, ` by ${c.used_by}`)] : [])];
      },
    },
    { label: '', cls: 'num', cell: (c) => (c.state === 'active' ? btn('Revoke', () => void revokeCode(c), { kind: 'danger', sm: true }) : '') },
  ];

  const renderCodes = (): void => {
    if (!ready) return;
    if (codes === null) return replace(codesBody, empty('alert', 'Could not load the join codes.'));
    if (!codes.length) return replace(codesBody, empty('link', 'No join codes yet', 'Codes made with "Add workstation" are listed here.'));
    replace(codesBody, ptable(codeColumns(), [...codes].sort((a, b) => b.created_ms - a.created_ms)));
  };

  /** The add dialog once a code is made: the token shown once, the command to run on the machine. */
  const showIssued = (token: string, expiresMs: number): void => {
    closeDialog?.();
    const close = dialog(
      'Add a workstation',
      [
        muted('Shown once. Single use. Anyone with it can enroll a machine until it expires.'),
        copyBox(token),
        h('p', null, 'Expires in ', h('span', { 'data-cd': expiresMs }, countdown(expiresMs - Date.now()))),
        h('p', null, 'On the machine, run:'),
        copyBox(joinCommand(token)),
        muted('The machine appears in the list once it joins.'),
      ],
      [btn('Close', () => close(), { kind: 'ghost' })],
      () => {
        closeDialog = null;
      },
    );
    closeDialog = close;
  };

  const openAdd = (): void => {
    closeDialog?.();
    const label = input({ placeholder: 'studio-mac', maxlength: MAX_NAME_CHARS, 'aria-label': 'Label', autocomplete: 'off' });
    const minutes = select(
      TTL_OPTIONS.map((m) => ({ value: String(m), label: `${m} minutes` })),
      '30',
    );
    const create = async (): Promise<void> => {
      const made = await attempt(() => api.issueStationCode(label.value.trim(), Number(minutes.value)));
      if (!made) return;
      showIssued(made.token, made.expires_ms);
      void load();
    };
    const close = dialog(
      'Add a workstation',
      [
        field('Label', label, 'What the machine is called in this list until it names itself. Optional.'),
        field('Valid for', minutes, 'How long the code can enroll a machine.'),
      ],
      [btn('Cancel', () => close(), { kind: 'ghost' }), btn('Create join code', () => void create(), { kind: 'primary' })],
      () => {
        closeDialog = null;
      },
    );
    closeDialog = close;
  };

  const load = async (): Promise<void> => {
    const [s, c] = await Promise.all([api.stations().catch((): null => null), api.stationCodes().catch((): null => null)]);
    if (!alive) return;
    stations = s;
    codes = c;
    ready = true;
    renderList();
    renderCodes();
    void defaults.refresh();
    void vms.refresh();
    if (!picturesAt) void refreshPictures();
  };

  void load();
  const poll = setInterval(() => void load(), POLL_MS);
  const pictureTimer = setInterval(() => {
    if (document.visibilityState === 'visible') void refreshPictures();
  }, THUMB_MS);
  // The countdowns of the join codes (the list's and an open dialog's) tick every second.
  const ticker = setInterval(() => {
    document.querySelectorAll<HTMLElement>('[data-cd]').forEach((el) => {
      el.textContent = countdown(Number(el.dataset.cd) - Date.now());
    });
  }, 1000);
  const offChanges = onChanges(() => void load());
  return () => {
    alive = false;
    clearInterval(poll);
    clearInterval(pictureTimer);
    clearInterval(ticker);
    offChanges();
    closeDialog?.();
    defaults.dispose();
    vms.dispose();
    for (const url of pictures.values()) URL.revokeObjectURL(url);
    pictures.clear();
  };
}

// ---- One workstation -------------------------------------------------------

function detailScreen(root: HTMLElement, id: string): Cleanup {
  let alive = true;
  // The last answer that loaded; the page keeps it when a refresh fails.
  let station: StationView | null = null;
  // The agents' names by CID, for the leases and the audit.
  let names = new Map<number, string>();
  // null: the audit request failed.
  let audit: StationAuditRow[] | null = null;
  // The settings as the form holds them when it was last loaded or saved: the form is dirty when it differs.
  let baseline = '';
  let closeDialog: (() => void) | null = null;

  const back = h('a.ws-back', { href: href('workstations') }, 'All workstations');
  const title = h('h1');
  const chipBox = h('div.ws-chips-l');
  const actionsBox = h('div.page-actions');
  const factsBody = h('div');
  const leasesBody = h('div');
  const auditBody = h('div');
  const liveBody = h('div', null, empty('machine', 'Loading…'));
  // The live view starts with the first load: it needs the workstation's sessions.
  let live: LiveHandle | null = null;
  const nameOf = (cid: number): string => names.get(cid) ?? agentLabel(cid);
  const sync: Section = syncCard(id);
  const recordings: Section = recordingsCard(id, nameOf);

  // ---- settings: built once, filled from the station on each refresh unless the owner is editing ----
  const takeover = select(TAKEOVER_OPTIONS.map((o) => ({ value: o.value, label: o.label })));
  const takeoverHint = h('small.hint');
  const record = h('input', { type: 'checkbox' });
  const takeovers = h('input', { type: 'checkbox' });
  const snapshot = h('input', { type: 'checkbox' });
  const retention = input({ type: 'number', min: RETENTION_DAYS.min, max: RETENTION_DAYS.max, step: 1, inputmode: 'numeric', 'aria-label': 'Keep recordings for, in days' });
  const fps = input({ type: 'number', min: RECORD_FPS.min, max: RECORD_FPS.max, step: 1, inputmode: 'numeric', 'aria-label': 'Frames per second' });
  const rate = input({ type: 'number', min: 0, max: RATE_MAX, step: 1, inputmode: 'numeric', 'aria-label': 'Rate, in credits an hour' });
  const rateHint = h('small.hint');
  const folder = input({ maxlength: WORK_FOLDER_MAX, placeholder: '~/AgentPlaza', 'aria-label': 'Work folder', autocomplete: 'off', spellcheck: 'false' });
  const settingsError = h('div.ws-problem');
  const saveButton = btn('Save settings', () => void save(), { kind: 'primary' });
  const settingsFields = h(
    'fieldset.ws-fieldset',
    null,
    withHint(field('Takeover policy', takeover), takeoverHint),
    check(record, 'Record screen sessions'),
    check(takeovers, "Record the owner's takeovers too"),
    field('Keep recordings for (days)', retention, `${RETENTION_DAYS.min} to ${RETENTION_DAYS.max}. Older recordings are deleted.`),
    field('Frames per second', fps, `${RECORD_FPS.min} to ${RECORD_FPS.max}. What a recording keeps.`),
    withHint(field('Rate, credits an hour', rate), rateHint),
    field('Work folder', folder, "Under the agent's OS account. It syncs with the hub both ways."),
    check(snapshot, 'Snapshot and reset (VMs)'),
    h('p.hint', null, 'Restore a clean snapshot after each lease.'),
    h('div.ws-cluster', null, saveButton),
    settingsError,
  );

  root.append(
    back,
    h('header.page-head', null, h('div.page-head-row', null, title, h('span.sp'), actionsBox)),
    chipBox,
    h(
      'div.ws-stack',
      null,
      card('Facts', factsBody),
      card('Live view', liveBody),
      card('Leases', leasesBody, { flush: true }),
      sync.el,
      recordings.el,
      card('Settings', settingsFields),
      card('Audit', auditBody, { flush: true }),
    ),
  );

  /** The rate's hint: what the credits come to in dollars an hour, as the owner types them. */
  const syncRateHint = (): void => {
    const credits = readRate(rate.value);
    rateHint.textContent = credits === null ? 'Whole credits an hour. 0 is free.' : `${rateText(credits)}. One credit is $0.001; 0 is free.`;
  };
  rate.addEventListener('input', syncRateHint);

  const syncHint = (): void => {
    takeoverHint.textContent = takeoverText(takeover.value as Takeover);
  };
  takeover.addEventListener('change', syncHint);

  const readForm = (): SettingsForm => ({
    takeover: takeover.value as Takeover,
    record: record.checked,
    retention_days: retention.value,
    record_fps: fps.value,
    record_takeovers: takeovers.checked,
    work_folder: folder.value,
    snapshot_reset: snapshot.checked,
    rate_per_hour: rate.value,
  });

  const showForm = (f: SettingsForm): void => {
    takeover.value = f.takeover;
    record.checked = f.record;
    retention.value = f.retention_days;
    fps.value = f.record_fps;
    takeovers.checked = f.record_takeovers;
    folder.value = f.work_folder;
    snapshot.checked = f.snapshot_reset;
    rate.value = f.rate_per_hour;
    syncHint();
    syncRateHint();
  };

  const dirty = (): boolean => JSON.stringify(readForm()) !== baseline;

  /** The station's settings into the form, unless the owner has changes in it that a refresh must not take. */
  const syncSettings = (s: StationView): void => {
    if (baseline && dirty()) return;
    const f = settingsForm(s.settings);
    showForm(f);
    baseline = JSON.stringify(f);
  };

  const save = async (): Promise<void> => {
    settingsError.textContent = '';
    const read = readSettings(readForm());
    if (!read.ok) {
      settingsError.textContent = read.error;
      return;
    }
    saveButton.disabled = true;
    try {
      const answer = await api.saveStationSettings(id, read.settings);
      const f = settingsForm(answer.settings);
      showForm(f);
      baseline = JSON.stringify(f);
      if (answer.pushed) toast('Settings saved and sent to the workstation');
      else if (answer.push_error) toast(`Saved. Not sent yet: ${answer.push_error}`, 'error');
      else toast('Saved. The workstation gets them when it connects.');
    } catch (err) {
      settingsError.textContent = err instanceof Error ? err.message : String(err);
    } finally {
      saveButton.disabled = false;
    }
    void load();
  };

  // ---- dialogs: confirm, rename ----

  /** Opens the one dialog of this page; `foot` gets the close function for its buttons. */
  const openDialog = (heading: string, body: Node | Node[], foot: (close: () => void) => Node[]): void => {
    closeDialog?.();
    const close = dialog(heading, body, foot(() => close()), () => {
      closeDialog = null;
    });
    closeDialog = close;
  };

  const openRename = (s: StationView): void => {
    const name = input({ value: s.name, maxlength: MAX_NAME_CHARS, 'aria-label': 'Name', autocomplete: 'off' });
    const problem = h('div.ws-problem');
    openDialog(
      'Rename workstation',
      [field('Name', name, 'Shown on the list and in the audit.'), problem],
      (close) => [
        btn('Cancel', close, { kind: 'ghost' }),
        btn(
          'Save name',
          async () => {
            const wrong = nameProblem(name.value);
            problem.textContent = wrong ?? '';
            if (wrong) return;
            const saved = await attempt(() => api.renameStation(id, name.value.trim()), 'Renamed');
            if (!saved) return;
            close();
            void load();
          },
          { kind: 'primary' },
        ),
      ],
    );
  };

  const drain = async (s: StationView): Promise<void> => {
    await attempt(() => api.drainStation(id, !s.draining), s.draining ? `${s.name} is back in service` : `${s.name} is draining`);
    void load();
  };

  const revoke = async (s: StationView): Promise<void> => {
    await attempt(() => api.revokeStation(id), `${s.name} revoked`);
    void load();
  };

  const release = async (lease: StationLease): Promise<void> => {
    await attempt(() => api.releaseStationLease(id, lease.id), 'Lease released');
    void load();
  };

  const confirmRelease = (lease: StationLease): void => {
    const who = names.get(lease.agent) ?? agentLabel(lease.agent);
    openDialog(
      'Release this lease?',
      h('p', null, `${who} gives the machine back: its session here ends, or its place in the queue is given up.`),
      (close) => [
        btn('Cancel', close, { kind: 'ghost' }),
        btn(
          'Release',
          () => {
            close();
            void release(lease);
          },
          { kind: 'danger' },
        ),
      ],
    );
  };

  const confirmRevoke = (s: StationView): void => {
    openDialog(
      `Revoke ${s.name}?`,
      h('p', null, 'Its link closes at once and its leases end. This cannot be undone.'),
      (close) => [
        btn('Cancel', close, { kind: 'ghost' }),
        btn(
          'Revoke',
          () => {
            close();
            void revoke(s);
          },
          { kind: 'danger' },
        ),
      ],
    );
  };

  // ---- rendering ----

  const actions = (s: StationView): Node[] => {
    if (s.revoked) return [h('span.muted', null, 'Revoked: its link is closed.')];
    return [
      btn('Rename', () => openRename(s), { kind: 'ghost' }),
      btn(s.draining ? 'Resume' : 'Drain', () => void drain(s), {
        kind: 'ghost',
        title: s.draining ? 'Back in service: it takes new work again.' : 'Takes no new work; its link closes once its current answers are in.',
      }),
      btn('Revoke', () => confirmRevoke(s), { kind: 'danger' }),
    ];
  };

  const renderHeader = (s: StationView): void => {
    replace(title, s.name);
    const chip = statusChip(s.status);
    const owner = accountBadge(s.facts.account);
    replace(chipBox, badge(chip.label, chip.tone, true), owner ? badge(owner.text, owner.tone) : null);
    replace(actionsBox, actions(s));
  };

  const renderFacts = (s: StationView): void => {
    const f = s.facts;
    const gpus = f.gpus.length ? f.gpus.map((g) => h('div', null, gpuLine(g))) : 'none';
    replace(
      factsBody,
      kv([
        ['System', `${osText(f)} · ${f.arch}`],
        ['Machine', formLabel(f.form)],
        ['Hostname', f.hostname || '—'],
        ['CPUs', `${f.cpus} cores`],
        ['Memory', `${Math.round(f.ram_mb / 1024)} GB`],
        ['GPUs', gpus],
        ['Displays', displaysText(f.displays)],
        ['Account', f.account.mode === 'owner' ? `Your account (${f.account.name})` : `A separate agent account: ${f.account.name}`],
        ['Apps found', String(f.apps)],
        ['Virtual displays', f.virtual_displays ? 'yes' : 'no'],
        ['UI tree', f.ui_tree ? 'yes' : 'no'],
        ['Version', s.version || '—'],
        ['Address', s.address || '—'],
        ['Joined', ago(s.joined_ms)],
        ['Last seen', ago(s.last_seen_ms)],
        ['Join code', s.code || '—'],
      ]),
    );
  };

  const renderLeases = (s: StationView): void => {
    if (!s.leases.length) {
      return replace(leasesBody, empty('machine', 'No leases', 'An agent holds a lease while it works here. Leases show up as agents start.'));
    }
    const columns: PCol<StationLease>[] = [
      { label: 'Agent', cell: (l) => nameOf(l.agent) },
      { label: 'Task', cell: (l) => l.task || h('span.muted', null, '—') },
      { label: 'Kind', cell: (l) => leaseKindLabel(l.kind) },
      {
        label: 'State',
        cell: (l) => {
          const chip = leaseStateChip(l);
          return badge(chip.label, chip.tone, true);
        },
      },
      { label: 'Started', cell: (l) => ago(l.started_ms) },
      { label: '', cls: 'num', cell: (l) => btn('Release', () => confirmRelease(l), { kind: 'danger', sm: true }) },
    ];
    replace(leasesBody, ptable(columns, s.leases));
  };

  const renderAudit = (): void => {
    if (audit === null) return replace(auditBody, empty('alert', 'Could not load the audit.'));
    if (!audit.length) return replace(auditBody, empty('clock', 'No events yet'));
    const columns: PCol<StationAuditRow>[] = [
      { label: 'When', cell: (r) => h('span', { title: new Date(r.at_ms).toLocaleString() }, ago(r.at_ms)) },
      { label: 'Event', cell: (r) => auditLabel(r.kind) },
      { label: 'Agent', cell: (r) => (r.agent === null ? '—' : nameOf(r.agent)) },
      { label: 'By', cell: (r) => r.actor },
    ];
    replace(auditBody, ptable(columns, audit));
  };

  const renderAll = (s: StationView): void => {
    renderHeader(s);
    renderFacts(s);
    if (live) live.update(s);
    else {
      live = liveView(s, nameOf);
      replace(liveBody, live.el);
    }
    renderLeases(s);
    settingsFields.disabled = s.revoked;
    syncSettings(s);
    renderAudit();
  };

  /** A workstation that is not there, or that could not be loaded before the page had one. */
  const renderUnavailable = (message: string): void => {
    replace(title, 'Workstation');
    replace(chipBox);
    replace(actionsBox);
    replace(factsBody, empty('alert', message));
    replace(leasesBody);
    replace(auditBody);
  };

  const load = async (): Promise<void> => {
    let fetched: StationView | null = null;
    let missing = false;
    try {
      fetched = await api.station(id);
    } catch (err) {
      missing = err instanceof ApiError && err.status === 404;
    }
    const [ag, au] = await Promise.all([api.agents().catch((): Agent[] => []), api.stationAudit(id, 50).catch((): null => null)]);
    if (!alive) return;
    names = new Map((ag ?? []).map((a): [number, string] => [a.cid, agentName(a)]));
    audit = au;
    void sync.refresh();
    void recordings.refresh();
    if (fetched) {
      station = fetched;
      renderAll(fetched);
    } else if (missing) {
      renderUnavailable('No such workstation.');
    } else if (!station) {
      renderUnavailable('Could not load this workstation.');
    } else {
      // A failed refresh keeps what the page shows; only the audit is redrawn.
      renderAudit();
    }
  };

  void load();
  const poll = setInterval(() => void load(), POLL_MS);
  const offChanges = onChanges(() => void load());
  return () => {
    alive = false;
    clearInterval(poll);
    offChanges();
    closeDialog?.();
    live?.dispose();
    sync.dispose();
    recordings.dispose();
  };
}
