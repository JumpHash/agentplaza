// Rooms: the builder's workshop and QA's test lab.

import * as THREE from 'three';
import type { RoomStyle } from './kit';

export const STYLES_A: Record<string, RoomStyle> = {
  builder: {
    name: 'Workshop',
    size: 4,
    shape: 'square',
    floor: 'planks',
    wall: 'brick',
    build(room) {
      const { k, s } = room;
      // A long workbench against the back wall, a vice and tools on it, a pegboard above.
      const bench = room.item(-0.6, -s + 0.75);
      room.box(bench, 2.4, 0.1, 0.7, k.wood, 0, 0.75, 0);
      for (const x of [-1.1, 1.1]) room.box(bench, 0.1, 0.75, 0.6, k.darkWood, x, 0, 0);
      room.box(bench, 2.2, 0.06, 0.5, k.darkWood, 0, 0.25, 0);
      room.box(bench, 0.22, 0.16, 0.2, k.metal, -0.8, 0.85, 0.1); // the vice
      room.box(bench, 0.32, 0.05, 0.08, k.red, 0.2, 0.85, 0.1); // a hammer: head and handle
      room.box(bench, 0.05, 0.05, 0.36, k.wood, 0.2, 0.85, 0.28);
      room.box(bench, 1.6, 0.9, 0.05, k.crate, 0, 1.05, -0.3); // the pegboard
      for (const [x, y, m] of [[-0.5, 1.5, k.red], [0, 1.3, k.blue], [0.5, 1.55, k.yellow]] as const) room.box(bench, 0.1, 0.36, 0.04, m, x, y - 0.2, -0.25);

      // A red toolbox, a sawhorse with a plank, and a stack of crates.
      const box = room.item(s - 0.7, -s + 0.8, -0.3);
      room.box(box, 0.6, 0.3, 0.35, k.red);
      room.box(box, 0.4, 0.06, 0.06, k.dark, 0, 0.36, 0);
      const horse = room.item(-s + 0.9, 0.9, Math.PI / 2);
      for (const x of [-0.35, 0.35]) {
        room.box(horse, 0.06, 0.5, 0.5, k.darkWood, x, 0, 0).rotation.x = 0.2;
      }
      room.box(horse, 1.2, 0.06, 0.3, k.wood, 0, 0.5, 0);
      room.crate(s - 0.6, 0.4, 0.55);
      room.crate(s - 0.55, 1.05, 0.5, 0.3);
      room.crate(s - 0.6, 0.6, 0.4).position.y = 0.55;

      // A big turning gear on a stand: the workshop's mascot.
      const stand = room.item(s - 0.6, -0.9);
      room.box(stand, 0.3, 0.5, 0.3, k.dark);
      const gear = new THREE.Group();
      gear.position.set(0, 1.0, 0);
      stand.add(gear);
      gear.userData.dynamic = true;
      room.cyl(gear, 0.38, 0.38, 0.12, k.gold, 0, -0.06, 0, 24).rotation.x = Math.PI / 2;
      for (let i = 0; i < 10; i++) {
        const a = (i / 10) * Math.PI * 2;
        const tooth = room.box(gear, 0.12, 0.12, 0.12, k.gold, Math.cos(a) * 0.44, Math.sin(a) * 0.44 - 0.06, 0);
        tooth.rotation.z = a;
      }
      room.anim.push((t) => (gear.rotation.z = t * 0.8));

      room.desk(-s + 1.0, s - 1.0, Math.PI / 2);
      room.lamp(s - 0.45, s - 0.5);
    },
  },

  qa: {
    name: 'Test Lab',
    size: 3.5,
    shape: 'square',
    floor: 'checker',
    wall: 'glass',
    build(room) {
      const { k, s } = room;
      // A lab bench with test tubes in a rack and a bubbling flask.
      const bench = room.item(0, -s + 0.7);
      room.box(bench, 2.2, 0.8, 0.6, k.white);
      room.box(bench, 2.3, 0.06, 0.66, k.dark, 0, 0.8, 0);
      const tubes = [k.green, k.blue, k.purple, k.red, k.yellow];
      tubes.forEach((m, i) => room.cyl(bench, 0.045, 0.045, 0.3, m, -0.8 + i * 0.13, 0.86, 0, 10));
      room.box(bench, 0.75, 0.05, 0.12, k.metal, -0.55, 1.0, 0);
      room.cyl(bench, 0.06, 0.2, 0.3, k.info, 0.6, 0.86, 0, 16);
      room.cyl(bench, 0.05, 0.05, 0.14, k.info, 0.6, 1.16, 0, 10);
      const bubbles: THREE.Mesh[] = [];
      for (let i = 0; i < 3; i++) bubbles.push(room.ball(bench, 0.04, k.white, 0.6, 1.3, 0));
      for (const b of bubbles) b.userData.dynamic = true; // they drift and shrink
      room.anim.push((t) => bubbles.forEach((b, i) => {
        const p = (t * 0.6 + i / 3) % 1;
        b.position.set(0.6 + Math.sin(p * 9 + i) * 0.04, 1.3 + p * 0.5, 0);
        b.scale.setScalar(1 - p * 0.7);
      }));

      // A pass/fail traffic light that cycles.
      const light = room.item(s - 0.5, -0.3);
      room.cyl(light, 0.05, 0.05, 1.3, k.metal, 0, 0, 0, 8);
      room.box(light, 0.3, 0.75, 0.25, k.dark, 0, 1.3, 0);
      const lamps = [k.glowRed, k.glowWarm, k.glowOk].map((m, i) => room.ball(light, 0.08, m, 0, 1.88 - i * 0.22, 0.13));
      for (const l of lamps) l.userData.dynamic = true; // visibility cycles
      room.anim.push((t) => lamps.forEach((l, i) => (l.visible = Math.floor(t / 1.5) % 3 === i)));

      // A test rig: a cube on a pedestal under a scanner arch.
      const rig = room.item(-s + 0.8, 0.2);
      room.cyl(rig, 0.35, 0.4, 0.3, k.metal, 0, 0, 0, 20);
      const cube = room.box(rig, 0.25, 0.25, 0.25, k.accent, 0, 0.42, 0);
      cube.userData.dynamic = true;
      room.anim.push((t) => (cube.rotation.y = t));
      for (const x of [-0.35, 0.35]) room.box(rig, 0.06, 1.0, 0.06, k.dark, x, 0.3, 0);
      room.box(rig, 0.76, 0.06, 0.12, k.dark, 0, 1.3, 0);
      room.box(rig, 0.5, 0.02, 0.06, k.glowOk, 0, 1.28, 0).userData.dynamic = true; // its glow colour is animated

      room.desk(s - 1.0, s - 0.9, -Math.PI / 2);
      room.plant(-s + 0.5, s - 0.5, 0.8);
    },
  },
};
