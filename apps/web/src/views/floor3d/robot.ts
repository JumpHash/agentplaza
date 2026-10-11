// A plaza robot: a small, cute, WALL·E-inspired 3D model (boxy body, tank
// treads, binocular eyes on a neck, little arms) in the plaza's palette.
// The scene owns placement; the robot owns its own animation.

import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import { outline, toonMat } from './toon';

export type RobotState = 'idle' | 'working' | 'walking' | 'offline';

export interface Palette {
  ink: string; sheet: string; paper: string; rule: string; signal: string; ok: string; info: string; inkMuted: string;
}

export interface Robot {
  /** Add this to the scene; position/rotate it from the scene. */
  readonly group: THREE.Group;
  /** World-space point just above the head, for the status label. */
  labelAnchor(target: THREE.Vector3): THREE.Vector3;
  setState(state: RobotState): void;
  /** Advance the animation: `dt` seconds since the last frame, `t` seconds since start. */
  update(dt: number, t: number): void;
  setHighlighted(on: boolean): void;
  dispose(): void;
}

// Layout in robot units: about one unit tall before the orchestrator's scale.
const BODY_Y = 0.46;      // body centre
const BODY_H = 0.5;
const NECK_Y = 0.78;
const HEAD_Y = 0.82;      // head pivot, at the top of the neck
const HEAD_H = 0.24;
const SCALE = 1.3; // a touch bigger than the furniture, so the robots read from afar
const ORCH_SCALE = 1.25 * SCALE;
const LABEL_LIFT = 0.35;

/**
 * `bodyHue` (0–360) colours the robot's shell and arms, so every agent has its own colour;
 * `hue` (its district's) tints the front panel. The orchestrator is larger with a signal-orange trim.
 */
export function makeRobot(hue: number, orchestrator: boolean, palette: Palette, bodyHue = hue): Robot {
  const root = new THREE.Group();
  root.name = orchestrator ? 'robot-orchestrator' : 'robot';
  root.scale.setScalar(orchestrator ? ORCH_SCALE : SCALE);

  const allMats: THREE.Material[] = [];
  // Materials that take the highlight glow (the shell and running gear).
  const shell: THREE.MeshToonMaterial[] = [];
  const std = (color: string | THREE.Color, roughness: number, metalness = 0.1): THREE.MeshToonMaterial => {
    const m = toonMat({ color, roughness, metalness });
    allMats.push(m);
    shell.push(m);
    return m;
  };

  const sheetMat = std(new THREE.Color().setHSL(bodyHue / 360, 0.6, 0.64, THREE.SRGBColorSpace), 0.65);
  const paperMat = std(palette.paper, 0.6);
  const panelMat = std(new THREE.Color().setHSL(hue / 360, 0.5, 0.82, THREE.SRGBColorSpace), 0.5);
  const rivetMat = std(palette.rule, 0.45);
  const inkMat = std(palette.ink, 0.8, 0.05);
  const signalMat = std(palette.signal, 0.5);

  // Glass and lights are not part of the glow, so the highlight never washes out the eyes.
  const glassMat = toonMat({ color: palette.ink, roughness: 0.12, metalness: 0.4 });
  const pupilMat = toonMat({
    color: palette.ok, emissive: palette.ok, emissiveIntensity: 1.2, roughness: 0.3,
  });
  const lightMat = toonMat({
    color: palette.signal, emissive: palette.signal, emissiveIntensity: 1.2, roughness: 0.4,
  });
  allMats.push(glassMat, pupilMat, lightMat);

  const add = (
    parent: THREE.Object3D, geometry: THREE.BufferGeometry, material: THREE.Material,
    x = 0, y = 0, z = 0,
  ): THREE.Mesh => {
    const mesh = new THREE.Mesh(geometry, material);
    mesh.position.set(x, y, z);
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    parent.add(mesh);
    return mesh;
  };

  // Treads and wheels: the tread group bobs when walking.
  const treadGroup = new THREE.Group();
  root.add(treadGroup);
  const treadGeo = new RoundedBoxGeometry(0.16, 0.22, 0.86, 3, 0.07);
  add(treadGroup, treadGeo, inkMat, -0.36, 0.11, 0);
  add(treadGroup, treadGeo, inkMat, 0.36, 0.11, 0);
  const wheelGeo = new THREE.CylinderGeometry(0.08, 0.08, 0.03, 16);
  wheelGeo.rotateZ(Math.PI / 2); // axle along x, so the wheel spins about rotation.x
  const wheels: THREE.Mesh[] = [];
  for (const side of [-1, 1]) {
    for (const z of [-0.28, 0, 0.28]) {
      wheels.push(add(treadGroup, wheelGeo, rivetMat, side * 0.445, 0.11, z));
    }
  }

  // Body: rounded shell, a tinted front panel with a panel line and rivets.
  const bodyGroup = new THREE.Group();
  root.add(bodyGroup);
  add(bodyGroup, new RoundedBoxGeometry(0.62, BODY_H, 0.6, 4, 0.12), sheetMat, 0, BODY_Y, 0);
  add(bodyGroup, new RoundedBoxGeometry(0.36, 0.26, 0.03, 2, 0.01), panelMat, 0, BODY_Y, 0.315);
  add(bodyGroup, new THREE.BoxGeometry(0.28, 0.012, 0.012), rivetMat, 0, BODY_Y, 0.33);
  const rivetGeo = new THREE.SphereGeometry(0.018, 8, 6);
  for (const x of [-0.15, 0.15]) {
    for (const y of [BODY_Y - 0.1, BODY_Y + 0.1]) {
      add(bodyGroup, rivetGeo, rivetMat, x, y, 0.33);
    }
  }
  add(bodyGroup, new THREE.CylinderGeometry(0.07, 0.08, 0.14, 12), inkMat, 0, NECK_Y, 0);

  // Little arms: a pivot at each shoulder, a simple two-finger gripper at the end.
  const armGeo = new THREE.CylinderGeometry(0.045, 0.04, 0.3, 12);
  armGeo.translate(0, -0.15, 0);
  const clawBaseGeo = new RoundedBoxGeometry(0.1, 0.05, 0.08, 2, 0.02);
  const fingerGeo = new THREE.BoxGeometry(0.025, 0.08, 0.03);
  const makeArm = (side: number): THREE.Group => {
    const pivot = new THREE.Group();
    pivot.position.set(side * 0.34, 0.6, 0);
    bodyGroup.add(pivot);
    add(pivot, armGeo, sheetMat);
    const grip = new THREE.Group();
    grip.position.set(0, -0.3, 0);
    pivot.add(grip);
    add(grip, clawBaseGeo, inkMat);
    add(grip, fingerGeo, inkMat, -0.035, -0.05, 0);
    add(grip, fingerGeo, inkMat, 0.035, -0.05, 0);
    return pivot;
  };
  const armL = makeArm(-1);
  const armR = makeArm(1);

  // Head: a paper housing with two big eye-cylinders (dark glass, glowing pupils).
  const head = new THREE.Group();
  head.position.set(0, HEAD_Y, 0);
  bodyGroup.add(head);
  add(head, new RoundedBoxGeometry(0.62, HEAD_H, 0.42, 3, 0.09), paperMat, 0, HEAD_H / 2, 0);
  const eyeGeo = new THREE.CylinderGeometry(0.13, 0.13, 0.18, 24);
  eyeGeo.rotateX(Math.PI / 2); // axis along z, so the eyes face forward
  const glassGeo = new THREE.CircleGeometry(0.1, 24);
  const pupilGeo = new THREE.CircleGeometry(0.07, 24);
  const pupils: THREE.Mesh[] = [];
  for (const x of [-0.16, 0.16]) {
    add(head, eyeGeo, paperMat, x, 0.12, 0.2);
    add(head, glassGeo, glassMat, x, 0.12, 0.292);
    pupils.push(add(head, pupilGeo, pupilMat, x, 0.12, 0.296));
  }

  let antennaLight: THREE.Mesh | undefined;
  if (orchestrator) {
    add(head, new THREE.CylinderGeometry(0.01, 0.01, 0.16, 6), inkMat, 0.22, HEAD_H + 0.08, -0.05);
    antennaLight = add(head, new THREE.SphereGeometry(0.03, 10, 8), lightMat, 0.22, HEAD_H + 0.17, -0.05);
    // Signal-orange trim across the front of the body, above the panel.
    add(bodyGroup, new RoundedBoxGeometry(0.36, 0.03, 0.02, 2, 0.008), signalMat, 0, 0.62, 0.31);
  }

  let state: RobotState = 'idle';
  let pupilIntensity = 1.2;
  const applyState = (next: RobotState): void => {
    state = next;
    const colour = next === 'offline' ? palette.inkMuted : next === 'working' ? palette.info : palette.ok;
    pupilMat.color.set(colour);
    pupilMat.emissive.set(colour);
    pupilIntensity = next === 'offline' ? 0.2 : next === 'working' ? 2.0 : 1.3;
  };
  applyState('idle');

  outline(root, 0.03);
  return {
    group: root,

    labelAnchor(target: THREE.Vector3): THREE.Vector3 {
      head.updateWorldMatrix(true, false);
      return head.localToWorld(target.set(0, HEAD_H + LABEL_LIFT, 0));
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
      bodyGroup.position.y = bodyY;
      bodyGroup.rotation.z = bodySway;

      // Treads bob and wheels roll while walking.
      treadGroup.position.y = s === 'walking' ? 0.012 * Math.abs(Math.sin(t * 10)) : 0;
      if (s === 'walking') {
        for (const w of wheels) w.rotation.x += dt * 8;
      }

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

    setHighlighted(on: boolean): void {
      for (const m of shell) {
        m.emissive.set(on ? palette.signal : 0x000000);
        m.emissiveIntensity = on ? 0.35 : 0;
      }
    },

    dispose(): void {
      root.traverse((obj) => {
        if (obj instanceof THREE.Mesh) obj.geometry.dispose();
      });
      for (const m of allMats) m.dispose();
    },
  };
}
