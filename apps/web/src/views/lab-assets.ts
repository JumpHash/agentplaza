// The asset registries the labs list their options from: the robot index, its species, tools and clips, and the
// hardware models. Each file is imported statically (the bundle carries it, as the three.js labs did). A new file goes
// in here; the lists follow it.

import robotIndex from '../../../../assets/robots/index.json';
import robotClips from '../../../../assets/robots/clips.json';
import speciesAstro from '../../../../assets/robots/species/astro.json';
import speciesBee from '../../../../assets/robots/species/bee.json';
import speciesBunny from '../../../../assets/robots/species/bunny.json';
import speciesCan from '../../../../assets/robots/species/can.json';
import speciesCrab from '../../../../assets/robots/species/crab.json';
import speciesCube from '../../../../assets/robots/species/cube.json';
import speciesDuck from '../../../../assets/robots/species/duck.json';
import speciesFrog from '../../../../assets/robots/species/frog.json';
import speciesGhost from '../../../../assets/robots/species/ghost.json';
import speciesHover from '../../../../assets/robots/species/hover.json';
import speciesJelly from '../../../../assets/robots/species/jelly.json';
import speciesKitty from '../../../../assets/robots/species/kitty.json';
import speciesLadybug from '../../../../assets/robots/species/ladybug.json';
import speciesLamp from '../../../../assets/robots/species/lamp.json';
import speciesLifty from '../../../../assets/robots/species/lifty.json';
import speciesMole from '../../../../assets/robots/species/mole.json';
import speciesMushroom from '../../../../assets/robots/species/mushroom.json';
import speciesOcto from '../../../../assets/robots/species/octo.json';
import speciesOracle from '../../../../assets/robots/species/oracle.json';
import speciesOwl from '../../../../assets/robots/species/owl.json';
import speciesPenguin from '../../../../assets/robots/species/penguin.json';
import speciesPogo from '../../../../assets/robots/species/pogo.json';
import speciesPup from '../../../../assets/robots/species/pup.json';
import speciesRover from '../../../../assets/robots/species/rover.json';
import speciesSnail from '../../../../assets/robots/species/snail.json';
import speciesSprout from '../../../../assets/robots/species/sprout.json';
import speciesTeapot from '../../../../assets/robots/species/teapot.json';
import speciesTurtle from '../../../../assets/robots/species/turtle.json';
import speciesTv from '../../../../assets/robots/species/tv.json';
import speciesUfo from '../../../../assets/robots/species/ufo.json';
import speciesUnicycle from '../../../../assets/robots/species/unicycle.json';
import toolAbacus from '../../../../assets/robots/tools/abacus.json';
import toolAntennaWand from '../../../../assets/robots/tools/antenna-wand.json';
import toolBall from '../../../../assets/robots/tools/ball.json';
import toolBaton from '../../../../assets/robots/tools/baton.json';
import toolBlueprint from '../../../../assets/robots/tools/blueprint.json';
import toolBook from '../../../../assets/robots/tools/book.json';
import toolBookStack from '../../../../assets/robots/tools/book-stack.json';
import toolBroom from '../../../../assets/robots/tools/broom.json';
import toolCableSpool from '../../../../assets/robots/tools/cable-spool.json';
import toolCalculator from '../../../../assets/robots/tools/calculator.json';
import toolClipboard from '../../../../assets/robots/tools/clipboard.json';
import toolCoffeeMug from '../../../../assets/robots/tools/coffee-mug.json';
import toolCoinBag from '../../../../assets/robots/tools/coin-bag.json';
import toolCrate from '../../../../assets/robots/tools/crate.json';
import toolDnaVial from '../../../../assets/robots/tools/dna-vial.json';
import toolExtinguisher from '../../../../assets/robots/tools/extinguisher.json';
import toolFeatherDuster from '../../../../assets/robots/tools/feather-duster.json';
import toolFlag from '../../../../assets/robots/tools/flag.json';
import toolFlashlight from '../../../../assets/robots/tools/flashlight.json';
import toolFlask from '../../../../assets/robots/tools/flask.json';
import toolGavel from '../../../../assets/robots/tools/gavel.json';
import toolHammer from '../../../../assets/robots/tools/hammer.json';
import toolLedger from '../../../../assets/robots/tools/ledger.json';
import toolMagnifier from '../../../../assets/robots/tools/magnifier.json';
import toolMarker from '../../../../assets/robots/tools/marker.json';
import toolMegaphone from '../../../../assets/robots/tools/megaphone.json';
import toolMultimeter from '../../../../assets/robots/tools/multimeter.json';
import toolNotebook from '../../../../assets/robots/tools/notebook.json';
import toolPadlock from '../../../../assets/robots/tools/padlock.json';
import toolPencil from '../../../../assets/robots/tools/pencil.json';
import toolRedPen from '../../../../assets/robots/tools/red-pen.json';
import toolScrewdriver from '../../../../assets/robots/tools/screwdriver.json';
import toolScroll from '../../../../assets/robots/tools/scroll.json';
import toolSeedling from '../../../../assets/robots/tools/seedling.json';
import toolSolderingIron from '../../../../assets/robots/tools/soldering-iron.json';
import toolStamp from '../../../../assets/robots/tools/stamp.json';
import toolStickyNotes from '../../../../assets/robots/tools/sticky-notes.json';
import toolTSquare from '../../../../assets/robots/tools/t-square.json';
import toolTapeMeasure from '../../../../assets/robots/tools/tape-measure.json';
import toolTestTube from '../../../../assets/robots/tools/test-tube.json';
import toolWateringCan from '../../../../assets/robots/tools/watering-can.json';
import toolWrench from '../../../../assets/robots/tools/wrench.json';
import hwBoardAppleSoc from '../../../../assets/hardware/board/apple-soc.json';
import hwBoardEdge from '../../../../assets/hardware/board/edge.json';
import hwBoardLaptop from '../../../../assets/hardware/board/laptop.json';
import hwBoardPhone from '../../../../assets/hardware/board/phone.json';
import hwBoardServer from '../../../../assets/hardware/board/server.json';
import hwBoardWorkstation from '../../../../assets/hardware/board/workstation.json';
import hwExteriorAppleDesktop from '../../../../assets/hardware/exterior/apple-desktop.json';
import hwExteriorAppleLaptop from '../../../../assets/hardware/exterior/apple-laptop.json';
import hwExteriorEdge from '../../../../assets/hardware/exterior/edge.json';
import hwExteriorLaptop from '../../../../assets/hardware/exterior/laptop.json';
import hwExteriorPhone from '../../../../assets/hardware/exterior/phone.json';
import hwExteriorServer from '../../../../assets/hardware/exterior/server.json';
import hwExteriorWorkstation from '../../../../assets/hardware/exterior/workstation.json';
import hwParts12Vhpwr from '../../../../assets/hardware/parts/12vhpwr.json';
import hwPartsAioPump from '../../../../assets/hardware/parts/aio-pump.json';
import hwPartsAppleLogo from '../../../../assets/hardware/parts/apple-logo.json';
import hwPartsArgbHeader from '../../../../assets/hardware/parts/argb-header.json';
import hwPartsAtx24 from '../../../../assets/hardware/parts/atx24.json';
import hwPartsAudioCaps from '../../../../assets/hardware/parts/audio-caps.json';
import hwPartsAudioCodec from '../../../../assets/hardware/parts/audio-codec.json';
import hwPartsBatteryCell from '../../../../assets/hardware/parts/battery-cell.json';
import hwPartsBgaIc from '../../../../assets/hardware/parts/bga-ic.json';
import hwPartsBiosChip from '../../../../assets/hardware/parts/bios-chip.json';
import hwPartsBlowerFan from '../../../../assets/hardware/parts/blower-fan.json';
import hwPartsCameraModule from '../../../../assets/hardware/parts/camera-module.json';
import hwPartsCapBank from '../../../../assets/hardware/parts/cap-bank.json';
import hwPartsChipsetHeatsink from '../../../../assets/hardware/parts/chipset-heatsink.json';
import hwPartsChoke from '../../../../assets/hardware/parts/choke.json';
import hwPartsCpuAmdIhs from '../../../../assets/hardware/parts/cpu-amd-ihs.json';
import hwPartsCpuIntelIhs from '../../../../assets/hardware/parts/cpu-intel-ihs.json';
import hwPartsCr2032 from '../../../../assets/hardware/parts/cr2032.json';
import hwPartsCrystal from '../../../../assets/hardware/parts/crystal.json';
import hwPartsDebugLeds from '../../../../assets/hardware/parts/debug-leds.json';
import hwPartsDimm from '../../../../assets/hardware/parts/dimm.json';
import hwPartsDimmSlot from '../../../../assets/hardware/parts/dimm-slot.json';
import hwPartsEps8 from '../../../../assets/hardware/parts/eps8.json';
import hwPartsFan120 from '../../../../assets/hardware/parts/fan-120.json';
import hwPartsFanHeader4Pin from '../../../../assets/hardware/parts/fan-header-4pin.json';
import hwPartsFfcConnector from '../../../../assets/hardware/parts/ffc-connector.json';
import hwPartsGpuPcie from '../../../../assets/hardware/parts/gpu-pcie.json';
import hwPartsHeader2X10 from '../../../../assets/hardware/parts/header-2x10.json';
import hwPartsHeatpipeStraight from '../../../../assets/hardware/parts/heatpipe-straight.json';
import hwPartsHeatsinkTower from '../../../../assets/hardware/parts/heatsink-tower.json';
import hwPartsIoShroud from '../../../../assets/hardware/parts/io-shroud.json';
import hwPartsJackStack from '../../../../assets/hardware/parts/jack-stack.json';
import hwPartsM2Heatsink from '../../../../assets/hardware/parts/m2-heatsink.json';
import hwPartsM2Ssd from '../../../../assets/hardware/parts/m2-ssd.json';
import hwPartsMosfet from '../../../../assets/hardware/parts/mosfet.json';
import hwPartsNic from '../../../../assets/hardware/parts/nic.json';
import hwPartsPcie8Pin from '../../../../assets/hardware/parts/pcie-8pin.json';
import hwPartsPcieX1Slot from '../../../../assets/hardware/parts/pcie-x1-slot.json';
import hwPartsPcieX16Slot from '../../../../assets/hardware/parts/pcie-x16-slot.json';
import hwPartsPowerButton from '../../../../assets/hardware/parts/power-button.json';
import hwPartsQcode from '../../../../assets/hardware/parts/qcode.json';
import hwPartsQfnIc from '../../../../assets/hardware/parts/qfn-ic.json';
import hwPartsRj45 from '../../../../assets/hardware/parts/rj45.json';
import hwPartsSataDual from '../../../../assets/hardware/parts/sata-dual.json';
import hwPartsShieldCan from '../../../../assets/hardware/parts/shield-can.json';
import hwPartsSmd0402Field from '../../../../assets/hardware/parts/smd-0402-field.json';
import hwPartsSocketAm5 from '../../../../assets/hardware/parts/socket-am5.json';
import hwPartsSocketLga1700 from '../../../../assets/hardware/parts/socket-lga1700.json';
import hwPartsSodimm from '../../../../assets/hardware/parts/sodimm.json';
import hwPartsSoicIc from '../../../../assets/hardware/parts/soic-ic.json';
import hwPartsUsbAStack from '../../../../assets/hardware/parts/usb-a-stack.json';
import hwPartsUsbC from '../../../../assets/hardware/parts/usb-c.json';
import hwPartsUsb2Header from '../../../../assets/hardware/parts/usb2-header.json';
import hwPartsUsb3Header from '../../../../assets/hardware/parts/usb3-header.json';
import hwPartsVrmBank from '../../../../assets/hardware/parts/vrm-bank.json';
import hwPartsVrmHeatsink from '../../../../assets/hardware/parts/vrm-heatsink.json';
import hwPartsWifiModule from '../../../../assets/hardware/parts/wifi-module.json';

/** The robot file a species or tool JSON is: its own clips (species) and its grip (tools). */
interface RobotFile {
  clips?: Record<string, unknown>;
  grip?: string;
}

const SPECIES: Record<string, RobotFile> = {
  'astro': speciesAstro as RobotFile,
  'bee': speciesBee as RobotFile,
  'bunny': speciesBunny as RobotFile,
  'can': speciesCan as RobotFile,
  'crab': speciesCrab as RobotFile,
  'cube': speciesCube as RobotFile,
  'duck': speciesDuck as RobotFile,
  'frog': speciesFrog as RobotFile,
  'ghost': speciesGhost as RobotFile,
  'hover': speciesHover as RobotFile,
  'jelly': speciesJelly as RobotFile,
  'kitty': speciesKitty as RobotFile,
  'ladybug': speciesLadybug as RobotFile,
  'lamp': speciesLamp as RobotFile,
  'lifty': speciesLifty as RobotFile,
  'mole': speciesMole as RobotFile,
  'mushroom': speciesMushroom as RobotFile,
  'octo': speciesOcto as RobotFile,
  'oracle': speciesOracle as RobotFile,
  'owl': speciesOwl as RobotFile,
  'penguin': speciesPenguin as RobotFile,
  'pogo': speciesPogo as RobotFile,
  'pup': speciesPup as RobotFile,
  'rover': speciesRover as RobotFile,
  'snail': speciesSnail as RobotFile,
  'sprout': speciesSprout as RobotFile,
  'teapot': speciesTeapot as RobotFile,
  'turtle': speciesTurtle as RobotFile,
  'tv': speciesTv as RobotFile,
  'ufo': speciesUfo as RobotFile,
  'unicycle': speciesUnicycle as RobotFile,
};

const TOOLS: Record<string, RobotFile> = {
  'abacus': toolAbacus as RobotFile,
  'antenna-wand': toolAntennaWand as RobotFile,
  'ball': toolBall as RobotFile,
  'baton': toolBaton as RobotFile,
  'blueprint': toolBlueprint as RobotFile,
  'book': toolBook as RobotFile,
  'book-stack': toolBookStack as RobotFile,
  'broom': toolBroom as RobotFile,
  'cable-spool': toolCableSpool as RobotFile,
  'calculator': toolCalculator as RobotFile,
  'clipboard': toolClipboard as RobotFile,
  'coffee-mug': toolCoffeeMug as RobotFile,
  'coin-bag': toolCoinBag as RobotFile,
  'crate': toolCrate as RobotFile,
  'dna-vial': toolDnaVial as RobotFile,
  'extinguisher': toolExtinguisher as RobotFile,
  'feather-duster': toolFeatherDuster as RobotFile,
  'flag': toolFlag as RobotFile,
  'flashlight': toolFlashlight as RobotFile,
  'flask': toolFlask as RobotFile,
  'gavel': toolGavel as RobotFile,
  'hammer': toolHammer as RobotFile,
  'ledger': toolLedger as RobotFile,
  'magnifier': toolMagnifier as RobotFile,
  'marker': toolMarker as RobotFile,
  'megaphone': toolMegaphone as RobotFile,
  'multimeter': toolMultimeter as RobotFile,
  'notebook': toolNotebook as RobotFile,
  'padlock': toolPadlock as RobotFile,
  'pencil': toolPencil as RobotFile,
  'red-pen': toolRedPen as RobotFile,
  'screwdriver': toolScrewdriver as RobotFile,
  'scroll': toolScroll as RobotFile,
  'seedling': toolSeedling as RobotFile,
  'soldering-iron': toolSolderingIron as RobotFile,
  'stamp': toolStamp as RobotFile,
  'sticky-notes': toolStickyNotes as RobotFile,
  't-square': toolTSquare as RobotFile,
  'tape-measure': toolTapeMeasure as RobotFile,
  'test-tube': toolTestTube as RobotFile,
  'watering-can': toolWateringCan as RobotFile,
  'wrench': toolWrench as RobotFile,
};

const HARDWARE: Record<string, unknown> = {
  'board/apple-soc': hwBoardAppleSoc,
  'board/edge': hwBoardEdge,
  'board/laptop': hwBoardLaptop,
  'board/phone': hwBoardPhone,
  'board/server': hwBoardServer,
  'board/workstation': hwBoardWorkstation,
  'exterior/apple-desktop': hwExteriorAppleDesktop,
  'exterior/apple-laptop': hwExteriorAppleLaptop,
  'exterior/edge': hwExteriorEdge,
  'exterior/laptop': hwExteriorLaptop,
  'exterior/phone': hwExteriorPhone,
  'exterior/server': hwExteriorServer,
  'exterior/workstation': hwExteriorWorkstation,
  'parts/12vhpwr': hwParts12Vhpwr,
  'parts/aio-pump': hwPartsAioPump,
  'parts/apple-logo': hwPartsAppleLogo,
  'parts/argb-header': hwPartsArgbHeader,
  'parts/atx24': hwPartsAtx24,
  'parts/audio-caps': hwPartsAudioCaps,
  'parts/audio-codec': hwPartsAudioCodec,
  'parts/battery-cell': hwPartsBatteryCell,
  'parts/bga-ic': hwPartsBgaIc,
  'parts/bios-chip': hwPartsBiosChip,
  'parts/blower-fan': hwPartsBlowerFan,
  'parts/camera-module': hwPartsCameraModule,
  'parts/cap-bank': hwPartsCapBank,
  'parts/chipset-heatsink': hwPartsChipsetHeatsink,
  'parts/choke': hwPartsChoke,
  'parts/cpu-amd-ihs': hwPartsCpuAmdIhs,
  'parts/cpu-intel-ihs': hwPartsCpuIntelIhs,
  'parts/cr2032': hwPartsCr2032,
  'parts/crystal': hwPartsCrystal,
  'parts/debug-leds': hwPartsDebugLeds,
  'parts/dimm': hwPartsDimm,
  'parts/dimm-slot': hwPartsDimmSlot,
  'parts/eps8': hwPartsEps8,
  'parts/fan-120': hwPartsFan120,
  'parts/fan-header-4pin': hwPartsFanHeader4Pin,
  'parts/ffc-connector': hwPartsFfcConnector,
  'parts/gpu-pcie': hwPartsGpuPcie,
  'parts/header-2x10': hwPartsHeader2X10,
  'parts/heatpipe-straight': hwPartsHeatpipeStraight,
  'parts/heatsink-tower': hwPartsHeatsinkTower,
  'parts/io-shroud': hwPartsIoShroud,
  'parts/jack-stack': hwPartsJackStack,
  'parts/m2-heatsink': hwPartsM2Heatsink,
  'parts/m2-ssd': hwPartsM2Ssd,
  'parts/mosfet': hwPartsMosfet,
  'parts/nic': hwPartsNic,
  'parts/pcie-8pin': hwPartsPcie8Pin,
  'parts/pcie-x1-slot': hwPartsPcieX1Slot,
  'parts/pcie-x16-slot': hwPartsPcieX16Slot,
  'parts/power-button': hwPartsPowerButton,
  'parts/qcode': hwPartsQcode,
  'parts/qfn-ic': hwPartsQfnIc,
  'parts/rj45': hwPartsRj45,
  'parts/sata-dual': hwPartsSataDual,
  'parts/shield-can': hwPartsShieldCan,
  'parts/smd-0402-field': hwPartsSmd0402Field,
  'parts/socket-am5': hwPartsSocketAm5,
  'parts/socket-lga1700': hwPartsSocketLga1700,
  'parts/sodimm': hwPartsSodimm,
  'parts/soic-ic': hwPartsSoicIc,
  'parts/usb-a-stack': hwPartsUsbAStack,
  'parts/usb-c': hwPartsUsbC,
  'parts/usb2-header': hwPartsUsb2Header,
  'parts/usb3-header': hwPartsUsb3Header,
  'parts/vrm-bank': hwPartsVrmBank,
  'parts/vrm-heatsink': hwPartsVrmHeatsink,
  'parts/wifi-module': hwPartsWifiModule,
};

const index = robotIndex as { species: string[]; tools: string[] };
const sharedClips = Object.keys((robotClips as { clips: Record<string, unknown> }).clips).sort();

/** The robot species, in the index's order. */
export const ROBOT_SPECIES: string[] = index.species;

/** The robot tools, in the index's order. */
export const ROBOT_TOOLS: string[] = index.tools;

/** The clips a species plays: the shared ones and its own (sorted); `all` (the line-up) has only the shared ones. */
export function robotClipOptions(species: string): string[] {
  const own = SPECIES[species]?.clips ?? {};
  return [...new Set([...sharedClips, ...Object.keys(own)])].sort();
}

/** The tools a robot can carry (their grip is carry). */
export function robotCarryTools(): string[] {
  return ROBOT_TOOLS.filter((id) => TOOLS[id]?.grip === 'carry');
}

/** The hardware models the lab can draw: boards, parts and exteriors, by their id (the file's path without .json). */
export const HARDWARE_MODELS: string[] = Object.keys(HARDWARE).sort();
