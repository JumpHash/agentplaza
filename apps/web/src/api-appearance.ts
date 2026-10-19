// The appearance document: the owner's theme choice and the themes saved on the hub. The console reads it at start,
// writes the choice when the owner changes it, and saves or deletes themes from the theme editor.

import { call } from './api';

/** The owner's mode: follow the system, or force light or dark. */
export type AppearanceMode = 'system' | 'light' | 'dark';

export type Density = 'compact' | 'default' | 'comfortable';

/** A background is named by its id, or given inline as an object the hub keeps as written. */
export type BackgroundRef = string | Record<string, unknown>;

/** The owner's look. `chosen` is false until the owner picks something. */
export interface Appearance {
  theme: string;
  skin?: string;
  background?: BackgroundRef;
  mode: AppearanceMode;
  density: Density;
  code_size: number;
  chosen: boolean;
}

/** One token-to-hex map for a mode, for example `{"paper": "#f6f4ef"}`. */
export type PaletteMap = Record<string, string>;

/** A theme the owner saved on the hub. `schema` is 1 for this version of the file. */
export interface ThemeFile {
  schema: 1;
  id: string;
  name: string;
  about?: string;
  skin: string;
  palette: { light?: PaletteMap; dark?: PaletteMap };
  background?: BackgroundRef;
  overrides?: Record<string, unknown>;
  source?: string;
}

/** What the hub answers for every appearance call: the choice and the saved themes. */
export interface AppearanceDoc {
  choice: Appearance;
  themes: ThemeFile[];
}

/** The hub's appearance document. */
export const getAppearance = (): Promise<AppearanceDoc> =>
  call<AppearanceDoc>('GET', '/v1/appearance');

/** Stores the owner's choice on the hub and returns the document it now holds. */
export const putAppearance = (a: Appearance): Promise<AppearanceDoc> =>
  call<AppearanceDoc>('PUT', '/v1/appearance', a);

/** Saves a theme on the hub (creating or replacing it) and returns the document. */
export const saveTheme = (file: ThemeFile): Promise<AppearanceDoc> =>
  call<AppearanceDoc>('PUT', `/v1/appearance/themes/${encodeURIComponent(file.id)}`, file);

/** Deletes a saved theme from the hub and returns the document. */
export const deleteTheme = (id: string): Promise<AppearanceDoc> =>
  call<AppearanceDoc>('DELETE', `/v1/appearance/themes/${encodeURIComponent(id)}`);
