/**
 * The ENTER sequence: the Machines view flies into a machine, its case opens, the camera dives over the case onto the
 * machine's board, and the board's rooms build in. Back (or Esc) runs the same timeline in reverse from
 * wherever it stands. `stageAt` is the sequence as keyframes on one time axis (seconds); Entry drives a scene with
 * them each frame. A direct load of #/floor/<id> has no exterior: only the board's build wave plays.
 */
import * as THREE from 'three';
import type { FleetResponse } from '../../api';
import type { FleetScene } from './fleet';
import type { Model } from './hardware';

export const clamp01 = (v: number): number => Math.min(1, Math.max(0, v));
export const easeInOut = (t: number): number => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
export const easeOutCubic = (t: number): number => 1 - Math.pow(1 - t, 3);
export const easeOutBack = (t: number): number => {
  const c1 = 1.4;
  const c3 = c1 + 1;
  return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2);
};
/** Smootherstep: 0 and 1 with zero first and second derivatives at the ends, so a span starts and stops without a jolt. */
export const smoother = (t: number): number => {
  const x = clamp01(t);
  return x * x * x * (x * (x * 6 - 15) + 10);
};
/** The progress of t through [a, b]: 0 before a, 1 after b, eased in between (smoother by default: C2 at both ends). */
export function span(t: number, a: number, b: number, ease: (x: number) => number = smoother): number {
  if (b <= a) return t >= b ? 1 : 0;
  return ease(clamp01((t - a) / (b - a)));
}

/** The sequence's length, seconds: the fly-in, the open, the dive and the first of the build wave. */
export const ENTRY_SECONDS = 2.4;
/** The keyframes: [start, end] in seconds on the timeline. */
export const KEY = {
  dim: [0, 0.4], // the other machines fold down, their cables fade
  fly: [0.15, 1.25], // the camera flies to the exterior's anchors.camera
  open: [0.95, 1.5], // the exterior's open part slides or swings (0 -> 1)
  light: [1.0, 1.4], // the soft light spills out of the opening
  lightOut: [1.6, 2.0],
  dive: [1.05, 2.15], // the camera dives over the case and settles at the board's default view (it starts within the fly's tail: no stop)
  extFade: [1.45, 1.95], // the exterior fades out
  boardFade: [1.45, 1.85], // the board fades in under it (a match cut)
  atom: [1.7, 2.1], // the atom glows up
  direct: [1.2, 1.6], // a direct load: the board fades in over this range (it is shown from the wave on)
} as const satisfies Record<string, readonly [number, number]>;
/** The build wave: each room rises from its slot, staggered, and the robots pop in after the last room. */
export const WAVE = {
  start: 1.7,
  rise: 0.25,
  stagger: 0.04, // the gap between the rooms' rises: 40 ms, compressed for a board with many rooms
  budget: 0.5, // the rooms' starts all fall within this many seconds
  robotsBefore: 0.15, // the robots pop in as the last room's rise finishes
  robotsStagger: 0.02,
  robotsBudget: 0.25,
  pop: 0.3,
} as const;
/** The direct load's board starts shown, at the wave's start. */
export const DIRECT_START = WAVE.start;
/** How far the other machines sink when the sequence starts, in units. */
const SINK = 0.6;
/** How far a room sits below its slot before it rises, in units. */
const ROOM_DROP = 0.8;

/** The gap between consecutive rooms' rises (seconds): WAVE.stagger, or less when the rooms would overrun the budget. */
export const roomGap = (rooms: number): number => (rooms <= 1 ? 0 : Math.min(WAVE.stagger, WAVE.budget / (rooms - 1)));
/** The gap between consecutive robots' pops (seconds). */
export const robotGap = (robots: number): number => (robots <= 1 ? 0 : Math.min(WAVE.robotsStagger, WAVE.robotsBudget / (robots - 1)));
/** When the room's rise starts and ends (seconds). */
export const riseSpan = (i: number, rooms: number): [number, number] => {
  const a = WAVE.start + i * roomGap(rooms);
  return [a, a + WAVE.rise];
};
/** When the last room's rise ends. */
export const waveEnd = (rooms: number): number => WAVE.start + Math.max(0, rooms - 1) * roomGap(rooms) + WAVE.rise;
/** When the first robot pops in: as the last room's rise finishes. */
export const robotsAt = (rooms: number): number => waveEnd(rooms) - WAVE.robotsBefore;
/** The timeline's end: at least ENTRY_SECONDS, after the last room rises and after the last robot has popped in. */
export const endAt = (rooms: number, robots: number): number =>
  Math.max(ENTRY_SECONDS, waveEnd(rooms) + 0.1, robotsAt(rooms) + Math.max(0, robots - 1) * robotGap(robots) + WAVE.pop + 0.02);

/** The sequence's state at one time. Every value is 0..1 unless noted. */
export interface Stage {
  /** The other machines fold down (0 at rest, 1 folded). */
  dim: number;
  /** The camera's fly-in to the exterior's anchors.camera. */
  fly: number;
  /** The exterior's open part (anchors.open). */
  open: number;
  /** The light spilling out of the opening. */
  light: number;
  /** The camera's dive, and the board's move from the exterior's place to its own (0 matched, 1 its own). */
  dive: number;
  /** The exterior's opacity. */
  ext: number;
  /** The other machines' opacity, and their cables'. */
  others: number;
  cables: number;
  /** The board's opacity and its atom's. */
  board: number;
  atom: number;
}

/** The sequence at time t. A direct load (no exterior) plays the board's fade and the build wave only. */
export function stageAt(t: number, direct: boolean): Stage {
  const at = (k: readonly [number, number], ease?: (x: number) => number): number => span(t, k[0], k[1], ease);
  if (direct) {
    return { dim: 0, fly: 0, open: 0, light: 0, dive: 1, ext: 0, others: 0, cables: 0, board: at(KEY.direct), atom: at(KEY.atom) };
  }
  const dim = at(KEY.dim);
  const extFade = at(KEY.extFade);
  const lightOn = at(KEY.light) * (1 - at(KEY.lightOut));
  return {
    dim,
    fly: at(KEY.fly),
    open: at(KEY.open),
    light: lightOn,
    dive: at(KEY.dive),
    ext: 1 - extFade,
    others: (1 - 0.65 * dim) * (1 - extFade),
    cables: (1 - 0.8 * dim) * (1 - extFade),
    board: at(KEY.boardFade),
    atom: at(KEY.atom),
  };
}

/** A camera pose: where it sits, what it looks at, and (optionally) the fog's near and far. */
export interface Pose {
  position: THREE.Vector3;
  target: THREE.Vector3;
  fog?: { near: number; far: number };
}

/** The board the plaza shows: its group (at the origin), its model and its atom's group. */
export interface BoardPart {
  group: THREE.Object3D;
  model: Model;
  atom: THREE.Object3D | null;
}

/** The exterior that the Machines view shows for the entered machine (built by the plaza from the handoff). */
export interface Exterior {
  fleet: FleetScene;
  data: FleetResponse;
  machineId: string;
  model: Model;
}

export interface EntryContext {
  camera: THREE.PerspectiveCamera;
  /** The orbit's target and enabled flag (OrbitControls, or a stub in tests). */
  orbit: { target: THREE.Vector3; enabled: boolean };
  scene: THREE.Scene;
  /** The exterior, or null for a direct load. */
  exterior: Exterior | null;
  /** The flight's light, in the scene already (the plaza adds it at its build): none is added at ENTER. */
  light?: THREE.PointLight;
  /** The Machines pose the sequence starts from (null for a direct load). */
  from: Pose | null;
  board: () => BoardPart | null;
  /** The rooms (their group is moved with the board). */
  rooms: THREE.Object3D;
  /** The robots' groups, in a stable order. */
  bots: () => THREE.Object3D[];
  /** The plaza's default view, or null until the board is built. */
  defaultPose: () => Pose | null;
  /** The scene's look at k (0 the Machines view's night, 1 the plaza's board floor): backdrop, ground and fog. */
  look: (k: number) => void;
  /** Called when a reverse sequence reaches its start. */
  onLeft: () => void;
}

/** The fleet a Machines view hands to the plaza for an ENTER. */
export interface Handoff {
  machineId: string;
  fleet: FleetResponse;
  from: Pose;
}

let pending: Handoff | null = null;
let returning: { pose: Pose; fleet: FleetResponse } | null = null;

/** The Machines view calls this before it navigates to the machine's plaza. */
export function beginEntry(h: Handoff): void {
  pending = h;
}
/** The plaza takes the handoff for its machine (once). */
export function takeEntry(machineId: string): Handoff | null {
  const h = pending && pending.machineId === machineId ? pending : null;
  pending = null;
  return h;
}
/** The plaza hands the Machines view the pose to return to (and the fleet it showed), when a Back has played out. */
export function setReturn(pose: Pose, fleet: FleetResponse): void {
  returning = { pose, fleet };
}
/** The Machines view takes the pose a Back returned from, with the fleet (once). */
export function takeReturn(): { pose: Pose; fleet: FleetResponse } | null {
  const r = returning;
  returning = null;
  return r;
}

interface Base {
  o: number;
  d: boolean;
  f: number | null;
}
const bases = new WeakMap<THREE.Material, Base>();
type Uniforms = Record<string, { value: number }> | undefined;
/** The last k applied to a tree: a steady tree (no change this frame) is not walked again. */
const lastK = new WeakMap<THREE.Object3D, number>();

/**
 * Multiplies every mesh material's opacity under root by k (0..1), against the opacity it had before any fade. A
 * shader's `fade` uniform (the ink hulls) follows. At k = 1 the material is back at its own state. The material's
 * `transparent` flag is never changed here: three.js keys a program on it (`opaque`), so a change would link a new
 * shader in mid-flight. prepareFade sets it once, before the tree is compiled. occ, when given, is each material's further
 * share (0..1: occlusionOf), multiplied in every frame; without it the fade is applied once per k, as before.
 */
export function fadeTree(root: THREE.Object3D, k: number, occ?: ReadonlyMap<THREE.Material, number>): void {
  if (!occ) {
    if (lastK.get(root) === k) return;
    lastK.set(root, k);
  } else lastK.delete(root); // an occluded fade is applied every frame: a steady k is applied again after it
  root.traverse((obj) => {
    const mesh = obj as THREE.Mesh;
    if (!mesh.isMesh) return;
    const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
    for (const m of mats) {
      const uniforms = (m as THREE.ShaderMaterial).uniforms as Uniforms;
      let b = bases.get(m);
      if (!b) {
        b = { o: m.opacity, d: m.depthWrite, f: uniforms?.fade ? uniforms.fade.value : null };
        bases.set(m, b);
      }
      const a = k * (occ?.get(m) ?? 1); // occ: the material's share the camera may see (1 unless occluded: see entry's occlusionOf)
      m.opacity = b.o * a;
      m.depthWrite = a >= 1 ? b.d : false;
      if (uniforms?.fade && b.f !== null) uniforms.fade.value = b.f * a;
    }
  });
}

/**
 * Makes every material under root transparent, once, so a fade never changes its program: the plaza calls it when it
 * builds a part (warm), before the part is compiled. A fully opaque material (opacity 1) draws the same either way.
 */
export function prepareFade(root: THREE.Object3D): void {
  root.traverse((obj) => {
    const mesh = obj as THREE.Mesh;
    if (!mesh.isMesh) return;
    const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
    for (const m of mats) m.transparent = true;
  });
}

/** Gives a room's materials their own copies, so its fade does not touch the other rooms that share them. */
function ownMaterials(root: THREE.Object3D, owned: THREE.Material[]): void {
  root.traverse((obj) => {
    const mesh = obj as THREE.Mesh;
    if (!mesh.isMesh || mesh.userData.entryOwned) return;
    const copy = (m: THREE.Material): THREE.Material => {
      const c = m.clone();
      owned.push(c);
      return c;
    };
    mesh.material = Array.isArray(mesh.material) ? mesh.material.map(copy) : copy(mesh.material);
    mesh.userData.entryOwned = true;
  });
}

/** Gives a room its own material copies, once. The plaza calls it when it builds each district, so no ENTER frame does it. */
export function ownRoomMaterials(root: THREE.Object3D): void {
  ownMaterials(root, []);
  prepareFade(root);
}

/** Lights under a part, with their base intensities (found once). Each follows the part fade k: intensity = base x k, never visible. */
/**
 * Shows or hides a part's drawn objects (meshes, points, lines, sprites), never its lights or its groups. A light counts
 * in every program only while it is visible under visible parents, so the lights stay as they are (intensity follows
 * the fade) and only the drawing is switched. Each object's own visibility is kept (some are hidden on purpose, e.g. a
 * glow plate), and restored when the part shows again. Called at a threshold crossing, not every frame.
 */
const shownOf = new WeakMap<THREE.Object3D, boolean>();
const savedVisible = new WeakMap<THREE.Object3D, boolean>();
export function setMeshesShown(root: THREE.Object3D, on: boolean): void {
  if (shownOf.get(root) === on) return;
  shownOf.set(root, on);
  root.traverse((o) => {
    const x = o as THREE.Object3D & { isMesh?: boolean; isPoints?: boolean; isLine?: boolean; isSprite?: boolean; isLight?: boolean };
    if (x.isLight || !(x.isMesh || x.isPoints || x.isLine || x.isSprite)) return;
    if (!on) {
      if (!savedVisible.has(o)) savedVisible.set(o, o.visible);
      o.visible = false;
    } else {
      o.visible = savedVisible.get(o) ?? true;
      savedVisible.delete(o);
    }
  });
}

const followLightsOf = new WeakMap<THREE.Object3D, { light: THREE.Light; base: number }[]>();
function followLights(room: THREE.Object3D, e: number): void {
  let lights = followLightsOf.get(room);
  if (!lights) {
    lights = [];
    room.traverse((o) => {
      const l = o as THREE.Light;
      if (l.isLight) lights!.push({ light: l, base: l.intensity });
    });
    followLightsOf.set(room, lights);
  }
  for (const { light, base } of lights) light.intensity = base * e;
}

const ORIGIN = new THREE.Vector3();
const UP = new THREE.Vector3(0, 1, 0);

/**
 * How far the flight's end stands back from the model's camera anchor, along the anchor's view line (world units). The
 * anchors sit about a unit outside the case: there the case's walls and lid fill the frame and the camera is among them
 * (a case's parts are merged per material, so their boxes are the whole case and the occlusion fade cannot separate
 * them). Pulled back by this, the camera keeps about four units from every drawn part (the entry-path test).
 */
export const FLY_STANDOFF = 5;
/** How far the dive's control point rises above the case's middle, world units: the dive goes over the case, not into it. */
export const DIVE_LIFT = 6;
/**
 * The exterior's distance fade, world units from its box to the camera: the exterior is gone at or inside EXT_NEAR and
 * whole beyond EXT_FAR, on top of the time fade (KEY.extFade). The dive then shows the board, never the case from close up.
 */
export const EXT_NEAR = 8;
export const EXT_FAR = 12;

/** The exterior's camera anchor and atom, in world space, from the machine's current transform. */
function exteriorView(x: Exterior): { position: THREE.Vector3; target: THREE.Vector3; atom: THREE.Vector3 } {
  const root = x.fleet.machines.get(x.machineId)!.root;
  root.updateMatrixWorld(true);
  const cam = x.model.anchors?.camera ?? { at: [0, 0, 0], look: [0, 0, 0] };
  const atom = x.model.anchors?.atom ?? [0, 0, 0];
  const position = new THREE.Vector3(...cam.at).applyMatrix4(root.matrixWorld);
  const target = new THREE.Vector3(...cam.look).applyMatrix4(root.matrixWorld);
  position.add(position.clone().sub(target).normalize().multiplyScalar(FLY_STANDOFF));
  return {
    position,
    target,
    atom: new THREE.Vector3(...atom).applyMatrix4(root.matrixWorld),
  };
}

/**
 * The board's transform at the match: its atom sits on the exterior's atom, and the board is scaled to the
 * exterior's width. The board eases from there to its own place at the origin as the camera dives.
 */
function matchOf(x: Exterior, board: BoardPart, atom: THREE.Vector3, rotY: number): { pos: THREE.Vector3; scale: number } {
  const scale = x.model.size[0] / Math.max(1e-6, board.model.size[0]);
  const bAtom = board.model.anchors?.atom ?? [0, 0, 0];
  const off = new THREE.Vector3(bAtom[0] * scale, bAtom[1] * scale, bAtom[2] * scale).applyAxisAngle(UP, rotY);
  return { pos: atom.clone().sub(off), scale };
}

/** Scratch for the box tests (probeInside, occlusionOf): an instance's world matrix, a corner, and a mesh's world box. */
const probeM = new THREE.Matrix4();
const probeV = new THREE.Vector3();
const probeBox = new THREE.Box3();

/** The world-space box of `local` (a geometry's box) carried by M: the box of its eight corners, in probeBox. */
function worldBox(local: THREE.Box3, M: THREE.Matrix4): THREE.Box3 {
  probeBox.makeEmpty();
  for (let i = 0; i < 8; i++) {
    probeV.set(i & 1 ? local.max.x : local.min.x, i & 2 ? local.max.y : local.min.y, i & 4 ? local.max.z : local.min.z);
    probeBox.expandByPoint(probeV.applyMatrix4(M));
  }
  return probeBox;
}

/**
 * The headless check (run only when __apzTrace is set): the drawn meshes of the exterior whose world-space bounding box
 * holds the camera. Hidden meshes and meshes under a hidden object are skipped; each instance of an InstancedMesh has
 * its own box. A mesh at or below opacity 0.05 is the sequence's own fade-out (transparent by then): it is not a solid
 * and is counted apart as `faded`. The root's world matrices are brought up to date first (the lid moves each frame).
 */
/** The headless check: a drawn part within this many world units of the camera counts as near (probeInside). */
const PROBE_NEAR = 0.3;
export function probeInside(root: THREE.Object3D, p: THREE.Vector3): { solid: string[]; faded: number; near: number } {
  const out = { solid: [] as string[], faded: 0, near: 0 };
  root.updateMatrixWorld(true);
  const walk = (o: THREE.Object3D): void => {
    if (!o.visible) return;
    const mesh = o as THREE.InstancedMesh;
    if (mesh.isMesh) {
      const g = mesh.geometry;
      if (!g.boundingBox) g.computeBoundingBox();
      const box = g.boundingBox!;
      // The least distance from the camera to the mesh's world box (any instance's): 0 when the camera is inside.
      let d = Infinity;
      if (box.isEmpty()) d = Infinity;
      else if (mesh.isInstancedMesh) {
        for (let i = 0; i < mesh.count; i++) d = Math.min(d, worldBox(box, mesh.getMatrixAt(i, probeM).premultiply(mesh.matrixWorld)).distanceToPoint(p));
      } else d = worldBox(box, mesh.matrixWorld).distanceToPoint(p);
      if (d < PROBE_NEAR) {
        const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
        const drawn = Math.max(...mats.map((m) => m.opacity)) > 0.05;
        if (!drawn) out.faded += d === 0 ? 1 : 0;
        else if (d === 0) out.solid.push(mesh.name || g.type);
        else out.near++;
      }
    }
    for (const child of o.children) walk(child);
  };
  walk(root);
  return out;
}

/** The occlusion ramp, world units: a part whose box the camera is inside is transparent; it is whole beyond OCC_FAR. */
const OCC_NEAR = 0.1;
const OCC_FAR = 1.2;
const occlusion = new Map<THREE.Material, number>(); // per frame: occlusionOf clears it

/**
 * How much of each material the camera may see, for the entered machine: each drawn mesh's world-space box (each
 * instance's own, for an InstancedMesh) ramps its factor from 0 at OCC_NEAR to 1 at OCC_FAR from the camera, and a material
 * shared by several meshes takes the least of theirs. The flight passes through the case's own parts (the rack, the
 * walls, the lid, whose boxes are the merged parts' boxes), so they fade as the camera reaches them, and the camera is not
 * inside a drawn part's box (the headless check counts that per frame). Only opacity follows (fadeTree); no material
 * changes its transparency or its program.
 */
function occlusionOf(root: THREE.Object3D, p: THREE.Vector3): Map<THREE.Material, number> {
  occlusion.clear();
  root.updateMatrixWorld(true); // the lid and the other moving parts were set this frame
  const walk = (o: THREE.Object3D): void => {
    if (!o.visible) return;
    const mesh = o as THREE.InstancedMesh;
    if (mesh.isMesh) {
      const g = mesh.geometry;
      if (!g.boundingBox) g.computeBoundingBox();
      const box = g.boundingBox!;
      if (!box.isEmpty()) {
        let d = Infinity;
        if (mesh.isInstancedMesh) {
          for (let i = 0; i < mesh.count; i++) {
            const M = mesh.getMatrixAt(i, probeM).premultiply(mesh.matrixWorld);
            if (Math.abs(M.determinant()) > 1e-12) d = Math.min(d, worldBox(box, M).distanceToPoint(p));
          }
        } else d = worldBox(box, mesh.matrixWorld).distanceToPoint(p);
        const f = smoother(clamp01((d - OCC_NEAR) / (OCC_FAR - OCC_NEAR)));
        for (const m of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) occlusion.set(m, Math.min(occlusion.get(m) ?? 1, f));
      }
    }
    for (const child of o.children) walk(child);
  };
  walk(root);
  return occlusion;
}

/**
 * Drives the sequence. step(dt) is called each frame before the scene draws; it moves the camera (the orbit is
 * disabled while the sequence drives it) and sets every stage's state. back() reverses from where it stands.
 */
export class EntrySequence {
  private t: number;
  private dir: 1 | -1 = 1;
  private finished = false;
  private left = false; // the reverse reached its start and called onLeft: no further Back
  private light: THREE.PointLight | null = null;
  private atomBase: THREE.Vector3 | null = null;
  private owned: THREE.Material[] = [];
  private readonly rest: Map<THREE.Object3D, { y: number; sy: number }> = new Map();
  private smoothDt = 1 / 60; // the frame time, smoothed (step)
  private held = false; // held at its start (an arrival that waits for its build): the state applies, the time does not run
  private lastPose: Pose | null = null; // the default view the dive last had (step)
  private readonly extBox = new THREE.Box3(); // the entered machine's world box, this frame (step)
  private readonly scratch = {
    fly: new THREE.Vector3(),
    flyT: new THREE.Vector3(),
    ctl: new THREE.Vector3(),
    pos: new THREE.Vector3(),
    tgt: new THREE.Vector3(),
  };

  constructor(
    private readonly ctx: EntryContext,
    t0 = 0,
  ) {
    this.t = t0;
    if (ctx.exterior) {
      this.light = ctx.light ?? new THREE.PointLight(0xffb070, 0, 40, 1.6);
      if (!ctx.light) ctx.scene.add(this.light);
    }
    ctx.orbit.enabled = false;
  }

  /** Whether the forward flight has finished (and no reverse has started since). */
  get isFinished(): boolean {
    return this.finished && !this.left;
  }

  /** Whether the sequence is driving the camera (the orbit must not update). */
  get driving(): boolean {
    return !this.finished;
  }

  /** Holds the sequence at its start (on) or lets its time run (off). Its state is applied every step either way. */
  hold(on: boolean): void {
    this.held = on;
  }

  /** The time on the timeline, seconds. */
  get time(): number {
    return this.t;
  }

  /**
   * Reverses the sequence from where it stands (Back, or Esc): mid-flight, or from its end once it has finished (the full
   * flight back to the Machines view). A second call while leaving does nothing; a leave that has completed is final.
   */
  back(): void {
    if (this.left || this.dir < 0) return;
    if (this.finished) {
      this.finished = false; // the camera is driven again: the orbit is off until the flight is back at its start
      this.ctx.orbit.enabled = false;
    }
    this.dir = -1;
  }

  /** Removes the light and the room copies' materials. */
  dispose(): void {
    if (this.light) {
      if (!this.ctx.light) {
        this.ctx.scene.remove(this.light);
        this.light.dispose();
      }
      this.light = null;
    }
    for (const m of this.owned.splice(0)) m.dispose();
  }

  /**
   * Advances the sequence by the frame's wall time, dt seconds, and applies its state to the scene. The time is smoothed:
   * a slow frame moves the sequence by its share, with no lurch; a gap over 250 ms (a hidden tab) advances one frame.
   * The plaza starts the flight only once its board and default view exist (floor.ts), so nothing holds mid-flight.
   */
  step(dt: number): void {
    if (this.finished) return;
    const c = this.ctx;
    const direct = c.from === null;
    const min = direct ? KEY.direct[0] : 0;
    const rooms = c.rooms.children.length;
    const robots = c.bots();
    const end = endAt(rooms, robots.length);
    const board = c.board();
    const live = c.defaultPose();
    if (live) this.lastPose = live;
    const pose = live ?? this.lastPose; // the dive's target: the last known default view, should the plaza be busy this frame
    if (!this.held) {
      const raw = dt > 0.25 ? 1 / 60 : Math.max(0, dt);
      this.smoothDt += (raw - this.smoothDt) * 0.3;
    }
    const t = Math.max(min, Math.min(end, this.t + (this.held ? 0 : this.dir * this.smoothDt)));
    this.t = t;

    const st = stageAt(t, direct);
    const x = c.exterior;
    let pos: THREE.Vector3 | null = null;
    let tgt: THREE.Vector3 | null = null;
    let rotY = 0;
    let ext = st.ext; // the entered machine's exterior opacity (the time fade, cut by the distance fade below)
    let boardK = st.board; // the board's opacity (takes over as the exterior goes: a match cut)

    if (x && c.from) {
      const E = exteriorView(x);
      const root = x.fleet.machines.get(x.machineId)!.root;
      rotY = root.rotation.y;
      x.fleet.update(dt, x.data);
      x.fleet.setOpen(x.machineId, st.open);
      x.fleet.lod(c.camera, E.target, dt);
      const roots = new Set<THREE.Object3D>();
      for (const [id, mc] of x.fleet.machines) {
        roots.add(mc.root);
        mc.root.visible = true; // the group stays (its lights count in every program); its drawn objects follow the fade
        if (id === x.machineId) continue; // the entered machine's fade is set below, once the camera is placed
        setMeshesShown(mc.root, st.others > 0.002);
        mc.root.position.y -= SINK * st.dim; // after update, which sets the lift each frame
        fadeTree(mc.root, st.others);
        followLights(mc.root, st.others);
      }
      // The cables and status rings: the fleet's other children, folded with the machines.
      for (const child of x.fleet.group.children) {
        if (roots.has(child)) continue;
        child.visible = true;
        setMeshesShown(child, st.cables > 0.002);
        fadeTree(child, st.cables);
        followLights(child, st.cables);
      }
      if (this.light) {
        this.light.position.copy(E.atom);
        this.light.intensity = 10 * st.light;
      }
      // The camera: one path, C1 throughout. The fly (from the Machines pose to the exterior's camera anchor) is the
      // start of a Bezier that dives over the case to the board's default view. The dive starts while the fly's
      // tail still eases out, so the camera never comes to rest between the two. Scratch vectors: no allocation a frame.
      const f = st.fly;
      const S = this.scratch;
      S.fly.copy(c.from.position).lerp(E.position, f);
      S.fly.y += Math.sin(Math.PI * f) * 0.12 * c.from.position.distanceTo(E.position);
      S.flyT.copy(c.from.target).lerp(E.target, f);
      if (st.dive > 0 && pose) {
        const d = st.dive;
        S.ctl.copy(E.position).lerp(E.atom, 0.5).addScaledVector(UP, DIVE_LIFT);
        const a = (1 - d) * (1 - d);
        const b = 2 * (1 - d) * d;
        const q = d * d;
        S.pos.set(
          a * S.fly.x + b * S.ctl.x + q * pose.position.x,
          a * S.fly.y + b * S.ctl.y + q * pose.position.y,
          a * S.fly.z + b * S.ctl.z + q * pose.position.z,
        );
        S.tgt.copy(S.flyT).lerp(pose.target, d);
        pos = S.pos;
        tgt = S.tgt;
      } else {
        pos = S.fly;
        tgt = S.flyT;
      }
      // The entered machine's parts. The exterior fades by the time keys and is gone within EXT_NEAR of its box (the dive
      // shows the board, never the case from close up); the board takes over as it goes (a match cut). Each part then
      // fades by its distance from the camera (opacity only), so the flight is never behind a solid.
      if (pos) {
        this.extBox.setFromObject(root);
        const seen = this.extBox.isEmpty() ? 1 : smoother(clamp01((this.extBox.distanceToPoint(pos) - EXT_NEAR) / (EXT_FAR - EXT_NEAR)));
        ext = st.ext * seen;
        boardK = Math.max(st.board, 1 - seen);
        setMeshesShown(root, ext > 0.002);
        followLights(root, ext);
        fadeTree(root, ext, occlusionOf(root, pos));
      }
    } else if (pose) {
      pos = pose.position;
      tgt = pose.target;
    }

    // The board: a match cut at the exterior's place (its atom on the exterior's atom), easing to its own place.
    if (board && x && pos) {
      const E = exteriorView(x);
      const m = matchOf(x, board, E.atom, rotY);
      const g = board.group;
      g.position.lerpVectors(m.pos, ORIGIN, st.dive);
      g.rotation.set(0, rotY * (1 - st.dive), 0);
      g.scale.setScalar(m.scale + (1 - m.scale) * st.dive);
    } else if (board) {
      board.group.position.set(0, 0, 0);
      board.group.rotation.set(0, 0, 0);
      board.group.scale.setScalar(1);
    }
    if (board) {
      const g = board.group;
      c.rooms.position.copy(g.position);
      c.rooms.rotation.copy(g.rotation);
      c.rooms.scale.copy(g.scale);
      g.visible = true; // the group stays (its lights count in every program); its drawn objects follow the fade
      setMeshesShown(g, boardK > 0.002);
      fadeTree(g, boardK, x && c.from && pos ? occlusionOf(g, pos) : undefined); // in the flight, the board's parts too fade by the camera
      if (board.atom) {
        this.atomBase ??= board.atom.scale.clone();
        board.atom.scale.copy(this.atomBase).multiplyScalar(0.2 + 0.8 * st.atom);
        board.atom.visible = true;
        fadeTree(board.atom, st.atom);
      }
    }

    // The rooms rise from their slots in a staggered wave; the robots pop in after the last one.
    c.rooms.children.forEach((room, i) => {
      const [a, b] = riseSpan(i, rooms);
      const e = span(t, a, b);
      let base = this.rest.get(room);
      if (!base) {
        base = { y: room.position.y, sy: room.scale.y };
        this.rest.set(room, base);
      }
      // Never hidden: a hidden room drops its lights out of every program (a relink), so it rises by its scale, its
      // fade and its lights, which share the room's materials and programs already compiled in warm.
      room.visible = true;
      setMeshesShown(room, e > 0.002);
      room.scale.y = base.sy * Math.max(0.02, e);
      room.position.y = base.y - (1 - e) * ROOM_DROP;
      fadeTree(room, e);
      followLights(room, e);
    });
    const r0 = robotsAt(rooms);
    const rg = robotGap(robots.length);
    robots.forEach((g, j) => {
      const p = span(t, r0 + j * rg, r0 + j * rg + WAVE.pop);
      g.visible = true; // the pop is its scale (the group stays: see the rooms)
      setMeshesShown(g, p > 0.002);
      g.scale.setScalar(Math.max(0.001, p));
    });

    if (pos && tgt) {
      c.camera.position.copy(pos);
      c.orbit.target.copy(tgt);
      c.camera.lookAt(tgt);
      c.camera.updateMatrixWorld();
    }
    c.look(st.dive);

    // Headless measurement (perf harness): the frame's time and camera position, when a harness has set __apzTrace.
    // The camera against the entered machine's solids (headless only): `hit` counts the visible meshes whose world box holds it.
    const probe = (globalThis as { __apzTrace?: unknown[] }).__apzTrace;
    if (probe && pos) {
      const hit = x && c.from ? probeInside(x.fleet.machines.get(x.machineId)!.root, pos) : null;
      const bh = board && x && c.from ? probeInside(board.group, pos) : null; // the matched board's parts, for the report
      probe.push({ now: performance.now(), t, pos: pos.toArray(), hit: hit?.solid.length ?? 0, names: hit?.solid.slice(0, 4) ?? [], faded: hit?.faded ?? 0, near: hit?.near ?? 0, bhit: bh?.solid.length ?? 0, bnear: bh?.near ?? 0 });
    }
    if (this.dir > 0 && t >= end) this.finish();
    if (this.dir < 0 && t <= min) this.finish(true);
  }

  private finish(left = false): void {
    this.finished = true;
    this.ctx.orbit.enabled = true;
    if (left) {
      this.left = true;
      this.ctx.onLeft();
      return;
    }
    // The robots are whole from here on: one that has not yet popped in gets no pop left to run.
    for (const g of this.ctx.bots()) {
      g.visible = true;
      setMeshesShown(g, true);
      g.scale.setScalar(1);
    }
    this.ctx.look(1);
  }
}

/** How many robots have popped in (scale above zero) at time t. */
export const robotsPopped = (t: number, rooms: number, robots: number): number => {
  const rg = robotGap(robots);
  let n = 0;
  for (let j = 0; j < robots; j++) if (span(t, robotsAt(rooms) + j * rg, robotsAt(rooms) + j * rg + WAVE.pop) > 0.002) n++;
  return n;
};
