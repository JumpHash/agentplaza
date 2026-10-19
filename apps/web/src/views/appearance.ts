// The Appearance screen: the owner's look for this hub, on the web and on the desktop. A change applies at once and is
// stored on the hub, or on this device while "This device only" is on. The first-run wizard's look step calls
// chooseLook, so a look is stored the same way from either place.

import '../styles/appearance.css';
import {
  deleteTheme,
  getAppearance,
  putAppearance,
  type Appearance,
  type AppearanceDoc,
  type AppearanceMode,
  type BackgroundRef,
  type Density,
  type ThemeFile,
} from '../api-appearance';
import {
  BACKGROUNDS,
  BUILTIN_THEMES,
  DEFAULT_THEME_ID,
  SKINS,
  apply,
  current,
  resolvedMode,
  UNSAVED_MESSAGE,
  applyUnsaved,
  hubKnown,
  setDeviceOverride,
  setHub,
  type BuiltinBackground,
  type BuiltinSkin,
  type BuiltinTheme,
} from '../ui/theme';
import { badge, button, card } from '../ui/components';
import { h, replace } from '../ui/dom';
import { mountEditor } from './theme-editor';
import type { Cleanup } from './index';

/** A skin beyond its name: the palette it starts from, the token its actions use, its label style and its corner radius. */
interface SkinSpec extends BuiltinSkin {
  palette: string;
  action: string;
  label: { mono: boolean; upper: boolean };
  radius: number;
}

/** A background preset beyond its name: the pattern drawn and its spacing. */
interface BackgroundSpec extends BuiltinBackground {
  pattern: string;
  scale: number;
}

type Tokens = Record<string, string>;
type Mode = 'light' | 'dark';
type Act = (action: () => Promise<void>) => void;

const SKIN_SPECS = SKINS as readonly SkinSpec[];
const BG_SPECS = BACKGROUNDS as readonly BackgroundSpec[];
const MODES: [AppearanceMode, string][] = [['system', 'System'], ['light', 'Light'], ['dark', 'Dark']];
const DENSITIES: [Density, string][] = [['compact', 'Compact'], ['default', 'Default'], ['comfortable', 'Comfortable']];
const DOT_TOKENS = ['paper', 'ink', 'signal', 'ok', 'info'];
const CODE_MIN = 11;
const CODE_MAX = 16;

/** The shared state of the screen: the hub's document, and the last error a change raised. */
let doc: AppearanceDoc | null = null;
let error = '';

// ---- What is in force ------------------------------------------------------------------------------------------

function savedNow(): ThemeFile[] {
  return doc?.themes ?? [];
}

function builtinOf(id: string): BuiltinTheme | undefined {
  return BUILTIN_THEMES.find((t) => t.id === id);
}

function backgroundId(bg: BackgroundRef | undefined): string {
  if (typeof bg === 'string') return bg;
  const id = bg?.id;
  return typeof id === 'string' ? id : '';
}

/** The palette, skin and background an appearance resolves to, as the runtime resolves them. */
function resolve(a: Appearance): { palette: string; skin: string; background: string } {
  const builtin = builtinOf(a.theme);
  const saved = builtin ? undefined : savedNow().find((t) => t.id === a.theme);
  const known = builtin ?? saved;
  const base = known ?? builtinOf(DEFAULT_THEME_ID);
  return {
    palette: known ? a.theme : DEFAULT_THEME_ID,
    skin: a.skin ?? base?.skin ?? 'loom',
    background: backgroundId(a.background ?? base?.background),
  };
}

/** The palette a skin starts from: the first built-in palette of that skin, as the generated set names it. */
function defaultPaletteOf(skin: string): BuiltinTheme {
  const spec = SKIN_SPECS.find((s) => s.id === skin);
  return builtinOf(spec?.palette ?? '') ?? builtinOf(DEFAULT_THEME_ID)!;
}

function modeTokens(p: { light?: Tokens; dark?: Tokens }, mode: Mode): Tokens {
  return (mode === 'light' ? p.light : p.dark) ?? {};
}

/** The tokens of a palette in one mode. A saved theme's missing tokens come from its skin's default palette. */
function tokensOf(id: string, mode: Mode): Tokens {
  const builtin = builtinOf(id);
  if (builtin) return modeTokens(builtin, mode);
  const saved = savedNow().find((t) => t.id === id);
  if (!saved) return modeTokens(defaultPaletteOf(DEFAULT_THEME_ID), mode);
  return { ...modeTokens(defaultPaletteOf(saved.skin), mode), ...modeTokens(saved.palette, mode) };
}

/** A colour from a token map, kept to plain CSS colour text so a saved palette cannot inject other CSS. */
function safeColor(value: string | undefined): string {
  return value && /^[#\w(),.%\s-]+$/.test(value) ? value : 'transparent';
}

function cssVars(pairs: [string, string][]): string {
  return pairs.map(([name, value]) => `${name}:${value}`).join(';');
}

/** The CSS background of a pattern preset, drawn in the palette's pattern colour. */
function patternCss(bg: BackgroundSpec, color: string): string {
  const s = bg.scale || 22;
  switch (bg.pattern) {
    case 'dots':
      return `radial-gradient(${color} 1px, transparent 1.5px) 0 0 / ${s}px ${s}px`;
    case 'grid':
    case 'blueprint':
      return `linear-gradient(${color} 1px, transparent 1px) 0 0 / ${s}px ${s}px, linear-gradient(90deg, ${color} 1px, transparent 1px) 0 0 / ${s}px ${s}px`;
    case 'weave':
      return `repeating-linear-gradient(45deg, ${color} 0 1px, transparent 1px 8px)`;
    case 'contours':
      return `repeating-radial-gradient(circle at 30% 40%, ${color} 0 1px, transparent 1px ${s}px)`;
    default:
      return 'none';
  }
}

// ---- Changes ---------------------------------------------------------------------------------------------------

/** Stores a change where the switch says: this device's override while it is on, otherwise the hub. */
export async function commitAppearance(next: Appearance): Promise<void> {
  if (current().device) {
    await setDeviceOverride(next);
    return;
  }
  if (!hubKnown()) {
    // The hub has not answered: the change shows and is cached on this device, and it is not sent to the hub.
    await applyUnsaved(next);
    throw new Error(UNSAVED_MESSAGE);
  }
  const saved = await putAppearance(next);
  doc = saved;
  setHub(saved);
  await apply(saved.choice, saved.themes);
}

/** The wizard's look step: a built-in palette becomes the owner's look, with that skin's own background. */
export function chooseLook(palette: string): Promise<void> {
  return commitAppearance({ ...current().appearance, theme: palette, skin: undefined, background: undefined, chosen: true });
}

async function pickSkin(s: SkinSpec): Promise<void> {
  if (resolve(current().appearance).skin === s.id) return;
  await commitAppearance({ ...current().appearance, theme: s.palette, skin: undefined, background: undefined, chosen: true });
}

async function pickPalette(id: string): Promise<void> {
  await commitAppearance({ ...current().appearance, theme: id, skin: undefined, chosen: true });
}

async function pickBackground(id: string): Promise<void> {
  await commitAppearance({ ...current().appearance, background: id, chosen: true });
}

async function pickMode(mode: AppearanceMode): Promise<void> {
  await commitAppearance({ ...current().appearance, mode, chosen: true });
}

async function pickDensity(density: Density): Promise<void> {
  await commitAppearance({ ...current().appearance, density, chosen: true });
}

const clampSize = (n: number): number => Math.min(CODE_MAX, Math.max(CODE_MIN, n));

async function stepSize(delta: number): Promise<void> {
  const size = clampSize(current().appearance.code_size + delta);
  await commitAppearance({ ...current().appearance, code_size: size, chosen: true });
}

/** Turns "This device only" on (this device starts from the hub's look) or off (the hub's look applies again). */
async function setDeviceOnly(on: boolean): Promise<void> {
  if (on) {
    await setDeviceOverride({ ...current().appearance, chosen: true });
    return;
  }
  const fresh = await getAppearance();
  doc = fresh;
  setHub(fresh);
  await setDeviceOverride(null);
}

async function removeTheme(id: string, name: string): Promise<void> {
  if (!window.confirm(`Delete the theme "${name}" from the hub? Anything that uses it falls back to the default look.`)) return;
  const fresh = await deleteTheme(id);
  doc = fresh;
  setHub(fresh);
  await apply(current().appearance, fresh.themes);
}

/** Runs one change: a failure is shown inline, and the screen is drawn again either way. */
async function attempt(action: () => Promise<void>, paint: () => void): Promise<void> {
  error = '';
  try {
    await action();
  } catch (err) {
    error = err instanceof Error ? err.message : 'The change could not be saved.';
  }
  paint();
}

// ---- Sections --------------------------------------------------------------------------------------------------

/** The page head: the title and the one line under it, as the prototype's settings pages have it. */
function pageHead(): HTMLElement {
  return h(
    'header.page-head',
    null,
    h('div.page-head-row', null, h('h1', null, 'Appearance')),
    h('p.lede', null, 'One look for this hub, on the web and on the desktop.'),
  );
}

/** A segmented control: one pressed button per choice. */
function segmented<T extends string>(label: string, items: [T, string][], value: T, onPick: (v: T) => void): HTMLElement {
  return h(
    'div.seg',
    { role: 'group', 'aria-label': label },
    ...items.map(([v, text]) => h('button', { type: 'button', 'aria-pressed': String(v === value), onclick: () => onPick(v) }, text)),
  );
}

/** A labelled control in a column: the prototype's field. */
function labelled(label: string, control: Node): HTMLElement {
  return h('div.field', null, h('span.field-l', null, label), control);
}

function skinCard(s: SkinSpec, selected: boolean, mode: Mode, act: Act): HTMLElement {
  const t = tokensOf(s.palette, mode);
  const c = (name: string): string => safeColor(t[name]);
  const style = cssVars([
    ['--pv-paper', c('paper')],
    ['--pv-sheet', c('sheet')],
    ['--pv-rule', c('rule')],
    ['--pv-ink', c('ink')],
    ['--pv-action', c(s.action)],
    ['--pv-on-action', c('on-action')],
    ['--pv-signal', c('signal')],
    ['--pv-radius', `${s.radius}px`],
  ]);
  return h(
    'button.card.look',
    { type: 'button', class: [`skin-${s.id}`, selected ? 'on' : ''].join(' '), 'aria-pressed': String(selected), style, onclick: () => act(() => pickSkin(s)) },
    h(
      'span.look-mock',
      null,
      h('span.mock-side'),
      h(
        'span.mock-main',
        null,
        h('span.mock-label', null, 'Agents'),
        h('span.mock-card', null, h('span.mock-line'), h('span.mock-line.short')),
        h('span.mock-btn', null, 'Run'),
      ),
    ),
    h('span.look-name', null, s.name),
    h('span.look-about', null, s.about),
  );
}

interface PaletteView {
  id: string;
  name: string;
  about: string;
  saved: boolean;
}

function paletteCard(p: PaletteView, selected: boolean, mode: Mode, act: Act): HTMLElement {
  const t = tokensOf(p.id, mode);
  const dots = DOT_TOKENS.map((name) => h('span.dot-swatch', { style: `background:${safeColor(t[name])}` }));
  const pick = h(
    'button.pal-pick',
    { type: 'button', 'aria-pressed': String(selected), onclick: () => act(() => pickPalette(p.id)) },
    h('span.pal-top', null, h('span.pal-name', null, p.name), badge(p.saved ? 'Yours' : 'Built in', p.saved ? 'accent' : 'muted')),
    p.about ? h('span.hint', null, p.about) : null,
    h('span.dots', null, ...dots),
  );
  const actions = p.saved
    ? [
        h('a.btn.ghost.sm', { href: `#/appearance/edit/${encodeURIComponent(p.id)}` }, 'Edit'),
        button('Delete', () => act(() => removeTheme(p.id, p.name)), { kind: 'ghost', size: 'sm' }),
      ]
    : [h('a.btn.ghost.sm', { href: `#/appearance/edit/new?from=${encodeURIComponent(p.id)}` }, 'Duplicate and edit')];
  return h('section.card.pal', { class: selected ? 'on' : '' }, h('div.card-b', null, pick, h('div.cluster', null, ...actions)));
}

function paletteViews(): PaletteView[] {
  const builtin = BUILTIN_THEMES.map((t) => ({ id: t.id, name: t.name, about: t.about, saved: false }));
  const saved = savedNow().map((t) => ({ id: t.id, name: t.name, about: t.about ?? '', saved: true }));
  return [...builtin, ...saved];
}

function backgroundCard(bg: BackgroundSpec, selected: boolean, color: string, act: Act): HTMLElement {
  return h(
    'button.card.bg-card',
    { type: 'button', class: selected ? 'on' : '', 'aria-pressed': String(selected), onclick: () => act(() => pickBackground(bg.id)) },
    h('span.bg-swatch', { style: `background:${patternCss(bg, color)}` }),
    h('span.bg-name', null, bg.name),
  );
}

/** "This device only" as the prototype's switch, with the note on what it changes. */
function deviceSwitch(device: boolean, act: Act): HTMLElement {
  return h(
    'div.stack',
    null,
    h(
      'div.cluster',
      null,
      h('button.tgl', {
        type: 'button',
        role: 'switch',
        'aria-checked': String(device),
        'aria-label': 'This device only',
        onclick: () => act(() => setDeviceOnly(!device)),
      }),
      h('span', null, 'This device only'),
    ),
    h(
      'p.hint',
      null,
      device
        ? 'In force on this device: changes here stay in this browser and the hub keeps its own look. Turning this off clears this device’s look and follows the hub’s choice again; nothing from this device is sent to the hub.'
        : 'In force: the hub’s look, the same on every device. Changes are stored on the hub.',
    ),
  );
}

// ---- The screen ------------------------------------------------------------------------------------------------

export function mount(root: HTMLElement, params: string[] = []): Cleanup {
  // The theme editor and the import screen are this section's routes: their parts come after "appearance".
  if (params[0] === 'edit' || params[0] === 'import') return mountEditor(root, params);
  let alive = true;
  const body = h('div.stack');

  const paint = (): void => {
    if (!alive) return;
    const a = current().appearance;
    const eff = resolve(a);
    const mode = resolvedMode();
    const act: Act = (action) => {
      void attempt(action, paint);
    };
    const bgColor = safeColor(tokensOf(eff.palette, mode).pattern);
    replace(
      body,
      error ? h('p.notice', { role: 'alert' }, error) : null,
      card('Skin', h('div.look-grid', null, ...SKIN_SPECS.map((s) => skinCard(s, s.id === eff.skin, mode, act)))),
      card('Palette', h('div.pal-grid', null, ...paletteViews().map((p) => paletteCard(p, p.id === eff.palette, mode, act))), {
        actions: [
          h('a.btn.sm', { href: '#/appearance/import' }, 'Import theme…'),
          h('a.btn.sm', { href: '#/appearance/import' }, 'Import from VS Code…'),
        ],
      }),
      card('Background', h('div.bg-grid', null, ...BG_SPECS.map((bg) => backgroundCard(bg, bg.id === eff.background, bgColor, act)))),
      card(
        'Display',
        h(
          'div.stack',
          null,
          labelled('Mode', segmented<AppearanceMode>('Mode', MODES, a.mode, (m) => act(() => pickMode(m)))),
          labelled('Density', segmented<Density>('Density', DENSITIES, a.density, (d) => act(() => pickDensity(d)))),
          labelled(
            'Code size',
            h(
              'div.cluster',
              { role: 'group', 'aria-label': 'Code size' },
              button('−', () => act(() => stepSize(-1)), { kind: 'ghost', size: 'sm', title: 'Smaller code', disabled: a.code_size <= CODE_MIN }),
              h('span.num', null, `${a.code_size}px`),
              button('+', () => act(() => stepSize(1)), { kind: 'ghost', size: 'sm', title: 'Larger code', disabled: a.code_size >= CODE_MAX }),
            ),
          ),
        ),
      ),
      card('Where changes go', deviceSwitch(current().device, act)),
    );
  };

  root.replaceChildren(pageHead(), body);
  const onApply = (): void => paint();
  window.addEventListener('apz:appearance', onApply);
  paint();
  void getAppearance().then(
    (fresh) => {
      doc = fresh;
      setHub(fresh);
      paint();
    },
    () => {
      error = 'The saved themes could not be read from the hub.';
      paint();
    },
  );

  return () => {
    alive = false;
    window.removeEventListener('apz:appearance', onApply);
    root.replaceChildren();
  };
}
