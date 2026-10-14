// A machine's picture spec: what a machine is drawn as (its look, its GPU cards and their tints, its memory sticks and
// its drives), and the key that names that picture. The engine draws the thumbnails from the spec; this module draws
// nothing and holds no renderer.

import { lookOf, type Look, type MachineKind } from './machine-look';

/** CSS px of a list thumbnail. */
export const THUMB_W = 160;
export const THUMB_H = 120;

export interface PictureSpec {
  kind: MachineKind;
  /** Lower-case maker as the fleet reports it ('apple', 'dell', ...). */
  vendor: string | null;
  /** Total RAM in MiB. */
  memory_mib: number;
  /** One entry per accelerator card, vendor as reported ('NVIDIA', 'AMD', 'Intel', 'Apple', ...). */
  gpus: { vendor: string }[];
  /** Storage device count: only the workstation's M.2 slot draws them. */
  drives?: number;
  /** The look drawn, when the kind and vendor do not say it (a workstation's form: laptop, mini, tower, rack). */
  look?: Look;
}

/** The look a spec is drawn as: its own when it has one, else the kind's (with the vendor's Apple shapes). */
export function lookOfSpec(spec: PictureSpec): Look {
  return spec.look ?? lookOf(spec.kind, spec.vendor);
}

// ---- The picture's fields ---------------------------------------------------------------------

/** Bumped when the same spec would draw differently, so cached pictures are not reused. */
const PICTURE_VERSION = 1;
const MAX_CARDS = 4;
/**
 * The GPU cards a look draws at most (the laptop has one chip). Looks not listed have none (Apple SoCs, edge, phone).
 * Workstation: capped at 2, the two the model has (a third card has no free place inside the case).
 * Server: capped at 4, the four card parts its model has.
 */
const CARD_LOOKS = new Map<Look, number>([
  ['workstation', 2],
  ['server', MAX_CARDS],
  ['laptop', 1],
]);
/** The DIMM slots each look's model has. Its `dimms` param is the stick count. */
const MEMORY_SLOTS = new Map<Look, number>([
  ['workstation', 4],
  ['laptop', 4],
  ['server', 16],
]);
/** The GPU shroud colour per vendor. Other vendors keep the palette's shroud. */
const CARD_TINT = new Map<string, number>([
  ['nvidia', 0x76b900],
  ['amd', 0xc8102e],
  ['intel', 0x0068b5],
]);

const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));

interface Picture {
  look: Look;
  /** The tint per visible card, in card order (null: the palette's shroud). */
  tints: (number | null)[];
  sticks: number;
  drives: number;
}

/** The sticks for `mib` of RAM: a stick is a power of two of at least 8 GiB, sized to fill the slots evenly. */
function sticksFor(mib: number, slots: number): number {
  const gib = Math.max(0, Number.isFinite(mib) ? mib : 0) / 1024;
  let p = 1;
  while (p < Math.ceil(gib / slots)) p *= 2;
  const stick = Math.max(8, p);
  return clamp(Math.round(gib / stick), 1, slots);
}

function drivesFor(drives: number | undefined): number {
  return drives === undefined || !Number.isFinite(drives) ? 1 : clamp(Math.round(drives), 0, 2);
}

function pictureOf(spec: PictureSpec): Picture {
  const look = lookOfSpec(spec);
  const max = CARD_LOOKS.get(look) ?? 0;
  const tints = spec.gpus.slice(0, max).map((g) => CARD_TINT.get(String(g.vendor ?? '').toLowerCase()) ?? null);
  const slots = MEMORY_SLOTS.get(look) ?? 0;
  return {
    look,
    tints,
    sticks: slots > 0 ? sticksFor(spec.memory_mib, slots) : 0,
    drives: look === 'workstation' ? drivesFor(spec.drives) : 0,
  };
}

/** FNV-1a twice, with two offsets: a 64-bit hex digest of the text. */
function digest(text: string): string {
  let a = 0x811c9dc5;
  let b = 0x9dc5811c;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    a = Math.imul(a ^ c, 0x01000193);
    b = Math.imul(b ^ c, 0x01000193);
  }
  return (a >>> 0).toString(16).padStart(8, '0') + (b >>> 0).toString(16).padStart(8, '0');
}

/** A deterministic key of exactly the fields that change the picture: the look, the cards and their tints, the sticks, the drives. */
export function specKey(spec: PictureSpec): string {
  const p = pictureOf(spec);
  const tints = p.tints.map((t) => (t === null ? '-' : t.toString(16))).join(',');
  return `m${digest([PICTURE_VERSION, p.look, tints, p.sticks, p.drives].join('|'))}`;
}
