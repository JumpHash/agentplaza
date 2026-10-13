// Every hardware model file under assets/hardware/, imported statically so the bundle carries them.
// A new file goes in here (its key is its path without .json); the test "every hardware file is in
// the index" fails until it does. The files are validated by `npm run check` (see hardware.ts).

import type { Model, Palette } from './hardware';
import paletteFile from '../../../../../assets/hardware/palette.json';
import exteriorAppleDesktop from '../../../../../assets/hardware/exterior/apple-desktop.json';
import exteriorAppleLaptop from '../../../../../assets/hardware/exterior/apple-laptop.json';
import boardAppleSoc from '../../../../../assets/hardware/board/apple-soc.json';
import exteriorEdge from '../../../../../assets/hardware/exterior/edge.json';
import exteriorLaptop from '../../../../../assets/hardware/exterior/laptop.json';
import exteriorPhone from '../../../../../assets/hardware/exterior/phone.json';
import exteriorServer from '../../../../../assets/hardware/exterior/server.json';
import exteriorWorkstation from '../../../../../assets/hardware/exterior/workstation.json';
import boardEdge from '../../../../../assets/hardware/board/edge.json';
import boardLaptop from '../../../../../assets/hardware/board/laptop.json';
import boardPhone from '../../../../../assets/hardware/board/phone.json';
import boardServer from '../../../../../assets/hardware/board/server.json';
import boardWorkstation from '../../../../../assets/hardware/board/workstation.json';

// The JSON is checked by validate() (a test runs it on every file), not by the TypeScript types.
const model = (raw: unknown): Model => raw as Model;

/** The palette file as written (schema 1, colors). */
export const PALETTE_FILE: unknown = paletteFile;
/** The named colours the models use. */
export const PALETTE = (paletteFile as unknown as { colors: Palette }).colors;

export const MODELS: Record<string, Model> = {
  'exterior/apple-desktop': model(exteriorAppleDesktop),
  'exterior/apple-laptop': model(exteriorAppleLaptop),
  'board/apple-soc': model(boardAppleSoc),
  'exterior/edge': model(exteriorEdge),
  'exterior/laptop': model(exteriorLaptop),
  'exterior/phone': model(exteriorPhone),
  'exterior/server': model(exteriorServer),
  'exterior/workstation': model(exteriorWorkstation),
  'board/edge': model(boardEdge),
  'board/laptop': model(boardLaptop),
  'board/phone': model(boardPhone),
  'board/server': model(boardServer),
  'board/workstation': model(boardWorkstation),
};
