// Rooms: the counting house, the ops room, the antenna yard, the town hall and the mayor's office.

import * as THREE from 'three';
import type { RoomStyle } from './kit';

export const STYLES_D: Record<string, RoomStyle> = {
  finance: {
    name: 'Counting House',
    size: 3.5,
    shape: 'square',
    floor: 'marble',
    wall: 'stone',
    build(room) {
      const { k } = room;

      // Two coin stacks; a coin flips over the taller one.
      const coinsA = room.item(-2.5, -1.8);
      for (let i = 0; i < 6; i++) room.cyl(coinsA, 0.16, 0.16, 0.05, k.gold, 0, i * 0.05, 0, 20);
      const flip = room.cyl(coinsA, 0.16, 0.16, 0.05, k.gold, 0, 0.4, 0, 20);
      room.anim.push((t) => {
        const p = (t * 0.7) % 1;
        flip.position.y = 0.4 + Math.sin(p * Math.PI) * 0.25;
        flip.rotation.x = p * Math.PI * 2;
      });
      const coinsB = room.item(-2.0, -2.7);
      for (let i = 0; i < 3; i++) room.cyl(coinsB, 0.16, 0.16, 0.05, k.gold, 0, i * 0.05, 0, 20);

      // A pyramid of gold bars: six at the base, four above, one on top.
      const bars = room.item(2.3, -1.9);
      for (const z of [-0.13, 0.13]) {
        for (const x of [-0.42, 0, 0.42]) room.box(bars, 0.4, 0.14, 0.22, k.gold, x, 0, z);
        for (const x of [-0.21, 0.21]) room.box(bars, 0.4, 0.14, 0.22, k.gold, x, 0.14, z);
      }
      room.box(bars, 0.4, 0.14, 0.22, k.gold, 0, 0.28, 0);

      // A safe whose dial turns.
      const safe = room.item(2.4, -2.9);
      room.box(safe, 0.9, 0.9, 0.6, k.dark, 0, 0, 0);
      room.box(safe, 0.7, 0.7, 0.03, k.metal, 0, 0.1, 0.31);
      const dialPivot = new THREE.Group();
      dialPivot.position.set(0, 0.45, 0.35);
      safe.add(dialPivot);
      room.cyl(dialPivot, 0.12, 0.12, 0.04, k.gold, 0, 0, 0, 16).rotation.x = Math.PI / 2;
      room.anim.push((t) => (dialPivot.rotation.z = t * 0.9));

      // A teller counter with a brass bar grille along its top.
      const counter = room.item(0, -2.85);
      room.box(counter, 2.4, 0.9, 0.5, k.darkWood, 0, 0, 0);
      room.box(counter, 2.5, 0.08, 0.6, k.wood, 0, 0.9, 0);
      room.box(counter, 2.3, 0.06, 0.05, k.metal, 0, 1.5, 0.26);
      for (let i = 0; i <= 11; i++) room.box(counter, 0.04, 0.52, 0.04, k.metal, -1.1 + i * 0.2, 0.98, 0.26);

      room.desk(-2.3, 1.2, 0);
      room.plant(2.5, 2.4, 0.9);
    },
  },

  incident: {
    name: 'Ops Room',
    size: 3.5,
    shape: 'square',
    floor: 'hazard',
    wall: 'panel',
    build(room) {
      const { k } = room;

      // A wall of six monitors on the back wall, held up by two posts.
      const wall = room.item(0, -3.3);
      room.box(wall, 2.6, 1.2, 0.12, k.dark, 0, 0.7, 0);
      for (const x of [-1.1, 1.1]) room.box(wall, 0.1, 0.7, 0.1, k.metal, x, 0, 0);
      for (const y of [0.9, 1.4]) {
        for (const x of [-0.82, 0, 0.82]) room.box(wall, 0.7, 0.4, 0.03, k.screen, x, y, 0.075);
      }

      // A rotating red siren with a flashing red light.
      const siren = room.item(2.4, -2.4);
      room.box(siren, 0.5, 0.08, 0.5, k.dark, 0, 0, 0);
      room.cyl(siren, 0.04, 0.04, 0.9, k.metal, 0, 0.08, 0, 8);
      const head = new THREE.Group();
      head.position.set(0, 0.95, 0);
      siren.add(head);
      room.ball(head, 0.22, k.red, 0, 0, 0).scale.set(1, 0.6, 1);
      room.box(head, 0.1, 0.1, 0.1, k.glowRed, 0.2, -0.02, 0);
      const light = new THREE.PointLight(0xff4a3a, 0.8, 4, 2);
      light.position.set(0, 0.05, 0);
      head.add(light);
      room.anim.push((t) => {
        head.rotation.y = t * 3;
        light.intensity = 0.2 + 0.8 * Math.max(0, Math.sin(t * 3));
      });

      // Two fire extinguishers in the back corner.
      for (const [x, z] of [[-2.8, -2.7], [-2.3, -2.8]] as const) {
        const ext = room.item(x, z);
        room.cyl(ext, 0.1, 0.1, 0.5, k.red, 0, 0, 0, 14);
        room.cyl(ext, 0.105, 0.105, 0.08, k.white, 0, 0.2, 0, 14);
        room.cyl(ext, 0.04, 0.05, 0.08, k.dark, 0, 0.5, 0, 8);
        room.box(ext, 0.04, 0.22, 0.04, k.dark, 0.13, 0.25, 0);
      }

      // A desk with a red phone on it.
      const desk = room.desk(-2.2, 1.0, 0);
      room.box(desk, 0.22, 0.06, 0.16, k.red, 0.3, 0.56, 0.1);
      room.box(desk, 0.26, 0.04, 0.05, k.red, 0.3, 0.62, 0.1);

      // A whiteboard on the left wall, with a few marker scribbles.
      const board = room.item(-3.3, 0, Math.PI / 2);
      room.box(board, 1.5, 1.0, 0.04, k.metal, 0, 0.8, -0.02);
      room.box(board, 1.4, 0.9, 0.05, k.white, 0, 0.85, 0);
      room.box(board, 0.5, 0.04, 0.02, k.red, -0.25, 1.3, 0.03);
      room.box(board, 0.8, 0.04, 0.02, k.blue, 0.1, 1.1, 0.03);
      room.box(board, 0.4, 0.04, 0.02, k.green, -0.3, 0.95, 0.03);

      room.plant(2.6, 2.6, 0.9);
    },
  },

  'edge-operator': {
    name: 'Antenna Yard',
    size: 4,
    shape: 'round',
    floor: 'gravel',
    wall: 'fence',
    build(room) {
      const { k } = room;

      // A lattice radio mast: six rings of struts, with a dish on top that turns and a blinking tip light.
      const mast = room.item(-2.0, -1.7);
      const corners = [[-1, -1], [1, -1], [1, 1], [-1, 1]] as const;
      const levelY = (i: number) => i * 0.5;
      const levelW = (i: number) => 0.3 - i * 0.035;
      const at = (i: number, c: number): [number, number, number] => {
        const [sx, sz] = corners[c]!;
        return [sx * levelW(i), levelY(i), sz * levelW(i)];
      };
      const strut = (a: [number, number, number], b: [number, number, number]) => {
        const dir = new THREE.Vector3(b[0] - a[0], b[1] - a[1], b[2] - a[2]);
        const len = dir.length();
        const m = room.box(mast, 0.035, len, 0.035, k.metal, (a[0] + b[0]) / 2, (a[1] + b[1]) / 2 - len / 2, (a[2] + b[2]) / 2);
        m.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), dir.normalize());
      };
      for (let i = 0; i < 6; i++) {
        for (let c = 0; c < 4; c++) {
          strut(at(i, c), at(i, (c + 1) % 4));
          if (i < 5) {
            strut(at(i, c), at(i + 1, c));
            strut(at(i, c), at(i + 1, (c + 1) % 4));
          }
        }
      }
      const arm = new THREE.Group();
      arm.position.set(0, 2.5, 0);
      mast.add(arm);
      room.cyl(arm, 0.5, 0.08, 0.16, k.white, 0, -0.08, 0.15, 24).rotation.x = Math.PI / 2;
      room.box(arm, 0.05, 0.05, 0.2, k.metal, 0, -0.025, 0.07);
      room.anim.push((t) => (arm.rotation.y = t * 0.5));
      const tip = room.ball(mast, 0.07, k.glowRed, 0, 3.1, 0);
      room.anim.push((t) => (tip.visible = Math.sin(t * 4) > 0));

      // Cable spools lying on their sides.
      for (const [x, z] of [[2.2, 1.2], [1.5, 2.3]] as const) {
        const spool = room.item(x, z);
        const drum = room.cyl(spool, 0.2, 0.2, 0.4, k.yellow, 0, 0, 0, 16);
        drum.rotation.z = Math.PI / 2;
        drum.position.set(0, 0.3, 0);
        for (const fx of [-0.22, 0.22]) {
          const flange = room.cyl(spool, 0.3, 0.3, 0.04, k.dark, 0, 0, 0, 16);
          flange.rotation.z = Math.PI / 2;
          flange.position.set(fx, 0.3, 0);
        }
      }

      // Two solar panels tilted toward the front, on legs.
      for (const [x, z] of [[2.3, -1.6], [2.3, -0.3]] as const) {
        const panel = room.item(x, z);
        room.cyl(panel, 0.03, 0.03, 0.6, k.metal, -0.35, 0, 0, 6);
        room.cyl(panel, 0.03, 0.03, 0.6, k.metal, 0.35, 0, 0, 6);
        const tilt = new THREE.Group();
        tilt.position.set(0, 0.6, 0);
        tilt.rotation.x = 0.5;
        panel.add(tilt);
        room.box(tilt, 0.8, 0.04, 0.5, k.blue, 0, -0.02, 0);
        room.box(tilt, 0.02, 0.05, 0.5, k.white, 0, -0.02, 0);
      }

      // A small equipment hut with a red roof and a lit window.
      const hut = room.item(-2.1, 1.3);
      room.box(hut, 0.8, 0.7, 0.8, k.white, 0, 0, 0);
      room.cone(hut, 0.65, 0.5, k.red, 0, 0.7, 0, 4).rotation.y = Math.PI / 4;
      room.box(hut, 0.25, 0.42, 0.02, k.darkWood, 0, 0, 0.41);
      room.box(hut, 0.22, 0.2, 0.02, k.glowWarm, -0.2, 0.42, 0.41);

      // The operator's desk, facing the fence.
      room.desk(0, 2.5, Math.PI);
    },
  },

  mayor: {
    name: 'Town Hall',
    size: 4.5,
    shape: 'square',
    floor: 'marble',
    wall: 'stone',
    build(room) {
      const { k } = room;

      // A row of five columns along the back wall, each with a base and a capital.
      for (const x of [-3.2, -1.6, 0, 1.6, 3.2]) {
        const col = room.item(x, -3.9);
        room.box(col, 0.6, 0.12, 0.6, k.white, 0, 0, 0);
        room.cyl(col, 0.2, 0.22, 2.0, k.white, 0, 0.12, 0, 16);
        room.box(col, 0.6, 0.14, 0.6, k.white, 0, 2.12, 0);
      }

      // The podium with its microphone, and the town seal on its front.
      const podium = room.item(0, -2.7);
      room.box(podium, 0.9, 1.0, 0.5, k.darkWood, 0, 0, 0);
      room.box(podium, 1.0, 0.08, 0.6, k.wood, 0, 1.0, 0);
      room.box(podium, 0.5, 0.4, 0.02, k.gold, 0, 0.3, 0.26);
      room.cyl(podium, 0.015, 0.015, 0.4, k.metal, 0.25, 1.08, 0.05, 6);
      room.ball(podium, 0.06, k.dark, 0.25, 1.52, 0.05);

      // A flag on a pole by the left wall, its cloth rippling in the wind.
      const flag = room.item(-3.6, -2.4);
      room.cyl(flag, 0.03, 0.03, 2.6, k.metal, 0, 0, 0, 8);
      room.ball(flag, 0.06, k.gold, 0, 2.62, 0);
      const wave = new THREE.Group();
      wave.position.set(0, 2.22, 0);
      flag.add(wave);
      const cloth = room.box(wave, 0.7, 0.45, 0.02, k.blue, 0.35, -0.225, 0);
      room.anim.push((t) => {
        wave.rotation.y = Math.sin(t * 3) * 0.3;
        cloth.rotation.z = Math.sin(t * 6) * 0.1;
        cloth.scale.y = 1 + 0.08 * Math.sin(t * 6);
      });

      // A red carpet from the podium to the front.
      room.rug(1.2, 6.6, k.red, 0, 0.9);

      // Two benches along the side walls, backs to the walls.
      for (const [x, rot] of [[-3.6, Math.PI / 2], [3.6, -Math.PI / 2]] as const) {
        const bench = room.item(x, 0.6, rot);
        for (const lx of [-0.7, 0.7]) room.box(bench, 0.08, 0.42, 0.45, k.darkWood, lx, 0, 0);
        room.box(bench, 1.6, 0.08, 0.5, k.wood, 0, 0.42, 0);
        room.box(bench, 1.6, 0.42, 0.06, k.wood, 0, 0.5, -0.22);
      }

      room.desk(2.4, 2.4, Math.PI);
      room.plant(-3.3, 2.9, 1);
    },
  },

  'mayor-office': {
    name: "Mayor's Office",
    size: 3,
    shape: 'square',
    floor: 'carpet',
    wall: 'panel',
    build(room) {
      const { k } = room;

      // A wide executive desk against the back wall, with a monitor.
      const desk = room.item(0, -2.3);
      room.box(desk, 2.0, 0.07, 0.8, k.wood, 0, 0.72, 0);
      for (const x of [-0.65, 0.65]) room.box(desk, 0.5, 0.72, 0.7, k.darkWood, x, 0, 0);
      room.box(desk, 0.28, 0.02, 0.18, k.metal, 0, 0.79, -0.25);
      room.box(desk, 0.07, 0.14, 0.05, k.metal, 0, 0.81, -0.25);
      room.box(desk, 0.6, 0.4, 0.04, k.screen, 0, 0.95, -0.25);

      // A tall armchair in the corner, angled toward the room.
      const chair = room.item(-2.1, -2.1, Math.PI / 4);
      room.box(chair, 0.6, 0.05, 0.6, k.darkWood, 0, 0, 0);
      room.box(chair, 0.7, 0.3, 0.7, k.accent, 0, 0.05, 0);
      room.box(chair, 0.12, 0.45, 0.7, k.accent, -0.29, 0.05, 0);
      room.box(chair, 0.12, 0.45, 0.7, k.accent, 0.29, 0.05, 0);
      room.box(chair, 0.7, 1.1, 0.14, k.accent, 0, 0.35, -0.28);

      // A bookshelf on the right wall, its books facing the room.
      room.shelf(2.75, -0.9, -Math.PI / 2, 1.2, 1.6);

      // A flag on a pole in the back-right corner.
      const flag = room.item(2.3, -2.5);
      room.cyl(flag, 0.12, 0.12, 0.05, k.gold, 0, 0, 0, 16);
      room.cyl(flag, 0.03, 0.03, 2.2, k.metal, 0, 0, 0, 8);
      room.box(flag, 0.5, 0.36, 0.02, k.blue, 0.27, 1.7, 0);
      room.ball(flag, 0.05, k.gold, 0, 2.23, 0);

      // A framed portrait on the back wall, above the desk.
      const portrait = room.item(0, -2.94);
      room.box(portrait, 0.6, 0.7, 0.04, k.gold, 0, 1.45, 0);
      room.box(portrait, 0.5, 0.6, 0.02, k.accentSoft, 0, 1.5, 0.02);
      room.box(portrait, 0.3, 0.18, 0.03, k.blue, 0, 1.45, 0.05);
      room.ball(portrait, 0.1, k.clay, 0, 1.68, 0.05);

      // A round rug at the room's centre.
      room.rug(1.8, 1.8, k.purple, 0, 0.9, true);

      room.plant(-2.4, 2.2, 0.9);
    },
  },
};
