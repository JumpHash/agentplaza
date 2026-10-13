// Hardware models (schema 1): the JSON under assets/hardware/ read by the web's three.js renderer.
// The types mirror docs/design/hardware-plazas.md ("Schema"); validate() checks a file the way the
// desktop renderer reads it; buildModel() turns one into a three.js group that animates by tag and
// by the machine's state; traceMeshes / currentPulses draw a board's copper; placeAtom puts the
// plaza's atom at anchors.atom.

import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import { outline, toonMat } from './toon';
import { xenonAtom, type PropPalette } from './props';
import { MODELS, PALETTE } from './hardware-index';

// ---- Types ---------------------------------------------------------------------------------

export type Vec3 = [number, number, number];
export type Axis = 'x' | 'y' | 'z';

/** A material: a colour, optionally glowing (emissive × glow), faded, additive, or unlit (flat). */
export interface Material {
  color: string;
  emissive?: string;
  glow?: number;
  opacity?: number;
  additive?: boolean;
  flat?: boolean;
}
/** A part's material: a palette name or an inline material. */
export type MatRef = string | Material;
export type Palette = Record<string, Material>;

export type Geo =
  | { type: 'box'; w: number; h: number; d: number }
  | { type: 'rbox'; w: number; h: number; d: number; r: number }
  | { type: 'cyl'; rt: number; rb: number; h: number; seg?: number }
  | { type: 'cone'; r: number; h: number; seg?: number }
  | { type: 'sphere'; r: number }
  | { type: 'torus'; r: number; tube: number; arc?: number }
  | { type: 'plane'; w: number; d: number }
  | { type: 'ring'; ri: number; ro: number }
  /** A flat outline (points [x, z], counter-clockwise seen from above) extruded up by h: logos, cut shapes. */
  | { type: 'shape'; points: [number, number][]; h: number };
export type GeoType = Geo['type'];

export interface Anim {
  spin?: { axis: Axis; rps: number };
  pulse?: { period: number };
  blink?: { period: number; phase?: number };
  slide?: { to: Vec3; on: 'enter' };
  swing?: { axis: Axis; to: number; on: 'enter' };
}

export type Tag = 'status' | 'activity' | 'fan' | 'screen';

export interface Label {
  text: string;
  size?: number;
  color?: string;
}

/**
 * Copies of a part. count: a number 1..64, a numeric param's name ("p_cores"), or "p_cores/4" (the param
 * divided by 4, rounded up). Copy i sits at step * i; with wrap, at step * (i % wrap) + row * floor(i / wrap).
 */
export interface Repeat {
  count: number | string;
  step: Vec3;
  wrap?: number;
  row?: Vec3;
}

export interface Part {
  id?: string;
  geo?: Geo;
  use?: string;
  at?: Vec3;
  rot?: Vec3;
  scale?: Vec3 | number;
  mat?: MatRef;
  outline?: boolean;
  repeat?: Repeat;
  when?: string;
  anim?: Anim;
  tag?: Tag;
  label?: Label;
  children?: Part[];
}

export type SlotType = 'cpu' | 'gpu' | 'memory' | 'storage' | 'network' | 'power' | 'io' | 'soc' | 'modem' | 'bmc' | 'npu';

export interface Slot {
  id: string;
  on: string;
  type: SlotType;
  top: number;
  rect: [number, number, number, number];
  capacity: number;
}

export interface Trace {
  path: [number, number][];
  y: number;
  width: number;
  from?: string;
  to?: string;
}

export interface Anchors {
  label?: Vec3;
  port?: Vec3;
  camera?: { at: Vec3; look: Vec3 };
  open?: string;
  atom?: Vec3;
}

export interface Model {
  schema: 1;
  kind: string;
  view: 'exterior' | 'board' | 'part';
  size: Vec3;
  params?: Record<string, number>;
  parts: Part[];
  anchors?: Anchors;
  slots?: Slot[];
  traces?: Trace[];
}

export type Params = Record<string, number>;
export type Status = 'online' | 'draining' | 'offline' | 'revoked';
/** The machine's state as the renderers see it. `open` is the ENTER progress, 0..1. */
export interface HwState {
  status?: Status;
  working?: number;
  load?: number;
  open?: number;
}
export type Resolver = (name: string) => Model | undefined;

// ---- Constants -----------------------------------------------------------------------------

const isParamName = (v: unknown): v is string => typeof v === 'string' && /^[a-z_]\w*$/.test(v);
/** "p_cores/4": a param's name, a slash, and a whole number 1 or more. */
const COUNT_REF = /^([a-z_]\w*)\/([1-9]\d*)$/;
const isCountRef = (v: unknown): boolean => isParamName(v) || (typeof v === 'string' && COUNT_REF.test(v));
const VIEWS = ['exterior', 'board', 'part'];
const TAGS: readonly string[] = ['status', 'activity', 'fan', 'screen'];
const AXES: readonly string[] = ['x', 'y', 'z'];
const SLOT_TYPES: readonly string[] = ['cpu', 'gpu', 'memory', 'storage', 'network', 'power', 'io', 'soc', 'modem', 'bmc', 'npu'];
const MODEL_KEYS = ['schema', 'kind', 'view', 'size', 'params', 'parts', 'anchors', 'slots', 'traces'];
const PART_KEYS = ['id', 'geo', 'use', 'at', 'rot', 'scale', 'mat', 'outline', 'repeat', 'when', 'anim', 'tag', 'label', 'children'];
const MATERIAL_KEYS = ['color', 'emissive', 'glow', 'opacity', 'additive', 'flat'];
const ANCHOR_KEYS = ['label', 'port', 'camera', 'open', 'atom'];
const ANIM_KEYS = ['spin', 'pulse', 'blink', 'slide', 'swing'];
const LED: Record<Status, string> = { online: 'led-ok', draining: 'led-warn', offline: 'led-off', revoked: 'led-crit' };
const GEO: Record<GeoType, { req: readonly string[]; opt: readonly string[] }> = {
  box: { req: ['w', 'h', 'd'], opt: [] },
  rbox: { req: ['w', 'h', 'd', 'r'], opt: [] },
  cyl: { req: ['rt', 'rb', 'h'], opt: ['seg'] },
  cone: { req: ['r', 'h'], opt: ['seg'] },
  sphere: { req: ['r'], opt: [] },
  torus: { req: ['r', 'tube'], opt: ['arc'] },
  plane: { req: ['w', 'd'], opt: [] },
  ring: { req: ['ri', 'ro'], opt: [] },
  shape: { req: ['h'], opt: ['points'] }, // points is checked on its own (an array, not a number)
};
const DEFAULT_MAT: Material = { color: '#9aa3b8' };
const TAU = Math.PI * 2;
const ZERO: Vec3 = [0, 0, 0];
/** The Floor's own colours, for the atom when no palette is given. */
const ATOM_PALETTE: PropPalette = {
  sheet: new THREE.Color(0x2c3554),
  rule: new THREE.Color(0x56638a),
  ink3: new THREE.Color(0x7d8bb5),
  signal: new THREE.Color(0xff7a2e),
  ok: new THREE.Color(0x3ee08f),
  info: new THREE.Color(0x4cc9ff),
};

/** The default resolver: the model files, keyed like "parts/gpu-pcie". */
const defaultResolve: Resolver = (name) => MODELS[name];

// ---- The `when` expression -----------------------------------------------------------------

export interface When {
  name: string;
  negate: boolean;
  op?: '>=' | '<=' | '==' | '!=' | '>' | '<';
  value?: number;
}

const WHEN_RE = /^\s*(!)?\s*([a-z_]\w*)\s*(?:(>=|<=|==|!=|>|<)\s*(-?\d+(?:\.\d+)?))?\s*$/;

/** Parses `gpus`, `!gpus` or `gpus >= n`; null when the expression is not one of those. */
export function parseWhen(expr: string): When | null {
  const m = WHEN_RE.exec(expr);
  if (!m) return null;
  const negate = m[1] === '!';
  if (negate && m[3]) return null;
  return { name: m[2]!, negate, op: m[3] as When['op'], value: m[4] !== undefined ? Number(m[4]) : undefined };
}

export function evalWhen(w: When, params: Params): boolean {
  const v = params[w.name] ?? 0;
  if (w.negate) return !v;
  if (!w.op) return v !== 0;
  const n = w.value ?? 0;
  switch (w.op) {
    case '>=': return v >= n;
    case '<=': return v <= n;
    case '==': return v === n;
    case '!=': return v !== n;
    case '>': return v > n;
    case '<': return v < n;
  }
}

// ---- Validation ----------------------------------------------------------------------------

export interface ValidateOptions {
  palette?: Palette;
  resolve?: Resolver;
}

interface VCtx {
  name: string;
  errs: string[];
  palette: Palette;
  resolve: Resolver;
  ids: Map<string, string>;
  refs: { path: string; id: string }[];
}

const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isVec = (v: unknown, n: number): v is number[] => Array.isArray(v) && v.length === n && v.every(isNum);
const isVec3 = (v: unknown): v is Vec3 => isVec(v, 3);
const isHex = (v: unknown): boolean => typeof v === 'string' && /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.test(v);
const isStr = (v: unknown): v is string => typeof v === 'string' && v.length > 0;

function fail(c: VCtx, path: string, msg: string): void {
  c.errs.push(`${c.name}: ${path}: ${msg}`);
}

function unknownKeys(o: Record<string, unknown>, allowed: readonly string[], c: VCtx, path: string): void {
  for (const k of Object.keys(o)) if (!allowed.includes(k)) fail(c, path, `unknown field "${k}"`);
}

/** Checks the fields of a small object: each present field passes its test; required ones are present. */
function shape(o: Record<string, unknown>, path: string, c: VCtx, fields: Record<string, [boolean, (v: unknown) => boolean, string]>): void {
  unknownKeys(o, Object.keys(fields), c, path);
  for (const [k, [req, test, what]] of Object.entries(fields)) {
    const v = o[k];
    if (v === undefined) {
      if (req) fail(c, `${path}.${k}`, `missing ${what}`);
    } else if (!test(v)) fail(c, `${path}.${k}`, `expected ${what}`);
  }
}

/** Validates a model file (or a parts file), as raw JSON. Returns the errors, empty when it is valid. */
export function validate(model: unknown, name: string, opts: ValidateOptions = {}): string[] {
  const c: VCtx = { name, errs: [], palette: opts.palette ?? PALETTE, resolve: opts.resolve ?? defaultResolve, ids: new Map(), refs: [] };
  if (!isObj(model)) {
    fail(c, 'model', 'not a JSON object');
    return c.errs;
  }
  unknownKeys(model, MODEL_KEYS, c, 'model');
  if (model.schema !== 1) fail(c, 'schema', 'must be 1');
  if (!isStr(model.kind)) fail(c, 'kind', 'missing kind');
  if (typeof model.view !== 'string' || !VIEWS.includes(model.view)) fail(c, 'view', `must be one of ${VIEWS.join(', ')}`);
  if (!isVec(model.size, 3) || model.size.some((v) => v < 0)) fail(c, 'size', 'needs three non-negative numbers [w, h, d]');
  if (model.params !== undefined) {
    if (!isObj(model.params)) fail(c, 'params', 'expected an object');
    else
      for (const [k, v] of Object.entries(model.params)) {
        if (!isParamName(k)) fail(c, `params.${k}`, 'a param name is lower-case letters, digits and _');
        if (!isNum(v)) fail(c, `params.${k}`, 'expected a number');
      }
  }
  if (!Array.isArray(model.parts)) fail(c, 'parts', 'expected an array');
  else model.parts.forEach((p, i) => checkPart(p, `parts[${i}]`, c));

  if (model.anchors !== undefined) checkAnchors(model.anchors, c, model.view);
  if (model.slots !== undefined) {
    if (model.view !== 'board') fail(c, 'slots', 'only a board has slots');
    if (!Array.isArray(model.slots)) fail(c, 'slots', 'expected an array');
    else checkSlots(model.slots, c);
  }
  if (model.traces !== undefined) {
    if (model.view !== 'board') fail(c, 'traces', 'only a board has traces');
    if (!Array.isArray(model.traces)) fail(c, 'traces', 'expected an array');
    else model.traces.forEach((t, i) => checkTrace(t, `traces[${i}]`, c));
  }
  for (const r of c.refs) if (!c.ids.has(r.id)) fail(c, r.path, `no part with id "${r.id}"`);
  return c.errs;
}

/** Validates the palette file (palette.json): every colour is a material; the status LEDs exist. */
export function validatePalette(file: unknown): string[] {
  const c: VCtx = { name: 'palette.json', errs: [], palette: {}, resolve: defaultResolve, ids: new Map(), refs: [] };
  if (!isObj(file) || file.schema !== 1 || !isObj(file.colors)) {
    fail(c, 'file', 'needs "schema": 1 and a "colors" object');
    return c.errs;
  }
  for (const [name, m] of Object.entries(file.colors)) checkMaterial(m, `colors.${name}`, c);
  for (const led of Object.values(LED)) if (!(led in file.colors)) fail(c, `colors.${led}`, 'the status LEDs need this colour');
  return c.errs;
}

function checkMaterial(m: unknown, path: string, c: VCtx): void {
  if (!isObj(m)) return fail(c, path, 'a material is an object with a color');
  unknownKeys(m, MATERIAL_KEYS, c, path);
  if (!isHex(m.color)) fail(c, `${path}.color`, 'needs a hex colour like "#1f6b3a"');
  if (m.emissive !== undefined && !isHex(m.emissive)) fail(c, `${path}.emissive`, 'expected a hex colour');
  if (m.glow !== undefined && !(isNum(m.glow) && m.glow >= 0 && m.glow <= 4)) fail(c, `${path}.glow`, 'expected a number 0..4');
  if (m.opacity !== undefined && !(isNum(m.opacity) && m.opacity >= 0 && m.opacity <= 1)) fail(c, `${path}.opacity`, 'expected a number 0..1');
  if (m.additive !== undefined && typeof m.additive !== 'boolean') fail(c, `${path}.additive`, 'expected true or false');
  if (m.flat !== undefined && typeof m.flat !== 'boolean') fail(c, `${path}.flat`, 'expected true or false');
}

function checkMatRef(m: unknown, path: string, c: VCtx): void {
  if (typeof m === 'string') {
    if (!(m in c.palette)) fail(c, path, `unknown palette name "${m}"`);
  } else checkMaterial(m, path, c);
}

function checkGeo(g: unknown, path: string, c: VCtx): void {
  if (!isObj(g) || typeof g.type !== 'string' || !(g.type in GEO)) {
    fail(c, `${path}.type`, `unknown geo type ${JSON.stringify(isObj(g) ? g.type : g)} (box, rbox, cyl, cone, sphere, torus, plane, ring, shape)`);
    return;
  }
  const spec = GEO[g.type as GeoType];
  unknownKeys(g, ['type', ...spec.req, ...spec.opt], c, path);
  for (const k of spec.req) {
    const v = g[k];
    if (v === undefined) fail(c, `${path}.${k}`, `missing dimension "${k}" for ${g.type}`);
    else if (!isNum(v)) fail(c, `${path}.${k}`, 'expected a number');
    else if (v < 0 || (v === 0 && !['rt', 'rb', 'ri'].includes(k))) fail(c, `${path}.${k}`, 'must be above zero');
  }
  if (spec.opt.includes('seg') && g.seg !== undefined && !(isNum(g.seg) && g.seg >= 3)) fail(c, `${path}.seg`, 'expected a number of 3 or more');
  if (g.type === 'rbox' && isNum(g.w) && isNum(g.h) && isNum(g.d) && isNum(g.r) && g.r > Math.min(g.w, g.h, g.d) / 2)
    fail(c, `${path}.r`, 'the radius is more than half the smallest side');
  if (g.type === 'ring' && isNum(g.ri) && isNum(g.ro) && g.ri >= g.ro) fail(c, `${path}.ri`, 'the inner radius must be below the outer');
  if (g.type === 'shape') {
    const pts = g.points;
    if (!Array.isArray(pts) || pts.length < 3) fail(c, `${path}.points`, 'expected 3 or more [x, z] points');
    else if (!pts.every((q) => Array.isArray(q) && q.length === 2 && isNum(q[0]) && isNum(q[1])))
      fail(c, `${path}.points`, 'every point is [x, z]');
  }
  if (g.type === 'torus' && g.arc !== undefined && !(isNum(g.arc) && g.arc > 0 && g.arc <= 360)) fail(c, `${path}.arc`, 'expected degrees in (0, 360]');
}

function checkWhen(expr: unknown, path: string, c: VCtx): void {
  const w = typeof expr === 'string' ? parseWhen(expr) : null;
  if (!w) return fail(c, path, `bad when expression ${JSON.stringify(expr)} (use "gpus", "!gpus" or "gpus >= n")`);
}

function checkAnim(a: unknown, path: string, c: VCtx): void {
  if (!isObj(a)) return fail(c, path, 'expected an object');
  unknownKeys(a, ANIM_KEYS, c, path);
  const axis = (v: unknown): boolean => typeof v === 'string' && AXES.includes(v);
  const period = (v: unknown): boolean => isNum(v) && v > 0;
  if (isObj(a.spin)) shape(a.spin, `${path}.spin`, c, { axis: [true, axis, 'axis x|y|z'], rps: [true, (v) => isNum(v) && v >= 0, 'rps (turns per second)'] });
  else if (a.spin !== undefined) fail(c, `${path}.spin`, 'expected an object');
  if (isObj(a.pulse)) shape(a.pulse, `${path}.pulse`, c, { period: [true, period, 'period'] });
  else if (a.pulse !== undefined) fail(c, `${path}.pulse`, 'expected an object');
  if (isObj(a.blink)) shape(a.blink, `${path}.blink`, c, { period: [true, period, 'period'], phase: [false, isNum, 'phase'] });
  else if (a.blink !== undefined) fail(c, `${path}.blink`, 'expected an object');
  const on = (v: unknown): boolean => v === 'enter';
  if (isObj(a.slide)) shape(a.slide, `${path}.slide`, c, { to: [true, isVec3, 'to [x, y, z]'], on: [true, on, 'on: "enter"'] });
  else if (a.slide !== undefined) fail(c, `${path}.slide`, 'expected an object');
  if (isObj(a.swing)) shape(a.swing, `${path}.swing`, c, { axis: [true, axis, 'axis x|y|z'], to: [true, isNum, 'to (degrees)'], on: [true, on, 'on: "enter"'] });
  else if (a.swing !== undefined) fail(c, `${path}.swing`, 'expected an object');
}

function checkPart(p: unknown, path: string, c: VCtx): void {
  if (!isObj(p)) return fail(c, path, 'expected an object');
  unknownKeys(p, PART_KEYS, c, path);
  if (p.id !== undefined) {
    if (!isStr(p.id)) fail(c, `${path}.id`, 'expected a non-empty string');
    else if (c.ids.has(p.id)) fail(c, `${path}.id`, `duplicate id "${p.id}" (first at ${c.ids.get(p.id)})`);
    else c.ids.set(p.id, path);
  }
  if ((p.geo === undefined) === (p.use === undefined)) fail(c, path, 'needs exactly one of "geo" or "use"');
  if (p.geo !== undefined) checkGeo(p.geo, `${path}.geo`, c);
  if (p.use !== undefined) {
    if (!isStr(p.use)) fail(c, `${path}.use`, 'expected a part name like "parts/gpu-pcie"');
    else if (!c.resolve(p.use)) fail(c, `${path}.use`, `unknown part "${p.use}"`);
  }
  if (p.at !== undefined && !isVec3(p.at)) fail(c, `${path}.at`, 'expected [x, y, z]');
  if (p.rot !== undefined && !isVec3(p.rot)) fail(c, `${path}.rot`, 'expected [rx, ry, rz] in degrees');
  if (p.scale !== undefined && !(isNum(p.scale) ? p.scale > 0 : isVec3(p.scale) && p.scale.every((v) => v > 0)))
    fail(c, `${path}.scale`, 'expected a positive number or [sx, sy, sz]');
  if (p.mat !== undefined) checkMatRef(p.mat, `${path}.mat`, c);
  if (p.outline !== undefined && typeof p.outline !== 'boolean') fail(c, `${path}.outline`, 'expected true or false');
  if (p.repeat !== undefined) {
    const r: unknown = p.repeat;
    if (!isObj(r)) fail(c, `${path}.repeat`, 'expected {count, step}');
    else {
      shape(r, `${path}.repeat`, c, {
        count: [true, (v) => (Number.isInteger(v) && (v as number) >= 1 && (v as number) <= 256) || isCountRef(v), 'count 1..256, a param name or "param/n"'],
        step: [true, isVec3, 'step [dx, dy, dz]'],
        wrap: [false, (v) => Number.isInteger(v) && (v as number) >= 1, 'wrap: a whole number 1 or more'],
        row: [false, isVec3, 'row [dx, dy, dz]'],
      });
      if (r.wrap !== undefined && r.row === undefined) fail(c, `${path}.repeat.row`, 'missing row [dx, dy, dz] (a wrap needs one)');
      if (r.row !== undefined && r.wrap === undefined) fail(c, `${path}.repeat.row`, 'row needs a wrap');
    }
  }
  if (p.when !== undefined) checkWhen(p.when, `${path}.when`, c);
  if (p.anim !== undefined) checkAnim(p.anim, `${path}.anim`, c);
  if (p.tag !== undefined && !(typeof p.tag === 'string' && TAGS.includes(p.tag))) fail(c, `${path}.tag`, `must be one of ${TAGS.join(', ')}`);
  if (p.label !== undefined) {
    if (!isObj(p.label)) fail(c, `${path}.label`, 'expected {text, size, color}');
    else shape(p.label, `${path}.label`, c, { text: [true, isStr, 'text'], size: [false, (v) => isNum(v) && v > 0, 'size > 0'], color: [false, isHex, 'hex color'] });
  }
  if (p.children !== undefined) {
    if (!Array.isArray(p.children)) fail(c, `${path}.children`, 'expected an array');
    else p.children.forEach((ch, i) => checkPart(ch, `${path}.children[${i}]`, c));
  }
}

function checkAnchors(a: unknown, c: VCtx, view: unknown): void {
  if (!isObj(a)) return fail(c, 'anchors', 'expected an object');
  unknownKeys(a, ANCHOR_KEYS, c, 'anchors');
  if (a.label !== undefined && !isVec3(a.label)) fail(c, 'anchors.label', 'expected [x, y, z]');
  if (a.port !== undefined && !isVec3(a.port)) fail(c, 'anchors.port', 'expected [x, y, z]');
  if (a.atom !== undefined && !isVec3(a.atom)) fail(c, 'anchors.atom', 'expected [x, y, z]');
  if (a.camera !== undefined) {
    if (!isObj(a.camera) || !isVec3(a.camera.at) || !isVec3(a.camera.look)) fail(c, 'anchors.camera', 'expected {at: [x,y,z], look: [x,y,z]}');
  } else if (view === 'exterior' || view === 'board') fail(c, 'anchors.camera', `a ${view} needs a camera`);
  if (view === 'board' && a.atom === undefined) fail(c, 'anchors.atom', 'a board needs the atom anchor (over the main processor)');
  if (a.open !== undefined) {
    if (!isStr(a.open)) fail(c, 'anchors.open', 'expected a part id');
    else c.refs.push({ path: 'anchors.open', id: a.open });
  }
}

function checkSlots(slots: unknown[], c: VCtx): void {
  const seen = new Set<string>();
  slots.forEach((s, i) => {
    const path = `slots[${i}]`;
    if (!isObj(s)) return fail(c, path, 'expected an object');
    shape(s, path, c, {
      id: [true, isStr, 'id'],
      on: [true, isStr, 'the part it sits on'],
      type: [true, (v) => typeof v === 'string' && SLOT_TYPES.includes(v), `type (${SLOT_TYPES.join(', ')})`],
      top: [true, isNum, 'top (the surface height)'],
      rect: [true, (v) => isVec(v, 4) && (v as number[])[2]! > 0 && (v as number[])[3]! > 0, 'rect [x, z, w, d] with w, d above zero'],
      capacity: [true, (v) => Number.isInteger(v) && (v as number) >= 1, 'capacity (a whole number of rooms, 1 or more)'],
    });
    if (isStr(s.id)) {
      if (seen.has(s.id)) fail(c, `${path}.id`, `duplicate slot id "${s.id}"`);
      seen.add(s.id);
    }
    if (isStr(s.on)) c.refs.push({ path: `${path}.on`, id: s.on });
  });
}

function checkTrace(t: unknown, path: string, c: VCtx): void {
  if (!isObj(t)) return fail(c, path, 'expected an object');
  shape(t, path, c, {
    path: [true, (v) => Array.isArray(v) && v.length >= 2 && v.every((p) => isVec(p, 2)), 'path: at least two [x, z] points'],
    y: [true, isNum, 'y'],
    width: [true, (v) => isNum(v) && v > 0, 'width above zero'],
    from: [false, isStr, 'the part id it leaves'],
    to: [false, isStr, 'the part id it reaches'],
  });
  if (isStr(t.from)) c.refs.push({ path: `${path}.from`, id: t.from });
  if (isStr(t.to)) c.refs.push({ path: `${path}.to`, id: t.to });
}

// ---- Geometry, materials, labels -----------------------------------------------------------

function makeGeometry(g: Geo): THREE.BufferGeometry {
  switch (g.type) {
    case 'box': return new THREE.BoxGeometry(g.w, g.h, g.d);
    case 'rbox': return new RoundedBoxGeometry(g.w, g.h, g.d, 3, g.r);
    case 'cyl': return new THREE.CylinderGeometry(g.rt, g.rb, g.h, g.seg ?? 24);
    case 'cone': return new THREE.ConeGeometry(g.r, g.h, g.seg ?? 24);
    case 'sphere': return new THREE.SphereGeometry(g.r, 24, 16);
    case 'torus': return new THREE.TorusGeometry(g.r, g.tube, 10, 32, THREE.MathUtils.degToRad(g.arc ?? 360));
    case 'plane': {
      const geo = new THREE.PlaneGeometry(g.w, g.d);
      geo.rotateX(-Math.PI / 2); // facing up
      return geo;
    }
    case 'ring': {
      const geo = new THREE.RingGeometry(g.ri, g.ro, 48);
      geo.rotateX(-Math.PI / 2);
      return geo;
    }
    case 'shape': {
      // The outline in the shape's plane as (x, -z), extruded along +z by h, then laid flat: the
      // rotation maps (sx, sy, e) to (sx, e, -sy), so y runs 0..h and z comes back as given.
      const shape = new THREE.Shape(g.points.map(([x, z]) => new THREE.Vector2(x, -z)));
      const geo = new THREE.ExtrudeGeometry(shape, { depth: g.h, bevelEnabled: false, curveSegments: 1 });
      geo.rotateX(-Math.PI / 2);
      return geo;
    }
  }
}

/** A material for a part. Animated or tagged parts get their own (they change); the rest share. */
function makeMaterial(m: Material, own: boolean): THREE.Material {
  const emissive = m.emissive ?? (own ? m.color : undefined);
  const glow = m.glow ?? (emissive ? (m.emissive ? 1 : 0.5) : 0);
  const additive = m.additive ?? false;
  const opacity = m.opacity ?? 1;
  const transparent = opacity < 1 || additive;
  const blending = additive ? THREE.AdditiveBlending : THREE.NormalBlending;
  if (m.flat) return new THREE.MeshBasicMaterial({ color: m.color, transparent, opacity, blending, depthWrite: !transparent, toneMapped: !additive });
  return toonMat({
    color: m.color,
    emissive: emissive ?? '#000000',
    emissiveIntensity: glow,
    transparent,
    opacity,
    blending,
    depthWrite: !transparent,
  });
}

interface Ctx {
  params: Params;
  palette: Palette;
  resolve: Resolver;
  parts: Map<string, THREE.Object3D>;
  mats: Map<string, THREE.Material>;
  drivers: ((t: number, dt: number, st: Required<HwState>) => void)[];
}

function materialFor(p: Part, ctx: Ctx): THREE.Material {
  const own = p.tag !== undefined || p.anim?.pulse !== undefined || p.anim?.blink !== undefined;
  const ref = p.mat;
  const m: Material = typeof ref === 'string' ? (ctx.palette[ref] ?? DEFAULT_MAT) : (ref ?? DEFAULT_MAT);
  const key = JSON.stringify(m);
  if (!own && ctx.mats.has(key)) return ctx.mats.get(key)!;
  const mat = makeMaterial(m, own);
  if (!own) ctx.mats.set(key, mat);
  return mat;
}

/** Silkscreen text on a part's top: a canvas texture on a flat plane. Null without a DOM (tests). */
function labelMesh(l: Label, top: number): THREE.Mesh | null {
  if (typeof document === 'undefined') return null;
  const canvas = document.createElement('canvas');
  canvas.width = 512;
  canvas.height = 128;
  const cx = canvas.getContext('2d');
  if (!cx) return null;
  cx.fillStyle = l.color ?? '#e8eadf';
  cx.font = 'bold 84px ui-monospace, Menlo, Consolas, monospace';
  cx.textAlign = 'center';
  cx.textBaseline = 'middle';
  cx.fillText(l.text, 256, 68);
  const tex = new THREE.CanvasTexture(canvas);
  tex.colorSpace = THREE.SRGBColorSpace;
  const size = l.size ?? 0.6;
  const geo = new THREE.PlaneGeometry(size * 4, size);
  geo.rotateX(-Math.PI / 2);
  const mesh = new THREE.Mesh(geo, new THREE.MeshBasicMaterial({ map: tex, transparent: true, depthWrite: false, polygonOffset: true, polygonOffsetFactor: -2 }));
  mesh.position.y = top + 0.02;
  mesh.userData.outline = false;
  mesh.raycast = () => {};
  return mesh;
}

// ---- Building ------------------------------------------------------------------------------

export interface Built {
  group: THREE.Group;
  /** Every part by id; a `use`d part's inner parts are keyed "outer/inner", repeats "id#n". */
  parts: Map<string, THREE.Object3D>;
  /** Per frame: advances the tags and the animations from the machine's state. */
  update: (dt: number, state?: HwState) => void;
}

export interface BuildOptions {
  resolve?: Resolver;
  palette?: Palette;
}

/** Builds a model's three.js group with the given params (gpus...). Unset params take the model's defaults. */
export function buildModel(model: Model, params: Params = {}, opts: BuildOptions = {}): Built {
  const ctx: Ctx = {
    params: { ...(model.params ?? {}), ...params },
    palette: opts.palette ?? PALETTE,
    resolve: opts.resolve ?? defaultResolve,
    parts: new Map(),
    mats: new Map(),
    drivers: [],
  };
  const group = new THREE.Group();
  for (const p of model.parts) {
    const o = buildPart(p, ctx, '');
    if (o) group.add(o);
  }
  outline(group);
  let time = 0;
  const update = (dt: number, state: HwState = {}): void => {
    time += dt;
    const st: Required<HwState> = {
      status: state.status ?? 'online',
      working: state.working ?? 0,
      load: state.load ?? 0,
      open: state.open ?? 0,
    };
    for (const d of ctx.drivers) d(time, dt, st);
  };
  return { group, parts: ctx.parts, update };
}

function buildPart(p: Part, ctx: Ctx, scope: string): THREE.Object3D | null {
  if (p.when !== undefined) {
    const w = parseWhen(p.when);
    if (!w || !evalWhen(w, ctx.params)) return null;
  }
  if (!p.repeat) return buildOne(p, ctx, scope, 0, ZERO);
  const holder = new THREE.Group();
  repeatOffsets(p.repeat, ctx.params).forEach((offset, i) => holder.add(buildOne(p, ctx, scope, i, offset)));
  return holder;
}

/** A repeat's copy count, read from the params now: a number, a param's value, or a param's value over n, rounded up. */
function repeatCount(count: number | string, params: Params): number {
  if (typeof count === 'number') return count;
  const ref = COUNT_REF.exec(count);
  if (ref) {
    const [, name = '', over = '1'] = ref;
    return Math.ceil(Math.round(params[name] ?? 0) / Number(over));
  }
  return Math.round(params[count] ?? 0);
}

/** Where each copy of a repeat sits: copy i at step * i, or with wrap at step * (i % wrap) + row * floor(i / wrap). */
export function repeatOffsets(r: Repeat, params: Params): Vec3[] {
  const [sx, sy, sz] = r.step;
  const [rx, ry, rz] = r.row ?? ZERO;
  const out: Vec3[] = [];
  for (let i = 0; i < Math.min(256, repeatCount(r.count, params)); i++) {
    const col = r.wrap ? i % r.wrap : i;
    const line = r.wrap ? Math.floor(i / r.wrap) : 0;
    out.push([sx * col + rx * line, sy * col + ry * line, sz * col + rz * line]);
  }
  return out;
}

function buildOne(p: Part, ctx: Ctx, scope: string, inst: number, offset: Vec3): THREE.Object3D {
  const key = p.id ? `${scope}${p.id}${inst ? `#${inst}` : ''}` : '';
  const childScope = key ? `${key}/` : scope;
  let obj: THREE.Object3D;
  let top = 0;
  if (p.use !== undefined) {
    const target = ctx.resolve(p.use);
    const g = new THREE.Group();
    for (const tp of target?.parts ?? []) {
      const c = buildPart(tp, ctx, childScope);
      if (c) g.add(c);
    }
    top = target?.size[1] ?? 0;
    obj = g;
  } else if (p.geo) {
    const mesh = new THREE.Mesh(makeGeometry(p.geo), materialFor(p, ctx));
    mesh.geometry.computeBoundingBox();
    top = mesh.geometry.boundingBox?.max.y ?? 0;
    // Decals (plane, ring) are not inked; solids are, unless the part says otherwise.
    if (p.outline !== undefined) mesh.userData.outline = p.outline;
    else if (p.geo.type === 'plane' || p.geo.type === 'ring') mesh.userData.outline = false;
    obj = mesh;
  } else {
    obj = new THREE.Group();
  }

  const at = p.at ?? ZERO;
  obj.position.set(at[0] + offset[0], at[1] + offset[1], at[2] + offset[2]);
  if (p.rot) obj.rotation.set(THREE.MathUtils.degToRad(p.rot[0]), THREE.MathUtils.degToRad(p.rot[1]), THREE.MathUtils.degToRad(p.rot[2]));
  if (typeof p.scale === 'number') obj.scale.setScalar(p.scale);
  else if (p.scale) obj.scale.set(p.scale[0], p.scale[1], p.scale[2]);

  if (key) ctx.parts.set(key, obj);
  for (const c of p.children ?? []) {
    const o = buildPart(c, ctx, childScope);
    if (o) obj.add(o);
  }
  if (p.label) {
    const l = labelMesh(p.label, top);
    if (l) obj.add(l);
  }
  attach(obj, p, ctx);
  return obj;
}

/** The part's tags and animations, as per-frame drivers. */
function attach(obj: THREE.Object3D, p: Part, ctx: Ctx): void {
  const mat = (obj as THREE.Mesh).isMesh ? (obj as THREE.Mesh).material : null;
  const toon = mat && (mat as THREE.MeshToonMaterial).isMeshToonMaterial ? (mat as THREE.MeshToonMaterial) : null;
  const base = { pos: obj.position.clone(), rot: obj.rotation.clone(), glow: toon?.emissiveIntensity ?? 0 };
  const a = p.anim;
  if (a?.spin) {
    const { axis, rps } = a.spin;
    ctx.drivers.push((t) => {
      obj.rotation[axis] = base.rot[axis] + TAU * rps * t;
    });
  }
  if (a?.slide || a?.swing) {
    ctx.drivers.push((_t, _dt, st) => {
      const e = easeInOut(st.open);
      if (a.slide) {
        const [x, y, z] = a.slide.to;
        obj.position.set(lerp(base.pos.x, x, e), lerp(base.pos.y, y, e), lerp(base.pos.z, z, e));
      }
      if (a.swing) obj.rotation[a.swing.axis] = base.rot[a.swing.axis] + THREE.MathUtils.degToRad(a.swing.to) * e;
    });
  }
  if (toon && a?.pulse) {
    const period = a.pulse.period;
    ctx.drivers.push((t) => {
      toon.emissiveIntensity = base.glow * (0.6 + 0.4 * Math.sin((TAU * t) / period));
    });
  }
  if (toon && a?.blink) {
    const { period, phase = 0 } = a.blink;
    ctx.drivers.push((t) => {
      const on = (t / period + phase) % 1 < 0.5;
      toon.emissiveIntensity = on ? base.glow : base.glow * 0.1;
    });
  }
  if (mat && p.tag === 'status') {
    // Works on flat (unlit) LEDs too: their colour is set, and their emissive only when lit by toon.
    ctx.drivers.push((_t, _dt, st) => {
      const c = ctx.palette[LED[st.status]]?.color ?? '#5b636b';
      (mat as THREE.MeshBasicMaterial).color.set(c);
      toon?.emissive.set(c);
    });
  }
  if (toon && p.tag === 'activity') {
    ctx.drivers.push((t, _dt, st) => {
      // Working agents make the LED blink (0.8 s); idle, it sits dim.
      const lit = st.working > 0 && t % 0.8 < 0.4;
      toon.emissiveIntensity = lit ? base.glow : base.glow * 0.1;
    });
  }
  if (p.tag === 'fan') {
    let angle = 0;
    ctx.drivers.push((_t, dt, st) => {
      const rps = st.status === 'online' ? 2 + 6 * st.load : 0;
      angle += TAU * rps * dt;
      obj.rotation.y = base.rot.y + angle;
    });
  }
  if (toon && p.tag === 'screen') {
    ctx.drivers.push((t, _dt, st) => {
      toon.emissiveIntensity = st.status === 'online' ? base.glow * (0.92 + 0.08 * Math.sin(t * 1.3)) : base.glow * 0.15;
    });
  }
}

const lerp = (a: number, b: number, t: number): number => a + (b - a) * t;
const easeInOut = (v: number): number => {
  const x = Math.min(1, Math.max(0, v));
  return x * x * (3 - 2 * x);
};

// ---- Boards: copper and current ------------------------------------------------------------

/** The copper of a board's traces: a flat strip along each segment, with its width and height. */
export function traceMeshes(model: Model, palette: Palette = PALETTE): THREE.Group {
  const g = new THREE.Group();
  const mat = makeMaterial(palette.copper ?? DEFAULT_MAT, false);
  for (const tr of model.traces ?? []) {
    for (let i = 0; i < tr.path.length - 1; i++) {
      const [x0, z0] = tr.path[i]!;
      const [x1, z1] = tr.path[i + 1]!;
      const dx = x1 - x0;
      const dz = z1 - z0;
      const len = Math.hypot(dx, dz);
      if (len === 0) continue;
      // The strip is `len` long along x, turned to the segment's bearing, with a half-width overlap at each end.
      const geo = new THREE.BoxGeometry(len + tr.width, 0.04, tr.width);
      const m = new THREE.Mesh(geo, mat);
      m.position.set((x0 + x1) / 2, tr.y, (z0 + z1) / 2);
      m.rotation.y = -Math.atan2(dz, dx);
      m.userData.outline = false;
      g.add(m);
    }
  }
  return g;
}

export interface Pulses {
  group: THREE.Group;
  /** Moves the pulses along the traces; `activity` 0..1 speeds them up and lights more of them. */
  update: (dt: number, activity?: number) => void;
}

function pointAt(path: [number, number][], cum: number[], s: number): [number, number] {
  let i = 0;
  while (i < path.length - 2 && cum[i + 1]! <= s) i++;
  const [ax, az] = path[i]!;
  const [bx, bz] = path[i + 1]!;
  const len = cum[i + 1]! - cum[i]!;
  const f = len > 0 ? (s - cum[i]!) / len : 0;
  return [ax + (bx - ax) * f, az + (bz - az) * f];
}

/** Current pulses: small additive spheres that travel along each trace, one per ~10 units of trace. */
export function currentPulses(model: Model, palette: Palette = PALETTE): Pulses {
  const group = new THREE.Group();
  const geo = new THREE.SphereGeometry(0.16, 10, 8);
  const colour = palette['copper-live']?.color ?? '#ffb347';
  const mat = new THREE.MeshBasicMaterial({ color: colour, transparent: true, blending: THREE.AdditiveBlending, depthWrite: false, toneMapped: false });
  const runs: { tr: Trace; cum: number[]; total: number; n: number; rank: number; s: number; mesh: THREE.Mesh }[] = [];
  for (const tr of model.traces ?? []) {
    const cum = [0];
    for (let i = 0; i < tr.path.length - 1; i++) {
      const [x0, z0] = tr.path[i]!;
      const [x1, z1] = tr.path[i + 1]!;
      cum.push(cum[i]! + Math.hypot(x1 - x0, z1 - z0));
    }
    const total = cum[cum.length - 1]!;
    if (total <= 0) continue;
    const n = Math.min(4, 1 + Math.floor(total / 10));
    for (let k = 0; k < n; k++) {
      const mesh = new THREE.Mesh(geo, mat);
      mesh.position.y = tr.y + 0.1;
      group.add(mesh);
      runs.push({ tr, cum, total, n, rank: k, s: (total * k) / n, mesh });
    }
  }
  const update = (dt: number, activity = 0): void => {
    const a = Math.min(1, Math.max(0, activity));
    for (const r of runs) {
      r.s = (r.s + 5 * (1 + 2 * a) * dt) % r.total;
      const [x, z] = pointAt(r.tr.path, r.cum, r.s);
      r.mesh.position.set(x, r.tr.y + 0.1, z);
      r.mesh.visible = r.rank < Math.ceil(r.n * (0.25 + 0.75 * a));
    }
  };
  return { group, update };
}

// ---- The atom ------------------------------------------------------------------------------

/**
 * Puts the plaza's atom at anchors.atom (exteriors at 0.15 of the Floor's size, boards at 1).
 * Returns its animation to call each frame, or null when the model has no atom anchor.
 */
export function placeAtom(model: Model, group: THREE.Object3D, palette: PropPalette = ATOM_PALETTE): { atom: THREE.Group; animate: (t: number) => void } | null {
  const at = model.anchors?.atom;
  if (!at) return null;
  const { group: atom, animate } = xenonAtom(palette, at[1]);
  atom.position.x = at[0];
  atom.position.z = at[2];
  atom.scale.multiplyScalar(model.view === 'exterior' ? 0.15 : 1);
  group.add(atom);
  return { atom, animate };
}

/**
 * The model files for a machine: vendor "apple" uses the Apple family (exterior/apple-laptop or
 * exterior/apple-desktop by kind, board/apple-soc); anything else uses its kind's generic models.
 */
export function modelsFor(machine: { kind: string; vendor?: string | null }): { exterior: string; board: string } {
  if (machine.vendor === 'apple') {
    return { exterior: machine.kind === 'laptop' ? 'exterior/apple-laptop' : 'exterior/apple-desktop', board: 'board/apple-soc' };
  }
  return { exterior: `exterior/${machine.kind}`, board: `board/${machine.kind}` };
}
