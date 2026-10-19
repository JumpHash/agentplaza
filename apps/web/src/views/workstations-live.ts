// The live view of one workstation, and the takeover. The screen's frames are drawn into a canvas over the screen socket
// (apz.station.screen.v1); a session picker, fps and scale set what the hub streams. "Take over" opens the control socket
// (apz.station.control.v1): while the owner holds the session, the mouse and keyboard on this page are sent to the
// station, the keyboard is held here (a focus trap), and Escape twice gives the screen back. The pure parts (the frame,
// the messages, the input mapping) are in workstations-model.ts.

import { type InputAction, type InputButton, type StationView, type Takeover } from '../api';
import { badge, field, select, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { btn } from './workstations-ui';
import {
  actionFrame,
  buttonAction,
  CONTROL_PROTOCOL,
  controlSocketUrl,
  ESCAPE_TWICE_MS,
  framePoint,
  giveBackFrame,
  inputButton,
  keyAction,
  LIVE_FPS,
  LIVE_SCALE_DEFAULT,
  LIVE_SCALES,
  MOVE_MS,
  moveAction,
  parseControlText,
  parseFrame,
  parseScreenText,
  pingFrame,
  PING_MS,
  type Point,
  type ScreenFrame,
  SCREEN_PROTOCOL,
  screenSocketUrl,
  sessionChoices,
  takeFrame,
  takeoverBanner,
  backoffMs,
  wheelAction,
} from '../views/workstations-model';

/** Where the live view is: off, opening, streaming, between two tries, ended by the hub, or the station is gone. */
type Phase = 'off' | 'connecting' | 'live' | 'reconnecting' | 'ended' | 'offline';

const PHASE_CHIP: Record<Phase, { label: string; tone: 'ok' | 'info' | 'warn' | 'muted' | 'danger' }> = {
  off: { label: 'Off', tone: 'muted' },
  connecting: { label: 'Connecting', tone: 'info' },
  live: { label: 'Live', tone: 'ok' },
  reconnecting: { label: 'Reconnecting', tone: 'warn' },
  ended: { label: 'Ended', tone: 'muted' },
  offline: { label: 'Offline', tone: 'danger' },
};

export interface LiveHandle {
  el: HTMLElement;
  /** The station's latest state: its sessions and whether it is reachable. */
  update(station: StationView): void;
  dispose(): void;
}

/** Sends a text frame when the socket is open; a socket that is not is skipped. */
function send(ws: WebSocket | null, text: string): void {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(text);
}

/** The live view for `initial`'s workstation. `nameOf` names an agent by its CID. */
export function liveView(initial: StationView, nameOf: (cid: number) => string): LiveHandle {
  let station = initial;
  let alive = true;
  let running = false;
  let phase: Phase = 'off';
  let reason = '';
  let view: { session: string; fps: number; scale: number } = { session: 'shared', fps: LIVE_FPS.default, scale: LIVE_SCALE_DEFAULT };
  let screen: WebSocket | null = null;
  let control: WebSocket | null = null;
  let attempt = 0;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let pingTimer: ReturnType<typeof setInterval> | undefined;
  // The session the stream shows, as the hub named it on `started`; the takeover names it.
  let sessionNo: number | null = null;
  // The last frame drawn, and the stream's last sequence number (a new stream counts from 1 again).
  let frameSize: { w: number; h: number } | null = null;
  let lastSeq = 0;
  let queued: ScreenFrame | null = null;
  let decoding = false;
  // The takeover: the policy in force while the owner holds the session, null otherwise.
  let taken: Takeover | null = null;
  let pendingMove: Point | null = null;
  let moveTimer: ReturnType<typeof setTimeout> | undefined;
  const held = new Set<InputButton>();
  let escapeAt = 0;

  // ---- the page -------------------------------------------------------------------------------------------------

  const sessionSelect = select([], view.session);
  const fpsSelect = select(
    Array.from({ length: LIVE_FPS.max - LIVE_FPS.min + 1 }, (_, i) => ({ value: String(LIVE_FPS.min + i), label: `${LIVE_FPS.min + i} fps` })),
    String(view.fps),
  );
  const scaleSelect = select(
    LIVE_SCALES.map((s) => ({ value: String(s), label: `${Math.round(s * 100)}%` })),
    String(view.scale),
  );
  const chip = h('span');
  const startButton = btn('Start live view', () => (running ? stop('off') : start()), { kind: 'primary' });
  const takeButton = btn('Take over', () => takeOver(), { kind: 'ghost' });
  const banner = h('div.ws-banner', { hidden: true });
  // The owner's log while they hold the screen: what was sent to the station, newest first.
  const sentList = h('ol.ws-sent', { id: 'ws-sent' });
  const sentLog: string[] = [];
  const canvas = h('canvas', { 'aria-label': 'The screen of this workstation', role: 'img' });
  const pictureBox = h('div.ws-stage-pic', { hidden: true }, canvas);
  const placeholder = h('span');
  const hint = h('p.hint');
  const context = canvas.getContext('2d');

  const controls = h(
    'div.ws-live-bar',
    null,
    field('Session', sessionSelect),
    field('Frames a second', fpsSelect),
    field('Size', scaleSelect),
    h('div.ws-cluster', null, chip, startButton, takeButton),
  );
  const stage = h('div.ws-stage', null, pictureBox, placeholder);
  const el = h('div', null, controls, banner, stage, hint);

  // ---- rendering ------------------------------------------------------------------------------------------------

  function fillSessions(): void {
    const choices = sessionChoices(station.leases, nameOf);
    sessionSelect.replaceChildren(...choices.map((c) => h('option', { value: c.value }, c.label)));
    if (choices.some((c) => c.value === view.session)) sessionSelect.value = view.session;
    else sessionSelect.value = 'shared';
  }

  /** The text for the stage while no picture shows. */
  function placeholderText(): string {
    if (!station.online || station.revoked) return 'The workstation is offline.';
    if (phase === 'ended') return reason ? `The session ended: ${reason}` : 'The session ended.';
    if (phase === 'reconnecting') return 'Reconnecting…';
    if (phase === 'connecting') return 'Connecting…';
    if (phase === 'live') return 'Waiting for the first picture…';
    return 'The live view is off. Start it to watch the screen.';
  }

  function render(): void {
    const chipInfo = PHASE_CHIP[phase];
    replace(chip, badge(chipInfo.label, chipInfo.tone, true));
    const canStart = station.online && !station.revoked;
    startButton.textContent = running ? 'Stop' : 'Start live view';
    startButton.className = running ? 'btn ghost' : 'btn primary';
    startButton.disabled = !running && !canStart;
    takeButton.hidden = taken !== null;
    takeButton.disabled = !canStart || phase !== 'live' || sessionNo === null;
    sessionSelect.disabled = taken !== null;
    fpsSelect.disabled = taken !== null;
    scaleSelect.disabled = taken !== null;
    stage.classList.toggle('is-taken', taken !== null);
    if (taken) {
      replace(
        banner,
        h('b', null, 'You have control'),
        h('div', null, takeoverBanner(taken)),
        h('div.muted', null, 'Press Escape twice to give the screen back.'),
        h('div.muted', null, 'Sent to the station'),
        sentList,
        btn('Release', () => giveBack(), { kind: 'primary' }),
      );
      banner.hidden = false;
    } else {
      banner.hidden = true;
      replace(banner);
    }
    const showPicture = frameSize !== null && (phase === 'live' || (phase === 'reconnecting' && running));
    pictureBox.hidden = !showPicture;
    placeholder.hidden = showPicture;
    placeholder.textContent = placeholderText();
    const tip = phase === 'live' && !taken && sessionNo !== null ? 'Take over to type and click into the station.' : '';
    hint.textContent = tip;
    hint.hidden = !tip;
  }

  // ---- the picture ----------------------------------------------------------------------------------------------

  function onFrame(f: ScreenFrame): void {
    if (f.seq <= lastSeq) return;
    lastSeq = f.seq;
    queued = f;
    void drain();
  }

  /** Decodes and draws the newest frame. Frames that arrive while one is decoding wait, and only the newest of them is drawn. */
  async function drain(): Promise<void> {
    if (decoding || !queued) return;
    decoding = true;
    const f = queued;
    queued = null;
    try {
      const picture = await createImageBitmap(new Blob([f.image], { type: `image/${f.format}` }));
      if (!alive || !context) {
        picture.close();
        return;
      }
      if (canvas.width !== f.w || canvas.height !== f.h) {
        canvas.width = f.w;
        canvas.height = f.h;
      }
      context.drawImage(picture, 0, 0, f.w, f.h);
      picture.close();
      frameSize = { w: f.w, h: f.h };
      // The page changes only with the first picture; the frames after it are drawn without a redraw of the page.
      if (pictureBox.hidden) render();
    } catch {
      // A frame the browser cannot decode: the next one is drawn.
    } finally {
      decoding = false;
      if (queued) void drain();
    }
  }

  // ---- the live stream ------------------------------------------------------------------------------------------

  function startPing(): void {
    clearInterval(pingTimer);
    pingTimer = setInterval(() => {
      send(screen, pingFrame());
      send(control, pingFrame());
    }, PING_MS);
  }

  function stopPing(): void {
    clearInterval(pingTimer);
    pingTimer = undefined;
  }

  function closeScreen(): void {
    const ws = screen;
    screen = null;
    if (ws) ws.close(1000, 'the owner left the live view');
  }

  function connect(): void {
    clearTimeout(retryTimer);
    retryTimer = undefined;
    if (!alive || !running) return;
    const ws = new WebSocket(screenSocketUrl(station.id, view, location), SCREEN_PROTOCOL);
    ws.binaryType = 'arraybuffer';
    screen = ws;
    lastSeq = 0;
    ws.onmessage = (ev: MessageEvent) => {
      if (screen !== ws) return;
      if (typeof ev.data === 'string') {
        const msg = parseScreenText(ev.data);
        if (!msg) return;
        if (msg.t === 'started') {
          sessionNo = msg.session;
          attempt = 0;
          phase = 'live';
          render();
        } else if (msg.t === 'ended') {
          reason = msg.reason;
          phase = 'ended';
          running = false;
          closeControl();
          closeScreen();
          stopPing();
          render();
        } else if (msg.t === 'error') {
          hint.textContent = msg.error;
        }
      } else if (ev.data instanceof ArrayBuffer) {
        const f = parseFrame(ev.data);
        if (f) onFrame(f);
      }
    };
    ws.onclose = () => {
      if (screen !== ws) return;
      screen = null;
      sessionNo = null;
      if (!running || phase === 'ended') {
        render();
        return;
      }
      // The stream dropped: try again, a little later each time.
      phase = 'reconnecting';
      render();
      retryTimer = setTimeout(connect, backoffMs(attempt));
      attempt++;
    };
    if (phase !== 'live' && phase !== 'reconnecting') phase = 'connecting';
    render();
  }

  function start(): void {
    if (!station.online || station.revoked) return;
    running = true;
    attempt = 0;
    reason = '';
    frameSize = null;
    sessionNo = null;
    phase = 'connecting';
    startPing();
    connect();
    render();
  }

  function stop(next: Phase): void {
    running = false;
    clearTimeout(retryTimer);
    retryTimer = undefined;
    releaseHeld();
    closeControl();
    closeScreen();
    stopPing();
    sessionNo = null;
    phase = next;
    render();
  }

  /** Restarts the stream with the picked session, fps or size. */
  function restart(): void {
    if (!running) return;
    clearTimeout(retryTimer);
    retryTimer = undefined;
    closeScreen();
    attempt = 0;
    sessionNo = null;
    frameSize = null;
    phase = 'connecting';
    connect();
    render();
  }

  // ---- the takeover -----------------------------------------------------------------------------------------------

  function takeOver(): void {
    // One takeover socket at a time: a second one would hold the session on the hub after this one is replaced.
    if (taken || control || sessionNo === null || !running) return;
    const session = sessionNo;
    const ws = new WebSocket(controlSocketUrl(station.id, session, location), CONTROL_PROTOCOL);
    control = ws;
    ws.onopen = () => {
      if (control === ws) send(ws, takeFrame());
    };
    ws.onmessage = (ev: MessageEvent) => {
      if (control !== ws || typeof ev.data !== 'string') return;
      const msg = parseControlText(ev.data);
      if (!msg) return;
      if (msg.t === 'taken') {
        taken = msg.policy;
        sentLog.length = 0;
        logSent('Took over the screen');
        render();
      } else if (msg.t === 'given_back') {
        closeControl();
      } else if (msg.t === 'error') {
        toast(msg.error, 'error');
        if (!taken) closeControl();
      } else if (msg.t === 'ended') {
        closeControl();
        toast('The session ended, so the screen is back with the agents.');
      }
    };
    ws.onclose = () => {
      if (control !== ws) return;
      const was = taken;
      control = null;
      releaseLocal();
      render();
      if (was) toast('The takeover ended: the screen is back with the agents.', 'error');
    };
    render();
  }

  /** Gives the session back: the hub answers `given_back`; closing the socket gives it back too. */
  function giveBack(): void {
    if (taken && control) send(control, giveBackFrame());
    else closeControl();
  }

  /** Ends the takeover socket. The buttons the owner holds are let go first, while the socket still carries them. */
  function closeControl(): void {
    releaseHeld();
    const ws = control;
    control = null;
    releaseLocal();
    if (ws) ws.close(1000, 'the owner gave the screen back');
    render();
  }

  /** Clears the takeover's local state: the keyboard is released and the pointer's pending move dropped. */
  function releaseLocal(): void {
    taken = null;
    cancelMove();
    held.clear();
  }

  /** Sends an owner action when the owner holds the session, and logs it in the banner. */
  function act(action: InputAction): void {
    if (!taken || !control) return;
    send(control, actionFrame(action));
    logSent(describe(action));
  }

  function logSent(text: string): void {
    sentLog.unshift(text);
    sentLog.length = Math.min(sentLog.length, 12);
    sentList.replaceChildren(...sentLog.map((t) => h('li', null, t)));
  }

  /** An owner action in words, as the banner's log shows it. */
  function describe(action: InputAction): string {
    const a = action as { action: string; x?: number; y?: number; button?: string };
    if (a.action === 'move') return `Pointer moved to ${a.x}, ${a.y}`;
    if (a.action === 'click') return `${a.button} button click at ${a.x}, ${a.y}`;
    if (a.action === 'down' || a.action === 'up') return `${a.button} button ${a.action === 'down' ? 'down' : 'up'}`;
    return `${a.action} sent`;
  }

  /** The pointer's frame point for a mouse event, or null before a picture has a size. */
  function pointOf(e: MouseEvent): Point | null {
    if (!frameSize) return null;
    return framePoint(e.clientX, e.clientY, canvas.getBoundingClientRect(), frameSize);
  }

  function queueMove(p: Point): void {
    pendingMove = p;
    if (moveTimer === undefined) moveTimer = setTimeout(flushMove, MOVE_MS);
  }

  function flushMove(): void {
    clearTimeout(moveTimer);
    moveTimer = undefined;
    if (!pendingMove) return;
    const p = pendingMove;
    pendingMove = null;
    act(moveAction(p));
  }

  function cancelMove(): void {
    clearTimeout(moveTimer);
    moveTimer = undefined;
    pendingMove = null;
  }

  /** Lifts any button the owner still holds (the pointer left the page, or the takeover ended). */
  function releaseHeld(): void {
    for (const b of held) act(buttonAction(b, false));
    held.clear();
  }

  canvas.addEventListener('mousemove', (e) => {
    if (!taken) return;
    const p = pointOf(e);
    if (p) queueMove(p);
  });
  canvas.addEventListener('mousedown', (e) => {
    if (!taken) return;
    e.preventDefault();
    const b = inputButton(e.button);
    const p = pointOf(e);
    if (!b || !p) return;
    cancelMove();
    act(moveAction(p));
    held.add(b);
    act(buttonAction(b, true));
  });
  canvas.addEventListener(
    'wheel',
    (e) => {
      if (!taken) return;
      e.preventDefault();
      const p = pointOf(e);
      if (!p) return;
      flushMove();
      act(wheelAction(p, e.deltaX, e.deltaY, e.deltaMode));
    },
    { passive: false },
  );
  canvas.addEventListener('contextmenu', (e) => {
    if (taken) e.preventDefault();
  });
  // A button let go anywhere on the page ends its press, so a drag that leaves the picture does not stick.
  const onMouseUp = (e: MouseEvent): void => {
    if (!taken) return;
    const b = inputButton(e.button);
    if (!b || !held.has(b)) return;
    held.delete(b);
    flushMove();
    act(buttonAction(b, false));
  };
  // While the owner holds the session the keyboard is theirs: every key goes to the station, and Tab stays here.
  const onKey = (e: KeyboardEvent): void => {
    if (!taken) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.type !== 'keydown') return;
    if (e.key === 'Escape') {
      const now = Date.now();
      if (now - escapeAt <= ESCAPE_TWICE_MS) {
        escapeAt = 0;
        giveBack();
        return;
      }
      escapeAt = now;
    }
    const action = keyAction(e);
    if (action) act(action);
  };
  const onBlur = (): void => releaseHeld();
  window.addEventListener('mouseup', onMouseUp);
  document.addEventListener('keydown', onKey, true);
  document.addEventListener('keyup', onKey, true);
  window.addEventListener('blur', onBlur);

  sessionSelect.addEventListener('change', () => {
    view = { ...view, session: sessionSelect.value };
    restart();
  });
  fpsSelect.addEventListener('change', () => {
    view = { ...view, fps: Number(fpsSelect.value) };
    restart();
  });
  scaleSelect.addEventListener('change', () => {
    view = { ...view, scale: Number(scaleSelect.value) };
    restart();
  });

  fillSessions();
  render();

  return {
    el,
    update(next: StationView): void {
      station = next;
      fillSessions();
      if (running && (!next.online || next.revoked)) {
        stop('offline');
        return;
      }
      render();
    },
    dispose(): void {
      alive = false;
      running = false;
      clearTimeout(retryTimer);
      stopPing();
      releaseHeld();
      closeControl();
      closeScreen();
      cancelMove();
      window.removeEventListener('mouseup', onMouseUp);
      document.removeEventListener('keydown', onKey, true);
      document.removeEventListener('keyup', onKey, true);
      window.removeEventListener('blur', onBlur);
    },
  };
}
