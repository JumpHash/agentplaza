// The robot data formats (schema 1), as the JSON under assets/robots/ is typed. docs/design/robots.md
// is the reference; validate.ts checks a file against these types, rig.ts builds them.

export type Vec3 = [number, number, number];
export type Axis = 'x' | 'y' | 'z';

export const LOCOMOTIONS = ['treads', 'wheels', 'ball', 'unicycle', 'hover', 'fly', 'float', 'hop', 'bounce', 'waddle', 'walk2', 'walk4', 'walk6', 'scuttle', 'slither', 'tumble'] as const;
export type Locomotion = (typeof LOCOMOTIONS)[number];
export const GEO_TYPES = ['box', 'rbox', 'cyl', 'cone', 'sphere', 'torus', 'plane', 'ring', 'shape'] as const;
export type GeoType = (typeof GEO_TYPES)[number];
/** The materials a part may name: palette colours, the per-robot hues, and the glowing or clear ones. */
export const MATS = ['shell', 'shell2', 'panel', 'paper', 'trim', 'ink', 'signal', 'glass', 'eye', 'light', 'thruster', 'accent', 'clear'] as const;
export const EYE_SHAPES = ['round', 'bar', 'visor', 'ring', 'heart', 'arc'] as const;
export type EyeShape = (typeof EYE_SHAPES)[number];
export const WAVES = ['sin', 'abs', 'tri', 'saw', 'square', 'pulse', 'hold', 'halfsin'] as const;
export type Wave = (typeof WAVES)[number];
export const FX_KINDS = ['sparks', 'steam', 'bubbles', 'zzz', 'notes', 'hearts', 'dots', 'keys', 'ink', 'glow', 'trail', 'confetti'] as const;
export type FxKind = (typeof FX_KINDS)[number];
/** The bones every species has: a species without arms still has the arm bones (tentacles, stalks). */
export const REQUIRED_BONES = ['base', 'body', 'head', 'armL', 'armR', 'gripL', 'gripR', 'carry'] as const;
export const PERSONALITY = ['social', 'energy', 'focus', 'curiosity', 'play', 'tidy'] as const;
export const GRIPS = ['one', 'two', 'carry'] as const;
/** The props a channel can drive: rotation (radians), offset from rest, a uniform scale, or the eye lids. */
export const PROP_RE = /^(rot\.[xyz]|pos\.[xyz]|scale|lid)$/;

/** A part's material: a name from MATS, or an inline material (a colour, glow, opacity, flat). */
export interface InlineMat {
  color: string;
  emissive?: string;
  glow?: number;
  opacity?: number;
  flat?: boolean;
}
export type MatRef = string | InlineMat;

export interface Geo {
  type: GeoType;
  w?: number;
  h?: number;
  d?: number;
  r?: number;
  rt?: number;
  rb?: number;
  ri?: number;
  ro?: number;
  tube?: number;
  /** Degrees of a torus, or 180 for a half ring. */
  arc?: number;
  /** Segments: rbox [n], cyl/cone [radial], sphere [w, h], torus [radial, tubular]. */
  seg?: number[];
  /** A sphere cap: `top` is the upper hemisphere (a dome). */
  half?: 'top';
  points?: [number, number][];
}

/** One mesh: a geometry at a bone (or at the tool's origin), with a material and transform. */
export interface Part {
  bone?: string;
  geo: Geo;
  mat: MatRef;
  at?: Vec3;
  /** Degrees, as in the hardware format. */
  rot?: Vec3;
  scale?: Vec3 | number;
  /** The cartoon ink line. Ink-named parts are inked by default, not drawn as hulls; false drops it. */
  outline?: boolean;
  /** A human name for the part (the audit and the fixers cite it); optional. */
  name?: string;
  /** The part is intentionally not mirrored (the symmetry check skips it). */
  asym?: boolean;
}

export interface Bone {
  parent: string;
  at: Vec3;
}

/** A glowing eye: an optional glass socket (a dark disc) and a pupil that blinks and takes the state colour. */
export interface Eye {
  bone: string;
  at: Vec3;
  /** The glass socket's radius (default 0.1). */
  r?: number;
  /** The pupil's radius. */
  pupil: number;
  shape: EyeShape;
  /** Whether a glass disc sits under the pupil (default true). */
  glass?: boolean;
  /** The pupil's stretch in y (1 = round). */
  stretch?: number;
  /** The eye is intentionally not mirrored (the symmetry check skips it). */
  asym?: boolean;
}

/** A bone that turns while the robot is in a state: rad/s about an axis (`offline` defaults to 0). */
export interface Spin {
  bone: string;
  axis: Axis;
  walk: number;
  idle: number;
  offline?: number;
}

export interface Gait {
  speed: number;
  freq: number;
  bob: number;
  sway: number;
  lean: number;
  sideways: boolean;
}

/** One procedural channel: value = base + amp × wave(2π·freq·t + phase), or `rate` rad/s accumulated. */
export interface Channel {
  bone: string;
  prop: string;
  /** A number, or "gait.<name>" for the species' gait value. */
  base?: number | string;
  amp?: number | string;
  freq?: number | string;
  phase?: number;
  wave?: Wave;
  /** For `pulse`: the fraction of each period the wave is high (0..1). */
  duty?: number;
  /** `in-out`: the amplitude follows a sine envelope over the clip's `dur`. */
  env?: 'in-out';
  /** Set the value at once, without the blend (a blink must snap). */
  snap?: boolean;
  /** rad/s, accumulated while the clip plays (wheels roll). Not blended. */
  rate?: number;
}

export interface FxSpec {
  kind: FxKind;
  at: string;
  /** Seconds between spawns while the clip plays (default 0.6). */
  every?: number;
}

export interface Clip {
  loop?: boolean;
  /** Seconds, for a one-shot. */
  dur?: number;
  channels: Channel[];
  fx?: FxSpec[];
}

export interface Species {
  schema: 1;
  id: string;
  name: string;
  blurb: string;
  accent: string;
  height: number;
  label_top: number;
  locomotion: Locomotion;
  gait: Gait;
  bones: Record<string, Bone>;
  parts: Part[];
  eyes: Eye[];
  spin: Spin[];
  feet: string[];
  floats: boolean;
  personality: Record<(typeof PERSONALITY)[number], number>;
  quirks: { clip: string; weight: number }[];
  clips: Record<string, Clip>;
  /** A shared clip id → a species clip id that replaces it for this species. */
  overrides: Record<string, string>;
  /** Extra parts only the orchestrator wears (the rover's antenna and trim). */
  orchestrator?: { parts: Part[] };
}

export interface Tool {
  schema: 1;
  id: string;
  name: string;
  grip: (typeof GRIPS)[number];
  /**
   * How a one-hand tool sits in the fist (its parts hang along −y from the grip): `forward` (the default:
   * turned to point ahead of the hand, a hammer or a pen), `down` (hangs: a mug, a book, a watering can),
   * `up` (stands: a flag, a wand).
   */
  aim?: 'forward' | 'down' | 'up';
  /** Parts relative to the tool's origin at the grip (no bone). */
  parts: Part[];
  tip: Vec3;
}

export interface Palette {
  ink: string;
  sheet: string;
  paper: string;
  rule: string;
  signal: string;
  ok: string;
  info: string;
  inkMuted: string;
}

export interface Index {
  schema: 1;
  species: string[];
  tools: string[];
  props: string[];
}

export interface ClipsFile {
  schema: 1;
  clips: Record<string, Clip>;
}
