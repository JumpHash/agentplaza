// The theme editor and the theme import screen. The Appearance screen mounts them through mountEditor at
// `#/appearance/edit/<id>`, `#/appearance/edit/new?from=<id>`, `#/appearance/edit/__import` (after "Edit first") and
// `#/appearance/import`. Every resolve, contrast check, fix and import goes through the theme engine (theme/wasm).

import '../styles/theme-editor.css';
import { badge, button, card } from '../ui/components';
import { h, replace } from '../ui/dom';
import {
  getAppearance,
  putAppearance,
  saveTheme,
  type Appearance,
  type AppearanceDoc,
  type ThemeFile as SavedThemeFile,
} from '../api-appearance';
import {
  BUILTIN_THEMES,
  DEFAULT_THEME_ID,
  SKINS,
  TOKENS,
  apply,
  current,
  setDeviceOverride,
  setHub,
  type BuiltinTheme,
} from '../ui/theme';
import {
  loadTheme,
  type ContrastRow,
  type ImportResult,
  type Mode,
  type ResolveResult,
  type ThemeFile,
  type ThemeWasm,
} from '../theme/wasm';

/** The token groups in the order the editor lists them; a group the engine adds later is listed after these. */
const GROUP_ORDER = ['Ground', 'Lines', 'Text', 'Accent', 'States', 'Diff', 'Code', 'Terminal', 'Data', 'Overlay'];
const MODES: Mode[] = ['light', 'dark'];
const MODE_LABEL: Record<Mode, string> = { light: 'Light', dark: 'Dark' };
const HEX = /^#([0-9a-f]{6}|[0-9a-f]{8})$/i;
/** Heights of the six sample bars, in per cent. */
const SAMPLE_BARS = [62, 84, 48, 92, 71, 56];

/** A theme imported on the Import screen, waiting for "Edit first". The editor reads it at `edit/__import`. */
let pendingImport: ThemeFile | null = null;

/** One token's controls in the editor, kept so a refresh can update them without rebuilding the list. */
interface RowHandle {
  swatch: HTMLInputElement;
  hex: HTMLInputElement;
  badge: HTMLElement;
  fix: HTMLButtonElement;
}

// ---- Helpers ----------------------------------------------------------------------------------------------------

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function setLine(el: HTMLElement, text: string, bad = false): void {
  el.textContent = text;
  el.classList.toggle('bad', bad);
}

/** Runs an engine call. A refusal goes to `fail` and the result is null. */
function attempt<T>(fn: () => T, fail: (message: string) => void): T | null {
  try {
    return fn();
  } catch (err) {
    fail(errorText(err));
    return null;
  }
}

function back(): void {
  location.hash = '#/appearance';
}

/** The `from` parameter of a `new?from=<id>` route, read from the address. */
function fromParam(): string {
  const m = /[?&]from=([^&]+)/.exec(location.hash);
  return m?.[1] ? decodeURIComponent(m[1]) : '';
}

function findBuiltin(id: string): BuiltinTheme | undefined {
  return BUILTIN_THEMES.find((t) => t.id === id);
}

function fromBuiltin(t: BuiltinTheme, id: string, name: string): ThemeFile {
  return {
    schema: 1,
    id,
    name,
    about: t.about,
    skin: t.skin,
    background: t.background,
    palette: { light: { ...t.light }, dark: { ...t.dark } },
  };
}

/** `custom-<slug>-<4 random characters>`, the id a new theme gets. */
function newId(name: string): string {
  const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '') || 'theme';
  const chars = 'abcdefghijklmnopqrstuvwxyz0123456789';
  let rand = '';
  for (let i = 0; i < 4; i++) rand += chars.charAt(Math.floor(Math.random() * chars.length));
  return `custom-${slug}-${rand}`;
}

/** Gives an imported theme a fresh id when its own id is taken, so an import never overwrites a theme silently. */
function uniqueId(file: ThemeFile, doc: AppearanceDoc): ThemeFile {
  if (doc.themes.some((t) => t.id === file.id) || findBuiltin(file.id) !== undefined) {
    file.id = newId(file.name);
  }
  return file;
}

/** A colour as a colour input takes it: `#rrggbb`, lower case, alpha dropped. */
function hex6(value: string): string {
  return /^#[0-9a-f]{6}/i.test(value) ? value.slice(0, 7).toLowerCase() : '#000000';
}

/** The theme in use for the owner: a saved theme, else the built-in it names, else the default. */
function activeFile(doc: AppearanceDoc): ThemeFile {
  const id = current().appearance.theme;
  const saved = doc.themes.find((t) => t.id === id);
  if (saved) return clone(saved) as unknown as ThemeFile;
  const b = findBuiltin(id) ?? findBuiltin(DEFAULT_THEME_ID);
  if (!b) throw new Error('this build has no built-in theme');
  return fromBuiltin(b, b.id, b.name);
}

/** The copy a `new?from=<id>` route starts from: a saved theme, else a built-in, else the default. */
function copyOf(doc: AppearanceDoc, from: string): ThemeFile {
  const saved = doc.themes.find((t) => t.id === from);
  if (saved) {
    const copy = clone(saved) as unknown as ThemeFile;
    copy.name = `${saved.name} copy`;
    copy.id = newId(copy.name);
    return copy;
  }
  const b = findBuiltin(from) ?? findBuiltin(DEFAULT_THEME_ID);
  if (!b) throw new Error('this build has no built-in theme');
  const name = `${b.name} copy`;
  return fromBuiltin(b, newId(name), name);
}

/**
 * Saves a theme on the hub and chooses it. With this device's own look, the theme is chosen in that override and the
 * hub keeps its choice. Otherwise the hub's choice is replaced. Either way the hub's current document is kept here.
 */
async function saveAndChoose(file: ThemeFile): Promise<void> {
  const saved = await saveTheme(file as unknown as SavedThemeFile);
  setHub(saved);
  if (current().device) {
    await setDeviceOverride({ ...current().appearance, theme: file.id, skin: file.skin, background: undefined, chosen: true });
    return;
  }
  const choice: Appearance = { ...saved.choice, theme: file.id, skin: file.skin, background: undefined, chosen: true };
  const doc = await putAppearance(choice);
  setHub(doc);
  await apply(doc.choice, doc.themes);
}

/** Starts a download of a theme as `<id>.apztheme.json`. */
function download(file: ThemeFile): void {
  const blob = new Blob([`${JSON.stringify(file, null, 2)}\n`], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const link = h('a', { href: url, download: `${file.id}.apztheme.json` });
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// ---- Preview ----------------------------------------------------------------------------------------------------

/** Sets the resolved tokens on a preview element as the CSS variables the console's own styles read. */
function paintPreview(el: HTMLElement, tokens: Record<string, string>): void {
  for (const [name, value] of Object.entries(tokens)) el.style.setProperty(`--${name}`, value);
}

/** A small page built from the console's own parts (a card, chips, buttons, a diff, a code block), in the theme's tokens. */
function samplePreview(): HTMLElement {
  const bars = SAMPLE_BARS.map((pct) => {
    const bar = h('span.te-bar');
    bar.style.height = `${pct}%`;
    return bar;
  });
  return h(
    'div.te-preview',
    { 'aria-label': 'Live preview' },
    h(
      'div.card',
      null,
      h('div.card-h', null, h('h3', null, 'Session overview'), h('span.sp'), h('span.hint', null, 'Updated 2 minutes ago')),
      h(
        'div.card-b',
        null,
        h('p', null, 'Three agents are working on the import pipeline. One waits for a review.'),
        h('div.cluster', null, h('span.btn.primary.sm', null, h('span', null, 'Approve')), h('span.btn.ghost.sm', null, h('span', null, 'Details'))),
      ),
    ),
    h('div.cluster', null, badge('Passing', 'ok'), badge('Waiting', 'warn'), badge('Failed', 'danger'), badge('Queued', 'info')),
    h(
      'div.dlines',
      null,
      h('div.dl.add', null, h('span.g', null, '12'), h('span.sg', null, '+'), h('code', null, 'const limit = 40;')),
      h('div.dl.del', null, h('span.g', null, '12'), h('span.sg', null, '−'), h('code', null, 'const limit = 20;')),
    ),
    h('div.te-chart', null, ...bars),
    h(
      'div.code',
      null,
      h(
        'div.cl',
        null,
        h('span.ln', null, '1'),
        h(
          'code',
          null,
          h('span.tk-k', null, 'const'),
          ' ',
          'page',
          ' = ',
          h('span.tk-n', null, '40'),
          '; ',
          h('span.te-comment', null, '// items per page'),
        ),
      ),
    ),
  );
}

/** The sample page painted with a theme's tokens for one mode. A theme the engine refuses shows the bare page. */
function previewOf(engine: ThemeWasm, file: ThemeFile, mode: Mode): HTMLElement {
  const el = samplePreview();
  try {
    paintPreview(el, engine.resolve(file, file.skin, mode).resolved.tokens);
  } catch {
    // The page stays unpainted; the editor's status line says why the theme is refused.
  }
  return el;
}

function modeTabs(active: Mode, pick: (m: Mode) => void): HTMLElement {
  return h(
    'div.tabs',
    { role: 'tablist' },
    ...MODES.map((m) =>
      h(
        'button.tab',
        {
          type: 'button',
          role: 'tab',
          'aria-selected': String(m === active),
          onclick: () => pick(m),
        },
        MODE_LABEL[m],
      ),
    ),
  );
}

// ---- The editor -------------------------------------------------------------------------------------------------

/** The editor for one theme: the token list with contrast badges on the left, the live preview on the right. */
function editor(root: HTMLElement, engine: ThemeWasm, start: ThemeFile): void {
  let file = start;
  let mode: Mode = 'light';
  let resolved: ResolveResult | null = null;
  let rows: ContrastRow[] = [];
  let problems: string[] = [];
  const handles = new Map<string, RowHandle>();

  const status = h('p.hint', { role: 'status' });
  const fail = (message: string): void => setLine(status, `The engine refused this theme: ${message}`, true);
  const title = h('h1', null, file.name);
  const idLabel = h('code', null, file.id);
  const nameInput = h('input.input', { type: 'text', maxlength: 80, 'aria-label': 'Theme name' });
  nameInput.value = file.name;
  const skinSelect = h('select.select', { 'aria-label': 'Skin' }, ...SKINS.map((s) => h('option', { value: s.id }, s.name)));
  skinSelect.value = file.skin;
  const tabsBox = h('div');
  const groupsBox = h('div.stack');
  const problemBox = h('div');
  const preview = samplePreview();

  nameInput.addEventListener('input', () => {
    file.name = nameInput.value;
    title.textContent = file.name.trim() || 'Theme';
  });
  skinSelect.addEventListener('change', () => {
    file.skin = skinSelect.value;
    recompute();
  });

  /** Writes one colour into the palette of the mode being edited, then re-resolves. */
  function setColour(token: string, value: string): void {
    const pal = file.palette[mode] ?? (file.palette[mode] = {});
    pal[token] = value;
    recompute();
  }

  /** Lets the engine move one colour until it passes its contrast minimum. */
  function fixToken(token: string): void {
    const next = attempt(() => engine.fix(file, file.skin, mode, token), fail);
    if (next === null) return;
    file = next;
    recompute();
    setLine(status, `Moved ${token} until it passes.`);
  }

  /** Re-resolves the theme and refreshes the preview, the badges and the problem list. */
  function recompute(): void {
    // Any refusal or throw clears the resolved form and puts the engine's message in the problem list, so save() refuses.
    const refusals: string[] = [];
    const refuse = (message: string): void => {
      refusals.push(message);
      fail(message);
    };
    const next = attempt(() => engine.resolve(file, file.skin, mode), refuse);
    const contrast = attempt(() => engine.contrast(file, file.skin, mode), refuse);
    const verdict = attempt(() => engine.validate(file), refuse);
    resolved = refusals.length === 0 ? next : null;
    rows = contrast ?? [];
    problems = refusals.length > 0 ? refusals : (verdict?.problems ?? []);
    if (refusals.length === 0) {
      setLine(status, '');
    }
    if (resolved !== null) paintPreview(preview, resolved.resolved.tokens);
    refreshRows();
    renderProblems();
  }

  /** Updates every token row from the resolved theme and the contrast rows. The field being typed in is left alone. */
  function refreshRows(): void {
    for (const [name, hd] of handles) {
      if (resolved !== null) {
        // A refused theme keeps the last colours shown; only the badges and the Fix buttons follow the new state.
        const shown = resolved.resolved.tokens[name] ?? '#000000';
        hd.swatch.value = hex6(shown);
        if (document.activeElement !== hd.hex) hd.hex.value = shown;
      }
      const own = rows.filter((r) => r.token === name);
      const failing = own.filter((r) => !r.ok).sort((a, b) => a.ratio - b.ratio)[0];
      const weakest = own.slice().sort((a, b) => a.ratio - b.ratio)[0];
      const pick = failing ?? weakest;
      if (pick === undefined) {
        hd.badge.textContent = '';
        hd.badge.className = 'chip';
        hd.badge.removeAttribute('title');
      } else {
        hd.badge.textContent = `${pick.ratio.toFixed(1)} : 1`;
        hd.badge.className = pick.ok ? 'chip t-ok' : 'chip t-danger';
        hd.badge.title = `${pick.ratio.toFixed(2)} against ${pick.against}; the minimum is ${pick.min.toFixed(1)}`;
      }
      hd.fix.hidden = failing === undefined;
    }
  }

  function renderProblems(): void {
    const failing = rows.filter((r) => !r.ok).length;
    const items = [...problems];
    if (failing > 0) {
      items.push(`${failing} contrast ${failing === 1 ? 'check fails' : 'checks fail'}; use Fix on the marked tokens.`);
    }
    replace(problemBox, items.length === 0
      ? badge('No problems. The theme can be saved.', 'ok')
      : h('ul.notes', null, ...items.map((text) => h('li', null, text))));
  }

  function tokenRow(name: string, about: string): HTMLElement {
    const swatch = h('input.te-swatch', { type: 'color', 'aria-label': `${name} colour` });
    const hex = h('input.input.te-hex', { type: 'text', 'aria-label': `${name} hex value` });
    const contrast = h('span.chip');
    const fix = h('button.btn.ghost.sm', { type: 'button', title: 'Move this colour until it passes the contrast minimum' }, 'Fix');
    fix.hidden = true;
    handles.set(name, { swatch, hex, badge: contrast, fix });
    swatch.addEventListener('input', () => setColour(name, swatch.value));
    hex.addEventListener('input', () => {
      const value = hex.value.trim();
      if (HEX.test(value)) {
        hex.classList.remove('te-invalid');
        setColour(name, value);
      } else {
        hex.classList.add('te-invalid');
      }
    });
    hex.addEventListener('blur', () => {
      hex.classList.remove('te-invalid');
      refreshRows();
    });
    fix.addEventListener('click', () => fixToken(name));
    return h(
      'tr',
      null,
      h('td', null, h('div.te-meta', null, h('span.mono', null, name), h('span.hint', null, about))),
      h('td', null, h('div.cluster', null, swatch, hex)),
      h('td', null, h('div.cluster', null, contrast, fix)),
    );
  }

  /** Builds the token list for the mode being edited. */
  function renderTokens(): void {
    handles.clear();
    replace(tabsBox, modeTabs(mode, (m) => {
      mode = m;
      renderTokens();
      recompute();
    }));
    const groups = [...new Set([...GROUP_ORDER, ...TOKENS.map((t) => t.group)])];
    replace(groupsBox, ...groups
      .filter((g) => TOKENS.some((t) => t.group === g))
      .map((g) => h('div.te-group', null,
        h('h4', null, g),
        h('div.tbl-wrap', null, h('table.tbl', null,
          h('thead', null, h('tr', null, h('th', null, 'Token'), h('th', null, 'Colour'), h('th', null, 'Contrast'))),
          h('tbody', null, ...TOKENS.filter((t) => t.group === g).map((t) => tokenRow(t.name, t.about))))))));
  }

  async function save(): Promise<void> {
    recompute();
    if (resolved === null || problems.length > 0) {
      setLine(status, 'Not saved: fix the problems listed on the right first.', true);
      return;
    }
    setLine(status, 'Saving...');
    try {
      await saveAndChoose(file);
    } catch (err) {
      setLine(status, `Not saved: ${errorText(err)}`, true);
      return;
    }
    setLine(status, `Saved. ${file.name} is the theme in use.`);
    if (/#\/appearance\/edit\/(new|__import)/.test(location.hash)) {
      location.hash = `#/appearance/edit/${encodeURIComponent(file.id)}`;
    }
  }

  const head = h(
    'header.page-head',
    null,
    h('div.kicker', null, h('a', { href: '#/appearance' }, 'Appearance'), h('span', null, '/'), h('span', null, 'Theme')),
    h(
      'div.page-head-row',
      null,
      title,
      h('span.sp'),
      h(
        'div.page-actions',
        null,
        button('Cancel', back, { kind: 'ghost' }),
        button('Export', () => download(file), { kind: 'ghost' }),
        button('Save', () => void save(), { kind: 'primary' }),
      ),
    ),
    h('p.lede', null, 'Each colour of this theme, in both modes. The preview shows the page with the colours in force.'),
  );

  const details = card(
    'Theme',
    h(
      'div.stack',
      null,
      h(
        'div.grid2',
        null,
        h('div.field', null, h('span.field-l', null, 'Name'), nameInput),
        h('div.field', null, h('span.field-l', null, 'Skin'), skinSelect),
      ),
      h('p.hint', null, 'ID ', idLabel),
    ),
  );
  const tokens = card('Colours', h('div.stack', null, tabsBox, groupsBox));
  const side = h('div.stack', null, card('Preview', preview), card('Problems', problemBox));

  replace(root, head, status, h('div.te-layout', null, h('div.stack', null, details, tokens), side));
  renderTokens();
  recompute();
}

// ---- The import screen ------------------------------------------------------------------------------------------

/** A drop target that also opens a file picker on click or Enter. */
function dropZone(title: string, hint: string, accept: string, onFile: (file: File) => void): HTMLElement {
  const input = h('input', { type: 'file', accept, hidden: true, tabindex: -1 });
  const zone = h('div.te-drop', { role: 'button', tabindex: 0 },
    h('strong', null, title),
    h('span.hint', null, hint),
    input);
  const open = (): void => input.click();
  zone.addEventListener('click', (e) => {
    if (e.target !== input) open();
  });
  zone.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      open();
    }
  });
  input.addEventListener('change', () => {
    const file = input.files?.[0];
    input.value = '';
    if (file) onFile(file);
  });
  zone.addEventListener('dragover', (e) => {
    e.preventDefault();
    zone.classList.add('over');
  });
  zone.addEventListener('dragleave', () => zone.classList.remove('over'));
  zone.addEventListener('drop', (e) => {
    e.preventDefault();
    zone.classList.remove('over');
    const file = e.dataTransfer?.files[0];
    if (file) onFile(file);
  });
  return zone;
}

/** The import screen: a theme file or a VS Code theme, a comparison with the theme in use, then Apply or Edit first. */
function importer(root: HTMLElement, engine: ThemeWasm, doc: AppearanceDoc): void {
  let skin = activeFile(doc).skin;
  let last: ImportResult | null = null;
  let mode: Mode = 'light';

  const status = h('p.hint', { role: 'status' });
  const fail = (message: string): void => setLine(status, message, true);
  const pickBox = h('div.cluster');
  const resultBox = h('div.stack');
  const skinSelect = h('select.select', { 'aria-label': 'Skin for VS Code themes' },
    ...SKINS.map((s) => h('option', { value: s.id }, s.name)));
  skinSelect.value = skin;
  skinSelect.addEventListener('change', () => {
    skin = skinSelect.value;
  });

  function showProblems(list: string[]): void {
    last = null;
    replace(resultBox, card('This file cannot be imported.', h('ul.notes', null,
      ...(list.length > 0 ? list : ['The engine did not accept this file.']).map((text) => h('li', null, text)))));
  }

  function renderResult(): void {
    if (last === null) {
      resultBox.replaceChildren();
      return;
    }
    const file = last.file;
    const report = last.report;
    const summary = report.summary
      ?? `${report.taken} colours taken, ${report.derived} derived, ${report.adjusted.length} adjusted for contrast`;
    const failing = report.failing.length;
    replace(resultBox, card(
      file.name,
      h(
        'div.stack',
        null,
        h('p.hint', null, failing > 0
          ? `${summary}. ${failing} ${failing === 1 ? 'colour is' : 'colours are'} still below the contrast minimum.`
          : `${summary}.`),
        modeTabs(mode, (m) => {
          mode = m;
          renderResult();
        }),
        h(
          'div.grid2',
          null,
          h('figure.te-cmp', null, h('figcaption.hint', null, 'In use now'), previewOf(engine, activeFile(doc), mode)),
          h('figure.te-cmp', null, h('figcaption.hint', null, 'Imported'), previewOf(engine, file, mode)),
        ),
        h(
          'div.cluster',
          null,
          button('Apply', () => void applyImport(), { kind: 'primary' }),
          button('Edit first', editFirst, { kind: 'ghost' }),
        ),
      ),
    ));
  }

  function show(result: ImportResult): void {
    last = { ...result, file: uniqueId(clone(result.file), doc) };
    mode = result.mode;
    pickBox.replaceChildren();
    setLine(status, '');
    renderResult();
  }

  async function applyImport(): Promise<void> {
    if (last === null) return;
    const file = last.file;
    const check = attempt(() => engine.validate(file), fail);
    if (check === null) return;
    if (!check.ok) {
      showProblems(check.problems);
      return;
    }
    setLine(status, 'Saving...');
    try {
      await saveAndChoose(file);
    } catch (err) {
      fail(`Not saved: ${errorText(err)}`);
      return;
    }
    back();
  }

  function editFirst(): void {
    if (last === null) return;
    pendingImport = clone(last.file);
    location.hash = '#/appearance/edit/__import';
  }

  async function fromApz(f: File): Promise<void> {
    const text = await f.text();
    const parsed = attempt(() => JSON.parse(text) as ThemeFile, fail);
    if (parsed === null) return;
    const check = attempt(() => engine.validate(parsed), fail);
    if (check === null) return;
    if (!check.ok) {
      showProblems(check.problems);
      return;
    }
    const shown = attempt(() => engine.resolve(parsed, parsed.skin, 'light'), fail);
    if (shown === null) return;
    show({ file: parsed, name: parsed.name, mode: 'light', report: shown.resolved.report });
  }

  async function fromVscodeJson(f: File): Promise<void> {
    const text = await f.text();
    const result = attempt(() => engine.importVscode(text, skin), fail);
    if (result !== null) show(result);
  }

  function takeVsix(bytes: Uint8Array, pick: string | undefined): void {
    pickBox.replaceChildren();
    const result = attempt(() => engine.importVsix(bytes, skin, pick), fail);
    if (result !== null) show(result);
  }

  async function fromVsix(f: File): Promise<void> {
    const bytes = new Uint8Array(await f.arrayBuffer());
    const themes = attempt(() => engine.vsixThemes(bytes), fail);
    if (themes === null) return;
    if (themes.length === 0) {
      fail('This extension has no colour theme.');
      return;
    }
    if (themes.length === 1) {
      takeVsix(bytes, themes[0]?.path);
      return;
    }
    replace(pickBox,
      h('p.hint', null, `${themes.length} colour themes in this extension. Pick one:`),
      ...themes.map((t) => button(`${t.label} (${t.ui_theme})`, () => takeVsix(bytes, t.path), { kind: 'ghost' })));
  }

  const head = h(
    'header.page-head',
    null,
    h('div.kicker', null, h('a', { href: '#/appearance' }, 'Appearance'), h('span', null, '/'), h('span', null, 'Import')),
    h(
      'div.page-head-row',
      null,
      h('h1', null, 'Import a theme'),
      h('span.sp'),
      h('div.page-actions', null, button('Back', back, { kind: 'ghost' })),
    ),
    h('p.lede', null, 'Drop an .apztheme.json file, or a VS Code colour theme as .json or .vsix. Nothing is saved until you apply it.'),
  );

  replace(root, head,
    card('Source', h(
      'div.stack',
      null,
      h('div.field', null, h('span.field-l', null, 'Skin for VS Code themes'), skinSelect),
      h('div.grid2', null,
        dropZone('Theme file', 'An .apztheme.json file, from this console or another install',
          '.apztheme.json,.json,application/json', (f) => void fromApz(f)),
        dropZone('VS Code theme', 'A colour theme as .json, or an extension as .vsix',
          '.json,.vsix,application/json', (f) => void (f.name.toLowerCase().endsWith('.vsix') ? fromVsix(f) : fromVscodeJson(f)))),
      pickBox,
    )),
    status,
    resultBox);
}

// ---- Routing ----------------------------------------------------------------------------------------------------

function notice(root: HTMLElement, text: string, href?: string, label?: string): void {
  replace(root, h('div.empty', null,
    h('b', null, text),
    href ? h('a.btn.sm', { href }, label ?? 'Back') : null));
}

function route(root: HTMLElement, engine: ThemeWasm, doc: AppearanceDoc, params: string[]): void {
  const [head = '', arg = ''] = params;
  if (head === 'import') {
    importer(root, engine, doc);
    return;
  }
  if (head !== 'edit') {
    notice(root, 'This address is not a theme page.', '#/appearance', 'Back to appearance');
    return;
  }
  const id = arg.split('?')[0] ?? '';
  if (id === '__import') {
    if (pendingImport === null) {
      notice(root, 'Nothing to edit yet. Import a theme first.', '#/appearance/import', 'Go to import');
      return;
    }
    editor(root, engine, clone(pendingImport));
    return;
  }
  if (id === 'new') {
    editor(root, engine, copyOf(doc, fromParam()));
    return;
  }
  const saved = doc.themes.find((t) => t.id === id);
  if (saved !== undefined) {
    editor(root, engine, clone(saved) as unknown as ThemeFile);
    return;
  }
  if (findBuiltin(id) !== undefined) {
    // A built-in theme is copied, never edited in place.
    location.hash = `#/appearance/edit/new?from=${encodeURIComponent(id)}`;
    return;
  }
  notice(root, `No theme called "${id}" is saved.`, '#/appearance', 'Back to appearance');
}

/**
 * Mounts the theme editor or the import screen into `root`. `params` are the route parts after "appearance":
 * ['edit', '<id>'], ['edit', 'new'] (with `?from=<id>` in the address), or ['import']. The returned function unmounts.
 */
export function mountEditor(root: HTMLElement, params: string[]): () => void {
  let alive = true;
  root.classList.add('te-root');
  replace(root, h('p.hint', null, 'Loading the theme engine...'));
  void (async () => {
    try {
      const [engine, doc] = await Promise.all([loadTheme(), getAppearance()]);
      if (alive) route(root, engine, doc, params);
    } catch (err) {
      if (alive) replace(root, h('p.notice', { role: 'alert' }, `The theme editor could not start: ${errorText(err)}`));
    }
  })();
  return () => {
    alive = false;
    root.classList.remove('te-root');
    root.replaceChildren();
  };
}
