// A plaza robot: a small, cute toon 3D model in the plaza's palette, in one of a few variants (a
// WALL·E-like rover, a hover bubble, a TV-head on a ball wheel, a can with a dome head, a kitty with
// ears). Every variant has the same parts (a base, a body, a head with glowing eyes, two arms), so
// they share one animation. The scene owns placement; the robot owns its own animation.

import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import { outline, toonMat } from './toon';

export type RobotState = 'idle' | 'working' | 'walking' | 'offline';

/** A still pose for a portrait. */
export type RobotPose = 'wave' | 'proud' | 'think' | 'cheer';

/** The robot designs: an agent keeps its own (`robotVariant`); the orchestrator is always the rover. */
export type RobotVariant = 'rover' | 'hover' | 'tv' | 'can' | 'kitty';
export const ROBOT_VARIANTS: RobotVariant[] = ['rover', 'hover', 'tv', 'can', 'kitty'];

/** An agent's design: neighbouring cids (a district's members) get different ones. */
export function robotVariant(cid: number): RobotVariant {
  return ROBOT_VARIANTS[((cid % ROBOT_VARIANTS.length) + ROBOT_VARIANTS.length) % ROBOT_VARIANTS.length]!;
}

export interface Palette {
  ink: string; sheet: string; paper: string; rule: string; signal: string; ok: string; info: string; inkMuted: string;
}

/**
 * The robots' own paint, the same in the light and the dark theme: white shells, graphite running
 * gear, brushed-metal trim, mint eyes that turn cyan at work.
 */
export const ROBOT_PALETTE: Palette = {
  ink: '#262c3a', sheet: '#ffffff', paper: '#eef1f6', rule: '#9aa5b8',
  signal: '#ff7a2e', ok: '#3ee08f', info: '#4cc9ff', inkMuted: '#5b6475',
};

export interface Robot {
  /** Add this to the scene; position/rotate it from the scene. */
  readonly group: THREE.Group;
  /** World-space point just above the head, for the status label. */
  labelAnchor(target: THREE.Vector3): THREE.Vector3;
  setState(state: RobotState): void;
  /** Advance the animation: `dt` seconds since the last frame, `t` seconds since start. */
  update(dt: number, t: number): void;
  setHighlighted(on: boolean): void;
  /** Strikes a still pose (for a portrait); the next `update` moves it again. */
  pose(kind: RobotPose): void;
  dispose(): void;
}

// Layout in robot units: about one unit tall before the orchestrator's scale.
const SCALE = 1.3; // a touch bigger than the furniture, so the robots read from afar
const ORCH_SCALE = 1.25 * SCALE;
const LABEL_LIFT = 0.35;

type Add = (parent: THREE.Object3D, geometry: THREE.BufferGeometry, material: THREE.Material, x?: number, y?: number, z?: number) => THREE.Mesh;

interface Mats {
  sheet: THREE.Material; paper: THREE.Material; panel: THREE.Material; rivet: THREE.Material;
  ink: THREE.Material; signal: THREE.Material; glass: THREE.Material; pupil: THREE.Material;
  light: THREE.Material; thruster: THREE.Material;
}

/** The parts a variant builds; the shared animation drives them. */
interface Parts {
  head: THREE.Group;
  /** The top of the head above its pivot, for the label. */
  headTop: number;
  armL: THREE.Group;
  armR: THREE.Group;
  /** The glowing eyes: they blink (scale y). */
  pupils: THREE.Mesh[];
  /** Wheels and rotors: they turn about their own x (or y, for `yaw`) while walking. */
  spin: { obj: THREE.Object3D; yaw: boolean }[];
  /** Feet that step while walking. */
  feet: THREE.Object3D[];
  /** Floats on a thruster (bobs, lands when offline). */
  hover: boolean;
}

/** A shoulder pivot with a straight arm hanging from it and a hand at its end. */
function arm(add: Add, parent: THREE.Object3D, x: number, y: number, len: number, armMat: THREE.Material, hand: (grip: THREE.Group) => void): THREE.Group {
  const pivot = new THREE.Group();
  pivot.position.set(x, y, 0);
  parent.add(pivot);
  const geo = new THREE.CylinderGeometry(0.045, 0.04, len, 12);
  geo.translate(0, -len / 2, 0);
  add(pivot, geo, armMat);
  const grip = new THREE.Group();
  grip.position.set(0, -len, 0);
  pivot.add(grip);
  hand(grip);
  return pivot;
}

/** A round mitten hand. */
const mitten = (add: Add, mat: THREE.Material) => (grip: THREE.Group): void => {
  add(grip, new THREE.SphereGeometry(0.065, 14, 10), mat);
};

/** Two eye sockets (dark glass, glowing pupils) on a face at depth `z`. */
function eyes(add: Add, head: THREE.Object3D, m: Mats, xs: number[], y: number, z: number, glassR: number, pupilR: number): THREE.Mesh[] {
  const glassGeo = new THREE.CircleGeometry(glassR, 24);
  const pupilGeo = new THREE.CircleGeometry(pupilR, 24);
  return xs.map((x) => {
    add(head, glassGeo, m.glass, x, y, z);
    return add(head, pupilGeo, m.pupil, x, y, z + 0.004);
  });
}

/** The original: boxy body on tank treads, binocular eyes on a neck, claw arms. */
function rover(add: Add, _root: THREE.Group, body: THREE.Group, base: THREE.Group, m: Mats): Parts {
  const BODY_Y = 0.46;
  const treadGeo = new RoundedBoxGeometry(0.16, 0.22, 0.86, 3, 0.07);
  add(base, treadGeo, m.ink, -0.36, 0.11, 0);
  add(base, treadGeo, m.ink, 0.36, 0.11, 0);
  const wheelGeo = new THREE.CylinderGeometry(0.08, 0.08, 0.03, 16);
  wheelGeo.rotateZ(Math.PI / 2); // axle along x, so the wheel spins about rotation.x
  const spin: Parts['spin'] = [];
  for (const side of [-1, 1]) {
    for (const z of [-0.28, 0, 0.28]) spin.push({ obj: add(base, wheelGeo, m.rivet, side * 0.445, 0.11, z), yaw: false });
  }

  add(body, new RoundedBoxGeometry(0.62, 0.5, 0.6, 4, 0.12), m.sheet, 0, BODY_Y, 0);
  add(body, new RoundedBoxGeometry(0.36, 0.26, 0.03, 2, 0.01), m.panel, 0, BODY_Y, 0.315);
  add(body, new THREE.BoxGeometry(0.28, 0.012, 0.012), m.rivet, 0, BODY_Y, 0.33);
  const rivetGeo = new THREE.SphereGeometry(0.018, 8, 6);
  for (const x of [-0.15, 0.15]) {
    for (const y of [BODY_Y - 0.1, BODY_Y + 0.1]) add(body, rivetGeo, m.rivet, x, y, 0.33);
  }
  add(body, new THREE.CylinderGeometry(0.07, 0.08, 0.14, 12), m.ink, 0, 0.78, 0);

  const claw = (grip: THREE.Group): void => {
    add(grip, new RoundedBoxGeometry(0.1, 0.05, 0.08, 2, 0.02), m.ink);
    const finger = new THREE.BoxGeometry(0.025, 0.08, 0.03);
    add(grip, finger, m.ink, -0.035, -0.05, 0);
    add(grip, finger, m.ink, 0.035, -0.05, 0);
  };
  const armL = arm(add, body, -0.34, 0.6, 0.3, m.sheet, claw);
  const armR = arm(add, body, 0.34, 0.6, 0.3, m.sheet, claw);

  const HEAD_H = 0.24;
  const head = new THREE.Group();
  head.position.set(0, 0.82, 0);
  body.add(head);
  add(head, new RoundedBoxGeometry(0.62, HEAD_H, 0.42, 3, 0.09), m.paper, 0, HEAD_H / 2, 0);
  const eyeGeo = new THREE.CylinderGeometry(0.13, 0.13, 0.18, 24);
  eyeGeo.rotateX(Math.PI / 2); // axis along z, so the eyes face forward
  for (const x of [-0.16, 0.16]) add(head, eyeGeo, m.paper, x, 0.12, 0.2);
  const pupils = eyes(add, head, m, [-0.16, 0.16], 0.12, 0.292, 0.1, 0.07);
  return { head, headTop: HEAD_H, armL, armR, pupils, spin, feet: [], hover: false };
}

/** A round bubble floating on a glowing thruster ring, with a goggle visor and mitten hands. */
function hover(add: Add, _root: THREE.Group, body: THREE.Group, base: THREE.Group, m: Mats): Parts {
  // The thruster: a dark ring with a glowing disc under it; the ring turns.
  const rotor = new THREE.Group();
  rotor.position.set(0, 0.16, 0);
  base.add(rotor);
  const ringGeo = new THREE.TorusGeometry(0.22, 0.05, 10, 28);
  ringGeo.rotateX(Math.PI / 2);
  add(rotor, ringGeo, m.ink);
  const vaneGeo = new THREE.BoxGeometry(0.4, 0.02, 0.05);
  add(rotor, vaneGeo, m.rivet);
  add(rotor, vaneGeo.clone().rotateY(Math.PI / 2), m.rivet);
  const glowGeo = new THREE.CircleGeometry(0.19, 24);
  glowGeo.rotateX(Math.PI / 2); // faces down
  add(base, glowGeo, m.thruster, 0, 0.13, 0);

  // The bubble body with a metal seam round its middle.
  add(body, new THREE.SphereGeometry(0.31, 28, 20).scale(1, 0.92, 1), m.sheet, 0, 0.46, 0);
  const seamGeo = new THREE.TorusGeometry(0.305, 0.022, 8, 36);
  seamGeo.rotateX(Math.PI / 2);
  add(body, seamGeo, m.rivet, 0, 0.42, 0);
  add(body, new THREE.CircleGeometry(0.08, 20), m.panel, 0, 0.36, 0.296);

  const armL = arm(add, body, -0.31, 0.5, 0.2, m.sheet, mitten(add, m.paper));
  const armR = arm(add, body, 0.31, 0.5, 0.2, m.sheet, mitten(add, m.paper));

  // The head: a white dome with a dark goggle visor wrapped round its front.
  const head = new THREE.Group();
  head.position.set(0, 0.7, 0);
  body.add(head);
  add(head, new THREE.SphereGeometry(0.25, 28, 20), m.paper, 0, 0.14, 0);
  // The visor: a flattened dark lens bulging out of the dome's front.
  add(head, new THREE.SphereGeometry(0.21, 24, 14).scale(1, 0.55, 0.6), m.glass, 0, 0.15, 0.15);
  const pupilGeo = new THREE.CircleGeometry(0.045, 20).scale(1, 1.35, 1);
  const pupils = [-0.085, 0.085].map((x) => add(head, pupilGeo, m.pupil, x, 0.15, 0.278));
  // Two little antenna nubs.
  const nubGeo = new THREE.CylinderGeometry(0.012, 0.012, 0.1, 6);
  for (const x of [-0.12, 0.12]) {
    add(head, nubGeo, m.ink, x, 0.38, 0);
    add(head, new THREE.SphereGeometry(0.03, 10, 8), m.signal, x, 0.44, 0);
  }
  return { head, headTop: 0.44, armL, armR, pupils, spin: [{ obj: rotor, yaw: true }], feet: [], hover: true };
}

/** A box head with a screen face (eyes and a smile) on a short neck, rolling on one ball wheel. */
function tv(add: Add, _root: THREE.Group, body: THREE.Group, base: THREE.Group, m: Mats): Parts {
  // The ball wheel in a fork; the stripe shows it rolling.
  const wheel = new THREE.Group();
  wheel.position.set(0, 0.15, 0);
  base.add(wheel);
  add(wheel, new THREE.SphereGeometry(0.15, 20, 14), m.ink);
  const stripeGeo = new THREE.TorusGeometry(0.15, 0.022, 8, 28);
  stripeGeo.rotateY(Math.PI / 2); // round the axle (x)
  add(wheel, stripeGeo, m.rivet);
  const forkGeo = new RoundedBoxGeometry(0.05, 0.2, 0.14, 2, 0.02);
  add(base, forkGeo, m.sheet, -0.19, 0.2, 0);
  add(base, forkGeo, m.sheet, 0.19, 0.2, 0);

  // A compact body with three buttons, a neck.
  add(body, new RoundedBoxGeometry(0.48, 0.32, 0.4, 4, 0.1), m.sheet, 0, 0.44, 0);
  const buttonGeo = new THREE.SphereGeometry(0.03, 10, 8);
  add(body, buttonGeo, m.signal, -0.09, 0.44, 0.2);
  add(body, buttonGeo, m.panel, 0, 0.44, 0.2);
  add(body, buttonGeo, m.rivet, 0.09, 0.44, 0.2);
  add(body, new THREE.CylinderGeometry(0.06, 0.07, 0.1, 12), m.ink, 0, 0.64, 0);

  const ball = (grip: THREE.Group): void => {
    add(grip, new THREE.SphereGeometry(0.055, 12, 10), m.rivet);
  };
  const armL = arm(add, body, -0.28, 0.54, 0.24, m.ink, ball);
  const armR = arm(add, body, 0.28, 0.54, 0.24, m.ink, ball);

  // The head: a white set with a dark screen; on it, two round eyes and a smile.
  const head = new THREE.Group();
  head.position.set(0, 0.68, 0);
  body.add(head);
  add(head, new RoundedBoxGeometry(0.62, 0.44, 0.44, 4, 0.12), m.paper, 0, 0.22, 0);
  add(head, new RoundedBoxGeometry(0.48, 0.32, 0.04, 3, 0.06), m.glass, 0, 0.22, 0.212);
  const pupils = [-0.11, 0.11].map((x) => add(head, new THREE.CircleGeometry(0.06, 22), m.pupil, x, 0.25, 0.236));
  const smileGeo = new THREE.TorusGeometry(0.06, 0.013, 6, 16, Math.PI);
  smileGeo.rotateZ(Math.PI); // the lower half: a smile
  add(head, smileGeo, m.pupil, 0, 0.17, 0.236);
  // An antenna with a ball on top.
  add(head, new THREE.CylinderGeometry(0.012, 0.012, 0.14, 6), m.ink, 0.16, 0.5, 0);
  add(head, new THREE.SphereGeometry(0.04, 12, 8), m.signal, 0.16, 0.58, 0);
  return { head, headTop: 0.6, armL, armR, pupils, spin: [{ obj: wheel, yaw: false }], feet: [], hover: false };
}

/** A round can on two stubby feet, with a dome head and one big eye. */
function can(add: Add, _root: THREE.Group, body: THREE.Group, base: THREE.Group, m: Mats): Parts {
  const footGeo = new RoundedBoxGeometry(0.16, 0.08, 0.24, 2, 0.035);
  const legGeo = new THREE.CylinderGeometry(0.045, 0.05, 0.14, 10);
  const feet = [-0.12, 0.12].map((x) => {
    const foot = new THREE.Group();
    foot.position.set(x, 0, 0);
    base.add(foot);
    add(foot, footGeo, m.ink, 0, 0.04, 0.03);
    add(foot, legGeo, m.rivet, 0, 0.14, 0);
    return foot;
  });

  // The can, banded top and bottom, with a front panel.
  add(body, new THREE.CylinderGeometry(0.27, 0.27, 0.44, 28), m.sheet, 0, 0.42, 0);
  const bandGeo = new THREE.TorusGeometry(0.27, 0.022, 8, 32);
  bandGeo.rotateX(Math.PI / 2);
  add(body, bandGeo, m.rivet, 0, 0.22, 0);
  add(body, bandGeo, m.rivet, 0, 0.62, 0);
  add(body, new RoundedBoxGeometry(0.2, 0.16, 0.04, 2, 0.015), m.panel, 0, 0.42, 0.262);

  const armL = arm(add, body, -0.3, 0.56, 0.24, m.sheet, mitten(add, m.ink));
  const armR = arm(add, body, 0.3, 0.56, 0.24, m.sheet, mitten(add, m.ink));

  // The dome head with one big eye in a dark housing, and a light on top.
  const head = new THREE.Group();
  head.position.set(0, 0.64, 0);
  body.add(head);
  add(head, new THREE.SphereGeometry(0.27, 28, 14, 0, Math.PI * 2, 0, Math.PI / 2), m.paper);
  add(head, new THREE.CircleGeometry(0.27, 28).rotateX(Math.PI / 2), m.paper); // the dome's floor, seen from below
  const housingGeo = new THREE.CylinderGeometry(0.11, 0.11, 0.1, 24);
  housingGeo.rotateX(Math.PI / 2);
  add(head, housingGeo, m.ink, 0, 0.12, 0.23);
  const pupils = eyes(add, head, m, [0], 0.12, 0.282, 0.09, 0.06);
  add(head, new THREE.CylinderGeometry(0.035, 0.045, 0.05, 10), m.ink, 0, 0.29, 0);
  add(head, new THREE.SphereGeometry(0.04, 12, 8), m.signal, 0, 0.33, 0);
  return { head, headTop: 0.36, armL, armR, pupils, spin: [], feet, hover: false };
}

/** A big round head with ears, a small body on a four-wheeled chassis. */
function kitty(add: Add, _root: THREE.Group, body: THREE.Group, base: THREE.Group, m: Mats): Parts {
  add(base, new RoundedBoxGeometry(0.44, 0.1, 0.4, 2, 0.04), m.ink, 0, 0.13, 0);
  const wheelGeo = new THREE.CylinderGeometry(0.085, 0.085, 0.06, 16);
  wheelGeo.rotateZ(Math.PI / 2);
  const spin: Parts['spin'] = [];
  for (const x of [-0.23, 0.23]) {
    for (const z of [-0.12, 0.12]) spin.push({ obj: add(base, wheelGeo, m.rivet, x, 0.085, z), yaw: false });
  }

  add(body, new RoundedBoxGeometry(0.42, 0.28, 0.36, 4, 0.1), m.sheet, 0, 0.32, 0);
  const bellyGeo = new THREE.CircleGeometry(0.09, 20);
  add(body, bellyGeo, m.panel, 0, 0.31, 0.182);

  const armL = arm(add, body, -0.25, 0.4, 0.16, m.sheet, mitten(add, m.paper));
  const armR = arm(add, body, 0.25, 0.4, 0.16, m.sheet, mitten(add, m.paper));

  const head = new THREE.Group();
  head.position.set(0, 0.48, 0);
  body.add(head);
  add(head, new RoundedBoxGeometry(0.68, 0.46, 0.5, 5, 0.17), m.paper, 0, 0.23, 0);
  // Ears: the body's colour outside, the district's inside.
  const earGeo = new THREE.ConeGeometry(0.1, 0.17, 4);
  const innerGeo = new THREE.ConeGeometry(0.055, 0.1, 4);
  for (const side of [-1, 1]) {
    const ear = add(head, earGeo, m.sheet, side * 0.2, 0.5, 0);
    ear.rotation.set(0, Math.PI / 4, -side * 0.3);
    const inner = add(head, innerGeo, m.panel, side * 0.205, 0.49, 0.045);
    inner.rotation.set(0, Math.PI / 4, -side * 0.3);
  }
  const pupils = eyes(add, head, m, [-0.14, 0.14], 0.25, 0.252, 0.1, 0.065);
  const cheekGeo = new THREE.CircleGeometry(0.04, 16);
  add(head, cheekGeo, m.panel, -0.25, 0.14, 0.252);
  add(head, cheekGeo, m.panel, 0.25, 0.14, 0.252);
  const mouthGeo = new THREE.TorusGeometry(0.03, 0.01, 6, 12, Math.PI);
  mouthGeo.rotateZ(Math.PI);
  add(head, mouthGeo, m.ink, -0.03, 0.15, 0.252);
  add(head, mouthGeo, m.ink, 0.03, 0.15, 0.252);
  return { head, headTop: 0.58, armL, armR, pupils, spin, feet: [], hover: false };
}

const BUILD: Record<RobotVariant, (add: Add, root: THREE.Group, body: THREE.Group, base: THREE.Group, m: Mats) => Parts> = {
  rover, hover, tv, can, kitty,
};

/**
 * `bodyHue` (0–360) colours the robot's shell and arms, so every agent has its own colour;
 * `hue` (its district's) tints the front panel. The orchestrator is a larger rover with an antenna
 * light and a signal-orange trim.
 */
export function makeRobot(hue: number, orchestrator: boolean, palette: Palette, bodyHue = hue, variant: RobotVariant = 'rover'): Robot {
  const kind: RobotVariant = orchestrator ? 'rover' : variant;
  const root = new THREE.Group();
  root.name = orchestrator ? 'robot-orchestrator' : 'robot';
  root.scale.setScalar(orchestrator ? ORCH_SCALE : SCALE);

  const allMats: THREE.Material[] = [];
  // Materials that take the highlight glow (the shell and running gear).
  const shell: THREE.MeshToonMaterial[] = [];
  const std = (color: string | THREE.Color): THREE.MeshToonMaterial => {
    const m = toonMat({ color });
    allMats.push(m);
    shell.push(m);
    return m;
  };
  const glow = (color: string, intensity: number): THREE.MeshToonMaterial => {
    const m = toonMat({ color, emissive: color, emissiveIntensity: intensity });
    allMats.push(m);
    return m;
  };

  // Glass and lights are not part of the highlight, so it never washes out the eyes.
  const glassMat = toonMat({ color: palette.ink });
  allMats.push(glassMat);
  const pupilMat = glow(palette.ok, 1.2);
  const lightMat = glow(palette.signal, 1.2);
  const thrusterMat = new THREE.MeshBasicMaterial({ color: palette.info, toneMapped: false });
  allMats.push(thrusterMat);
  const m: Mats = {
    sheet: std(new THREE.Color().setHSL(bodyHue / 360, 0.62, 0.62, THREE.SRGBColorSpace)),
    paper: std(palette.paper),
    panel: std(new THREE.Color().setHSL(hue / 360, 0.55, 0.78, THREE.SRGBColorSpace)),
    rivet: std(palette.rule),
    ink: std(palette.ink),
    signal: std(palette.signal),
    glass: glassMat, pupil: pupilMat, light: lightMat, thruster: thrusterMat,
  };

  const add: Add = (parent, geometry, material, x = 0, y = 0, z = 0) => {
    const mesh = new THREE.Mesh(geometry, material);
    mesh.position.set(x, y, z);
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    parent.add(mesh);
    return mesh;
  };

  // The base (treads, wheels, feet or thruster) bobs when walking; the body carries the rest.
  const baseGroup = new THREE.Group();
  root.add(baseGroup);
  const bodyGroup = new THREE.Group();
  root.add(bodyGroup);
  const { head, headTop, armL, armR, pupils, spin, feet, hover: floats } = BUILD[kind](add, root, bodyGroup, baseGroup, m);

  let antennaLight: THREE.Mesh | undefined;
  if (orchestrator) {
    add(head, new THREE.CylinderGeometry(0.01, 0.01, 0.16, 6), m.ink, 0.22, headTop + 0.08, -0.05);
    antennaLight = add(head, new THREE.SphereGeometry(0.03, 10, 8), lightMat, 0.22, headTop + 0.17, -0.05);
    // Signal-orange trim across the front of the body, above the panel.
    add(bodyGroup, new RoundedBoxGeometry(0.36, 0.03, 0.02, 2, 0.008), m.signal, 0, 0.62, 0.31);
  }

  let state: RobotState = 'idle';
  let pupilIntensity = 1.2;
  const applyState = (next: RobotState): void => {
    state = next;
    const colour = next === 'offline' ? palette.inkMuted : next === 'working' ? palette.info : palette.ok;
    pupilMat.color.set(colour);
    pupilMat.emissive.set(colour);
    pupilIntensity = next === 'offline' ? 0.2 : next === 'working' ? 2.0 : 1.3;
    thrusterMat.color.set(next === 'offline' ? palette.inkMuted : palette.info);
  };
  applyState('idle');

  outline(root, 0.03);
  return {
    group: root,

    labelAnchor(target: THREE.Vector3): THREE.Vector3 {
      head.updateWorldMatrix(true, false);
      return head.localToWorld(target.set(0, headTop + LABEL_LIFT, 0));
    },

    setState(next: RobotState): void {
      if (next !== state) applyState(next);
    },

    update(dt: number, t: number): void {
      const k = Math.min(1, dt * 8);
      const s = state;

      // Head: idle tilts, working nods, walking glances, offline droops and stays still.
      let headX = 0;
      let headZ = 0;
      if (s === 'idle') {
        headX = 0.03 * Math.sin(t * 0.5);
        headZ = 0.12 * Math.sin(t * 0.9);
      } else if (s === 'working') {
        headX = 0.14 + 0.08 * Math.sin(t * 5);
        headZ = 0.04 * Math.sin(t * 1.3);
      } else if (s === 'walking') {
        headX = 0.04;
        headZ = 0.06 * Math.sin(t * 3);
      } else {
        headX = 0.5;
        headZ = 0.12;
      }
      head.rotation.x += (headX - head.rotation.x) * k;
      head.rotation.z += (headZ - head.rotation.z) * k;

      // Body: slight bob when idle, a sway when walking, slumped when offline.
      let bodyY = 0;
      let bodySway = 0;
      if (s === 'idle') bodyY = 0.012 * Math.sin(t * 1.6);
      else if (s === 'working') bodyY = 0.004 * Math.sin(t * 2.5);
      else if (s === 'walking') {
        bodyY = 0.01 * Math.abs(Math.sin(t * 6));
        bodySway = 0.05 * Math.sin(t * 6);
      } else bodyY = -0.02;
      // A hover robot floats on its thruster, and sets down when offline.
      const lift = floats ? (s === 'offline' ? -0.07 : 0.035 * Math.sin(t * 2.2)) : 0;
      bodyGroup.position.y = bodyY + lift;
      bodyGroup.rotation.z = bodySway;

      // The base bobs, wheels roll and feet step while walking; a rotor always turns.
      baseGroup.position.y = (s === 'walking' && !floats ? 0.012 * Math.abs(Math.sin(t * 10)) : 0) + lift;
      for (const w of spin) {
        if (w.yaw) w.obj.rotation.y += dt * (s === 'offline' ? 0 : s === 'walking' ? 9 : 3);
        else if (s === 'walking') w.obj.rotation.x += dt * 8;
      }
      feet.forEach((f, i) => {
        const p = s === 'walking' ? Math.sin(t * 9 + i * Math.PI) : 0;
        f.position.z = 0.06 * p;
        f.position.y = 0.035 * Math.max(0, p);
      });

      // Arms: quick alternating typing when working, a swing when walking, rest otherwise.
      if (s === 'working') {
        const p = Math.sin(t * 11);
        armL.rotation.x = -0.9 + 0.2 * p;
        armR.rotation.x = -0.9 - 0.2 * p;
      } else if (s === 'walking') {
        const w = 0.25 * Math.sin(t * 6);
        armL.rotation.x = w;
        armR.rotation.x = -w;
      } else if (s === 'idle') {
        const i = 0.05 * Math.sin(t * 1.1);
        armL.rotation.x = i;
        armR.rotation.x = -i;
      } else {
        armL.rotation.x = 0.2;
        armR.rotation.x = 0.2;
      }

      // Eyes: occasional blink when idle, half-lidded when offline, brighter and pulsing when working.
      let lid = 1;
      if (s === 'idle' && t % 3.6 < 0.14) lid = 0.1;
      else if (s === 'offline') lid = 0.45;
      for (const p of pupils) p.scale.y = lid;
      pupilMat.emissiveIntensity = s === 'working' ? pupilIntensity + 0.3 * Math.sin(t * 5) : pupilIntensity;

      if (antennaLight) lightMat.emissiveIntensity = 1.1 + 0.5 * Math.sin(t * 3);
    },

    pose(kind: RobotPose): void {
      armL.rotation.set(0, 0, 0);
      armR.rotation.set(0, 0, 0);
      head.rotation.set(0, 0, 0);
      if (kind === 'wave') {
        armR.rotation.z = 2.5;
        armL.rotation.z = -0.25;
        head.rotation.z = -0.15;
      } else if (kind === 'proud') {
        armL.rotation.z = -0.55;
        armR.rotation.z = 0.55;
        head.rotation.x = -0.12;
      } else if (kind === 'think') {
        armR.rotation.x = -1.9;
        armR.rotation.z = -0.35;
        head.rotation.z = 0.2;
        head.rotation.x = 0.08;
      } else {
        armL.rotation.z = -2.7;
        armR.rotation.z = 2.7;
        head.rotation.x = -0.15;
      }
    },

    setHighlighted(on: boolean): void {
      for (const mat of shell) {
        mat.emissive.set(on ? palette.signal : 0x000000);
        mat.emissiveIntensity = on ? 0.35 : 0;
      }
    },

    dispose(): void {
      root.traverse((obj) => {
        if (obj instanceof THREE.Mesh) obj.geometry.dispose();
      });
      for (const mat of allMats) mat.dispose();
    },
  };
}
