// Rooms: the researcher's observatory, the librarian's library, the planner's planning room and
// the auditor's ledger office.

import * as THREE from 'three';
import type { RoomStyle } from './kit';

export const STYLES_C: Record<string, RoomStyle> = {
  researcher: {
    name: 'Observatory',
    size: 4,
    shape: 'round',
    floor: 'stars',
    wall: 'stone',
    build(room) {
      const { k } = room;
      // A big telescope on a splayed tripod: the whole scope sweeps slowly and the tube nods.
      const scope = room.item(0, -2.4);
      scope.userData.dynamic = true; // the whole scope sweeps; its tube nods
      room.cyl(scope, 0.2, 0.2, 0.05, k.dark, 0, 0, 0, 16);
      for (let i = 0; i < 3; i++) {
        const a = (i / 3) * Math.PI * 2;
        const leg = new THREE.Group();
        leg.rotation.order = 'YXZ';
        leg.rotation.y = -a - Math.PI / 2;
        leg.rotation.x = 0.32;
        leg.position.set(0, 1.08, 0);
        scope.add(leg);
        room.box(leg, 0.05, 1.12, 0.05, k.metal, 0, -1.12, 0);
      }
      const tube = new THREE.Group();
      tube.position.set(0, 1.08, 0);
      tube.rotation.x = 0.9;
      scope.add(tube);
      room.cyl(tube, 0.12, 0.09, 0.9, k.white, 0, 0, 0, 16);
      room.cyl(tube, 0.15, 0.15, 0.06, k.gold, 0, 0.9, 0, 16);
      room.anim.push((t) => {
        scope.rotation.y = Math.sin(t * 0.25) * 0.7;
        tube.rotation.x = 0.9 + Math.sin(t * 0.4) * 0.12;
      });

      // A star-chart board on an easel.
      const chart = room.item(1.5, -2.6);
      for (const x of [-0.5, 0.5]) room.box(chart, 0.06, 0.4, 0.06, k.darkWood, x, 0, -0.04);
      room.box(chart, 1.2, 0.9, 0.05, k.dark, 0, 0.35, 0);
      const stars: [number, number][] = [[-0.4, 1.1], [-0.1, 1.0], [0.2, 1.15], [0.4, 0.95], [-0.25, 0.8], [0.1, 0.75]];
      for (const [x, y] of stars) room.ball(chart, 0.035, k.gold, x, y, 0.04);
      room.box(chart, 0.36, 0.012, 0.012, k.accentSoft, -0.15, 0.92, 0.035).rotation.z = 0.5;
      room.box(chart, 0.3, 0.012, 0.012, k.accentSoft, 0.3, 0.85, 0.035).rotation.z = -0.4;

      // A curved run of bookshelves following the round wall on the left.
      for (const phi of [-0.9, -1.25, -1.6]) room.shelf(3.0 * Math.sin(phi), -3.0 * Math.cos(phi), -phi, 1.2, 1.6);

      // A globe on a stand that spins on its post, ringed by a brass meridian.
      const globeStand = room.item(2.4, 0.4);
      room.cyl(globeStand, 0.3, 0.3, 0.06, k.dark, 0, 0, 0, 16);
      room.cyl(globeStand, 0.04, 0.04, 1.2, k.gold, 0, 0.06, 0, 8);
      const ring = new THREE.Mesh(new THREE.TorusGeometry(0.46, 0.03, 8, 32), k.gold);
      ring.position.y = 1.3;
      ring.rotation.x = Math.PI / 2;
      ring.castShadow = true;
      globeStand.add(ring);
      const globe = new THREE.Group();
      globe.position.set(0, 1.3, 0);
      globeStand.add(globe);
      globe.userData.dynamic = true;
      room.ball(globe, 0.4, k.blue, 0, 0, 0);
      room.ball(globe, 0.16, k.green, 0.2, 0.1, 0.32);
      room.ball(globe, 0.13, k.green, -0.25, -0.12, 0.25);
      room.ball(globe, 0.12, k.green, 0.05, -0.2, -0.36);
      room.anim.push((t) => (globe.rotation.y = t * 0.6));

      room.desk(-2.0, 1.9, Math.PI / 2);
      room.lamp(2.1, 1.9);
    },
  },

  librarian: {
    name: 'Library',
    size: 4.5,
    shape: 'square',
    floor: 'parquet',
    wall: 'panel',
    build(room) {
      const { k, s } = room;
      // Bookshelves along the back wall and both side walls.
      for (const x of [-3, -1.5, 0, 1.5, 3]) room.shelf(x, -s + 0.25, 0);
      for (const z of [-2.6, -1.1, 0.4]) {
        room.shelf(-s + 0.25, z, Math.PI / 2);
        room.shelf(s - 0.25, z, -Math.PI / 2);
      }

      // A rolling ladder leaning on the left-hand shelves.
      const ladder = room.item(-3.0, 0.9);
      for (const z of [-0.27, 0.27]) {
        const rail = room.box(ladder, 0.06, 2.2, 0.06, k.wood, 0, 0, z);
        rail.rotation.z = 0.2;
      }
      for (const y of [0.35, 0.7, 1.05, 1.4, 1.75]) {
        room.box(ladder, 0.05, 0.05, 0.56, k.wood, -(y - 1.1) * Math.tan(0.2), y - 0.025, 0);
      }

      // A reading corner: a chunky armchair on a round rug, with a floor lamp beside it.
      room.rug(2.2, 2.2, k.mat(0xc96a5a), 2.5, 2.0, true);
      const chair = room.item(2.5, 2.2, Math.atan2(-2.5, -2.2));
      room.box(chair, 1.0, 0.36, 0.9, k.accent, 0, 0.14, 0);
      room.box(chair, 0.86, 0.12, 0.8, k.accentSoft, 0, 0.5, 0.02);
      room.box(chair, 1.0, 0.8, 0.22, k.accent, 0, 0.14, -0.34);
      for (const x of [-0.41, 0.41]) room.box(chair, 0.18, 0.38, 0.9, k.accent, x, 0.14, 0);
      for (const x of [-0.4, 0.4]) for (const z of [-0.36, 0.36]) room.box(chair, 0.1, 0.14, 0.1, k.dark, x, 0, z);
      room.lamp(3.6, 1.0);

      // A book trolley whose wheels turn as it rolls.
      const cart = room.item(2.0, 3.4);
      room.box(cart, 0.7, 0.04, 0.45, k.wood, 0, 0.4, 0);
      room.box(cart, 0.7, 0.04, 0.45, k.wood, 0, 0.9, 0);
      for (const x of [-0.32, 0.32]) for (const z of [-0.2, 0.2]) room.box(cart, 0.04, 0.9, 0.04, k.metal, x, 0, z);
      room.box(cart, 0.1, 0.22, 0.3, k.red, -0.2, 0.44, 0);
      room.box(cart, 0.1, 0.22, 0.3, k.blue, -0.08, 0.44, 0);
      room.box(cart, 0.1, 0.26, 0.3, k.green, -0.2, 0.94, 0);
      room.box(cart, 0.1, 0.22, 0.3, k.yellow, -0.08, 0.94, 0);
      const wheels: THREE.Group[] = [];
      for (const x of [-0.32, 0.32]) for (const z of [-0.2, 0.2]) {
        const w = new THREE.Group();
        w.position.set(x, 0.07, z);
        cart.add(w);
        w.userData.dynamic = true;
        room.cyl(w, 0.07, 0.07, 0.04, k.dark, 0, -0.02, 0, 10).rotation.z = Math.PI / 2;
        wheels.push(w);
      }
      room.anim.push((t) => wheels.forEach((w) => (w.rotation.x = t * 3)));

      // A card catalogue: a cabinet of little drawers with knobs.
      const cat = room.item(-2.8, 2.6);
      room.box(cat, 1.0, 1.5, 0.5, k.darkWood, 0, 0, 0);
      room.box(cat, 1.04, 0.05, 0.54, k.wood, 0, 1.5, 0);
      for (let r = 0; r < 5; r++) {
        for (let c = 0; c < 3; c++) {
          room.box(cat, 0.26, 0.2, 0.03, k.crate, -0.3 + c * 0.3, 0.1 + r * 0.27, 0.265);
          room.ball(cat, 0.025, k.gold, -0.3 + c * 0.3, 0.2 + r * 0.27, 0.29);
        }
      }

      room.desk(-1.6, 3.6, Math.PI);
    },
  },

  planner: {
    name: 'Planning Room',
    size: 3.5,
    shape: 'round',
    floor: 'carpet',
    wall: 'hedge',
    build(room) {
      const { k } = room;
      // A kanban board on two posts: three columns of sticky notes. One note wanders between columns.
      const kb = room.item(0, -2.5);
      for (const x of [-0.9, 0.9]) room.cyl(kb, 0.04, 0.04, 2.1, k.dark, x, 0, -0.1, 8);
      room.box(kb, 2.1, 1.2, 0.06, k.white, 0, 0.9, 0);
      const cols = [-0.62, 0, 0.62];
      const heads = [k.red, k.yellow, k.green];
      cols.forEach((x, i) => room.box(kb, 0.55, 0.08, 0.03, heads[i]!, x, 1.9, 0.04));
      const notes: [number, number, THREE.Material][] = [
        [-0.62, 1.68, k.yellow], [-0.62, 1.44, k.purple], [-0.62, 1.2, k.blue],
        [0, 1.68, k.green], [0, 1.44, k.gold],
        [0.62, 1.68, k.red], [0.62, 1.44, k.blue], [0.62, 1.2, k.purple],
      ];
      for (const [x, y, m] of notes) room.box(kb, 0.42, 0.18, 0.02, m, x, y, 0.05);
      const mover = room.box(kb, 0.42, 0.18, 0.02, k.gold, -0.62, 0.96, 0.06);
      mover.userData.dynamic = true;
      const path = [-0.62, 0, 0.62, 0];
      room.anim.push((t) => {
        const p = (t * 0.2) % 4;
        const i = Math.floor(p);
        const f = p - i;
        const a = path[i]!;
        const b = path[(i + 1) % 4]!;
        mover.position.x = a + (b - a) * f * f * (3 - 2 * f);
      });

      // A wall calendar board on a foot and post, its days in a grid with a few marks.
      const cal = room.item(2.75, 0.2, -Math.PI / 2);
      room.box(cal, 0.5, 0.04, 0.3, k.dark, 0, 0, -0.05);
      room.cyl(cal, 0.03, 0.03, 1.0, k.dark, 0, 0, -0.05, 8);
      room.box(cal, 1.0, 1.2, 0.06, k.wood, 0, 1.0, 0);
      room.box(cal, 0.88, 1.08, 0.03, k.paper, 0, 1.06, 0.04);
      for (let r = 0; r < 4; r++) {
        for (let c = 0; c < 5; c++) room.box(cal, 0.15, 0.2, 0.01, k.white, -0.36 + c * 0.18, 1.12 + r * 0.25, 0.06);
      }
      const marks: [number, number, THREE.Material][] = [[0, 1, k.red], [2, 3, k.blue], [4, 0, k.green], [1, 2, k.yellow]];
      for (const [c, r, m] of marks) room.box(cal, 0.11, 0.06, 0.01, m, -0.36 + c * 0.18, 1.12 + r * 0.25 + 0.07, 0.08);

      // A round table near the back with three chairs, each turned to face it.
      const table = room.item(-1.9, -1.25);
      room.cyl(table, 0.42, 0.42, 0.05, k.wood, 0, 0.72, 0, 20);
      room.cyl(table, 0.07, 0.2, 0.7, k.darkWood, 0, 0, 0, 12);
      room.chair(-1.9, -1.95, 0);
      room.chair(-2.55, -1.25, Math.PI / 2);
      room.chair(-1.9, -0.35, Math.PI);

      // A timeline pole with three milestone flags that wave in the air.
      const pole = room.item(1.9, -1.6);
      room.cyl(pole, 0.3, 0.3, 0.08, k.dark, 0, 0, 0, 16);
      room.cyl(pole, 0.04, 0.04, 2.4, k.metal, 0, 0, 0, 8);
      room.ball(pole, 0.07, k.gold, 0, 2.45, 0);
      const milestones: [number, THREE.Material][] = [[0.9, k.red], [1.4, k.blue], [1.9, k.green]];
      for (const [y, m] of milestones) {
        const flag = new THREE.Group();
        flag.position.set(0, y, 0);
        pole.add(flag);
        flag.userData.dynamic = true;
        room.box(flag, 0.36, 0.2, 0.02, m, 0.2, -0.1, 0);
        room.anim.push((t) => (flag.rotation.y = Math.sin(t * 1.5 + y * 2) * 0.35));
      }

      room.desk(-2.2, 1.0, Math.PI / 2);
    },
  },

  auditor: {
    name: 'Ledger Office',
    size: 3,
    shape: 'square',
    floor: 'tiles',
    wall: 'brick',
    build(room) {
      const { k } = room;
      // A row of filing cabinets along the back wall, four drawers each, with handles.
      const drawer = k.mat(0xa9b4c4);
      for (const x of [-2.1, -1.5, -0.9, -0.3]) {
        const cab = room.item(x, -2.5);
        room.box(cab, 0.6, 1.25, 0.5, k.metal, 0, 0, 0);
        for (let i = 0; i < 4; i++) {
          const y = 0.06 + i * 0.29;
          room.box(cab, 0.54, 0.25, 0.03, drawer, 0, y, 0.265);
          room.box(cab, 0.18, 0.03, 0.04, k.dark, 0, y + 0.17, 0.29);
        }
      }

      // A desk on the right, its back to the wall, with stacks of paper and a ledger.
      const desk = room.desk(2.3, -0.9, -Math.PI / 2);
      room.box(desk, 0.24, 0.1, 0.2, k.paper, 0.25, 0.56, 0.1);
      room.box(desk, 0.2, 0.14, 0.18, k.white, -0.3, 0.56, 0.12);
      room.box(desk, 0.22, 0.03, 0.16, k.blue, 0.28, 0.66, 0.1);

      // A giant magnifying glass on a stand: a gold rim, a lens and a handle, tilting back and forth.
      const mag = room.item(-1.9, 0.6);
      room.cyl(mag, 0.35, 0.35, 0.08, k.dark, 0, 0, 0, 16);
      room.cyl(mag, 0.05, 0.05, 1.42, k.metal, 0, 0.08, 0, 8);
      const glass = new THREE.Group();
      glass.position.set(0, 1.5, 0);
      mag.add(glass);
      glass.userData.dynamic = true;
      const rim = new THREE.Mesh(new THREE.TorusGeometry(0.5, 0.06, 12, 32), k.gold);
      rim.castShadow = true;
      glass.add(rim);
      room.cyl(glass, 0.47, 0.47, 0.02, k.mat(0xbfe6f2), 0, -0.01, 0, 32).rotation.x = Math.PI / 2;
      const handle = room.box(glass, 0.1, 0.7, 0.1, k.darkWood, 0, 0, 0);
      handle.position.set(-0.6, -0.6, 0);
      handle.rotation.z = (3 * Math.PI) / 4;
      room.anim.push((t) => (glass.rotation.x = Math.sin(t * 1.1) * 0.22));

      // A table with an abacus: a frame, four rods and beads.
      const tb = room.item(1.9, 1.6);
      room.box(tb, 0.8, 0.05, 0.5, k.wood, 0, 0.7, 0);
      for (const x of [-0.34, 0.34]) for (const z of [-0.2, 0.2]) room.box(tb, 0.05, 0.7, 0.05, k.darkWood, x, 0, z);
      room.box(tb, 0.5, 0.04, 0.08, k.darkWood, 0, 0.75, 0);
      for (const x of [-0.25, 0.25]) room.box(tb, 0.04, 0.38, 0.08, k.darkWood, x, 0.79, 0);
      room.box(tb, 0.5, 0.04, 0.08, k.darkWood, 0, 1.13, 0);
      for (const y of [0.87, 0.95, 1.03, 1.11]) {
        const rod = room.cyl(tb, 0.008, 0.008, 0.5, k.metal, 0, 0, 0, 6);
        rod.position.set(0, y, 0);
        rod.rotation.z = Math.PI / 2;
        for (let j = 0; j < 5; j++) room.ball(tb, 0.028, j % 2 ? k.red : k.yellow, -0.2 + j * 0.1, y, 0.02);
      }

      room.plant(2.3, 2.3, 0.9);
    },
  },
};
