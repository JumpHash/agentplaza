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
import partGpuPcie from '../../../../../assets/hardware/parts/gpu-pcie.json';
import partDimm from '../../../../../assets/hardware/parts/dimm.json';
import partSodimm from '../../../../../assets/hardware/parts/sodimm.json';
import partM2Ssd from '../../../../../assets/hardware/parts/m2-ssd.json';
import partFan120 from '../../../../../assets/hardware/parts/fan-120.json';
import partHeatsinkTower from '../../../../../assets/hardware/parts/heatsink-tower.json';
import partCapBank from '../../../../../assets/hardware/parts/cap-bank.json';
import partVrmBank from '../../../../../assets/hardware/parts/vrm-bank.json';
import partNic from '../../../../../assets/hardware/parts/nic.json';
import partUsbC from '../../../../../assets/hardware/parts/usb-c.json';
import partRj45 from '../../../../../assets/hardware/parts/rj45.json';

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
  'parts/gpu-pcie': model(partGpuPcie),
  'parts/dimm': model(partDimm),
  'parts/sodimm': model(partSodimm),
  'parts/m2-ssd': model(partM2Ssd),
  'parts/fan-120': model(partFan120),
  'parts/heatsink-tower': model(partHeatsinkTower),
  'parts/cap-bank': model(partCapBank),
  'parts/vrm-bank': model(partVrmBank),
  'parts/nic': model(partNic),
  'parts/usb-c': model(partUsbC),
  'parts/rj45': model(partRj45),
};
