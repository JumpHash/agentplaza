import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { outline, toonMat } from './toon';
import type { KitPalette } from './rooms/kit';
import type { LiftState } from './nav-types';

// The lift, a chunky toy elevator: four thick pillars on footing plates, a header housing with pulley drums at the top,
// a cab (a solid accent deck, railing tubes on its closed sides, a light bar that glows while it moves), two sliding gate
// leaves at each stop, and a call post with a round lamp beside each gate. The counterweight rides a guide outside the
// frame. The mechanics are the nav world's (NavWorld.lift, lift-ctrl.ts): this file only places the cab, the gates, the
// lamps and the cables from the controller's state. The frame is in the lift's frame: z = 0 is the front cell's centre
// (the board cell beside the plate edge), +z out toward the walkway (the queue cell beyond it), -z toward the plate. The
// cab is open at both landings: the plate side (stop 1, at the plate's edge) and the walkway side (stop 0).
// docs/design/navigation.md ("Lifts") has the geometry and the controller.

export interface LiftSpec {
  x: number;
  z: number;
  /** Yaw of the outward normal (three.js rotation.y: the normal is (sin face, cos face)). */
  face: number;
  /** The stop heights: the PCB top (low) and the plate top (high). */
  low: number;
  high: number;
}

export interface LiftProp {
  group: THREE.Group;
  /** Places the cab, its cable and counterweight, the gates and the call lamps from the controller's state. */
  setState(s: LiftState): void;
  dispose(): void;
}

/** The cab's width along the wall (x), and its plane limits in z (the plate edge and the walkway side). */
export const CAB_W = 1.6;
export const CAB_BACK = -0.12;
export const CAB_FRONT = 0.37; // the queue is two cells out (0.375 and beyond), so the deck stays on the front cell and the cell between
/** A stop's gate height above the stop, and the frame's top above the plate (the lintel over the gates). */
export const GATE_H = 0.45;

const ZC = (CAB_BACK + CAB_FRONT) / 2; // the shaft's centre line in z
const FOOT = 0.07;                      // footing plates on the board, under the pillars, the guide and the call posts
const TOP_PAD = 0.04;                   // the frame's top above the gate lintel's underside
const HEAD = 0.2;                       // the header housing on top of the frame
const PX = 0.92;                        // pillar centre x (the pillars' inner faces, at ±0.83, clear the cab's rails)
const PIL = 0.18;                       // pillar section
const GATE_W = 0.8;                     // one gate leaf: two leaves, each 0.8 wide, close the opening (0.01 off the centre line and the pillars)
const DECK_LIFT = 0.01;                 // the cab's deck sits this far above its stop: clear of the board and the plate, a step above the gangway
const RAIL_R = 0.03;                    // cab railing tube radius
const RAIL_H = 0.34;                    // cab railing height above the deck
const CANOPY_Y = 0.42;                  // the light bar's centre above the deck
const CANOPY_T = 0.07;                  // the light bar's thickness
const DECK_T = 0.12;                    // cab deck thickness (its top is the cab's height)
const CW_X = 1.25;                      // the counterweight's guide, outside the frame
const CW_W = 0.24;
const CW_H = 0.34;
const CW_D = 0.2;
const CALL_X = 1.15;                    // the call posts stand beside the pillars' outer faces
const CALL_H = 0.7;                     // a call post's height above its footing
const CABLE = 0.03;                     // cable section

/** The palette a lift needs: the info accent (cab deck, light bar); signal and ok fall back to the plaza's own colours. */
export type LiftPalette = Pick<KitPalette, 'info'> & Partial<Pick<KitPalette, 'signal' | 'ok'>>;

export interface StopMats {
  steel: THREE.Material;
  deck: THREE.Material;      // the gangway's deck (gangway.ts)
  cabDeck: THREE.Material;   // the cab's accent deck
  stripe: THREE.Material;    // the cab's signal-coloured edge
  tower: THREE.Material;     // the pillars, footings, header and counterweight
  panel: THREE.Material;     // the call panels' housings
  gate: THREE.Material;
  lampIdle: THREE.Material;  // a lamp with nothing waiting
  lampCall: THREE.Material;  // robots are waiting at the stop
  lampHere: THREE.Material;  // the cab is at the stop
}
const matsByPalette = new WeakMap<object, StopMats>();

/**
 * The lift's and the gangway's materials, created once per palette and shared (never disposed with a lift): the lamps'
 * three states are three materials, so a lamp changes by swapping, never by a new program. warm.ts compiles them (prewarmLift).
 */
export function liftMats(palette: LiftPalette): StopMats {
  let m = matsByPalette.get(palette);
  if (!m) {
    const info = palette.info;
    const signal = palette.signal ?? new THREE.Color(0xff7a2e);
    const ok = palette.ok ?? new THREE.Color(0x3ee08f);
    m = {
      steel: toonMat({ color: 0xdfe6f1, emissive: 0x2a3446 }),
      deck: toonMat({ color: 0x7d8aa0, emissive: 0x1e2636 }),
      cabDeck: toonMat({ color: info, emissive: info, emissiveIntensity: 0.15 }),
      stripe: toonMat({ color: signal, emissive: signal, emissiveIntensity: 0.4 }),
      tower: toonMat({ color: 0x8c9cb8, emissive: 0x1f2a3e }),
      panel: toonMat({ color: 0x2d3642 }),
      gate: toonMat({ color: 0xc9d2e0, emissive: 0x20283a }),
      lampIdle: new THREE.MeshBasicMaterial({ color: new THREE.Color(info).multiplyScalar(0.3), toneMapped: false }),
      lampCall: new THREE.MeshBasicMaterial({ color: signal, toneMapped: false }),
      lampHere: new THREE.MeshBasicMaterial({ color: ok, toneMapped: false }),
    };
    matsByPalette.set(palette, m);
  }
  return m;
}

const UP = new THREE.Vector3(0, 1, 0);

/**
 * The static pieces of one material, merged into one geometry: one mesh, one draw call. A piece is a box, a square member
 * (a brace or a rail), a round tube (a railing or a post) or a disc (a pulley, a lamp).
 */
class Parts {
  private readonly geos: THREE.BufferGeometry[] = [];

  box(w: number, h: number, d: number, x: number, y: number, z: number): void {
    this.geos.push(new THREE.BoxGeometry(w, h, d).translate(x, y, z));
  }

  /** A square member from a to b. */
  beam(a: THREE.Vector3, b: THREE.Vector3, sec: number): void {
    const dir = new THREE.Vector3().subVectors(b, a);
    const g = new THREE.BoxGeometry(sec, dir.length(), sec);
    g.applyQuaternion(new THREE.Quaternion().setFromUnitVectors(UP, dir.clone().normalize()));
    g.translate((a.x + b.x) / 2, (a.y + b.y) / 2, (a.z + b.z) / 2);
    this.geos.push(g);
  }

  /** A round tube from a to b. */
  tube(a: THREE.Vector3, b: THREE.Vector3, r: number): void {
    const dir = new THREE.Vector3().subVectors(b, a);
    const g = new THREE.CylinderGeometry(r, r, dir.length(), 12);
    g.applyQuaternion(new THREE.Quaternion().setFromUnitVectors(UP, dir.clone().normalize()));
    g.translate((a.x + b.x) / 2, (a.y + b.y) / 2, (a.z + b.z) / 2);
    this.geos.push(g);
  }

  /** A disc whose axis is z (a pulley drum, a lamp), centred at (x, y, z). */
  disc(r: number, depth: number, x: number, y: number, z: number): void {
    const g = new THREE.CylinderGeometry(r, r, depth, 20);
    g.rotateX(Math.PI / 2);
    g.translate(x, y, z);
    this.geos.push(g);
  }

  merge(): THREE.BufferGeometry {
    const out = mergeGeometries(this.geos, false);
    if (!out) throw new Error('lift: the parts do not merge');
    return out;
  }
}

/** A lift on a plate's landing: the frame, the cab at the controller's height, the gates, the call panels, the counterweight. */
export function buildLift(lift: LiftSpec, palette: LiftPalette): LiftProp {
  const { low, high } = lift;
  const top = high + GATE_H + TOP_PAD; // the frame's top; the header sits on it
  const base = low + FOOT;             // the pillars stand on their footing plates
  const mats = liftMats(palette);
  const group = new THREE.Group();
  group.position.set(lift.x, 0, lift.z);
  group.rotation.y = lift.face;
  group.userData.lift = true;
  const geos: THREE.BufferGeometry[] = [];
  const V = (x: number, y: number, z: number): THREE.Vector3 => new THREE.Vector3(x, y, z);

  /** One mesh with its geometry kept for dispose. Shadows on: the frame and cab are solid. */
  const add = (parent: THREE.Object3D, geo: THREE.BufferGeometry, mat: THREE.Material, shadow = true): THREE.Mesh => {
    geos.push(geo);
    const m = new THREE.Mesh(geo, mat);
    m.castShadow = shadow;
    m.receiveShadow = shadow;
    parent.add(m);
    return m;
  };

  // The static frame: pillars, footings, the header, the braces and lintels, the drums, the guide, the call posts.
  const tower = new Parts();
  const steel = new Parts();
  const panel = new Parts();
  for (const sx of [-1, 1]) {
    for (const z of [CAB_BACK, CAB_FRONT]) {
      tower.box(PIL, top - base + 0.02, PIL, sx * PX, (base + top) / 2, z); // sunk 0.01 into the footing and the header
      tower.box(0.2, FOOT + 0.03, 0.2, sx * PX, low + (FOOT - 0.03) / 2, z); // sunk 0.03 under the board; 0.01 clear of the gates
    }
  }
  tower.box(2.44, HEAD, 0.7, 0.17, top + HEAD / 2, ZC); // the header housing: over the pillars and the counterweight's drum
  tower.box(0.2, FOOT + 0.03, 0.2, CW_X, low + (FOOT - 0.03) / 2, ZC - 0.12); // the guide's footing
  for (const z of [CAB_BACK, CAB_FRONT]) {
    steel.box(2 * PX + PIL - 0.02, 0.06, 0.1, 0, top - 0.05, z); // the lintel over the gates, inset from the pillars' outer faces
    steel.box(2 * PX + PIL - 0.02, 0.06, 0.1, 0, (low + top) / 2, z); // the mid rail
  }
  for (const sx of [-1, 1]) {
    steel.beam(V(sx * PX, base, CAB_BACK), V(sx * PX, top, CAB_FRONT), 0.07); // the X brace on each side
    steel.beam(V(sx * PX, top, CAB_BACK), V(sx * PX, base, CAB_FRONT), 0.07);
  }
  steel.beam(V(CW_X, base - 0.01, ZC - 0.12), V(CW_X, top + 0.01, ZC - 0.12), 0.07); // sunk into the header // the counterweight's guide
  steel.disc(0.085, 0.05, 0, top + HEAD / 2, ZC + 0.37); // the cab's pulley drum on the header's front face
  steel.disc(0.085, 0.05, CW_X, top + HEAD / 2, ZC + 0.37); // the counterweight's pulley drum

  // The call posts: one at each stop, beside the pillar on the side away from the cab's centre; a dark housing with a round lamp.
  const lamps: Record<0 | 1, THREE.Mesh> = {} as Record<0 | 1, THREE.Mesh>;
  for (const st of [0, 1] as const) {
    const b = st === 0 ? low : high;
    const x = st === 0 ? -CALL_X : CALL_X;
    const z = st === 0 ? CAB_FRONT + 0.16 : CAB_BACK - 0.16;
    const face = st === 0 ? 1 : -1; // the lamp looks out to the approach
    const y = b + FOOT + CALL_H;
    tower.box(0.2, FOOT + 0.03, 0.2, x, b + (FOOT - 0.03) / 2, z);
    steel.tube(V(x, b + FOOT - 0.02, z), V(x, y, z), 0.03);
    panel.box(0.2, 0.26, 0.06, x, y, z);
    const lamp = new THREE.CylinderGeometry(0.085, 0.085, 0.04, 20);
    lamp.rotateX(Math.PI / 2);
    lamp.translate(x, y, z + face * 0.045);
    lamps[st] = add(group, lamp, mats.lampIdle, false);
  }

  add(group, tower.merge(), mats.tower);
  add(group, steel.merge(), mats.steel);
  add(group, panel.merge(), mats.panel);

  // The cab: its accent deck at the cab's height, a signal-coloured lip on the walkway side, railing tubes on the two closed
  // sides, and a light bar across the cab on two posts. The light bar glows while the cab travels.
  const cab = new THREE.Group();
  group.add(cab);
  const deck = add(cab, new THREE.BoxGeometry(CAB_W, DECK_T, CAB_FRONT - CAB_BACK).translate(0, -DECK_T / 2, ZC), mats.cabDeck);
  deck.userData.platform = true;
  add(cab, new THREE.BoxGeometry(CAB_W - 0.02, 0.035, 0.05).translate(0, 0.01, CAB_FRONT - 0.02), mats.stripe, false);
  const rails = new Parts();
  for (const sx of [-1, 1]) {
    const x = sx * (CAB_W / 2 - RAIL_R);
    for (const z of [CAB_BACK, CAB_FRONT]) rails.tube(V(x, -0.02, z), V(x, RAIL_H, z), RAIL_R);
    rails.tube(V(x, RAIL_H, CAB_BACK), V(x, RAIL_H, CAB_FRONT), RAIL_R);
    rails.tube(V(x, RAIL_H / 2, CAB_BACK), V(x, RAIL_H / 2, CAB_FRONT), RAIL_R);
    rails.tube(V(x, RAIL_H, ZC), V(x, CANOPY_Y, ZC), RAIL_R); // the light bar's post
  }
  add(cab, rails.merge(), mats.steel);
  const barBase = new THREE.Color(palette.info);
  const barMat = new THREE.MeshBasicMaterial({ color: barBase.clone().multiplyScalar(0.3), toneMapped: false });
  add(cab, new THREE.BoxGeometry(CAB_W - 0.1, CANOPY_T, 0.08).translate(0, CANOPY_Y, ZC), barMat, false);

  // The cables and the counterweight: the cab's cable hangs from the header to the light bar (x = 0), the counterweight's
  // from the header to the counterweight on its guide (x = CW_X). Each is a unit box scaled to its length.
  const cabCable = add(group, new THREE.BoxGeometry(CABLE, 1, CABLE), mats.steel, false);
  cabCable.position.z = ZC;
  const cw = add(group, new THREE.BoxGeometry(CW_W, CW_H, CW_D), mats.tower);
  cw.position.set(CW_X, 0, ZC);
  const cwCable = add(group, new THREE.BoxGeometry(CABLE, 1, CABLE), mats.steel, false);
  cwCable.position.set(CW_X, 0, ZC);

  // The gates: one leaf pair per stop, the leaves sliding apart as the doors open. A stop's gate is open only while the cab is there.
  const gateGeo = (() => {
    const p = new Parts();
    p.box(GATE_W, GATE_H, 0.03, 0, 0, 0);
    for (const dy of [-0.15, -0.05, 0.05, 0.15]) p.box(GATE_W - 0.06, 0.04, 0.05, 0, dy, 0.02); // the slats, short of the leaf's edges
    return p.merge();
  })();
  geos.push(gateGeo);
  const gates = {} as Record<0 | 1, [THREE.Mesh, THREE.Mesh]>;
  for (const st of [0, 1] as const) {
    const y = (st === 0 ? low : high) - 0.02 + GATE_H / 2; // its foot sinks into the board or the plate
    const z = st === 0 ? CAB_FRONT : CAB_BACK;
    const leaves = [-1, 1].map((sx) => {
      const leaf = new THREE.Mesh(gateGeo, mats.gate);
      leaf.position.set(sx * (GATE_W / 2 + 0.01), y, z);
      leaf.castShadow = true;
      leaf.receiveShadow = true;
      group.add(leaf);
      return leaf;
    });
    gates[st] = [leaves[0]!, leaves[1]!];
  }

  // Each frame: the cab's height, the cable and counterweight lengths, the glow, the gates and the lamps.
  let glow = 0.3; // the light bar's level (eased toward its target each frame)
  const setState = (s: LiftState): void => {
    cab.position.y = s.cabY + DECK_LIFT;
    const barY = s.cabY + DECK_LIFT + CANOPY_Y + CANOPY_T / 2;
    const cabLen = Math.max(0.01, top + 0.02 - (barY - 0.01)); // sunk 0.01 into the bar and the header
    cabCable.position.y = barY - 0.01 + cabLen / 2;
    cabCable.scale.y = cabLen;
    const cwY = low + FOOT + 0.02 + CW_H / 2 + (high - s.cabY); // the counterweight runs opposite the cab, never below the board
    cw.position.y = cwY;
    const cwTop = cwY + CW_H / 2;
    const cwLen = Math.max(0.01, top + 0.02 - (cwTop - 0.01));
    cwCable.position.y = cwTop - 0.01 + cwLen / 2;
    cwCable.scale.y = cwLen;
    glow += ((s.phase === 'travel' ? 1 : 0.3) - glow) * 0.15;
    barMat.color.copy(barBase).multiplyScalar(glow);
    for (const st of [0, 1] as const) {
      const open = s.stop === st ? s.doorT : 0;
      const [l, r] = gates[st];
      const off = GATE_W / 2 + 0.01 + open * GATE_W;
      l.position.x = -off;
      r.position.x = off;
      const here = s.stop === st && s.phase !== 'travel';
      lamps[st].material = here ? mats.lampHere : s.calls[st] > 0 ? mats.lampCall : mats.lampIdle;
    }
  };
  setState({ cabY: low, doorsOpen: false, boarding: [], doorT: 0, phase: 'idle', stop: 0, calls: [0, 0] });

  outline(group);

  const dispose = (): void => {
    for (const g of geos) g.dispose();
    barMat.dispose();
  };

  return { group, setState, dispose };
}

/** Warm-up: one lift built hidden in the scene, so its shared materials compile before ENTER (warm.ts compileSteps). */
export function prewarmLift(scene: THREE.Scene, palette: LiftPalette): void {
  const holder = new THREE.Group();
  holder.name = 'lift-warm';
  holder.visible = false;
  holder.add(buildLift({ x: 0, z: 0, face: 0, low: 0, high: 1.5 }, palette).group);
  scene.add(holder);
}
