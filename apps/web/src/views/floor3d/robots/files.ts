// Every robot file under assets/robots/, imported statically so the bundle carries them. A new file goes
// in here and in index.json; the test "every robot file is in the index" fails until it does. The
// registry validates them all once (`registry()`): errors are listed, and a species with errors is left out.

import indexFile from '../../../../../../assets/robots/index.json';
import paletteFile from '../../../../../../assets/robots/palette.json';
import clipsFile from '../../../../../../assets/robots/clips.json';
import speciesRover from '../../../../../../assets/robots/species/rover.json';
import speciesHover from '../../../../../../assets/robots/species/hover.json';
import speciesTv from '../../../../../../assets/robots/species/tv.json';
import speciesCan from '../../../../../../assets/robots/species/can.json';
import speciesKitty from '../../../../../../assets/robots/species/kitty.json';
import speciesOwl from '../../../../../../assets/robots/species/owl.json';
import speciesCrab from '../../../../../../assets/robots/species/crab.json';
import speciesOcto from '../../../../../../assets/robots/species/octo.json';
import speciesBee from '../../../../../../assets/robots/species/bee.json';
import speciesSnail from '../../../../../../assets/robots/species/snail.json';
import speciesPenguin from '../../../../../../assets/robots/species/penguin.json';
import speciesFrog from '../../../../../../assets/robots/species/frog.json';
import speciesMole from '../../../../../../assets/robots/species/mole.json';
import speciesLamp from '../../../../../../assets/robots/species/lamp.json';
import speciesTeapot from '../../../../../../assets/robots/species/teapot.json';
import speciesJelly from '../../../../../../assets/robots/species/jelly.json';
import speciesBunny from '../../../../../../assets/robots/species/bunny.json';
import speciesTurtle from '../../../../../../assets/robots/species/turtle.json';
import speciesPup from '../../../../../../assets/robots/species/pup.json';
import speciesUfo from '../../../../../../assets/robots/species/ufo.json';
import speciesCube from '../../../../../../assets/robots/species/cube.json';
import speciesDuck from '../../../../../../assets/robots/species/duck.json';
import speciesMushroom from '../../../../../../assets/robots/species/mushroom.json';
import speciesAstro from '../../../../../../assets/robots/species/astro.json';
import speciesGhost from '../../../../../../assets/robots/species/ghost.json';
import speciesUnicycle from '../../../../../../assets/robots/species/unicycle.json';
import speciesLadybug from '../../../../../../assets/robots/species/ladybug.json';
import speciesPogo from '../../../../../../assets/robots/species/pogo.json';
import speciesLifty from '../../../../../../assets/robots/species/lifty.json';
import speciesSprout from '../../../../../../assets/robots/species/sprout.json';
import speciesOracle from '../../../../../../assets/robots/species/oracle.json';
import toolWrench from '../../../../../../assets/robots/tools/wrench.json';
import toolClipboard from '../../../../../../assets/robots/tools/clipboard.json';
import toolCrate from '../../../../../../assets/robots/tools/crate.json';
import toolBaton from '../../../../../../assets/robots/tools/baton.json';
import toolGavel from '../../../../../../assets/robots/tools/gavel.json';
import toolMegaphone from '../../../../../../assets/robots/tools/megaphone.json';
import toolScroll from '../../../../../../assets/robots/tools/scroll.json';
import toolBlueprint from '../../../../../../assets/robots/tools/blueprint.json';
import toolTSquare from '../../../../../../assets/robots/tools/t-square.json';
import toolTapeMeasure from '../../../../../../assets/robots/tools/tape-measure.json';
import toolPencil from '../../../../../../assets/robots/tools/pencil.json';
import toolScrewdriver from '../../../../../../assets/robots/tools/screwdriver.json';
import toolSolderingIron from '../../../../../../assets/robots/tools/soldering-iron.json';
import toolHammer from '../../../../../../assets/robots/tools/hammer.json';
import toolMagnifier from '../../../../../../assets/robots/tools/magnifier.json';
import toolMultimeter from '../../../../../../assets/robots/tools/multimeter.json';
import toolRedPen from '../../../../../../assets/robots/tools/red-pen.json';
import toolFlag from '../../../../../../assets/robots/tools/flag.json';
import toolCableSpool from '../../../../../../assets/robots/tools/cable-spool.json';
import toolAntennaWand from '../../../../../../assets/robots/tools/antenna-wand.json';
import toolExtinguisher from '../../../../../../assets/robots/tools/extinguisher.json';
import toolFlashlight from '../../../../../../assets/robots/tools/flashlight.json';
import toolPadlock from '../../../../../../assets/robots/tools/padlock.json';
import toolBook from '../../../../../../assets/robots/tools/book.json';
import toolBookStack from '../../../../../../assets/robots/tools/book-stack.json';
import toolFeatherDuster from '../../../../../../assets/robots/tools/feather-duster.json';
import toolFlask from '../../../../../../assets/robots/tools/flask.json';
import toolTestTube from '../../../../../../assets/robots/tools/test-tube.json';
import toolNotebook from '../../../../../../assets/robots/tools/notebook.json';
import toolStickyNotes from '../../../../../../assets/robots/tools/sticky-notes.json';
import toolMarker from '../../../../../../assets/robots/tools/marker.json';
import toolCoinBag from '../../../../../../assets/robots/tools/coin-bag.json';
import toolCalculator from '../../../../../../assets/robots/tools/calculator.json';
import toolAbacus from '../../../../../../assets/robots/tools/abacus.json';
import toolLedger from '../../../../../../assets/robots/tools/ledger.json';
import toolStamp from '../../../../../../assets/robots/tools/stamp.json';
import toolSeedling from '../../../../../../assets/robots/tools/seedling.json';
import toolWateringCan from '../../../../../../assets/robots/tools/watering-can.json';
import toolDnaVial from '../../../../../../assets/robots/tools/dna-vial.json';
import toolCoffeeMug from '../../../../../../assets/robots/tools/coffee-mug.json';
import toolBroom from '../../../../../../assets/robots/tools/broom.json';
import toolBall from '../../../../../../assets/robots/tools/ball.json';
import propDraftingTable from '../../../../../../assets/robots/props/drafting-table.json';
import propEasel from '../../../../../../assets/robots/props/easel.json';
import propWhiteboard from '../../../../../../assets/robots/props/whiteboard.json';
import propLabBench from '../../../../../../assets/robots/props/lab-bench.json';
import propMicroscopeDesk from '../../../../../../assets/robots/props/microscope-desk.json';
import propWorkbench from '../../../../../../assets/robots/props/workbench.json';
import propBookCart from '../../../../../../assets/robots/props/book-cart.json';
import propCounter from '../../../../../../assets/robots/props/counter.json';
import propPodium from '../../../../../../assets/robots/props/podium.json';
import propCrateStack from '../../../../../../assets/robots/props/crate-stack.json';
import propServerRack from '../../../../../../assets/robots/props/server-rack.json';
import propAlarmPost from '../../../../../../assets/robots/props/alarm-post.json';
import propGuardPost from '../../../../../../assets/robots/props/guard-post.json';
import propPlantBed from '../../../../../../assets/robots/props/plant-bed.json';
import propMeetingTable from '../../../../../../assets/robots/props/meeting-table.json';
import propChargingDock from '../../../../../../assets/robots/props/charging-dock.json';
import propBench from '../../../../../../assets/robots/props/bench.json';
import { validateClips, validateIndex, validatePalette, validateSpecies, validateTool } from './validate';
import type { Clip, Index, Palette, Species, Tool } from './types';

/** The files by the ids index.json lists. */
export const FILES = {
  index: indexFile as unknown,
  palette: paletteFile as unknown,
  clips: clipsFile as unknown,
  species: { rover: speciesRover, hover: speciesHover, tv: speciesTv, can: speciesCan, kitty: speciesKitty, owl: speciesOwl, crab: speciesCrab, octo: speciesOcto, bee: speciesBee, snail: speciesSnail, penguin: speciesPenguin, frog: speciesFrog, mole: speciesMole, lamp: speciesLamp, teapot: speciesTeapot, jelly: speciesJelly, bunny: speciesBunny, turtle: speciesTurtle, pup: speciesPup, ufo: speciesUfo, cube: speciesCube, duck: speciesDuck, mushroom: speciesMushroom, astro: speciesAstro, ghost: speciesGhost, unicycle: speciesUnicycle, ladybug: speciesLadybug, pogo: speciesPogo, lifty: speciesLifty, sprout: speciesSprout, oracle: speciesOracle } as Record<string, unknown>,
  tools: { baton: toolBaton, gavel: toolGavel, megaphone: toolMegaphone, scroll: toolScroll, blueprint: toolBlueprint, 't-square': toolTSquare, 'tape-measure': toolTapeMeasure, pencil: toolPencil, wrench: toolWrench, screwdriver: toolScrewdriver, 'soldering-iron': toolSolderingIron, hammer: toolHammer, magnifier: toolMagnifier, multimeter: toolMultimeter, clipboard: toolClipboard, 'red-pen': toolRedPen, crate: toolCrate, flag: toolFlag, 'cable-spool': toolCableSpool, 'antenna-wand': toolAntennaWand, extinguisher: toolExtinguisher, flashlight: toolFlashlight, padlock: toolPadlock, book: toolBook, 'book-stack': toolBookStack, 'feather-duster': toolFeatherDuster, flask: toolFlask, 'test-tube': toolTestTube, notebook: toolNotebook, 'sticky-notes': toolStickyNotes, marker: toolMarker, 'coin-bag': toolCoinBag, calculator: toolCalculator, abacus: toolAbacus, ledger: toolLedger, stamp: toolStamp, seedling: toolSeedling, 'watering-can': toolWateringCan, 'dna-vial': toolDnaVial, 'coffee-mug': toolCoffeeMug, broom: toolBroom, ball: toolBall } as Record<string, unknown>,
  props: { 'drafting-table': propDraftingTable, easel: propEasel, whiteboard: propWhiteboard, 'lab-bench': propLabBench, 'microscope-desk': propMicroscopeDesk, workbench: propWorkbench, 'book-cart': propBookCart, counter: propCounter, podium: propPodium, 'crate-stack': propCrateStack, 'server-rack': propServerRack, 'alarm-post': propAlarmPost, 'guard-post': propGuardPost, 'plant-bed': propPlantBed, 'meeting-table': propMeetingTable, 'charging-dock': propChargingDock, bench: propBench } as Record<string, unknown>,
};

export interface Registry {
  index: Index;
  palette: Palette;
  clips: Record<string, Clip>;
  /** The valid species, in index order. */
  species: Record<string, Species>;
  tools: Record<string, Tool>;
  /** Every validation error, by file path. Empty when every listed file is valid. */
  errors: string[];
}

let cached: Registry | undefined;

/** The validated robot data (built once). A file with errors is left out, and its errors are listed. */
export function registry(): Registry {
  if (cached) return cached;
  const errors: string[] = [...validateIndex(FILES.index), ...validatePalette(FILES.palette), ...validateClips(FILES.clips)];
  const index = FILES.index as Index;
  const sharedClips = (FILES.clips as { clips: Record<string, Clip> }).clips;
  const shared = new Set(Object.keys(sharedClips));

  const species: Record<string, Species> = {};
  for (const id of index.species ?? []) {
    const file = FILES.species[id];
    if (!file) {
      errors.push(`index.species: "${id}" has no file in robots/files.ts`);
      continue;
    }
    const e = validateSpecies(file, id, shared);
    if (e.length) errors.push(...e);
    else species[id] = file as Species;
  }

  const tools: Record<string, Tool> = {};
  for (const id of index.tools ?? []) {
    const file = FILES.tools[id];
    if (!file) {
      errors.push(`index.tools: "${id}" has no file in robots/files.ts`);
      continue;
    }
    const e = validateTool(file, id);
    if (e.length) errors.push(...e);
    else tools[id] = file as Tool;
  }

  // The palette without its schema field: the floor reads the eight colours by name.
  const { schema: _schema, ...palette } = FILES.palette as Palette & { schema?: number };
  cached = { index, palette, clips: sharedClips, species, tools, errors };
  return cached;
}

/** The shared clip of a name, or undefined. */
export function sharedClip(id: string): Clip | undefined {
  return registry().clips[id];
}
