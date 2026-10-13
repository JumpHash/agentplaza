// Graphics cards (assets/hardware/gpus): the types of specs.json and match.json, the validator for both,
// and matchCard, which maps a GPU that hardware detection reports to a card id. The format is described in
// assets/hardware/README.md ("Graphics cards").

import specsFile from '../../../../../assets/hardware/gpus/specs.json';
import matchFile from '../../../../../assets/hardware/gpus/match.json';

// `generic` is for the class fallbacks, which are not any one vendor's card.
export const VENDORS = ['nvidia', 'amd', 'intel', 'apple', 'generic'] as const;
export const SEGMENTS = ['consumer', 'workstation', 'datacenter', 'edge', 'laptop', 'integrated'] as const;
export const FORMS = ['pcie', 'sxm', 'oam', 'module', 'bga', 'integrated'] as const;
export const MACHINE_KINDS = ['server', 'workstation', 'laptop', 'edge', 'phone'] as const;
export const COOLING_TYPES = ['axial', 'flowthrough', 'blower', 'passive'] as const;
export const FIN_DIRECTIONS = ['along-length', 'across-length', 'vertical', 'toward-bracket', 'none'] as const;
export const SHAPE_KINDS = ['x-frame', 'stripe', 'light-bar', 'fin-stack', 'chamfer', 'logo', 'frame-rail'] as const;
export const POWER_TYPES = ['12v-2x6', '8pin', '6pin', 'eps8'] as const;
export const POWER_POSITIONS = ['top-end', 'end', 'top-mid'] as const;
export const PORT_TYPES = ['dp', 'hdmi', 'usbc'] as const;
export const LIGHT_KINDS = ['logo', 'light-bar', 'fan-ring', 'status-led', 'strip'] as const;
export const LIGHT_POSITIONS = ['side', 'top', 'fan', 'end', 'logo'] as const;
export const VENT_LEVELS = ['none', 'partial', 'full'] as const;
export const DIE_ARRANGEMENTS = ['ring', 'one-side', 'two-side'] as const;

export type Vendor = (typeof VENDORS)[number];
export type Segment = (typeof SEGMENTS)[number];
export type Form = (typeof FORMS)[number];
export type MachineKind = (typeof MACHINE_KINDS)[number];

export interface Dims {
  length: number;
  height: number;
  thickness: number;
}
export interface Fan {
  d_mm: number | null;
  x_mm: number | null;
  side: 'front' | 'back';
}
export interface Shape {
  kind: (typeof SHAPE_KINDS)[number];
  color?: string;
  at_mm?: number;
}
export interface Light {
  kind: (typeof LIGHT_KINDS)[number];
  color: string;
  at: (typeof LIGHT_POSITIONS)[number];
}
export interface Port {
  type: (typeof PORT_TYPES)[number];
  count: number;
}

// One reference card, module or baseboard. Fields follow the schema in assets/hardware/README.md.
export interface GpuSpec {
  id: string;
  name: string;
  vendor: Vendor;
  segment: Segment;
  form: Form;
  reference: boolean;
  size_mm: Dims | null;
  slots: number;
  cooling: { type: (typeof COOLING_TYPES)[number]; fans: Fan[]; fin_direction: (typeof FIN_DIRECTIONS)[number] };
  shroud: { base_color: string; accent_color: string | null; finish: 'matte' | 'metal'; shapes: Shape[] };
  backplate: { color: string | null; vents: (typeof VENT_LEVELS)[number] };
  power: { type: (typeof POWER_TYPES)[number]; count: number; position: (typeof POWER_POSITIONS)[number] }[];
  bracket: { color: string | null; ports: Port[] };
  lighting: Light[];
  layout?: { rows: number; cols: number; pitch_mm: [number, number] };
  die?: { die_mm: [number, number] | null; vram: { chips: number; arrangement: (typeof DIE_ARRANGEMENTS)[number]; gddr: string | null } };
  sources: string[];
  approx: string[];
  notes: string;
}
export interface GpuSpecFile {
  schema: 1;
  specs: GpuSpec[];
}

// A rule maps a detected GPU to a card. `vendor` '*' matches any vendor. Rules are tried in order and the first
// match wins, so the specific names come first and the fallbacks (fallback: true) come last.
export interface MatchRule {
  vendor: Vendor | '*';
  name_regex: string;
  machine_kind?: MachineKind;
  memory_mib?: { min?: number; max?: number };
  card: string;
  fallback?: boolean;
}
export interface MatchFile {
  schema: 1;
  rules: MatchRule[];
}
// What hardware detection reports for one GPU (noded: vendor "NVIDIA" | "AMD" | "Intel" | "Apple").
export interface GpuInput {
  vendor: string;
  name: string;
  memory_mib?: number | null;
}
export interface Match {
  card: string;
  fallback: boolean;
  rule: number;
}

export const GPU_SPECS = specsFile as unknown as GpuSpecFile;
export const GPU_MATCH = matchFile as unknown as MatchFile;

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v);
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const isInt = (v: unknown, lo: number, hi: number): boolean => isNum(v) && Number.isInteger(v) && v >= lo && v <= hi;
const oneOf = (list: readonly string[], v: unknown): boolean => typeof v === 'string' && list.includes(v);
const isHex = (v: unknown): boolean => typeof v === 'string' && /^#[0-9A-Fa-f]{6}$/.test(v);
const nullableHex = (v: unknown): boolean => v === null || isHex(v);
const dimOk = (v: unknown): boolean => isNum(v) && v >= 0.5 && v <= 1000;

// Every problem with one spec, as messages prefixed with its id. An empty list means the entry is valid.
export function validateSpec(spec: unknown): string[] {
  if (!isObj(spec)) return ['spec: not an object'];
  const errs: string[] = [];
  const id = typeof spec.id === 'string' ? spec.id : '?';
  const bad = (msg: string): number => errs.push(`${id}: ${msg}`);

  if (typeof spec.id !== 'string' || !/^[a-z0-9]+(-[a-z0-9]+)*$/.test(spec.id)) bad('id must be lower-case words joined by hyphens');
  if (typeof spec.name !== 'string' || spec.name === '') bad('name is required');
  if (!oneOf(VENDORS, spec.vendor)) bad(`vendor must be one of ${VENDORS.join(', ')}`);
  if (!oneOf(SEGMENTS, spec.segment)) bad(`segment must be one of ${SEGMENTS.join(', ')}`);
  if (!oneOf(FORMS, spec.form)) bad(`form must be one of ${FORMS.join(', ')}`);
  if (typeof spec.reference !== 'boolean') bad('reference must be true or false');
  if ((spec.form === 'integrated') !== (spec.segment === 'integrated')) bad('form integrated and segment integrated go together');
  if (spec.segment === 'laptop' && spec.form !== 'bga') bad('a laptop GPU is a bga package');
  if (spec.form === 'bga' && spec.segment !== 'laptop') bad('bga is for laptop GPUs only');
  if (spec.vendor === 'apple' && spec.segment !== 'integrated') bad('Apple GPUs are integrated');
  if ((spec.form === 'sxm' || spec.form === 'oam') && spec.segment !== 'datacenter') bad('sxm and oam are data-centre forms');

  // approx names top-level fields of this entry; a null size must say so.
  const approx = Array.isArray(spec.approx) && spec.approx.every((k) => typeof k === 'string' && k in spec) ? (spec.approx as string[]) : null;
  if (!approx) bad('approx must list top-level field names of this entry');
  const approxHas = (k: string): boolean => approx?.includes(k) === true;

  // size_mm: a board, module or box. Integrated entries have none.
  let length: number | null = null;
  if (spec.size_mm === null) {
    if (spec.form !== 'integrated' && !approxHas('size_mm')) bad('size_mm is null, so approx must say so');
  } else if (!isObj(spec.size_mm) || !dimOk(spec.size_mm.length) || !dimOk(spec.size_mm.height) || !dimOk(spec.size_mm.thickness)) {
    bad('size_mm needs length, height and thickness, each 0.5 to 1000 mm');
  } else {
    const { length: l, height: h, thickness: t } = spec.size_mm as unknown as Dims;
    length = l;
    if (spec.form === 'pcie' && l > 340) bad(`PCIe length ${l} mm is over 340`);
    if (spec.form === 'pcie' && (h < 30 || h > 150 || t < 15 || t > 70)) bad(`PCIe height ${h} or thickness ${t} mm is out of range`);
    if ((spec.form === 'sxm' || spec.form === 'oam') && (t < 5 || t > 200)) bad(`module thickness ${t} mm is out of range`);
  }
  if (!isNum(spec.slots) || spec.slots < 0 || spec.slots > 3) bad('slots must be 0 to 3');
  if (spec.form === 'pcie' && isNum(spec.slots) && spec.slots < 1) bad('a PCIe card takes at least one slot');

  const c = spec.cooling;
  if (!isObj(c) || !oneOf(COOLING_TYPES, c.type) || !oneOf(FIN_DIRECTIONS, c.fin_direction) || !Array.isArray(c.fans)) {
    bad('cooling needs type, fin_direction and fans');
  } else {
    if (c.type === 'passive' && c.fans.length > 0) bad('a passive card has no fans');
    if (c.type !== 'passive' && c.fans.length === 0) bad(`${c.type} cooling needs fans`);
    for (const f of c.fans) {
      if (!isObj(f) || !(f.d_mm === null || (isNum(f.d_mm) && f.d_mm >= 30 && f.d_mm <= 200)) || !(f.x_mm === null || isNum(f.x_mm)) || !oneOf(['front', 'back'], f.side)) {
        bad('a fan needs d_mm (30 to 200, or null), x_mm (a number, or null) and side front or back');
      } else if (isNum(f.x_mm) && length !== null && (f.x_mm < 0 || f.x_mm > length)) {
        bad(`fan x_mm ${f.x_mm} is outside the length`);
      }
    }
  }

  const sh = spec.shroud;
  if (!isObj(sh) || !isHex(sh.base_color) || !nullableHex(sh.accent_color) || !oneOf(['matte', 'metal'], sh.finish) || !Array.isArray(sh.shapes)) {
    bad('shroud needs base_color, accent_color, finish and shapes');
  } else {
    for (const s of sh.shapes) {
      const at = isObj(s) ? s.at_mm : undefined;
      if (!isObj(s) || !oneOf(SHAPE_KINDS, s.kind) || !(s.color === undefined || isHex(s.color))) bad('a shape needs a known kind and a hex colour');
      else if (at !== undefined && (!isNum(at) || at < 0 || (length !== null && at > length))) bad(`shape at_mm ${String(at)} is outside the length`);
    }
  }

  const bp = spec.backplate;
  if (!isObj(bp) || !nullableHex(bp.color) || !oneOf(VENT_LEVELS, bp.vents)) bad('backplate needs color (hex or null) and vents');

  if (!Array.isArray(spec.power) || !spec.power.every((p) => isObj(p) && oneOf(POWER_TYPES, p.type) && isInt(p.count, 1, 8) && oneOf(POWER_POSITIONS, p.position))) {
    bad('power is a list of {type, count 1 to 8, position}');
  }

  const br = spec.bracket;
  if (!isObj(br) || !nullableHex(br.color) || !Array.isArray(br.ports) || !br.ports.every((p) => isObj(p) && oneOf(PORT_TYPES, p.type) && isInt(p.count, 1, 8))) {
    bad('bracket needs color (hex or null) and ports');
  }

  if (!Array.isArray(spec.lighting) || !spec.lighting.every((l) => isObj(l) && oneOf(LIGHT_KINDS, l.kind) && isHex(l.color) && oneOf(LIGHT_POSITIONS, l.at))) {
    bad('lighting is a list of {kind, color, at}');
  }

  if (spec.layout !== undefined) {
    const lay = spec.layout;
    if (!isObj(lay) || !isInt(lay.rows, 1, 16) || !isInt(lay.cols, 1, 16) || !Array.isArray(lay.pitch_mm) || lay.pitch_mm.length !== 2 || !lay.pitch_mm.every((v) => isNum(v) && v > 0)) {
      bad('layout needs rows, cols and pitch_mm [x, z]');
    }
  }

  if (spec.form === 'bga') {
    const d = spec.die;
    if (!isObj(d) || !(d.die_mm === null || (Array.isArray(d.die_mm) && d.die_mm.length === 2 && d.die_mm.every(dimOk))) || !isObj(d.vram) || !isInt(d.vram.chips, 1, 16) || !oneOf(DIE_ARRANGEMENTS, d.vram.arrangement)) {
      bad('a bga entry needs die {die_mm, vram {chips, arrangement}}');
    } else if (d.die_mm === null && !approxHas('die')) {
      bad('die_mm is null, so approx must say so');
    }
  } else if (spec.die !== undefined) {
    bad('die is only for bga entries');
  }

  if (!Array.isArray(spec.sources) || !spec.sources.every((u) => typeof u === 'string' && u.trim() !== '')) bad('sources must be citation strings (URLs where the doc gives one)');
  else if (spec.reference === true && spec.form !== 'integrated' && spec.sources.length === 0) bad('a reference card needs a source');
  if (typeof spec.notes !== 'string') bad('notes must be a string');

  return errs;
}

// Every problem with specs.json: the schema, each entry, and duplicate ids.
export function validateSpecs(file: unknown): string[] {
  if (!isObj(file) || file.schema !== 1 || !Array.isArray(file.specs)) return ['specs file: needs schema 1 and a specs list'];
  const errs = file.specs.flatMap((s) => validateSpec(s));
  const ids = file.specs.map((s) => (isObj(s) ? s.id : undefined));
  for (const id of new Set(ids.filter((id) => typeof id === 'string'))) {
    if (ids.filter((x) => x === id).length > 1) errs.push(`${String(id)}: duplicate id`);
  }
  return errs;
}

// Every problem with match.json against the spec ids: each rule points at a spec, its regex compiles, and the
// fallbacks come after every specific rule.
export function validateMatch(file: unknown, specIds: readonly string[]): string[] {
  if (!isObj(file) || file.schema !== 1 || !Array.isArray(file.rules)) return ['match file: needs schema 1 and a rules list'];
  const errs: string[] = [];
  let seenFallback = false;
  file.rules.forEach((r, i) => {
    const at = `rules[${i}]`;
    if (!isObj(r)) return errs.push(`${at}: not an object`);
    if (r.vendor !== '*' && !oneOf(VENDORS, r.vendor)) errs.push(`${at}: vendor`);
    if (r.machine_kind !== undefined && !oneOf(MACHINE_KINDS, r.machine_kind)) errs.push(`${at}: machine_kind`);
    if (typeof r.name_regex !== 'string') errs.push(`${at}: name_regex is required`);
    else {
      try {
        new RegExp(r.name_regex, 'i');
      } catch {
        errs.push(`${at}: name_regex does not compile`);
      }
    }
    if (r.memory_mib !== undefined && !(isObj(r.memory_mib) && (r.memory_mib.min === undefined || isNum(r.memory_mib.min)) && (r.memory_mib.max === undefined || isNum(r.memory_mib.max)))) {
      errs.push(`${at}: memory_mib needs min and max numbers`);
    }
    if (typeof r.card !== 'string' || !specIds.includes(r.card)) errs.push(`${at}: card "${String(r.card)}" is not a spec`);
    if (r.fallback === true) seenFallback = true;
    else if (seenFallback) errs.push(`${at}: a specific rule comes after a fallback`);
  });
  return errs;
}

// The card for one detected GPU, or null when no rule matches. Vendors compare case-insensitively.
export function matchCard(gpu: GpuInput, machineKind?: string | null, rules: readonly MatchRule[] = GPU_MATCH.rules): Match | null {
  const vendor = gpu.vendor.toLowerCase();
  for (const [rule, r] of rules.entries()) {
    if (r.vendor !== '*' && r.vendor !== vendor) continue;
    if (r.machine_kind !== undefined && r.machine_kind !== machineKind) continue;
    if (r.memory_mib !== undefined) {
      const m = gpu.memory_mib;
      if (typeof m !== 'number') continue;
      if (r.memory_mib.min !== undefined && m < r.memory_mib.min) continue;
      if (r.memory_mib.max !== undefined && m > r.memory_mib.max) continue;
    }
    if (!new RegExp(r.name_regex, 'i').test(gpu.name)) continue;
    return { card: r.card, fallback: r.fallback === true, rule };
  }
  return null;
}
