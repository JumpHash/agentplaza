// The console's theme runtime. The owner's appearance comes from the hub (GET /v1/appearance) and can be overridden on
// this device. The choice is applied as attributes on the root element, and the stylesheets do the rest. A saved
// theme's resolved CSS comes from the theme engine (theme/wasm) and goes into one style element. The result is cached
// in localStorage so boot-theme.js can paint the right look before the console starts.

import { h, icon, replace } from './dom';
import { on } from '../store';
import {
  getAppearance,
  putAppearance,
  type Appearance,
  type AppearanceDoc,
  type AppearanceMode,
  type BackgroundRef,
  type ThemeFile,
} from '../api-appearance';
import generated from '../../../shared/themes.gen.json';

// ---- The built-in themes (generated from apps/shared) ----------------------------------------------------------

export interface BuiltinToken { name: string; group: string; about: string }
export interface BuiltinSkin { id: string; name: string; about: string }
export interface BuiltinBackground { id: string; name: string }
export interface BuiltinTheme {
  id: string;
  name: string;
  about: string;
  skin: string;
  background: string;
  light: Record<string, string>;
  dark: Record<string, string>;
}
export interface BuiltinSet {
  default: string;
  tokens: BuiltinToken[];
  skins: BuiltinSkin[];
  backgrounds: BuiltinBackground[];
  themes: BuiltinTheme[];
}

/** The generated set, typed. The JSON carries more fields than these views name; the extra ones are not read here. */
export const BUILTINS = generated as unknown as BuiltinSet;
export const TOKENS: readonly BuiltinToken[] = BUILTINS.tokens;
export const SKINS: readonly BuiltinSkin[] = BUILTINS.skins;
export const BACKGROUNDS: readonly BuiltinBackground[] = BUILTINS.backgrounds;
export const BUILTIN_THEMES: readonly BuiltinTheme[] = BUILTINS.themes;
export const DEFAULT_THEME_ID: string = BUILTINS.default;

// ---- State -----------------------------------------------------------------------------------------------------

const DEVICE_KEY = 'apz.appearance.device';
const CACHE_KEY = 'apz.appearance';
const STYLE_ID = 'apz-custom-theme';
const DARK_QUERY = '(prefers-color-scheme: dark)';
const FALLBACK_CODE_SIZE = 13;
const CODE_SIZE_MIN = 9;
const CODE_SIZE_MAX = 24;

/** The built-in palette a skin falls back to when a saved theme of that skin cannot be shown. */
const SKIN_DEFAULT_PALETTE: Record<string, string> = { loom: 'loom-clay', graphite: 'graphite-cobalt' };

/** The message a change carries when the hub cannot be reached: the change shows on this device and is not saved. */
export const UNSAVED_MESSAGE = 'The hub is unreachable; this change is not saved.';

/** Used until the hub answers, or when it cannot be reached and the device has no override. */
const FALLBACK: Appearance = {
  theme: DEFAULT_THEME_ID,
  mode: 'system',
  density: 'default',
  code_size: FALLBACK_CODE_SIZE,
  chosen: false,
};

/**
 * The last document the hub returned (or the one an event carried). It stays null until a read succeeds, and nothing is
 * sent to the hub while it is null, so a failed read cannot overwrite the owner's choice with the fallback.
 */
let hub: AppearanceDoc | null = null;

/** The look of a change made while the hub is unknown. It is shown and cached on this device, and never sent to the hub. */
let offline: Appearance | null = null;

/** Replaces the hub document this module holds, after a read or a change on the hub. */
export function setHub(doc: AppearanceDoc): void {
  hub = doc;
  offline = null;
}

/** True once the hub has answered, so a change may be sent to it. */
export function hubKnown(): boolean {
  return hub !== null;
}

/** This device's override, or null when the hub's choice applies. */
let device: Appearance | null = null;
let started = false;
/** Counts applies, so a slow theme load cannot overwrite a newer choice. */
let applySeq = 0;

// ---- Storage ---------------------------------------------------------------------------------------------------

function readDevice(): Appearance | null {
  try {
    const raw = localStorage.getItem(DEVICE_KEY);
    if (raw === null) return null;
    const a = JSON.parse(raw) as Partial<Appearance> | null;
    return a && typeof a.theme === 'string' && typeof a.mode === 'string' ? (a as Appearance) : null;
  } catch {
    return null;
  }
}

function writeDevice(a: Appearance | null): void {
  try {
    if (a === null) localStorage.removeItem(DEVICE_KEY);
    else localStorage.setItem(DEVICE_KEY, JSON.stringify(a));
  } catch {
    // Storage blocked: the override lasts until the page reloads.
  }
}

/** Caches what boot-theme.js needs to paint before the console starts. The mode is kept as chosen (system included). */
function cache(a: Appearance, paletteId: string, skin: string, background: string, css: string): void {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify({
      theme: paletteId,
      skin,
      mode: a.mode,
      density: a.density,
      code_size: a.code_size,
      background,
      css,
    }));
  } catch {
    // Storage blocked: the page still themes itself, only the first paint of the next visit is affected.
  }
}

// ---- Resolution and application --------------------------------------------------------------------------------

/** The appearance in force: this device's override, else the hub's choice, else a look changed while the hub was unknown. */
function effectiveNow(): Appearance {
  return device ?? hub?.choice ?? offline ?? FALLBACK;
}

function savedNow(): ThemeFile[] {
  return hub?.themes ?? [];
}

function systemMode(): 'light' | 'dark' {
  return window.matchMedia(DARK_QUERY).matches ? 'dark' : 'light';
}

function resolveMode(mode: AppearanceMode): 'light' | 'dark' {
  if (mode === 'light' || mode === 'dark') return mode;
  return systemMode();
}

function backgroundId(bg: BackgroundRef | undefined): string {
  if (typeof bg === 'string') return bg;
  const id = bg?.id;
  return typeof id === 'string' ? id : '';
}

/** One style element holds the resolved CSS of the saved theme in use; built-in themes need none. */
function setCustomCss(css: string): void {
  let el = document.getElementById(STYLE_ID) as HTMLStyleElement | null;
  if (css === '') {
    el?.remove();
    return;
  }
  if (el === null) {
    el = document.createElement('style');
    el.id = STYLE_ID;
    document.head.append(el);
  }
  el.textContent = css;
}

/**
 * Applies an appearance: the root's attributes, the saved theme's CSS when it is one, and the cache. An unknown theme
 * id (a deleted theme, say) falls back to the built-in default. A saved theme whose CSS cannot be produced falls back
 * wholly to the built-in default palette of its skin, and that palette is what gets cached.
 */
export async function apply(a: Appearance, saved: ThemeFile[]): Promise<void> {
  const seq = ++applySeq;
  const root = document.documentElement;
  const builtin = BUILTIN_THEMES.find((t) => t.id === a.theme);
  const custom = builtin ? undefined : saved.find((t) => t.id === a.theme);
  const known = builtin ?? custom;
  const base = known ?? BUILTIN_THEMES.find((t) => t.id === DEFAULT_THEME_ID);
  const skin = a.skin ?? base?.skin ?? 'loom';
  const mode = resolveMode(a.mode);

  let css = '';
  let failed = false;
  if (custom) {
    try {
      // Loaded on demand: a built-in theme never needs the engine.
      const { loadTheme } = await import('../theme/wasm');
      const engine = await loadTheme();
      css = `${engine.resolve(custom, skin, 'light').css}\n${engine.resolve(custom, skin, 'dark').css}`;
    } catch (err) {
      failed = true;
      console.warn('The saved theme could not be applied; the built-in default of its skin is used instead.', err);
    }
  }
  if (seq !== applySeq) return;

  const paletteId = failed ? (SKIN_DEFAULT_PALETTE[skin] ?? DEFAULT_THEME_ID) : known ? a.theme : DEFAULT_THEME_ID;
  const shownBase = failed ? BUILTIN_THEMES.find((t) => t.id === paletteId) : base;
  const background = backgroundId(a.background ?? shownBase?.background);

  root.dataset.palette = paletteId;
  root.dataset.skin = skin;
  root.dataset.mode = mode;
  // The older light/dark attribute: the terminal and the engine views still watch it, so it follows the mode in force.
  root.dataset.theme = mode;
  root.dataset.density = a.density;
  root.dataset.background = background;
  if (typeof a.code_size === 'number' && a.code_size >= CODE_SIZE_MIN && a.code_size <= CODE_SIZE_MAX) {
    root.style.setProperty('--code-size', `${a.code_size}px`);
  } else {
    root.style.removeProperty('--code-size');
  }
  setCustomCss(css);
  cache(a, paletteId, skin, background, css);
  window.dispatchEvent(new CustomEvent('apz:appearance', { detail: a }));
}

// ---- Public runtime --------------------------------------------------------------------------------------------

/** The appearance in force, and whether this device's override is the one applied. */
export function current(): { appearance: Appearance; device: boolean } {
  return { appearance: effectiveNow(), device: device !== null };
}

/** The mode actually shown: light or dark, with the system resolved. */
export function resolvedMode(): 'light' | 'dark' {
  return document.documentElement.dataset.mode === 'light' ? 'light' : 'dark';
}

/** The value of a design token (`--name` on the root), trimmed; for canvas code that cannot read CSS directly. */
export function tokenValue(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(`--${name}`).trim();
}

/** Sets or clears this device's override; it wins over the hub's choice until cleared. */
export async function setDeviceOverride(a: Appearance | null): Promise<void> {
  device = a;
  writeDevice(a);
  await apply(effectiveNow(), savedNow());
}

/**
 * Applies a change made while the hub cannot be read. It shows and is cached on this device, and it is never sent to the
 * hub, so the hub's own choice is not replaced by a fallback.
 */
export async function applyUnsaved(a: Appearance): Promise<void> {
  offline = a;
  await apply(a, savedNow());
}

/** Reads the device override and the hub's document, applies the effective appearance, and follows events. */
export async function startAppearance(): Promise<void> {
  if (started) return;
  started = true;
  device = readDevice();
  // Subscribed before the first read, so a change the hub announces while the read is pending is not missed.
  on('appearance', (data) => onHubFrame(data));
  try {
    setHub(await getAppearance());
  } catch (err) {
    // Without the hub, the device's override or the built-in default still applies, and changes are not saved.
    console.warn('The appearance could not be read from the hub.', err);
  }
  await apply(effectiveNow(), savedNow());
  window.matchMedia(DARK_QUERY).addEventListener('change', () => {
    if (effectiveNow().mode === 'system') void apply(effectiveNow(), savedNow());
  });
}

/** An appearance event: the frame is the document, or a bare appearance (the themes are then kept). */
function onHubFrame(data: unknown): void {
  const frame = (data ?? {}) as Partial<AppearanceDoc> & Partial<Appearance>;
  if (frame.choice !== undefined) {
    setHub({ choice: frame.choice, themes: frame.themes ?? savedNow() });
  } else if (frame.theme !== undefined && frame.mode !== undefined) {
    setHub({ choice: frame as Appearance, themes: savedNow() });
  } else {
    return;
  }
  void apply(effectiveNow(), savedNow());
}

/**
 * Changes the mode of the effective appearance. With a device override the change stays on this device. Otherwise the
 * change replaces only the mode of the hub's last choice and is stored on the hub; while the hub is unknown it applies
 * on this device only and is not sent. The change shows at once and is reverted if the hub refuses it.
 */
export async function setMode(mode: AppearanceMode): Promise<void> {
  if (device !== null) {
    const next: Appearance = { ...device, mode, chosen: true };
    device = next;
    writeDevice(next);
    await apply(next, savedNow());
    return;
  }
  const known = hub;
  if (known === null) {
    await applyUnsaved({ ...effectiveNow(), mode, chosen: true });
    throw new Error(UNSAVED_MESSAGE);
  }
  const before = known.choice;
  const next: Appearance = { ...before, mode, chosen: true };
  await apply(next, savedNow());
  try {
    const doc = await putAppearance(next);
    setHub(doc);
    await apply(effectiveNow(), doc.themes);
  } catch (err) {
    await apply(before, savedNow());
    throw err;
  }
}

// ---- The switch ------------------------------------------------------------------------------------------------

const LABELS: Record<AppearanceMode, string> = { system: 'System', light: 'Light', dark: 'Dark' };

function nextMode(mode: AppearanceMode): AppearanceMode {
  return mode === 'system' ? 'light' : mode === 'light' ? 'dark' : 'system';
}

/** Every switch on the page. One window listener repaints them; a switch that has left the page is dropped then. */
const switches = new Set<HTMLButtonElement>();
let switchesListening = false;

function paintSwitch(btn: HTMLButtonElement): void {
  const mode = current().appearance.mode;
  const shown = resolvedMode();
  btn.title = `${LABELS[mode]} theme. Switch to ${LABELS[nextMode(mode)]}.`;
  btn.setAttribute('aria-label', btn.title);
  replace(btn, icon(shown === 'light' ? 'sun' : 'moon'), h('span.lbl', null, `${LABELS[mode]} theme`));
}

function paintSwitches(): void {
  for (const btn of switches) {
    if (btn.isConnected) paintSwitch(btn);
    else switches.delete(btn);
  }
}

/** The switch in the sidebar's foot: cycles System, Light, Dark, and shows the mode in force. */
export function themeSwitch(): HTMLButtonElement {
  const btn = h('button.btn.ghost.sm.theme-switch', { type: 'button' });
  btn.addEventListener('click', () => {
    void setMode(nextMode(current().appearance.mode)).catch((err: unknown) => {
      console.warn('The theme could not be changed.', err);
    });
  });
  switches.add(btn);
  if (!switchesListening) {
    switchesListening = true;
    window.addEventListener('apz:appearance', paintSwitches);
  }
  paintSwitch(btn);
  return btn;
}
