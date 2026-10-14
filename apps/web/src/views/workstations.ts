// The Workstations screen: the machines agents operate through the hub. The list enrolls new ones
// with a join code (its token shown once, with the command to run on the machine) and shows each one's
// state. `#/workstations/<id>` is one workstation: its facts, leases, settings (the takeover policy and
// recording), its actions and its audit. The live view and the recordings come in a later pass.
// Refreshes on the station and lease frames, and every few seconds.

import '../styles/workstations.css';
import { api, ApiError, type Agent, type StationAuditRow, type StationCode, type StationLease, type StationView, type Takeover } from '../api';
import { href } from '../router';
import { on } from '../store';
import { ago, attempt, badge, button, card, empty, field, input, modal, select, table, toast, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import {
  accountBadge,
  accountText,
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
  ramText,
  RECORD_FPS,
  readSettings,
  RETENTION_DAYS,
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
import type { Cleanup } from './index';

const POLL_MS = 10_000;
/** The frames that change a workstation or its leases. */
const EVENTS = ['station', 'lease_waiting', 'lease_granted', 'lease_released', 'lease_ended', 'lease_failed', 'lease_owner_input'];

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

/** A value to copy: a mono box with a Copy button. */
function copyBox(value: string): HTMLElement {
  return h(
    'div.ws-copy',
    null,
    h('code', null, value),
    button('Copy', () => void attempt(() => navigator.clipboard.writeText(value), 'Copied'), { kind: 'ghost', size: 'sm' }),
  );
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0];
  return id ? detailScreen(root, id) : listScreen(root);
}

// ---- The list: the workstations and the join codes -------------------------

function listScreen(root: HTMLElement): Cleanup {
  let alive = true;
  let ready = false;
  // null: the last request failed.
  let stations: StationView[] | null = null;
  let codes: StationCode[] | null = null;
  // The open dialog and the countdown it runs, closed on leaving the screen.
  let closeDialog: (() => void) | null = null;
  let stopTick: (() => void) | null = null;

  const listBody = h('div');
  const codesBody = h('div');
  root.append(
    h('div.view-head.ws-head', null, h('h1', null, 'Workstations'), button('Add workstation', () => openAdd(), { kind: 'primary' })),
    h('p.ws-lede', null, 'Machines that agents operate through the hub: their screen, shell, files and every program installed there.'),
    card('Workstations', listBody, { flush: true }),
    card('Join codes', codesBody, { flush: true }),
  );

  const row = (s: StationView): HTMLElement => {
    const chip = statusChip(s.status);
    const owner = accountBadge(s.facts.account);
    const leases = leaseCount(s.leases);
    return h(
      'li.ws-row',
      null,
      h(
        'a.ws-row-link',
        { href: href('workstations', s.id) },
        h(
          'span.ws-row-main',
          null,
          h('strong', null, s.name),
          h('span.faint', null, stationSummary(s.facts)),
          h('span.ws-row-meta', null, `${leases.holding} active · ${leases.waiting} queued · seen ${ago(s.last_seen_ms)}`),
        ),
        h('span.ws-row-side', null, badge(chip.label, chip.tone, true), owner ? badge(owner.text, owner.tone) : null),
      ),
    );
  };

  const renderList = (): void => {
    if (!ready) return;
    if (stations === null) return replace(listBody, empty('alert', 'Could not load the workstations.'));
    if (!stations.length) {
      return replace(listBody, empty('machine', 'No workstations yet', 'Add one here, then run the join command on the machine.'));
    }
    replace(listBody, h('ul.ws-list', null, sortStations(stations).map(row)));
  };

  const revokeCode = async (c: StationCode): Promise<void> => {
    await attempt(() => api.revokeStationCode(c.id), 'Join code revoked');
    void load();
  };

  const renderCodes = (): void => {
    if (!ready) return;
    const columns: Column<StationCode>[] = [
      { label: 'Label', cell: (c) => h('strong', null, c.label || 'no label') },
      { label: 'ID', cell: (c) => h('span.faint', null, c.id) },
      { label: 'Created', cell: (c) => ago(c.created_ms) },
      { label: 'Expires', cell: (c) => (c.state === 'active' ? `in ${countdown(c.expires_ms - Date.now())}` : ago(c.expires_ms)) },
      {
        label: 'State',
        cell: (c) => {
          const chip = codeChip(c.state);
          return h('span', null, badge(chip.label, chip.tone, true), c.used_by ? h('span.faint', null, ` by ${c.used_by}`) : null);
        },
      },
      { label: '', cell: (c) => (c.state === 'active' ? button('Revoke', () => void revokeCode(c), { kind: 'danger', size: 'sm' }) : null) },
    ];
    replace(
      codesBody,
      codes === null
        ? empty('alert', 'Could not load the join codes.')
        : codes.length
          ? h('div.ws-scroll', null, table(columns, [...codes].sort((a, b) => b.created_ms - a.created_ms)))
          : empty('link', 'No join codes yet', 'Codes made with "Add workstation" are listed here.'),
    );
  };

  /** Opens the add dialog's countdown on the issued code: mm:ss until it expires. */
  const issued = (token: string, expiresMs: number): HTMLElement => {
    const left = h('span');
    const paint = (): void => {
      left.textContent = countdown(expiresMs - Date.now());
    };
    paint();
    stopTick?.();
    const timer = setInterval(paint, 1000);
    stopTick = () => clearInterval(timer);
    return h(
      'div.stack',
      null,
      h('p.ws-warn', null, 'Shown once. Single use. Anyone with it can enroll a machine until it expires.'),
      copyBox(token),
      h('p.faint', null, 'Expires in ', left),
      h('p', null, 'On the machine, run:'),
      copyBox(joinCommand(token)),
      h('p.faint', null, 'The machine appears in the list once it joins.'),
    );
  };

  const openAdd = (): void => {
    const label = input({ placeholder: 'studio-mac', maxlength: MAX_NAME_CHARS, 'aria-label': 'Label', autocomplete: 'off' });
    const minutes = select(
      TTL_OPTIONS.map((m) => ({ value: String(m), label: `${m} minutes` })),
      '30',
    );
    const pane = h('div.stack');
    const create = async (): Promise<void> => {
      const made = await attempt(() => api.issueStationCode(label.value.trim(), Number(minutes.value)));
      if (!made) return;
      replace(pane, issued(made.token, made.expires_ms));
      void load();
    };
    replace(
      pane,
      field('Label', label, 'What the machine is called in this list until it names itself. Optional.'),
      field('Valid for', minutes, 'How long the code can enroll a machine.'),
      h('div.ws-buttons', null, button('Create join code', () => void create(), { kind: 'primary' })),
    );
    closeDialog?.();
    closeDialog = modal('Add a workstation', pane, () => {
      stopTick?.();
      stopTick = null;
      closeDialog = null;
    });
  };

  const load = async (): Promise<void> => {
    const [s, c] = await Promise.all([api.stations().catch((): null => null), api.stationCodes().catch((): null => null)]);
    if (!alive) return;
    stations = s;
    codes = c;
    ready = true;
    renderList();
    renderCodes();
  };

  void load();
  const poll = setInterval(() => void load(), POLL_MS);
  const offChanges = onChanges(() => void load());
  return () => {
    alive = false;
    clearInterval(poll);
    offChanges();
    stopTick?.();
    closeDialog?.();
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
  const chipBox = h('span.ws-chips');
  const actionsBox = h('div.ws-actions');
  const factsBody = h('div');
  const leasesBody = h('div');
  const auditBody = h('div');

  // ---- settings: built once, filled from the station on each refresh unless the owner is editing ----
  const takeover = select(TAKEOVER_OPTIONS.map((o) => ({ value: o.value, label: o.label })));
  const takeoverHint = h('span.hint');
  const record = h('input', { type: 'checkbox' });
  const takeovers = h('input', { type: 'checkbox' });
  const snapshot = h('input', { type: 'checkbox' });
  const retention = input({ type: 'number', min: RETENTION_DAYS.min, max: RETENTION_DAYS.max, step: 1, inputmode: 'numeric', 'aria-label': 'Keep recordings for, in days' });
  const fps = input({ type: 'number', min: RECORD_FPS.min, max: RECORD_FPS.max, step: 1, inputmode: 'numeric', 'aria-label': 'Frames per second' });
  const folder = input({ maxlength: WORK_FOLDER_MAX, placeholder: '~/AgentPlaza', 'aria-label': 'Work folder', autocomplete: 'off', spellcheck: 'false' });
  const settingsError = h('p.error-text');
  const saveButton = button('Save settings', () => void save(), { kind: 'primary' });
  const settingsFields = h(
    'fieldset.ws-settings',
    null,
    h(
      'label.field',
      null,
      h('span', null, 'Takeover policy'),
      takeover,
      takeoverHint,
    ),
    h(
      'div.ws-form',
      null,
      h('label.ws-check', null, record, h('span', null, 'Record screen sessions')),
      h('label.ws-check', null, takeovers, h('span', null, "Record the owner's takeovers too")),
      field('Keep recordings for (days)', retention, `${RETENTION_DAYS.min} to ${RETENTION_DAYS.max}. Older recordings are deleted.`),
      field('Frames per second', fps, `${RECORD_FPS.min} to ${RECORD_FPS.max}. What a recording keeps.`),
    ),
    field('Work folder', folder, "Under the agent's OS account. It syncs with the hub both ways."),
    h('label.ws-check', null, snapshot, h('span', null, 'Snapshot and reset (VMs)'), h('span.hint', null, 'Restore a clean snapshot after each lease.')),
    h('div.ws-buttons', null, saveButton),
    settingsError,
  );
  const settingsBody = h('div', null, settingsFields);

  root.append(
    h('div.view-head.ws-head', null, h('div.ws-heading', null, back, h('div.ws-title', null, title, chipBox)), actionsBox),
    card('Facts', factsBody),
    card('Leases', leasesBody, { flush: true }),
    card('Settings', settingsBody),
    card('Audit', auditBody, { flush: true }),
  );

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
  });

  const showForm = (f: SettingsForm): void => {
    takeover.value = f.takeover;
    record.checked = f.record;
    retention.value = f.retention_days;
    fps.value = f.record_fps;
    takeovers.checked = f.record_takeovers;
    folder.value = f.work_folder;
    snapshot.checked = f.snapshot_reset;
    syncHint();
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

  const confirmDialog = (heading: string, text: string, action: string, run: () => Promise<void>): void => {
    closeDialog?.();
    const pane = h(
      'div.stack',
      null,
      h('p', null, text),
      h('div.ws-buttons', null, button('Cancel', () => close()), button(action, () => { close(); void run(); }, { kind: 'danger' })),
    );
    const close = modal(heading, pane, () => {
      closeDialog = null;
    });
    closeDialog = close;
  };

  const openRename = (s: StationView): void => {
    const name = input({ value: s.name, maxlength: MAX_NAME_CHARS, 'aria-label': 'Name', autocomplete: 'off' });
    const problem = h('p.error-text');
    const rename = async (): Promise<void> => {
      const wrong = nameProblem(name.value);
      problem.textContent = wrong ?? '';
      if (wrong) return;
      const saved = await attempt(() => api.renameStation(id, name.value.trim()), 'Renamed');
      if (!saved) return;
      close();
      void load();
    };
    const pane = h(
      'div.stack',
      null,
      field('Name', name, 'Shown on the list and in the audit.'),
      problem,
      h('div.ws-buttons', null, button('Save name', () => void rename(), { kind: 'primary' }), button('Cancel', () => close())),
    );
    closeDialog?.();
    const close = modal('Rename workstation', pane, () => {
      closeDialog = null;
    });
    closeDialog = close;
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
    const who = names.get(lease.agent) ?? `agent ${lease.agent}`;
    confirmDialog(
      'Release this lease?',
      `${who} gives the machine back: its session here ends, or its place in the queue is given up.`,
      'Release',
      () => release(lease),
    );
  };

  const confirmRevoke = (s: StationView): void => {
    confirmDialog(
      `Revoke ${s.name}?`,
      'Its link closes at once and its leases end. This cannot be undone.',
      'Revoke',
      () => revoke(s),
    );
  };

  // ---- rendering ----

  const actions = (s: StationView): HTMLElement[] => {
    if (s.revoked) return [h('span.faint', null, 'Revoked: its link is closed.')];
    return [
      button('Rename', () => openRename(s), { kind: 'ghost' }),
      button(s.draining ? 'Resume' : 'Drain', () => void drain(s), {
        title: s.draining ? 'Back in service: it takes new work again.' : 'Takes no new work; its link closes once its current answers are in.',
      }),
      button('Revoke', () => confirmRevoke(s), { kind: 'danger' }),
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
    const pairs: [string, Node | string | Node[]][] = [
      ['System', `${osText(f)} · ${f.arch}`],
      ['Machine', formLabel(f.form)],
      ['Hostname', f.hostname || '—'],
      ['CPUs', `${f.cpus} cores`],
      ['Memory', ramText(f.ram_mb)],
      ['GPUs', gpus],
      ['Displays', displaysText(f.displays)],
      ['Account', accountText(f.account)],
      ['Apps found', String(f.apps)],
      ['Virtual displays', f.virtual_displays ? 'yes' : 'no'],
      ['UI tree', f.ui_tree ? 'yes' : 'no'],
      ['Version', s.version || '—'],
      ['Address', s.address || '—'],
      ['Joined', ago(s.joined_ms)],
      ['Last seen', ago(s.last_seen_ms)],
      ['Join code', s.code || '—'],
    ];
    replace(factsBody, h('dl.node-facts', null, pairs.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])));
  };

  const renderLeases = (s: StationView): void => {
    if (!s.leases.length) {
      return replace(leasesBody, empty('machine', 'No leases', 'An agent holds a lease while it works here. Leases show up as agents start.'));
    }
    const columns: Column<StationLease>[] = [
      { label: 'Agent', cell: (l) => names.get(l.agent) ?? `agent ${l.agent}` },
      { label: 'Task', cell: (l) => l.task || h('span.faint', null, '—') },
      { label: 'Kind', cell: (l) => leaseKindLabel(l.kind) },
      {
        label: 'State',
        cell: (l) => {
          const chip = leaseStateChip(l);
          return badge(chip.label, chip.tone, true);
        },
      },
      { label: 'Started', cell: (l) => ago(l.started_ms) },
      { label: '', cell: (l) => button('Release', () => confirmRelease(l), { kind: 'danger', size: 'sm' }) },
    ];
    replace(leasesBody, h('div.ws-scroll', null, table(columns, s.leases)));
  };

  const renderAudit = (): void => {
    if (audit === null) return replace(auditBody, empty('alert', 'Could not load the audit.'));
    if (!audit.length) return replace(auditBody, empty('clock', 'No events yet'));
    const columns: Column<StationAuditRow>[] = [
      { label: 'When', cell: (r) => h('span', { title: new Date(r.at_ms).toLocaleString() }, ago(r.at_ms)) },
      { label: 'Event', cell: (r) => auditLabel(r.kind) },
      { label: 'Agent', cell: (r) => (r.agent === null ? '—' : (names.get(r.agent) ?? `agent ${r.agent}`)) },
      { label: 'By', cell: (r) => r.actor },
    ];
    replace(auditBody, h('div.ws-scroll', null, table(columns, audit)));
  };

  const renderAll = (s: StationView): void => {
    renderHeader(s);
    renderFacts(s);
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
    names = new Map((ag ?? []).map((a): [number, string] => [a.cid, a.name]));
    audit = au;
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
  };
}
