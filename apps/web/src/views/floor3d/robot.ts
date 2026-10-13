// A plaza robot: a small toon 3D model built from its species' JSON (assets/robots/species/<id>.json).
// The species decides the parts, bones, eyes and motion; the clips (assets/robots/clips.json and the
// species' own) decide what it does. The scene owns placement; the robot owns its own animation.
//
// The floor's API is unchanged: makeRobot(hue, orchestrator, palette, bodyHue?, variant?), robotVariant,
// ROBOT_PALETTE, Robot, RobotState, RobotPose. The variant is now a species id.

import * as THREE from 'three';
import { evalClip, restOf } from './robots/clips';
import { bakeSpecies, bakeTool, instantiate, robotMats, type RigInstance, type SpeciesGeo, type BoneGeo } from './robots/rig';
import { Fx } from './robots/fx';
import { registry, sharedClip } from './robots/files';
import type { Clip, FxKind, Palette as PaletteT, Species, Tool, Vec3 } from './robots/types';

export type RobotState = 'idle' | 'working' | 'walking' | 'offline';
/** A still pose for a portrait: the shared clips of the same names. */
export type RobotPose = 'wave' | 'proud' | 'think' | 'cheer';
/** A species id (the designs are in assets/robots/species/); the orchestrator is always `rover`. */
export type RobotVariant = string;
export type Palette = PaletteT;

/** An agent's design: neighbouring cids (a district's members) get different ones. */
export function robotVariant(cid: number): RobotVariant {
  // The oracle is reserved (the hub never deals it to a robot): the client's fallback deal skips it too.
  const list = registry().index.species.filter((id) => id !== 'oracle');
  return list[((cid % list.length) + list.length) % list.length] ?? 'rover';
}

/** A robot's species: the hub's `species` on the agent record, else the cid's `robotVariant` when the record has none. */
export function speciesOf(hub: string | null | undefined, cid: number): RobotVariant {
  const id = hub?.trim();
  return id ? id : robotVariant(cid);
}

/** The robots' own paint, the same in the light and the dark theme. */
export const ROBOT_PALETTE: Palette = registry().palette;

export interface PlayOptions {
  /** Hold the clip (it repeats until play(null)); a one-shot clip is held when this is true. */
  loop?: boolean;
}

export interface Robot {
  /** Add this to the scene; position/rotate it from the scene. */
  readonly group: THREE.Group;
  /** The species id this robot was built from. */
  readonly species: string;
  /** World-space point above the head, for the status label. */
  labelAnchor(target: THREE.Vector3): THREE.Vector3;
  setState(state: RobotState): void;
  /** Advance the animation: `dt` seconds since the last frame, `t` seconds since start. */
  update(dt: number, t: number): void;
  setHighlighted(on: boolean): void;
  /** Strikes a still pose (for a portrait), held until the next play or pose. */
  pose(kind: RobotPose): void;
  /** Plays a clip: held when it loops (or with `loop`), else a one-shot that returns to the state's clip. `null` ends a held clip. */
  play(clipId: string | null, opts?: PlayOptions): void;
  /**
   * The body layer (gravity and heights): a clip that overrides the hold, the one-shot and the state's clip while it is set
   * (crouch, jump, fall, land). It starts when set and is kept while the same id is set again. `null` ends it.
   */
  setBody(clipId: string | null): void;
  /** Holds a tool in the grip: one hand, or both (a two-handed tool aims at the other grip). `null` puts it down. */
  hold(toolId: string | null): void;
  /** Carries an item at the carry bone (a crate). `null` puts it down. */
  carry(itemId: string | null): void;
  /** Spawns one effect at the head (sparks, zzz, dots, hearts, keys). */
  fx(kind: FxKind): void;
  dispose(): void;
}

const SCALE = 1.3; // a touch bigger than the furniture, so the robots read from afar
const ORCH_SCALE = 1.25 * SCALE;
const BLEND = 8; // the per-second rate each channel moves toward its target (k = min(1, dt·8))
const WALK_FEET_RATE = 9; // rad/s of the feet's step while walking
/** The feet's height offset while a body clip plays (tucked in the jump, hanging in the fall). Shared clips cannot name the
 *  feet (each species has its own), so the state's feet take it here. */
const AIR_FEET: Record<string, number> = { 'jump.air': 0.06, 'fall.air': -0.03 };
const FLOAT_OFFLINE = -0.07; // a hover robot sets down when offline

const warned = new Set<string>();
function warnOnce(msg: string): void {
  if (warned.has(msg)) return;
  warned.add(msg);
  console.warn(`robots: ${msg}`);
}

const geoCache = new Map<string, SpeciesGeo>();
const toolCache = new Map<string, BoneGeo>();
function speciesGeo(s: Species, palette: Palette, orch: boolean): SpeciesGeo {
  const key = `${s.id}|${orch}`;
  let g = geoCache.get(key);
  if (!g) {
    g = bakeSpecies(s, palette, orch);
    geoCache.set(key, g);
  }
  return g;
}
function toolGeo(id: string, tool: Tool, palette: Palette): BoneGeo {
  let g = toolCache.get(id);
  if (!g) {
    g = bakeTool(tool, palette);
    toolCache.set(id, g);
  }
  return g;
}

/** The joint values of the bones, by "bone|prop" (and "lid" for the eyes). */
type Key = string;
const propOf = (key: Key): string => (key === 'lid' ? 'lid' : key.slice(key.indexOf('|') + 1));
const boneOf = (key: Key): string => (key === 'lid' ? 'head' : key.slice(0, key.indexOf('|')));

/**
 * `bodyHue` (0–360) colours the robot's shell, its darker shade and its arms; `hue` (its district's)
 * tints the front panel. The orchestrator wears the species' extras (the rover's antenna and trim).
 */
export function makeRobot(hue: number, orchestrator: boolean, palette: Palette, bodyHue = hue, variant: RobotVariant = 'rover'): Robot {
  const reg = registry();
  const id = orchestrator ? 'rover' : variant;
  let species = reg.species[id];
  if (!species) {
    warnOnce(`species "${id}" is not valid (see registry().errors); the rover is used`);
    species = reg.species.rover!;
  }
  const sp = species;

  const root = new THREE.Group();
  root.name = orchestrator ? 'robot-orchestrator' : 'robot';
  root.scale.setScalar(orchestrator ? ORCH_SCALE : SCALE);

  const mats = robotMats(palette, hue, bodyHue);
  const rig: RigInstance = instantiate(sp, speciesGeo(sp, palette, orchestrator), mats, root);
  const fx = new Fx();

  // The grip: the tool's aim (a two-handed tool points at the other grip, at rest).
  root.updateMatrixWorld(true);
  const gripR = rig.bones.get('gripR')!;
  const gripL = rig.bones.get('gripL')!;
  const toGripL = gripR.worldToLocal(gripL.getWorldPosition(new THREE.Vector3()));

  // ---- State ---------------------------------------------------------------------------------
  let state: RobotState = 'idle';
  let eyeIntensity = 1.3;
  let lastT = 0;
  let hold: { id: string; clip: Clip; start: number } | null = null;
  let once: { id: string; clip: Clip; start: number; dur: number } | null = null;
  let body: { id: string; clip: Clip; start: number } | null = null;
  let heldTool: { id: string; holder: THREE.Object3D } | null = null;
  let carried: { id: string; holder: THREE.Object3D } | null = null;
  /** The blended value of each joint, and the rate channels' and spins' accumulated angles. */
  const cur = new Map<Key, number>();
  const acc = new Map<Key, number>();
  /** Clip effects: the seconds since each one last spawned, for the clip they came from. */
  const fxTimer = new Map<number, number>();
  let fxFor: string | null = null;

  const eyeColour = (s: RobotState): string => (s === 'offline' ? palette.inkMuted : s === 'working' ? palette.info : palette.ok);

  const applyState = (next: RobotState): void => {
    state = next;
    mats.eye.color.set(eyeColour(next));
    mats.eye.emissive.set(eyeColour(next));
    eyeIntensity = next === 'offline' ? 0.2 : next === 'working' ? 2.0 : 1.3;
    mats.thruster.color.set(next === 'offline' ? palette.inkMuted : palette.info);
  };

  /** A shared clip, or the species' own (overrides first), by name. */
  const clipNamed = (name: string): Clip | undefined => {
    const override = sp.overrides[name];
    if (override && sp.clips[override]) return sp.clips[override];
    return sp.clips[name] ?? sharedClip(name);
  };
  const stateClipId = (): string => {
    if (state === 'idle') return 'idle';
    if (state === 'working') return 'type';
    if (state === 'offline') return 'offline';
    const walk = `walk.${sp.locomotion}`;
    if (clipNamed(walk)) return walk;
    warnOnce(`no clip "${walk}" for locomotion "${sp.locomotion}" (the treads walk is used)`);
    return 'walk.treads';
  };

  /** The clip playing now: a one-shot over a hold over the state's clip. */
  const activeClip = (t: number): { id: string; clip: Clip; elapsed: number } | null => {
    if (body) return { id: body.id, clip: body.clip, elapsed: t - body.start };
    if (once) {
      if (t - once.start >= once.dur) once = null;
      else return { id: once.id, clip: once.clip, elapsed: t - once.start };
    }
    if (hold) return { id: hold.id, clip: hold.clip, elapsed: (t - hold.start) % (hold.clip.dur ?? Infinity) };
    const id = stateClipId();
    const clip = clipNamed(id);
    return clip ? { id, clip, elapsed: 0 } : null;
  };

  /** Clip effects, spawned on their interval while the clip plays. */
  const clipFx = (id: string, clip: Clip, dt: number): void => {
    if (fxFor !== id) {
      fxFor = id;
      fxTimer.clear();
    }
    (clip.fx ?? []).forEach((f, i) => {
      const every = f.every ?? 0.6;
      const next = (fxTimer.get(i) ?? every) - dt;
      if (next <= 0) {
        spawnAt(f.kind, f.at);
        fxTimer.set(i, every);
      } else fxTimer.set(i, next);
    });
  };

  const anchorOf = (name: string): { obj: THREE.Object3D; at: Vec3 } => {
    if (name === 'head') return { obj: rig.bones.get('head')!, at: [0, 0.3, 0] };
    const obj = rig.bones.get(name);
    return obj ? { obj, at: [0, 0, 0] } : { obj: rig.bones.get('head')!, at: [0, 0.3, 0] };
  };
  const spawnAt = (kind: FxKind, anchor: string): void => {
    const a = anchorOf(anchor);
    fx.spawn(kind, a.obj, a.at);
  };

  /** The state's motion on top of the clip: floating, the feet, the spins. */
  const extras = new Map<string, Vec3>();
  const extra = (bone: string): Vec3 => {
    let v = extras.get(bone);
    if (!v) extras.set(bone, (v = [0, 0, 0]));
    return v;
  };

  const apply = (): void => {
    for (const [name, obj] of rig.bones) {
      const r = rig.rest.get(name)!;
      obj.position.set(r[0], r[1], r[2]);
      obj.rotation.set(0, 0, 0);
      obj.scale.setScalar(1);
    }
    const keys = new Set<Key>([...cur.keys(), ...acc.keys()]);
    for (const key of keys) {
      if (key === 'lid') continue;
      const obj = rig.bones.get(boneOf(key));
      if (!obj) continue;
      const prop = propOf(key);
      const v = cur.get(key) ?? restOf(prop);
      if (prop.startsWith('rot.')) {
        const axis = prop.slice(4) as 'x' | 'y' | 'z';
        obj.rotation[axis] = v + (acc.get(key) ?? 0);
      } else if (prop.startsWith('pos.')) {
        const axis = prop.slice(4) as 'x' | 'y' | 'z';
        obj.position[axis] += v;
      } else if (prop === 'scale') obj.scale.setScalar(v);
    }
    for (const [bone, e] of extras) {
      const obj = rig.bones.get(bone);
      if (obj) obj.position.add(new THREE.Vector3(e[0], e[1], e[2]));
    }
    const lid = cur.get('lid') ?? 1;
    for (const p of rig.pupils) p.scale.y = lid;
  };

  // The grip and carry holders: tools are baked once (cached), held at the bone.
  const holdTool = (id: string | null): void => {
    if (heldTool) {
      heldTool.holder.removeFromParent();
      heldTool = null;
    }
    if (id === null) return;
    const tool = reg.tools[id];
    if (!tool) return void warnOnce(`no tool "${id}"`);
    const bone = tool.grip === 'carry' ? 'carry' : 'gripR';
    const holder = rig.attach(bone, toolGeo(id, tool, palette));
    if (tool.grip === 'two') {
      const dir = toGripL.clone().normalize();
      holder.quaternion.setFromUnitVectors(new THREE.Vector3(1, 0, 0), dir);
    } else if (tool.grip === 'one') {
      // The parts hang along −y; a fist turns them ahead (−y → +z), or stands them up.
      const aim = tool.aim ?? 'forward';
      holder.rotation.x = aim === 'forward' ? -Math.PI / 2 : aim === 'up' ? Math.PI : 0;
    }
    heldTool = { id, holder };
  };
  const carryItem = (id: string | null): void => {
    if (carried) {
      carried.holder.removeFromParent();
      carried = null;
    }
    if (id === null) return;
    const item = reg.tools[id];
    if (!item) return void warnOnce(`no item "${id}"`);
    carried = { id, holder: rig.attach('carry', toolGeo(id, item, palette)) };
  };

  applyState('idle');
  apply();

  return {
    group: root,
    species: sp.id,

    labelAnchor(target: THREE.Vector3): THREE.Vector3 {
      root.updateWorldMatrix(true, false);
      return root.localToWorld(target.set(0, sp.label_top, 0));
    },

    setState(next: RobotState): void {
      if (next !== state) applyState(next);
    },

    update(dt: number, t: number): void {
      lastT = t;
      const k = Math.min(1, dt * BLEND);
      const active = activeClip(t);
      const targets = active ? evalClip(active.clip, t, sp.gait, active.elapsed) : new Map<Key, { value: number; snap: boolean }>();

      if (active) {
        for (const ch of active.clip.channels) {
          if (ch.rate === undefined) continue;
          const key = `${ch.bone}|${ch.prop}`;
          acc.set(key, (acc.get(key) ?? 0) + ch.rate * dt);
        }
        clipFx(active.id, active.clip, dt);
      }
      // Blend every joint toward its target; a joint no clip names eases back to rest.
      for (const key of new Set<Key>([...cur.keys(), ...targets.keys()])) {
        const tg = targets.get(key);
        const goal = tg ? tg.value : restOf(propOf(key));
        const v = cur.get(key) ?? restOf(propOf(key));
        cur.set(key, tg?.snap ? goal : v + (goal - v) * k);
      }

      // Spins: the species' rates by state (a rotor, a ball wheel).
      for (const s of sp.spin) {
        const rate = state === 'walking' ? s.walk : state === 'offline' ? (s.offline ?? 0) : s.idle;
        const key = `${s.bone}|rot.${s.axis}`;
        acc.set(key, (acc.get(key) ?? 0) + rate * dt);
      }
      // Feet step alternately while walking; floating robots bob, and sit down when offline.
      extras.clear();
      sp.feet.forEach((bone, i) => {
        const p = state === 'walking' ? Math.sin(WALK_FEET_RATE * t + i * Math.PI) : 0;
        const e = extra(bone);
        e[1] += 0.035 * Math.max(0, p);
        e[2] += 0.06 * p;
      });
      const tuck = body ? AIR_FEET[body.id] : undefined;
      if (tuck !== undefined) for (const bone of sp.feet) extra(bone)[1] += tuck;
      if (sp.floats) {
        const lift = state === 'offline' ? FLOAT_OFFLINE : 0.035 * Math.sin(2.2 * t);
        extra('base')[1] += lift;
        extra('body')[1] += lift;
      }

      apply();
      fx.update(dt);
      mats.eye.emissiveIntensity = state === 'working' ? eyeIntensity + 0.3 * Math.sin(t * 5) : eyeIntensity;
      if (orchestrator) mats.light.emissiveIntensity = 1.1 + 0.5 * Math.sin(t * 3);
    },

    setHighlighted(on: boolean): void {
      mats.matte.emissive.set(on ? palette.signal : 0x000000);
      mats.matte.emissiveIntensity = on ? 0.35 : 0;
    },

    pose(kind: RobotPose): void {
      const clip = clipNamed(kind);
      if (!clip) return void warnOnce(`no pose clip "${kind}"`);
      hold = { id: kind, clip, start: lastT };
      once = null;
      snapTo(clip, lastT);
    },

    play(clipId: string | null, opts?: PlayOptions): void {
      if (clipId === null) {
        hold = null;
        return;
      }
      const clip = clipNamed(clipId);
      if (!clip) return void warnOnce(`no clip "${clipId}" for species "${sp.id}"`);
      if (opts?.loop ?? clip.loop !== false) {
        hold = { id: clipId, clip, start: lastT };
        once = null;
      } else {
        once = { id: clipId, clip, start: lastT, dur: clip.dur ?? 1.6 };
      }
    },

    setBody(clipId: string | null): void {
      if (clipId === null) {
        body = null;
        return;
      }
      if (body?.id === clipId) return;
      const clip = clipNamed(clipId);
      if (!clip) return void warnOnce(`no body clip "${clipId}" for species "${sp.id}"`);
      body = { id: clipId, clip, start: lastT };
    },

    hold(toolId: string | null): void {
      holdTool(toolId);
    },

    carry(itemId: string | null): void {
      carryItem(itemId);
    },

    fx(kind: FxKind): void {
      spawnAt(kind, 'head');
    },

    dispose(): void {
      fx.clear();
      rig.dispose();
      root.removeFromParent();
    },
  };

  /** Sets the joints to a clip's targets at once (a pose: no blend). */
  function snapTo(clip: Clip, t: number): void {
    const targets = evalClip(clip, t, sp.gait, 0);
    for (const key of new Set<Key>([...cur.keys(), ...targets.keys()])) {
      const tg = targets.get(key);
      cur.set(key, tg ? tg.value : restOf(propOf(key)));
    }
    apply();
  }
}
