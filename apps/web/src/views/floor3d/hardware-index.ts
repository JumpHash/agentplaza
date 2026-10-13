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
import partAppleLogo from '../../../../../assets/hardware/parts/apple-logo.json';
import partAtx24 from '../../../../../assets/hardware/parts/atx24.json';
import partEps8 from '../../../../../assets/hardware/parts/eps8.json';
import partPcie8pin from '../../../../../assets/hardware/parts/pcie-8pin.json';
import part12vhpwr from '../../../../../assets/hardware/parts/12vhpwr.json';
import partSataDual from '../../../../../assets/hardware/parts/sata-dual.json';
import partUsb3Header from '../../../../../assets/hardware/parts/usb3-header.json';
import partUsb2Header from '../../../../../assets/hardware/parts/usb2-header.json';
import partFanHeader4pin from '../../../../../assets/hardware/parts/fan-header-4pin.json';
import partHeader2x10 from '../../../../../assets/hardware/parts/header-2x10.json';
import partArgbHeader from '../../../../../assets/hardware/parts/argb-header.json';
import partCr2032 from '../../../../../assets/hardware/parts/cr2032.json';
import partBiosChip from '../../../../../assets/hardware/parts/bios-chip.json';
import partAudioCodec from '../../../../../assets/hardware/parts/audio-codec.json';
import partAudioCaps from '../../../../../assets/hardware/parts/audio-caps.json';
import partQcode from '../../../../../assets/hardware/parts/qcode.json';
import partDebugLeds from '../../../../../assets/hardware/parts/debug-leds.json';
import partPowerButton from '../../../../../assets/hardware/parts/power-button.json';
import partDimmSlot from '../../../../../assets/hardware/parts/dimm-slot.json';
import partPcieX16Slot from '../../../../../assets/hardware/parts/pcie-x16-slot.json';
import partPcieX1Slot from '../../../../../assets/hardware/parts/pcie-x1-slot.json';
import partM2Heatsink from '../../../../../assets/hardware/parts/m2-heatsink.json';
import partVrmHeatsink from '../../../../../assets/hardware/parts/vrm-heatsink.json';
import partBgaIc from '../../../../../assets/hardware/parts/bga-ic.json';
import partSocketLga1700 from '../../../../../assets/hardware/parts/socket-lga1700.json';
import partSocketAm5 from '../../../../../assets/hardware/parts/socket-am5.json';
import partCpuIntelIhs from '../../../../../assets/hardware/parts/cpu-intel-ihs.json';
import partCpuAmdIhs from '../../../../../assets/hardware/parts/cpu-amd-ihs.json';
import partAioPump from '../../../../../assets/hardware/parts/aio-pump.json';
import partChipsetHeatsink from '../../../../../assets/hardware/parts/chipset-heatsink.json';
import partIoShroud from '../../../../../assets/hardware/parts/io-shroud.json';
import partUsbAStack from '../../../../../assets/hardware/parts/usb-a-stack.json';
import partJackStack from '../../../../../assets/hardware/parts/jack-stack.json';
import partWifiModule from '../../../../../assets/hardware/parts/wifi-module.json';
import partShieldCan from '../../../../../assets/hardware/parts/shield-can.json';
import partBatteryCell from '../../../../../assets/hardware/parts/battery-cell.json';
import partBlowerFan from '../../../../../assets/hardware/parts/blower-fan.json';
import partHeatpipeStraight from '../../../../../assets/hardware/parts/heatpipe-straight.json';
import partCameraModule from '../../../../../assets/hardware/parts/camera-module.json';
import partSoicIc from '../../../../../assets/hardware/parts/soic-ic.json';
import partQfnIc from '../../../../../assets/hardware/parts/qfn-ic.json';
import partCrystal from '../../../../../assets/hardware/parts/crystal.json';
import partSmd0402Field from '../../../../../assets/hardware/parts/smd-0402-field.json';
import partChoke from '../../../../../assets/hardware/parts/choke.json';
import partMosfet from '../../../../../assets/hardware/parts/mosfet.json';
import partFfcConnector from '../../../../../assets/hardware/parts/ffc-connector.json';

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
  'parts/apple-logo': model(partAppleLogo),
  'parts/atx24': model(partAtx24),
  'parts/eps8': model(partEps8),
  'parts/pcie-8pin': model(partPcie8pin),
  'parts/12vhpwr': model(part12vhpwr),
  'parts/sata-dual': model(partSataDual),
  'parts/usb3-header': model(partUsb3Header),
  'parts/usb2-header': model(partUsb2Header),
  'parts/fan-header-4pin': model(partFanHeader4pin),
  'parts/header-2x10': model(partHeader2x10),
  'parts/argb-header': model(partArgbHeader),
  'parts/dimm-slot': model(partDimmSlot),
  'parts/pcie-x16-slot': model(partPcieX16Slot),
  'parts/pcie-x1-slot': model(partPcieX1Slot),
  'parts/m2-heatsink': model(partM2Heatsink),
  'parts/vrm-heatsink': model(partVrmHeatsink),
  'parts/cr2032': model(partCr2032),
  'parts/bios-chip': model(partBiosChip),
  'parts/audio-codec': model(partAudioCodec),
  'parts/audio-caps': model(partAudioCaps),
  'parts/qcode': model(partQcode),
  'parts/debug-leds': model(partDebugLeds),
  'parts/power-button': model(partPowerButton),
  'parts/socket-lga1700': model(partSocketLga1700),
  'parts/socket-am5': model(partSocketAm5),
  'parts/cpu-intel-ihs': model(partCpuIntelIhs),
  'parts/cpu-amd-ihs': model(partCpuAmdIhs),
  'parts/aio-pump': model(partAioPump),
  'parts/chipset-heatsink': model(partChipsetHeatsink),
  'parts/io-shroud': model(partIoShroud),
  'parts/usb-a-stack': model(partUsbAStack),
  'parts/jack-stack': model(partJackStack),
  'parts/wifi-module': model(partWifiModule),
  'parts/shield-can': model(partShieldCan),
  'parts/battery-cell': model(partBatteryCell),
  'parts/blower-fan': model(partBlowerFan),
  'parts/heatpipe-straight': model(partHeatpipeStraight),
  'parts/camera-module': model(partCameraModule),
  'parts/bga-ic': model(partBgaIc),
  'parts/soic-ic': model(partSoicIc),
  'parts/qfn-ic': model(partQfnIc),
  'parts/crystal': model(partCrystal),
  'parts/smd-0402-field': model(partSmd0402Field),
  'parts/choke': model(partChoke),
  'parts/mosfet': model(partMosfet),
  'parts/ffc-connector': model(partFfcConnector),
};
