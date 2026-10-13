// Builds a generated room (rooms/gen.ts) as a THREE.Group in the room's local frame: the floor at y = 0, walls
// outside the inner size, the name board on the back (-z) wall, and every item from its catalogue model.
// Models: kit:<name> is the room kit's builder, prop:<id> is the station prop (assets/robots/props), built with
// the robot palette; a prop with no file (or an unknown model) stands in as a placeholder box of its footprint.

import * as THREE from 'three';
import { buildProp, propRegistry } from '../robots/props';
import { ROBOT_PALETTE } from '../robot';
import { canvasTexture, rng } from '../textures';
import { outline, toonMat } from '../toon';
import { floorTexture, makeKit, Room, wallLook, type FloorKind, type KitPalette, type WallKind } from './kit';
import { layoutHash, type Catalog, type CatItem, type Door, type Layout, type LayoutItem, type LayoutSpot } from './gen';

export type RoomSpot = LayoutSpot;

export interface BuiltLayout {
  /** Furniture, walls and the name board, in the room's local frame (rotate it to face the plaza). */
  group: THREE.Group;
  /** The floor's top surface material. */
  floor: THREE.Material;
  /** Monitor materials that light up while the room's agents work. */
  screens: THREE.MeshToonMaterial[];
  animate: (t: number) => void;
  /** The layout's spots (where a robot stands to use an item), already in the room's frame. */
  spots: RoomSpot[];
}

// Shadow gates (perf): an item casts only from a bounding radius of CAST_RADIUS; a dynamic item (a lit screen)
// needs DYNAMIC_CAST_RADIUS. Trim, decals, thin rods and tori never cast (see thinPart).
const CAST_RADIUS = 0.3;
const DYNAMIC_CAST_RADIUS = 0.6;
/** The plant builder's height at scale 1 (pot base to leaf tip); a catalogue height scales it. */
const PLANT_HEIGHT = 1.07;
/** The name board's text must fit the canvas, with this margin each side. */
const SIGN_MARGIN = 40;

const offsetMats = new WeakMap<THREE.Material, THREE.Material>();
const _size = new THREE.Vector3();
const _partSize = new THREE.Vector3();

/** A rug's material with a polygon offset, so the rug never z-fights the floor. Cached per source material. */
function rugMat(m: THREE.Material): THREE.Material {
  let c = offsetMats.get(m);
  if (!c) {
    c = m.clone();
    c.polygonOffset = true;
    c.polygonOffsetFactor = -1;
    c.polygonOffsetUnits = -1;
    offsetMats.set(m, c);
  }
  return c;
}

/**
 * The room's meshes for a layout. `w` and `d` are the inner size (x by z); `s` is the half-size the floor texture
 * is drawn for; `name` is the room's name on its board. Every number comes from `layout`, so the same layout
 * always builds the same room.
 */
export function buildLayout(
  layout: Layout,
  catalog: Catalog,
  hue: number,
  palette: KitPalette,
  w: number,
  d: number,
  s: number,
  name: string,
): BuiltLayout {
  const g = new THREE.Group();
  const kit = makeKit(hue, palette);
  const room = new Room(g, s, hue, kit, rng(parseInt(layoutHash(layout).slice(0, 8), 16)));
  buildWalls(room, layout.shell.walls, layout.shell.wall as WallKind, w, d, layout.shell.door);
  for (const item of layout.items) placeItem(room, item, catalog);
  nameBoard(room, name, d);
  outline(g);
  const anims = room.anim;
  const { glowOk, glowInfo } = kit;
  return {
    group: g,
    // Tinted toward the night's blue, so a room's floor reads as lit by the plaza's moonlight.
    floor: toonMat({ map: floorTexture(layout.shell.floor as FloorKind, hue, s), color: 0xb4bde0 }),
    screens: [kit.screen],
    animate: (t) => {
      glowOk.color.copy(palette.ok).multiplyScalar(0.4 + 0.6 * Math.max(0, Math.sin(t * 2.7)));
      glowInfo.color.copy(palette.info).multiplyScalar(0.4 + 0.6 * Math.max(0, Math.sin(t * 3.9 + 1)));
      for (const a of anims) a(t);
    },
    spots: layout.spots,
  };
}

/**
 * The walls on the sides the layout names. A wall sits outside the inner size: its inner face is at the edge.
 * The door side has two stubs with the opening (door.width, centred at door.at) between them, and a thin lintel
 * over the opening (not on hedges and glass).
 */
function buildWalls(room: Room, sides: Door[], kind: WallKind, w: number, d: number, door?: { side: Door; at: number; width: number }): void {
  const look = wallLook(kind, room.hue);
  if (!look || sides.length === 0) return;
  const { mat, h, thick: t } = look;
  const seg = (x: number, z: number, len: number, rot: number): void => {
    const it = room.item(x, z, rot);
    room.box(it, len, h, t, mat);
    // A cap rail in the darker wood, the cartoon trim.
    if (kind !== 'hedge' && kind !== 'glass') room.box(it, len + 0.04, 0.06, t + 0.06, room.k.darkWood, 0, h, 0);
  };
  for (const side of sides) {
    // The north and south walls run the full outer width (they cover the corners); east and west the inner depth.
    const nsSide = side === 'n' || side === 's';
    const wallX = side === 'e' ? w / 2 + t / 2 : side === 'w' ? -w / 2 - t / 2 : 0;
    const wallZ = side === 'n' ? -d / 2 - t / 2 : side === 's' ? d / 2 + t / 2 : 0;
    const rot = nsSide ? 0 : Math.PI / 2;
    if (!door || side !== door.side) {
      if (nsSide) seg(0, wallZ, w + 2 * t, rot);
      else seg(wallX, 0, d, rot);
      continue;
    }
    // The door side: two stubs from each outer end to the opening's edge, then a lintel over the opening.
    const outer = nsSide ? w / 2 + t : d / 2;
    const hw = door.width / 2;
    const stub = (a: number, b: number): void => {
      const len = b - a;
      if (!(len > 0)) return;
      const c = (a + b) / 2;
      if (nsSide) seg(c, wallZ, len, rot);
      else seg(wallX, c, len, rot);
    };
    stub(-outer, -hw);
    stub(hw, outer);
    if (kind !== 'hedge' && kind !== 'glass' && door.width > 0) {
      const it = nsSide ? room.item(0, wallZ, rot) : room.item(wallX, 0, rot);
      room.box(it, door.width, 0.2, t, mat, 0, h - 0.2, 0);
    }
  }
}

/** The room's name on a painted board at the middle of the back (-z) wall, always kept. */
function nameBoard(room: Room, name: string, d: number): void {
  const tex = canvasTexture(`roomsign:${name}`, 512, 128, 1, (g, w, h) => {
    g.fillStyle = '#fff6e0';
    g.fillRect(0, 0, w, h);
    g.strokeStyle = '#2b2420';
    g.lineWidth = 10;
    g.strokeRect(5, 5, w - 10, h - 10);
    g.fillStyle = '#2b2420';
    g.textAlign = 'center';
    g.textBaseline = 'middle';
    const font = (px: number): string => `700 ${px}px ui-rounded, "Comic Sans MS", system-ui, sans-serif`;
    g.font = font(64);
    const measured = g.measureText(name).width;
    if (measured > w - SIGN_MARGIN) g.font = font(Math.floor((64 * (w - SIGN_MARGIN)) / measured));
    g.fillText(name, w / 2, h / 2 + 4);
  });
  const it = room.item(0, -d / 2 + 0.3);
  // The posts are thin rods: they never cast.
  room.cyl(it, 0.04, 0.04, 1.5, room.k.darkWood, -0.8, 0, 0, 8).castShadow = false;
  room.cyl(it, 0.04, 0.04, 1.5, room.k.darkWood, 0.8, 0, 0, 8).castShadow = false;
  // One material for the whole board: a material array draws each face group separately, and in the shadow pass
  // that is six draws per sign (the board is 0.06 thick, so the sign's text on its edges reads as paint).
  const board = new THREE.Mesh(new THREE.BoxGeometry(1.9, 0.48, 0.06), toonMat({ map: tex }));
  board.position.set(0, 1.42, 0.02);
  board.castShadow = true; // the wall sign keeps casting
  it.add(board);
}

/** One layout item, placed at its (x, y, z) and turned by its rot (degrees about y). */
function placeItem(room: Room, item: LayoutItem, catalog: Catalog): void {
  const cat = catalog.items[item.id];
  const size = cat?.size ?? [0.5, 0.5, 0.5];
  const rot = (item.rot * Math.PI) / 180;
  if (cat?.layer === 'floor') {
    // Floor layer (rugs): the kit's rug, lifted a hair above the floor top (kit.rug) and polygon-offset.
    const rug = room.rug(size[0], size[2], rugMat(room.k.accentSoft), item.x, item.z);
    rug.userData.id = item.id;
    rug.castShadow = false;
    return;
  }
  const model = cat?.model ?? '';
  const obj = model.startsWith('prop:')
    ? propObject(room, model.slice(5), item, rot, size)
    : kitObject(room, model.startsWith('kit:') ? model.slice(4) : model, item, cat, rot);
  obj.position.y = item.y;
  obj.userData.id = item.id;
  // A surface item (a monitor) lights its screen while the room works: it is dynamic, so it is never baked.
  if (cat?.place === 'surface') obj.userData.dynamic = true;
  castPolicy(obj, cat?.place === 'wallmount');
}

/** A kit model by its builder's name. Unknown names stand in as a placeholder box of the catalogue size. */
function kitObject(room: Room, name: string, item: LayoutItem, cat: CatItem | undefined, rot: number): THREE.Object3D {
  const { x, z } = item;
  const [w, h, depth] = cat?.size ?? [0.5, 0.5, 0.5];
  switch (name) {
    case 'deskTop': return room.deskTop(x, z, rot);
    case 'chair': return room.chair(x, z, rot);
    case 'monitor': return room.monitor(x, z, rot, item.y);
    case 'shelf': return room.shelf(x, z, rot, w, h);
    case 'plant': return room.plant(x, z, h / PLANT_HEIGHT);
    case 'lamp': return room.lamp(x, z);
    case 'poster': return room.poster(x, z, rot);
    case 'clock': return room.clock(x, z, rot);
    case 'sofa': return room.sofa(x, z, rot);
    case 'bin': return room.bin(x, z);
    case 'coatStand': return room.coatStand(x, z);
    case 'smallCrate': return room.smallCrate(x, z, rot);
    case 'safe': return room.safe(x, z, rot);
    case 'armchair': return room.armchair(x, z, rot);
    case 'coffeeTable': return room.coffeeTable(x, z, rot);
    case 'stool': return room.stool(x, z, rot);
    case 'smallShelf': return room.smallShelf(x, z, rot);
    case 'sideTable': return room.sideTable(x, z, rot);
    default: return room.propPlaceholder(x, z, rot, w, h, depth, name);
  }
}

/** A station prop from its file, built with the robot palette at the floor's centre; a placeholder without one. */
function propObject(room: Room, id: string, item: LayoutItem, rot: number, size: [number, number, number]): THREE.Object3D {
  const prop = propRegistry().props[id];
  if (!prop) return room.propPlaceholder(item.x, item.z, rot, size[0], size[1], size[2], `prop:${id}`);
  const built = buildProp(prop, ROBOT_PALETTE, room.hue);
  built.group.position.set(item.x, item.y, item.z);
  built.group.rotation.y = rot;
  room.g.add(built.group);
  return built.group;
}

/**
 * Cast-shadow policy for one item (its meshes): the item's bounding radius must reach CAST_RADIUS (a dynamic item
 * DYNAMIC_CAST_RADIUS), wall-mounted pieces never cast, and each mesh casts unless it is thin (thinPart).
 */
function castPolicy(obj: THREE.Object3D, wallmount: boolean): void {
  const box = new THREE.Box3().setFromObject(obj);
  const radius = box.isEmpty() ? 0 : box.getSize(_size).length() / 2;
  let dynamic = false;
  obj.traverse((o) => {
    if (o.userData.dynamic === true) dynamic = true;
  });
  const cast = !wallmount && radius >= CAST_RADIUS && (!dynamic || radius >= DYNAMIC_CAST_RADIUS);
  obj.traverse((o) => {
    const m = o as THREE.Mesh;
    if (m.isMesh) m.castShadow = cast && !thinPart(m);
  });
}

/** Trim, decals, thin rods and tori: they never cast a shadow. */
function thinPart(m: THREE.Mesh): boolean {
  const geo = m.geometry;
  if (geo instanceof THREE.TorusGeometry) return true;
  if (geo instanceof THREE.CylinderGeometry) return Math.max(geo.parameters.radiusTop, geo.parameters.radiusBottom) < 0.05;
  if (!geo.boundingBox) geo.computeBoundingBox();
  const s = geo.boundingBox ? geo.boundingBox.getSize(_partSize) : _partSize.set(1, 1, 1);
  return Math.min(s.x, s.y, s.z) < 0.02;
}
