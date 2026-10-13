// The station props (assets/robots/props/<id>.json, schema 1): furniture a robot works at, a drafting table,
// a bench, a server rack. A prop is parts on the floor (no bones, the tools' part format) and spots: where a
// robot stands (`at`, on the floor), which way it faces (`face`, degrees of yaw: 0 is +z, 180 is -z), and the
// shared clip it plays there. Props are built with the same part builder as the species (bakeSpecies and
// instantiate in rig.ts), so they share the toon materials and the ink line.

import * as THREE from 'three';
import { bakeSpecies, instantiate, robotMats, type RigInstance } from './rig';
import { FILES, registry } from './files';
import { validateTool } from './validate';
import type { Index, Palette, Part, Species, Vec3 } from './types';

export const PROP_KEYS = ['schema', 'id', 'name', 'footprint', 'parts', 'spots'] as const;
const SPOT_KEYS = ['at', 'face', 'clip'] as const;

export interface PropSpot {
  at: Vec3;
  /** Degrees of yaw about the floor's up axis: 0 faces +z, 180 faces -z. */
  face: number;
  /** A shared clip (clips.json): the robot plays it at this spot. */
  clip: string;
}

export interface Prop {
  schema: 1;
  id: string;
  name: string;
  /** The floor the prop takes, in robot units: width (x) and depth (z). */
  footprint: [number, number];
  parts: Part[];
  spots: PropSpot[];
}

type Errs = string[];
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const isVec3 = (v: unknown): v is Vec3 => Array.isArray(v) && v.length === 3 && v.every(isNum);

/** A prop file's errors, each named by its path. Empty when the file is valid. `clips` are the shared clip ids. */
export function validateProp(file: unknown, name: string, clips: ReadonlySet<string>): Errs {
  if (!isObj(file)) return [`${name}: a JSON object`];
  const errs: Errs = [];
  for (const k of Object.keys(file)) if (!(PROP_KEYS as readonly string[]).includes(k)) errs.push(`${name}.${k}: unknown field`);
  if (file.schema !== 1) errs.push(`${name}.schema: 1`);
  if (file.id !== name) errs.push(`${name}.id: must match the file name "${name}"`);
  if (typeof file.name !== 'string' || file.name.length === 0) errs.push(`${name}.name: a name`);
  const fp = file.footprint;
  if (!Array.isArray(fp) || fp.length !== 2 || !fp.every((n) => isNum(n) && n > 0)) errs.push(`${name}.footprint: [width, depth], both above 0`);
  // The parts are the tools' part format (no bone, built at the origin): the tool validator checks them.
  errs.push(...validateTool({ schema: 1, id: name, name, grip: 'one', tip: [0, 0, 0], parts: file.parts }, name));
  if (!Array.isArray(file.spots) || file.spots.length === 0) errs.push(`${name}.spots: a list of at least one spot`);
  else file.spots.forEach((s: unknown, i) => {
    const sp = `${name}.spots[${i}]`;
    if (!isObj(s)) return void errs.push(`${sp}: { at, face, clip }`);
    for (const k of Object.keys(s)) if (!(SPOT_KEYS as readonly string[]).includes(k)) errs.push(`${sp}.${k}: unknown field`);
    if (!isVec3(s.at)) errs.push(`${sp}.at: [x, y, z] on the floor`);
    if (!isNum(s.face)) errs.push(`${sp}.face: degrees of yaw`);
    if (typeof s.clip !== 'string' || !clips.has(s.clip)) errs.push(`${sp}.clip: "${String(s.clip)}" is not a shared clip (clips.json)`);
  });
  return errs;
}

export interface PropRegistry {
  /** The valid props, by id, in index order. */
  props: Record<string, Prop>;
  order: string[];
  /** Every validation error, by file path. */
  errors: string[];
}

let cached: PropRegistry | undefined;

/** The validated props (built once), in the order index.json lists them. A file with errors is left out. */
export function propRegistry(): PropRegistry {
  if (cached) return cached;
  const clips = new Set(Object.keys(registry().clips));
  const index = FILES.index as Index;
  const props: Record<string, Prop> = {};
  const order: string[] = [];
  const errors: string[] = [];
  for (const id of index.props ?? []) {
    const file = FILES.props[id];
    if (!file) {
      errors.push(`index.props: "${id}" has no file in robots/files.ts`);
      continue;
    }
    const e = validateProp(file, id, clips);
    if (e.length) errors.push(...e);
    else {
      props[id] = file as Prop;
      order.push(id);
    }
  }
  cached = { props, order, errors };
  return cached;
}

export interface BuiltProp {
  /** The prop's meshes at the origin (the floor's centre). The caller places and scales it. */
  group: THREE.Group;
  spots: PropSpot[];
  dispose(): void;
}

/** A prop's meshes, built as a species with one bone (the root) and no eyes, so the rig's own builders do the work. */
export function buildProp(prop: Prop, palette: Palette, hue: number, accent = '#5b8cff'): BuiltProp {
  const species = { schema: 1, id: prop.id, name: prop.name, accent, parts: prop.parts, eyes: [], bones: {} } as unknown as Species;
  const geo = bakeSpecies(species, palette, false);
  const mats = robotMats(palette, hue, hue);
  const group = new THREE.Group();
  group.name = `prop:${prop.id}`;
  const rig: RigInstance = instantiate(species, geo, mats, group);
  return {
    group,
    spots: prop.spots,
    dispose(): void {
      rig.dispose();
      group.removeFromParent();
    },
  };
}
