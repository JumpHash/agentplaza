// District rooms: each role's room has its own size, shape, floor, walls and furniture (the styles
// live in styles-*.ts); this module builds a room from its style and gives the scene what it needs.

import * as THREE from 'three';
import { canvasTexture, rng } from '../textures';
import { outline, toonMat } from '../toon';
import { floorTexture, makeKit, Room, wallLook, type KitPalette, type RoomStyle } from './kit';
import { STYLES_A } from './styles-a';
import { STYLES_B } from './styles-b';
import { STYLES_C } from './styles-c';
import { STYLES_D } from './styles-d';

export type { RoomStyle } from './kit';

/** A plain studio: desks and plants, for a role without a room of its own. */
const STUDIO: RoomStyle = {
  name: 'Studio',
  size: 3,
  shape: 'square',
  floor: 'tiles',
  wall: 'panel',
  build(room) {
    const s = room.s;
    room.desk(-s + 1.2, -s + 1.0);
    room.desk(s - 1.2, -s + 1.0);
    room.plant(-s + 0.5, s - 0.6, 0.9);
    room.plant(s - 0.5, s - 0.6, 0.9);
    room.lamp(s - 0.45, 0);
  },
};

const STYLES: Record<string, RoomStyle> = { ...STYLES_A, ...STYLES_B, ...STYLES_C, ...STYLES_D };

/** The room style of a role. */
export function roomStyle(role: string): RoomStyle {
  return STYLES[role] ?? STUDIO;
}

export interface BuiltRoom {
  /** Furniture, walls and sign, in the room's local frame (rotate it to face the plaza). */
  group: THREE.Group;
  /** The floor's top surface material. */
  floor: THREE.Material;
  /** Monitor materials that light up while the room's agents work. */
  screens: THREE.MeshToonMaterial[];
  animate: (t: number) => void;
}

/** Builds a role's room at half-size `s` (the style's size, or more for a crowded district). */
export function buildRoom(role: string, s: number, hue: number, palette: KitPalette): BuiltRoom {
  const style = roomStyle(role);
  const g = new THREE.Group();
  const kit = makeKit(hue, palette);
  const room = new Room(g, s, hue, kit, rng(hue * 131 + role.length));
  walls(room, style);
  sign(room, style.name, style.shape === 'round');
  style.build(room);
  outline(g);
  const anims = room.anim;
  const { glowOk, glowInfo } = kit;
  return {
    group: g,
    // Tinted toward the night's blue, so a room's floor reads as lit by the plaza's moonlight.
    floor: toonMat({ map: floorTexture(style.floor, hue, s), color: 0xb4bde0 }),
    screens: [kit.screen],
    animate: (t) => {
      glowOk.color.copy(palette.ok).multiplyScalar(0.4 + 0.6 * Math.max(0, Math.sin(t * 2.7)));
      glowInfo.color.copy(palette.info).multiplyScalar(0.4 + 0.6 * Math.max(0, Math.sin(t * 3.9 + 1)));
      for (const a of anims) a(t);
    },
  };
}

/** Walls on the back half: square rooms get a back wall and two half side walls; round ones an arc. */
function walls(room: Room, style: RoomStyle): void {
  const look = wallLook(style.wall, room.hue);
  if (!look) return;
  const { mat, h, thick } = look;
  const s = room.s;
  const seg = (x: number, z: number, len: number, rot: number) => {
    const it = room.item(x, z, rot);
    room.box(it, len, h, thick, mat);
    // A cap rail in the darker wood, the cartoon trim.
    if (style.wall !== 'hedge' && style.wall !== 'glass') room.box(it, len + 0.04, 0.06, thick + 0.06, room.k.darkWood, 0, h, 0);
  };
  if (style.shape === 'round') {
    const n = 9;
    const R = s - 0.15;
    for (let i = 0; i < n; i++) {
      const a = Math.PI + (i + 0.5) * (Math.PI / n); // the back half, z < 0
      const len = 2 * R * Math.sin(Math.PI / (2 * n)) + 0.05;
      seg(Math.cos(a) * R, Math.sin(a) * R, len, -a - Math.PI / 2);
    }
  } else {
    seg(0, -s + thick / 2 + 0.05, 2 * s - 0.1, 0);
    for (const side of [-1, 1]) {
      const len = s * 0.9;
      seg(side * (s - thick / 2 - 0.05), -s + len / 2 + 0.05, len, Math.PI / 2);
    }
  }
}

/** The room's name on a painted board at the middle of the back wall. */
function sign(room: Room, name: string, round: boolean): void {
  const tex = canvasTexture(`roomsign:${name}`, 512, 128, 1, (g, w, h) => {
    g.fillStyle = '#fff6e0';
    g.fillRect(0, 0, w, h);
    g.strokeStyle = '#2b2420';
    g.lineWidth = 10;
    g.strokeRect(5, 5, w - 10, h - 10);
    g.fillStyle = '#2b2420';
    g.font = '700 64px ui-rounded, "Comic Sans MS", system-ui, sans-serif';
    g.textAlign = 'center';
    g.textBaseline = 'middle';
    g.fillText(name, w / 2, h / 2 + 4);
  });
  const face = toonMat({ map: tex });
  const it = room.item(0, -room.s + (round ? 0.45 : 0.3));
  room.cyl(it, 0.04, 0.04, 1.5, room.k.darkWood, -0.8, 0, 0, 8);
  room.cyl(it, 0.04, 0.04, 1.5, room.k.darkWood, 0.8, 0, 0, 8);
  const board = new THREE.Mesh(new THREE.BoxGeometry(1.9, 0.48, 0.06), [room.k.wood, room.k.wood, room.k.wood, room.k.wood, face, room.k.wood]);
  board.position.set(0, 1.42, 0.02);
  board.castShadow = true;
  it.add(board);
}
