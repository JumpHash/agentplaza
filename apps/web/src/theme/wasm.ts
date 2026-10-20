// The theme engine (crates/apz-theme-web), loaded on first use. The console never fetches it at start: nothing imports this
// module then, and `loadTheme()` imports the glue (and so the wasm) only when the Appearance screen first needs it. Every
// export takes and returns JSON text; the wrapper here gives callers the shapes below and throws on a refusal.

/** Light or dark. */
export type Mode = 'light' | 'dark';

/** A palette as a file writes it: token name to colour text (`#rrggbb` or `#rrggbbaa`). */
export type PaletteText = Record<string, string>;

/** A `.apztheme.json` file. The background and skin are carried as the engine writes them. */
export interface ThemeFile {
  schema: number;
  id: string;
  name: string;
  about?: string;
  skin: string;
  palette: { light?: PaletteText; dark?: PaletteText };
  background?: unknown;
  overrides?: { light?: PaletteText; dark?: PaletteText };
  source?: string;
}

/** One colour the contrast pass moved, or one that still fails. */
export interface Adjusted {
  token: string;
  from: string;
  to: string;
  against: string;
  ratio: number;
  min: number;
}

/** What resolving did: how many colours came from the file and how many were derived, and what the pass moved. */
export interface Report {
  taken: number;
  derived: number;
  adjusted: Adjusted[];
  failing: Adjusted[];
  /** Only on an import's report: "41 colours taken, 19 derived, 3 adjusted for contrast". */
  summary?: string;
}

/** A theme resolved for one skin and mode. Tokens are colour text; the skin and background are as the engine writes them. */
export interface Resolved {
  id: string;
  name: string;
  skin: Record<string, unknown>;
  mode: Mode;
  tokens: Record<string, string>;
  background: unknown;
  report: Report;
}

export interface ResolveResult {
  resolved: Resolved;
  css: string;
}

export interface ValidateResult {
  ok: boolean;
  problems: string[];
}

export interface ContrastRow {
  token: string;
  against: string;
  ratio: number;
  min: number;
  ok: boolean;
}

/** One colour theme in a `.vsix` archive. */
export interface VsixTheme {
  label: string;
  ui_theme: string;
  path: string;
}

export interface ImportResult {
  file: ThemeFile;
  name: string;
  mode: Mode;
  report: Report;
}

/** The glue's exports, as wasm-bindgen generates them (`--target web`). Each returns JSON text. */
export interface ThemeGlue {
  default(): Promise<unknown>;
  resolve(file: string, skin: string, mode: string): string;
  validate(file: string): string;
  contrast(file: string, skin: string, mode: string): string;
  fix(file: string, skin: string, mode: string, token: string): string;
  import_vscode(text: string, skin: string): string;
  vsix_themes(bytes: Uint8Array): string;
  import_vsix(bytes: Uint8Array, pick: string | undefined, skin: string): string;
  backdrop_uniforms(bg: string, tokens: string, w: number, h: number, scale: number, time: number, px: number, py: number, hasPointer: boolean): Float32Array;
  backdrop_animates(bg: string): boolean;
}

/** The theme engine, typed. A refusal (a file that does not parse, an unknown skin or token) throws an Error with the
 * engine's message. */
export interface ThemeWasm {
  resolve(file: ThemeFile, skin: string, mode: Mode): ResolveResult;
  validate(file: ThemeFile): ValidateResult;
  contrast(file: ThemeFile, skin: string, mode: Mode): ContrastRow[];
  fix(file: ThemeFile, skin: string, mode: Mode, token: string): ThemeFile;
  importVscode(text: string, skin: string): ImportResult;
  vsixThemes(bytes: Uint8Array): VsixTheme[];
  importVsix(bytes: Uint8Array, skin: string, pick?: string): ImportResult;
  /** The backdrop's 40 uniform floats (ten vec4s, the shader's order) for a background spec on a palette. */
  backdropUniforms(bg: unknown, tokens: Record<string, string>, size: BackdropSize, pointer: BackdropPoint | null): Float32Array;
  /** Whether a background spec moves (slow motion or the pointer light). */
  backdropAnimates(bg: unknown): boolean;
}

/** The size and time of one backdrop frame: the target in device px, device px per css px, and seconds. */
export interface BackdropSize {
  w: number;
  h: number;
  scale: number;
  time: number;
}

/** A pointer position in css px. */
export interface BackdropPoint {
  x: number;
  y: number;
}

/** Runs a glue call whose refusal is a thrown string, so the caller gets an Error with the engine's message. */
function throwing<T>(call: () => T): T {
  try {
    return call();
  } catch (err) {
    throw err instanceof Error ? err : new Error(String(err));
  }
}

/** Reads one export's JSON text, throwing the engine's message when the reply is an error object. */
function read<T>(text: string): T {
  const value = JSON.parse(text) as T & { error?: unknown };
  if (value !== null && typeof value === 'object' && typeof value.error === 'string') {
    throw new Error(value.error);
  }
  return value as T;
}

/** The typed wrapper over a glue. Exported so the wrapping is testable without the wasm. */
export function wrapTheme(glue: ThemeGlue): ThemeWasm {
  return {
    resolve: (file, skin, mode) =>
      read<ResolveResult>(glue.resolve(JSON.stringify(file), skin, mode)),
    validate: (file) => read<ValidateResult>(glue.validate(JSON.stringify(file))),
    contrast: (file, skin, mode) =>
      read<ContrastRow[]>(glue.contrast(JSON.stringify(file), skin, mode)),
    fix: (file, skin, mode, token) =>
      read<ThemeFile>(glue.fix(JSON.stringify(file), skin, mode, token)),
    importVscode: (text, skin) => read<ImportResult>(glue.import_vscode(text, skin)),
    vsixThemes: (bytes) => read<VsixTheme[]>(glue.vsix_themes(bytes)),
    importVsix: (bytes, skin, pick) =>
      read<ImportResult>(glue.import_vsix(bytes, pick, skin)),
    backdropUniforms: (bg, tokens, size, pointer) =>
      throwing(() =>
        glue.backdrop_uniforms(
          JSON.stringify(bg),
          JSON.stringify(tokens),
          size.w,
          size.h,
          size.scale,
          size.time,
          pointer?.x ?? 0,
          pointer?.y ?? 0,
          pointer !== null,
        ),
      ),
    backdropAnimates: (bg) => throwing(() => glue.backdrop_animates(JSON.stringify(bg))),
  };
}

/** The glue's file name at the dist root: esbuild replaces this at build time (null when the theme crate is missing). */
declare const __APZ_THEME_JS__: string | null;

let loading: Promise<ThemeWasm> | null = null;

/**
 * The theme engine, loaded and initialised on the first call; later calls share it. Rejects when this build has no
 * theme engine or the wasm does not load; a failed load is retried by the next call.
 */
export function loadTheme(): Promise<ThemeWasm> {
  if (!loading) {
    loading = (async () => {
      const name = typeof __APZ_THEME_JS__ === 'string' ? __APZ_THEME_JS__ : '';
      if (name === '') throw new Error('this build has no theme engine');
      const glueUrl = new URL('./' + name, document.baseURI).href;
      const glue = (await import(/* @vite-ignore */ glueUrl)) as ThemeGlue;
      await glue.default();
      return wrapTheme(glue);
    })();
    loading.catch(() => {
      loading = null;
    });
  }
  return loading;
}
