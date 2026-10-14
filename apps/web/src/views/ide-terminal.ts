// The IDE's terminal: xterm.js over the hub's terminal socket (crates/apid/src/ide_term.rs). It fits the container, takes
// the console's theme (and follows the [data-theme] switch), re-attaches to a running shell after a reload or a dropped
// socket, ends a shell only on request, and holds up to four terminals per workspace (terminalTabs). The frames, the
// base64, the offsets and the session keys are in ide-terminal-model.ts, which the tests load on their own.
//
// Loaded with the IDE's lazy chunk: only IDE code imports this module, so the console's start payload does not change.

import { FitAddon } from '@xterm/addon-fit';
import { Terminal, type ITheme } from '@xterm/xterm';
import xtermCss from '@xterm/xterm/css/xterm.css';
import terminalCss from '../styles/ide-terminal.css';
import {
  MAX_TERMINALS,
  PING_MS,
  PROTOCOL,
  attachFrame,
  browserSession,
  clearSession,
  killFrame,
  nextOffset,
  openFrame,
  parseServerFrame,
  pingFrame,
  plainText,
  readSession,
  reconnectDelay,
  resizeFrame,
  sessionKey,
  inputFrame,
  splitInput,
  terminalSocketUrl,
  writeSession,
} from './ide-terminal-model';

export { MAX_TERMINALS, bytesToBase64, base64ToBytes, reconnectDelay, sessionKey } from './ide-terminal-model';

const STYLE_ID = 'ide-terminal-style';
/** WebSocket.OPEN, written out so this file reads no WebSocket global at load. */
const WS_OPEN = 1;
/** How long a kill may take before the view gives up waiting and closes its socket. */
const KILL_WAIT_MS = 5_000;

/** The phases a terminal view moves through. */
type Phase = 'connecting' | 'live' | 'reconnecting' | 'killing' | 'failed' | 'ended';

export interface TerminalOptions {
  /** A directory of the workspace (relative to its root) that a new shell starts in. */
  cwd?: string;
  /** The tab slot, "0" to "3": the session is kept under it, so one workspace can hold several. Default "0". */
  slot?: string;
  /** false hides the view's own toolbar (a tab strip then supplies New and Kill). Default true. */
  toolbar?: boolean;
}

export interface TerminalHandle {
  /** Closes the socket and frees the view. The guest shell keeps running; mounting again re-attaches to it. */
  dispose(): void;
  /** Ends the guest shell. Resolves once it has ended, or after a short wait. The caller confirms first. */
  kill(): Promise<void>;
  focus(): void;
  /** Starts a new shell in this view. A running shell is ended first, after a confirm. */
  restart(): Promise<void>;
}

export interface TabsOptions {
  cwd?: string;
}

export interface TabsHandle {
  dispose(): void;
  focus(): void;
  /** Adds a terminal tab. False at the cap of four per workspace. */
  add(): boolean;
}

// ---- the page -----------------------------------------------------------------------------------------------------

/** The colors the terminal takes from the console's tokens (tokens.css), by ANSI name. */
type ColorKey = Exclude<keyof ITheme, 'extendedAnsi'>;
const THEME_TOKENS: Array<[ColorKey, string]> = [
  ['background', 'var(--sheet)'],
  ['foreground', 'var(--ink)'],
  ['cursor', 'var(--signal)'],
  ['cursorAccent', 'var(--sheet)'],
  ['selectionBackground', 'color-mix(in srgb, var(--signal) 28%, transparent)'],
  ['black', 'var(--paper)'],
  ['red', 'var(--crit)'],
  ['green', 'var(--ok-ink)'],
  ['yellow', 'var(--warn-ink)'],
  ['blue', 'var(--info-ink)'],
  ['magenta', 'var(--signal-ink)'],
  ['cyan', 'color-mix(in srgb, var(--info-ink) 55%, var(--ok-ink))'],
  ['white', 'var(--ink-2)'],
  ['brightBlack', 'var(--ink-3)'],
  ['brightRed', 'var(--crit)'],
  ['brightGreen', 'var(--ok-ink)'],
  ['brightYellow', 'var(--warn-ink)'],
  ['brightBlue', 'var(--info-ink)'],
  ['brightMagenta', 'var(--signal-ink)'],
  ['brightCyan', 'color-mix(in srgb, var(--info-ink) 55%, var(--ok-ink))'],
  ['brightWhite', 'var(--ink)'],
];

/** The computed value of a CSS expression, read through a hidden probe so var() and color-mix() resolve. */
function resolveCss(host: HTMLElement, prop: 'color' | 'fontFamily', expr: string): string {
  const probe = el('span');
  probe.style.visibility = 'hidden';
  probe.style[prop] = expr;
  host.append(probe);
  const value = getComputedStyle(probe)[prop].trim();
  probe.remove();
  return value;
}

/** The theme for the current tokens. A color xterm cannot read is left out, so xterm's default stands for it. */
function readTheme(host: HTMLElement): ITheme {
  const theme: ITheme = {};
  for (const [name, expr] of THEME_TOKENS) {
    const color = resolveCss(host, 'color', expr);
    if (color.startsWith('#') || color.startsWith('rgb')) theme[name] = color;
  }
  return theme;
}

function readFont(host: HTMLElement): string {
  return resolveCss(host, 'fontFamily', 'var(--mono)') || 'monospace';
}

function ensureStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = el('style');
  style.id = STYLE_ID;
  style.textContent = `${xtermCss}\n${terminalCss}`;
  document.head.append(style);
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function button(label: string, onClick: () => void, className = 'ide-term-btn'): HTMLButtonElement {
  const node = el('button', className, label);
  node.type = 'button';
  node.addEventListener('click', onClick);
  return node;
}

/** A notice in the terminal's own text: dim, and with no control characters from the hub. */
function dim(text: string): string {
  return `\u001b[2m${plainText(text)}\u001b[0m`;
}

// ---- one terminal -------------------------------------------------------------------------------------------------

/**
 * Mounts one terminal in `container`. It attaches to the shell stored for this workspace and slot when there is one,
 * else opens a new shell. A dropped socket reconnects with a backoff and re-attaches; the hub's `exit` ends the view.
 */
export function mountTerminal(container: HTMLElement, workspace: string, opts: TerminalOptions = {}): TerminalHandle {
  ensureStyle();
  const key = sessionKey(workspace, opts.slot ?? '0');
  const store = browserSession();
  const cwd = opts.cwd;

  const root = el('div', 'ide-term');
  const bar = el('div', 'ide-term-bar');
  const idLabel = el('span', 'ide-term-id');
  const newButton = button('New terminal', () => void restart());
  const killButton = button('Kill', () => confirmKill());
  const status = el('div', 'ide-term-status');
  status.setAttribute('role', 'status');
  const body = el('div', 'ide-term-body');
  if (opts.toolbar !== false) bar.append(idLabel, newButton, killButton);
  root.append(...(opts.toolbar !== false ? [bar] : []), status, body);
  container.replaceChildren(root);

  const term = new Terminal({
    cursorBlink: true,
    fontSize: 13,
    fontFamily: readFont(root),
    scrollback: 5_000,
    theme: readTheme(root),
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(body);

  const stored = readSession(store, key);
  let disposed = false;
  let phase: Phase = 'connecting';
  /** The guest terminal this view is attached to, once it has one. */
  let guest: string | undefined = stored?.term;
  /** The output offset this view holds: one past the last byte it has written. */
  let offset = stored?.offset ?? 0;
  let ws: WebSocket | null = null;
  /** The first frame of a connection: `open` or `attach`, until `opened` (or an error) answers it. */
  let pending: 'open' | 'attach' | null = null;
  let attempt = 0;
  let killRequested = false;
  let lastSize = '';
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let pingTimer: ReturnType<typeof setInterval> | undefined;
  let killTimer: ReturnType<typeof setTimeout> | undefined;
  const killWaiters: Array<() => void> = [];

  const isOpen = (): boolean => ws !== null && ws.readyState === WS_OPEN;
  const ready = (): boolean => isOpen() && pending === null && guest !== undefined && phase === 'live';
  const send = (text: string): void => {
    const socket = ws;
    if (socket && socket.readyState === WS_OPEN) socket.send(text);
  };

  function setStatus(text: string, actions: Array<[string, () => void]> = [], tone: 'warn' | 'danger' = 'warn'): void {
    status.className = `ide-term-status is-${tone}`;
    status.replaceChildren();
    if (text) status.append(el('span', '', text));
    for (const [label, run] of actions) status.append(button(label, run));
  }

  function render(): void {
    idLabel.textContent = guest ?? (phase === 'ended' ? 'ended' : 'not started');
    killButton.disabled = phase === 'ended' || (guest === undefined && ws === null);
  }

  function startPing(): void {
    stopPing();
    pingTimer = setInterval(() => send(pingFrame()), PING_MS);
  }
  function stopPing(): void {
    clearInterval(pingTimer);
    pingTimer = undefined;
  }

  function connect(): void {
    if (disposed || ws) return;
    clearTimeout(retryTimer);
    retryTimer = undefined;
    let socket: WebSocket;
    try {
      socket = new WebSocket(terminalSocketUrl(workspace, location), PROTOCOL);
    } catch {
      failed('The terminal socket cannot be opened from this page.');
      return;
    }
    ws = socket;
    socket.onopen = () => {
      if (ws !== socket) return;
      if (guest !== undefined) {
        pending = 'attach';
        send(attachFrame(guest, offset));
      } else {
        pending = 'open';
        send(openFrame(term.cols, term.rows, cwd));
      }
    };
    socket.onmessage = (event: MessageEvent) => {
      if (ws === socket) onFrame(event.data);
    };
    socket.onclose = () => {
      if (ws !== socket) return;
      ws = null;
      pending = null;
      stopPing();
      onDropped();
    };
  }

  function onFrame(raw: unknown): void {
    if (typeof raw !== 'string') return;
    const frame = parseServerFrame(raw);
    if (!frame) return;
    switch (frame.t) {
      case 'opened':
        pending = null;
        guest = frame.term;
        attempt = 0;
        phase = 'live';
        writeSession(store, key, { term: guest, offset });
        setStatus('');
        render();
        startPing();
        lastSize = '';
        sendResize();
        if (killRequested) sendKill();
        return;
      case 'out':
        term.write(frame.data);
        offset = nextOffset(frame);
        if (guest !== undefined) writeSession(store, key, { term: guest, offset });
        return;
      case 'lost':
        term.writeln(dim(`${frame.bytes} bytes of earlier output were dropped`));
        return;
      case 'exit':
        ended(`[process exited ${frame.code ?? 'without a code'}]`);
        return;
      case 'error':
        // An error that answers `open` or `attach` means the shell did not start or attach: the socket is done.
        if (pending !== null) failed(plainText(frame.message));
        else term.writeln(dim(frame.message));
        return;
      case 'pong':
        return;
    }
  }

  /** The socket closed. The shell keeps running on the guest, so a drop reconnects; a kill or an exit does not. */
  function onDropped(): void {
    if (disposed) return;
    if (phase === 'killing') {
      finishKill('[terminal killed]');
      return;
    }
    if (phase === 'ended' || phase === 'failed') return;
    phase = 'reconnecting';
    setStatus('Reconnecting…');
    render();
    const wait = reconnectDelay(attempt);
    attempt += 1;
    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      connect();
    }, wait);
  }

  /** The shell cannot be started or attached. The socket is closed for good; Retry or New terminal goes on. */
  function failed(message: string): void {
    phase = 'failed';
    pending = null;
    killRequested = false;
    clearTimeout(retryTimer);
    retryTimer = undefined;
    stopPing();
    const socket = ws;
    ws = null;
    if (socket) socket.close(1000, 'the terminal did not attach');
    settleKill();
    setStatus(message, [['Retry', () => retry()], ['New terminal', () => void restart()]], 'danger');
    render();
  }

  function retry(): void {
    if (disposed || ws) return;
    phase = 'connecting';
    setStatus(guest !== undefined ? 'Reattaching…' : 'Connecting…');
    render();
    connect();
  }

  /** The shell ended by itself (the hub's `exit`). Its session is no longer kept. */
  function ended(text: string): void {
    phase = 'ended';
    pending = null;
    killRequested = false;
    guest = undefined;
    offset = 0;
    stopPing();
    clearSession(store, key);
    term.writeln(dim(text));
    settleKill();
    setStatus('', [['New terminal', () => void restart()]]);
    render();
  }

  function sendKill(): void {
    phase = 'killing';
    setStatus('Ending…');
    render();
    send(killFrame());
  }

  /** The shell ended on request. The socket closed, or the wait ran out. */
  function finishKill(note: string): void {
    clearTimeout(killTimer);
    killTimer = undefined;
    clearTimeout(retryTimer);
    retryTimer = undefined;
    const socket = ws;
    ws = null;
    pending = null;
    stopPing();
    if (socket) socket.close(1000, 'the terminal ended');
    phase = 'ended';
    guest = undefined;
    offset = 0;
    killRequested = false;
    clearSession(store, key);
    term.writeln(dim(note));
    settleKill();
    setStatus('', [['New terminal', () => void restart()]]);
    render();
  }

  function settleKill(): void {
    clearTimeout(killTimer);
    killTimer = undefined;
    for (const done of killWaiters.splice(0)) done();
  }

  function kill(): Promise<void> {
    return new Promise<void>((resolve) => {
      if (disposed || phase === 'ended') {
        resolve();
        return;
      }
      killWaiters.push(resolve);
      // A shell that never started, or one whose attach was refused, has nothing this view can still end.
      if (phase === 'failed' || (guest === undefined && ws === null)) {
        finishKill('[terminal closed]');
        return;
      }
      killRequested = true;
      if (ready()) {
        sendKill();
      } else if (ws === null) {
        // Dropped: reconnect now, so the attach can carry the kill.
        clearTimeout(retryTimer);
        retryTimer = undefined;
        connect();
      }
      // Otherwise the socket is still connecting, and its `opened` sends the kill.
      clearTimeout(killTimer);
      killTimer = setTimeout(() => finishKill('[terminal closed]'), KILL_WAIT_MS);
    });
  }

  function confirmKill(): void {
    if (!window.confirm('Kill this terminal? Its shell ends.')) return;
    void kill();
  }

  async function restart(): Promise<void> {
    if (disposed) return;
    if (guest !== undefined && (phase === 'connecting' || phase === 'live' || phase === 'reconnecting' || phase === 'killing')) {
      if (!window.confirm('Start a new terminal? The current one ends.')) return;
      await kill();
      if (disposed) return;
    }
    const socket = ws;
    ws = null;
    pending = null;
    stopPing();
    clearTimeout(retryTimer);
    retryTimer = undefined;
    if (socket) socket.close(1000, 'a new terminal');
    guest = undefined;
    offset = 0;
    attempt = 0;
    lastSize = '';
    clearSession(store, key);
    term.writeln(dim('[new terminal]'));
    phase = 'connecting';
    setStatus('Connecting…');
    render();
    connect();
  }

  // The size: the shell follows the container, and the hub is told each change.
  const fitNow = (): void => {
    try {
      fit.fit();
    } catch {
      // Not laid out yet (or hidden): the next resize fits it.
    }
  };
  function sendResize(): void {
    if (!ready()) return;
    const size = `${term.cols}x${term.rows}`;
    if (size === lastSize) return;
    lastSize = size;
    send(resizeFrame(term.cols, term.rows));
  }
  const resizer = new ResizeObserver(() => fitNow());
  resizer.observe(body);
  term.onResize(() => sendResize());

  // Typing: only while a shell is attached; a dropped socket shows Reconnecting, and keys wait for nothing.
  const encoder = new TextEncoder();
  term.onData((data) => {
    if (!ready()) return;
    for (const chunk of splitInput(encoder.encode(data))) send(inputFrame(chunk));
  });

  // The theme: the console's [data-theme] switch, and the system's scheme when the page follows it.
  const applyTheme = (): void => {
    term.options.theme = readTheme(root);
    term.options.fontFamily = readFont(root);
  };
  const themeWatch = new MutationObserver(applyTheme);
  themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
  const scheme = typeof matchMedia === 'function' ? matchMedia('(prefers-color-scheme: dark)') : null;
  scheme?.addEventListener('change', applyTheme);

  fitNow();
  setStatus(guest !== undefined ? 'Reattaching…' : 'Connecting…');
  render();
  connect();

  return {
    dispose(): void {
      if (disposed) return;
      disposed = true;
      clearTimeout(retryTimer);
      clearTimeout(killTimer);
      stopPing();
      const socket = ws;
      ws = null;
      if (socket) socket.close(1000, 'the page left the terminal');
      themeWatch.disconnect();
      scheme?.removeEventListener('change', applyTheme);
      resizer.disconnect();
      settleKill();
      term.dispose();
      container.replaceChildren();
    },
    kill,
    focus(): void {
      term.focus();
    },
    restart,
  };
}

// ---- several terminals --------------------------------------------------------------------------------------------

interface Tab {
  slot: number;
  pane: HTMLElement;
  button: HTMLButtonElement;
  handle: TerminalHandle;
}

/**
 * A strip of terminals for one workspace, up to MAX_TERMINALS. The tabs that have a stored session come back on mount,
 * so a reload keeps them; with none, one new terminal opens. Kill ends the active tab's shell after a confirm.
 */
export function terminalTabs(container: HTMLElement, workspace: string, opts: TabsOptions = {}): TabsHandle {
  ensureStyle();
  const store = browserSession();
  const strip = el('div', 'ide-term-tabs');
  const panes = el('div', 'ide-term-panes');
  const addButton = button('+ New terminal', () => {
    add();
  });
  const killButton = button('Kill', () => killActive());
  strip.append(el('span', 'ide-term-spacer'), addButton, killButton);
  container.replaceChildren(strip, panes);

  let tabs: Tab[] = [];
  let active = 0;
  let disposed = false;

  function show(index: number): void {
    active = index;
    tabs.forEach((tab, i) => {
      tab.button.setAttribute('aria-selected', String(i === index));
      tab.pane.hidden = i !== index;
    });
  }

  function freeSlot(): number | undefined {
    for (let slot = 0; slot < MAX_TERMINALS; slot++) {
      if (!tabs.some((tab) => tab.slot === slot)) return slot;
    }
    return undefined;
  }

  function insertTab(slot: number, fresh: boolean): boolean {
    if (disposed || tabs.length >= MAX_TERMINALS || tabs.some((tab) => tab.slot === slot)) return false;
    // A new terminal must not pick up a session a slot left behind.
    if (fresh) clearSession(store, sessionKey(workspace, String(slot)));
    const pane = el('div', 'ide-term-pane');
    panes.append(pane);
    const handle = mountTerminal(pane, workspace, { slot: String(slot), toolbar: false, cwd: opts.cwd });
    const tabButton = button(`Terminal ${slot + 1}`, () => show(tabs.indexOf(tab)), 'ide-term-btn ide-term-tab');
    tabButton.setAttribute('role', 'tab');
    const tab: Tab = { slot, pane, button: tabButton, handle };
    const after = tabs.find((other) => other.slot > slot);
    tabs.splice(after ? tabs.indexOf(after) : tabs.length, 0, tab);
    strip.insertBefore(tabButton, after?.button ?? addButton);
    show(tabs.indexOf(tab));
    addButton.disabled = tabs.length >= MAX_TERMINALS;
    return true;
  }

  function removeTab(tab: Tab): void {
    const index = tabs.indexOf(tab);
    if (index === -1) return;
    tab.handle.dispose();
    tab.pane.remove();
    tab.button.remove();
    tabs = tabs.filter((other) => other !== tab);
    addButton.disabled = tabs.length >= MAX_TERMINALS;
    if (tabs.length === 0) {
      const slot = freeSlot();
      if (slot !== undefined) insertTab(slot, true);
    } else {
      show(Math.max(0, Math.min(index, tabs.length - 1)));
    }
  }

  function add(): boolean {
    const slot = freeSlot();
    return slot === undefined ? false : insertTab(slot, true);
  }

  function killActive(): void {
    const tab = tabs[active];
    if (!tab) return;
    if (!window.confirm(`Kill Terminal ${tab.slot + 1}? Its shell ends.`)) return;
    void tab.handle.kill().then(() => {
      if (!disposed) removeTab(tab);
    });
  }

  // The stored tabs come back in slot order; with none, one new terminal opens.
  const restored: number[] = [];
  for (let slot = 0; slot < MAX_TERMINALS; slot++) {
    if (readSession(store, sessionKey(workspace, String(slot)))) restored.push(slot);
  }
  if (restored.length === 0) insertTab(0, false);
  for (const slot of restored) insertTab(slot, false);
  show(0);

  return {
    dispose(): void {
      if (disposed) return;
      disposed = true;
      for (const tab of tabs) tab.handle.dispose();
      tabs = [];
      container.replaceChildren();
    },
    focus(): void {
      tabs[active]?.handle.focus();
    },
    add,
  };
}
