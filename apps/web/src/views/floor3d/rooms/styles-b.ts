// Rooms: the launch pad, the vault, the review room and the drafting studio.

import * as THREE from 'three';
import type { RoomStyle } from './kit';

/** A thin mesh (a torus, a spoke, a beam) placed by its centre, casting shadows. */
function mesh(parent: THREE.Object3D, geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number): THREE.Mesh {
  const m = new THREE.Mesh(geo, mat);
  m.position.set(x, y, z);
  m.castShadow = true;
  m.receiveShadow = true;
  parent.add(m);
  return m;
}

export const STYLES_B: Record<string, RoomStyle> = {
  release: {
    name: 'Launch Pad',
    size: 4,
    shape: 'square',
    floor: 'hazard',
    wall: 'fence',
    build(room) {
      const { k } = room;
      // A rocket on a round pad: white body, signal nose, three fins and a porthole. It hovers.
      const pad = room.item(-2.0, -2.2);
      room.cyl(pad, 0.85, 0.85, 0.12, k.dark, 0, 0, 0, 32);
      room.cyl(pad, 0.66, 0.66, 0.02, k.signal, 0, 0.12, 0, 32);
      const rocket = new THREE.Group();
      rocket.position.y = 0.2;
      pad.add(rocket);
      rocket.userData.dynamic = true;
      room.cyl(rocket, 0.24, 0.24, 1.5, k.white, 0, 0, 0, 20);
      room.cone(rocket, 0.24, 0.55, k.signal, 0, 1.5, 0, 20);
      for (let i = 0; i < 3; i++) {
        const a = (i / 3) * Math.PI * 2;
        room.box(rocket, 0.18, 0.5, 0.04, k.signal, Math.cos(a) * 0.22, 0.02, Math.sin(a) * 0.22).rotation.y = -a;
      }
      room.ball(rocket, 0.11, k.gold, 0, 0.95, 0.22);
      room.ball(rocket, 0.08, k.info, 0, 0.95, 0.27);
      room.anim.push((t) => (rocket.position.y = 0.2 + 0.04 * Math.sin(t * 1.3)));

      // A lattice gantry tower beside the pad, with an access arm to the rocket and a blinking beacon.
      const gantry = room.item(-3.3, -2.2);
      for (const [x, z] of [[-0.25, -0.25], [0.25, -0.25], [-0.25, 0.25], [0.25, 0.25]] as const) {
        room.box(gantry, 0.06, 3.2, 0.06, k.dark, x, 0, z);
      }
      for (const y of [0.4, 1.2, 2.0, 2.8]) {
        for (const z of [-0.25, 0.25]) room.box(gantry, 0.5, 0.05, 0.05, k.metal, 0, y, z);
        for (const x of [-0.25, 0.25]) room.box(gantry, 0.05, 0.05, 0.5, k.metal, x, y, 0);
      }
      const diag = Math.atan2(0.8, 0.5);
      for (let i = 0; i < 3; i++) {
        room.box(gantry, 0.94, 0.04, 0.04, k.metal, 0, 0.8 + i * 0.8, 0.25).rotation.z = i % 2 ? -diag : diag;
      }
      room.box(gantry, 0.8, 0.06, 0.12, k.signal, 0.65, 2.2, 0);
      const beacon = room.ball(gantry, 0.09, k.glowRed, 0, 3.3, 0);
      beacon.userData.dynamic = true;
      room.anim.push((t) => (beacon.visible = Math.sin(t * 5) > 0));

      // The control desk, traffic cones at the front-left, crates along both sides.
      room.desk(2.6, 1.6, -Math.PI / 2);
      for (const [x, z] of [[-2.5, 2.2], [-1.6, 2.7], [-3.0, 3.1]] as const) {
        const c = room.item(x, z);
        room.cyl(c, 0.17, 0.17, 0.05, k.dark, 0, 0, 0, 12);
        room.cone(c, 0.13, 0.42, k.signal, 0, 0.05, 0, 12);
        room.cyl(c, 0.075, 0.09, 0.07, k.white, 0, 0.2, 0, 12);
      }
      room.crate(-3.1, 0.6, 0.5);
      room.crate(-3.1, 0.6, 0.4).position.y = 0.5;
      room.crate(-3.1, 1.25, 0.45, 0.2);
      room.crate(3.1, -1.2, 0.55);
      room.crate(3.1, -1.2, 0.42).position.y = 0.55;
      room.crate(3.1, -0.3, 0.5, 0.3);
    },
  },

  'security-reviewer': {
    name: 'Vault',
    size: 3,
    shape: 'square',
    floor: 'slate',
    wall: 'stone',
    build(room) {
      const { k, s } = room;
      // A round vault door set into the back wall; its spoked wheel slowly turns.
      const vault = room.item(0, -s + 0.25);
      room.cyl(vault, 0.95, 0.95, 0.24, k.mat(0x7a8696), 0, 1.0, 0, 40).rotation.x = Math.PI / 2;
      mesh(vault, new THREE.TorusGeometry(0.95, 0.07, 10, 40), k.gold, 0, 1.12, 0.13);
      for (let i = 0; i < 8; i++) {
        const a = (i / 8) * Math.PI * 2;
        room.ball(vault, 0.045, k.dark, Math.cos(a) * 0.78, 1.12 + Math.sin(a) * 0.78, 0.14);
      }
      const wheel = new THREE.Group();
      wheel.position.set(0, 1.12, 0.17);
      vault.add(wheel);
      wheel.userData.dynamic = true;
      mesh(wheel, new THREE.TorusGeometry(0.42, 0.045, 8, 28), k.gold, 0, 0, 0);
      for (let i = 0; i < 3; i++) mesh(wheel, new THREE.BoxGeometry(0.06, 0.84, 0.05), k.gold, 0, 0, 0).rotation.z = (i * Math.PI) / 3;
      mesh(wheel, new THREE.SphereGeometry(0.09, 12, 10), k.dark, 0, 0, 0.03);
      room.anim.push((t) => (wheel.rotation.z = t * 0.5));

      // A CCTV camera on a pole in the back-right corner; its head pans slowly.
      const pole = room.item(2.4, -2.4);
      room.cyl(pole, 0.05, 0.05, 2.0, k.metal, 0, 0, 0, 8);
      const head = new THREE.Group();
      head.position.set(0, 2.0, 0);
      pole.add(head);
      head.userData.dynamic = true;
      room.box(head, 0.26, 0.2, 0.42, k.dark, 0, -0.1, 0.1);
      room.cyl(head, 0.07, 0.07, 0.06, k.info, 0, -0.03, 0.32, 12).rotation.x = Math.PI / 2;
      room.ball(head, 0.035, k.glowRed, 0.09, 0.12, 0.2);
      room.anim.push((t) => (head.rotation.y = 0.9 * Math.sin(t * 0.5)));

      // A row of lockboxes along the left wall.
      for (const [z, m] of [[-2.3, k.red], [-1.75, k.blue], [-1.2, k.gold]] as const) {
        const lb = room.item(-2.3, z);
        room.box(lb, 0.5, 0.36, 0.42, m, 0, 0, 0);
        room.box(lb, 0.05, 0.04, 0.2, k.dark, 0.27, 0.24, 0);
        room.ball(lb, 0.05, k.white, 0.27, 0.26, 0);
      }

      // A laser fence across the front: two posts and two red beams. The beams are light, not walls.
      for (const x of [-2.6, 2.6]) {
        const post = room.item(x, 1.8);
        room.cyl(post, 0.05, 0.06, 1.0, k.metal, 0, 0, 0, 8);
        room.ball(post, 0.07, k.glowRed, 0, 1.02, 0);
      }
      for (const y of [0.35, 0.8]) {
        mesh(room.g, new THREE.BoxGeometry(5.2, 0.03, 0.03), k.glowRed, 0, y, 1.8).userData.walk = true;
      }

      room.desk(2.2, 0.4, -Math.PI / 2);
    },
  },

  reviewer: {
    name: 'Review Room',
    size: 3,
    shape: 'square',
    floor: 'carpet',
    wall: 'panel',
    build(room) {
      const { k, s } = room;
      // A whiteboard on the back wall with a few marker strokes.
      const board = room.item(0, -s + 0.12);
      room.box(board, 2.0, 1.2, 0.06, k.white, 0, 0.9, 0);
      room.box(board, 2.08, 0.06, 0.08, k.metal, 0, 2.1, 0);
      room.box(board, 2.1, 0.05, 0.1, k.metal, 0, 0.85, 0.02);
      for (const x of [-1.02, 1.02]) room.box(board, 0.06, 1.25, 0.08, k.metal, x, 0.9, 0);
      for (const [x, y, w, a, m] of [
        [-0.6, 1.6, 0.5, 0.15, k.blue], [-0.5, 1.3, 0.7, -0.05, k.red], [0.35, 1.7, 0.4, 0.4, k.green],
        [0.1, 1.1, 0.9, 0, k.blue], [-0.25, 0.95, 0.4, 0.2, k.red],
      ] as const) {
        room.box(board, w, 0.035, 0.02, m, x, y, 0.04).rotation.z = a;
      }

      // A giant rubber stamp: a wooden handle and a red base. It stamps up and down on a paper pad.
      const stand = room.item(-1.9, -1.9);
      room.box(stand, 0.9, 0.8, 0.7, k.darkWood, 0, 0, 0);
      room.box(stand, 0.7, 0.06, 0.5, k.paper, 0, 0.8, 0);
      room.box(stand, 0.68, 0.06, 0.48, k.white, 0, 0.86, 0);
      const stamp = new THREE.Group();
      stamp.position.y = 0.92;
      stand.add(stamp);
      stamp.userData.dynamic = true;
      room.box(stamp, 0.6, 0.12, 0.42, k.red, 0, 0, 0);
      room.cyl(stamp, 0.07, 0.09, 0.5, k.wood, 0, 0.12, 0, 12);
      room.ball(stamp, 0.11, k.gold, 0, 0.66, 0);
      room.anim.push((t) => (stamp.position.y = 0.92 + 0.5 * Math.abs(Math.sin(t * 1.8))));

      // A lectern on the right, with a book on its slope.
      const lectern = room.item(1.9, -1.0);
      room.box(lectern, 0.5, 1.0, 0.4, k.darkWood, 0, 0, 0);
      room.box(lectern, 0.6, 0.06, 0.5, k.wood, 0, 1.0, 0.02).rotation.x = -0.35;
      room.box(lectern, 0.4, 0.03, 0.3, k.paper, 0, 1.1, 0.02);

      // Two chairs for the audience, a desk, and a plant.
      room.chair(1.7, 1.9, Math.PI);
      room.chair(2.3, 1.9, Math.PI);
      room.desk(-2.2, 1.0, Math.PI / 2);
      room.plant(-2.4, 2.3, 0.9);
    },
  },

  architect: {
    name: 'Drafting Studio',
    size: 3.5,
    shape: 'round',
    floor: 'blueprint',
    wall: 'glass',
    build(room) {
      const { k } = room;
      // A tilted drafting table, with a lamp arm rising from its left edge.
      const table = room.item(-1.9, -1.5);
      const top = new THREE.Group();
      top.position.y = 0.85;
      top.rotation.x = 0.35;
      table.add(top);
      room.box(top, 1.0, 0.05, 0.7, k.wood, 0, -0.05, 0);
      for (const x of [-0.42, 0.42]) for (const z of [-0.25, 0.25]) room.box(table, 0.06, 0.82, 0.06, k.darkWood, x, 0, z);
      room.cyl(table, 0.03, 0.03, 1.4, k.metal, -0.52, 0, -0.3, 8);
      const arm = new THREE.Group();
      arm.position.set(-0.52, 1.4, -0.3);
      table.add(arm);
      arm.userData.dynamic = true;
      room.box(arm, 0.04, 0.04, 0.45, k.metal, 0, -0.02, 0.22);
      room.cone(arm, 0.14, 0.16, k.signal, 0, -0.16, 0.45, 16).rotation.x = Math.PI;
      room.ball(arm, 0.05, k.glowWarm, 0, -0.1, 0.45);
      room.anim.push((t) => (arm.rotation.y = 0.15 * Math.sin(t * 0.7)));
      room.chair(-1.9, -0.5, Math.PI);

      // An easel with a painted canvas, at the front-left.
      const easel = room.item(-2.0, 1.5);
      for (const x of [-0.3, 0.3]) room.box(easel, 0.05, 1.6, 0.05, k.darkWood, x, 0, 0.2).rotation.x = -0.2;
      room.box(easel, 0.7, 0.9, 0.04, k.paper, 0, 0.7, -0.05);
      room.box(easel, 0.3, 0.22, 0.02, k.purple, -0.1, 0.95, -0.03);
      room.ball(easel, 0.07, k.yellow, 0.15, 1.2, -0.03);

      // A scale model city on a green board: stacked boxes, a tower with a cone roof, and a tiny crane that turns.
      const city = room.item(1.8, -1.7);
      room.box(city, 0.9, 0.08, 0.7, k.leaf, 0, 0, 0);
      room.box(city, 0.2, 0.16, 0.18, k.blue, -0.25, 0.08, -0.15);
      room.box(city, 0.16, 0.14, 0.16, k.white, -0.25, 0.24, -0.15);
      room.box(city, 0.18, 0.12, 0.2, k.yellow, -0.2, 0.08, 0.15);
      room.cone(city, 0.13, 0.12, k.red, -0.2, 0.2, 0.15, 4);
      room.cyl(city, 0.07, 0.07, 0.45, k.white, 0.1, 0.08, -0.1, 12);
      room.cone(city, 0.1, 0.2, k.red, 0.1, 0.53, -0.1, 12);
      const crane = new THREE.Group();
      crane.position.set(0.22, 0.08, 0.14);
      city.add(crane);
      crane.userData.dynamic = true;
      room.cyl(crane, 0.04, 0.04, 0.3, k.gold, 0, 0, 0, 8);
      room.box(crane, 0.36, 0.03, 0.03, k.gold, 0, 0.3, 0);
      room.box(crane, 0.02, 0.1, 0.02, k.dark, 0.15, 0.2, 0);
      room.anim.push((t) => (crane.rotation.y = t * 0.7));

      // A bin of rolled plans, beside the desk.
      const bin = room.item(2.5, -0.7);
      room.cyl(bin, 0.22, 0.18, 0.5, k.mat(0x6a7f96), 0, 0, 0, 16);
      room.cyl(bin, 0.04, 0.04, 0.42, k.paper, -0.08, 0.3, 0.02, 8).rotation.z = 0.1;
      room.cyl(bin, 0.04, 0.04, 0.42, k.yellow, 0.05, 0.3, -0.05, 8).rotation.z = -0.12;
      room.cyl(bin, 0.04, 0.04, 0.38, k.paper, 0.02, 0.3, 0.08, 8).rotation.z = 0.05;

      room.desk(2.2, 0.6, -Math.PI / 2);
    },
  },
};
