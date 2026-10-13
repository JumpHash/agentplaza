// The grounding audit of a board scene. Every resting object (userData.rest: a solid part that is not attached, a plate
// body, a column, a room) must stand on what is directly under it. Its bottom is tested at the centre and four inset
// corners of its bottom face: a ray cast down from inside the object finds the highest solid below, and the bottom
// must not be more than GROUND_TOL above it (floating) nor below the ground (sinking). Its box must not overlap a solid
// it does not belong to by more than GROUND_TOL on every axis, when that solid stands at or under the object's bottom
// (sinking: the object's bottom is pushed into what it stands on).
//
// Exempt: attached parts (schema "attached": true), decals (flat plane and ring parts, traces), additive glows; none
// of these holds anything up, and only the resting objects are checked. The debug hook window.__apzGroundAudit reports
// the live board (floor.ts); the tests read the same report (tests.ts).

import * as THREE from 'three';
import { FIELD_CELL, GROUND_TOL, keyOf, type SupportField } from './drawn-tops';

export { FIELD_CELL, GROUND_TOL, topBelow, type Support, type SupportField } from './drawn-tops';

/** The sample points sit this share of the box's extent in from its bottom face's edges. */
const INSET = 0.1;
/** A ray starts this far above the bottom face (inside the object: the object's own meshes are not hit). */
const SEEK = 0.1;

export interface GroundFinding {
  /** The part id (or the object's position), with its model's name as the report gives it. */
  item: string;
  kind: 'floating' | 'sinking';
  /** Floating: the gap under the bottom at the worst sample. Sinking: the overlap depth (or the depth below ground). */
  amount: number;
  /** The worst sample (x, y, z): the bottom point of the floating sample, or the overlap's lowest point. */
  at: [number, number, number];
  /** What it floats above or sinks into (the solid's name), or "ground". */
  against: string;
}

export interface GroundReport {
  /** The resting objects checked. */
  items: number;
  findings: GroundFinding[];
}

/** A solid that holds things up: a mesh that is neither a decal nor an additive glow. */
export function isSolid(o: THREE.Object3D): o is THREE.Mesh {
  if (!(o as THREE.Mesh).isMesh || o.userData.decal === true) return false;
  return ((o as THREE.Mesh).material as THREE.Material).blending !== THREE.AdditiveBlending;
}

/** A solid of the scene: a mesh, or one copy of an instanced mesh, with its own world box. */
interface Solid {
  mesh: THREE.Mesh;
  inst: number; // -1 for a plain mesh
  box: THREE.Box3;
  name: string;
}

const _v = new THREE.Vector3();
const _m = new THREE.Matrix4();

/** The world box of one mesh's geometry (vertex by vertex, so a rotated part is not boxed loosely), or of one instance. */
function boxOf(mesh: THREE.Mesh, inst: number, out: THREE.Box3): THREE.Box3 {
  out.makeEmpty();
  const pos = mesh.geometry.getAttribute('position');
  if (!pos) return out;
  if (inst >= 0) _m.multiplyMatrices(mesh.matrixWorld, instanceMatrix(mesh as THREE.InstancedMesh, inst));
  else _m.copy(mesh.matrixWorld);
  for (let i = 0; i < pos.count; i++) out.expandByPoint(_v.fromBufferAttribute(pos, i).applyMatrix4(_m));
  return out;
}

function instanceMatrix(mesh: THREE.InstancedMesh, i: number): THREE.Matrix4 {
  const m = new THREE.Matrix4();
  mesh.getMatrixAt(i, m);
  return m;
}

function nameOf(o: THREE.Object3D): string {
  if (o.name) return o.name;
  const w = new THREE.Vector3().setFromMatrixPosition(o.matrixWorld);
  return `${o.type}@${w.x.toFixed(2)},${w.y.toFixed(2)},${w.z.toFixed(2)}`;
}

/** Is a an ancestor or a descendant of b (or the same object)? */
function related(a: THREE.Object3D, b: THREE.Object3D): boolean {
  for (let o: THREE.Object3D | null = a; o; o = o.parent) if (o === b) return true;
  for (let o: THREE.Object3D | null = b; o; o = o.parent) if (o === a) return true;
  return false;
}

/** The solids of a scene (instanced meshes split into their copies), each with its box. */
function solidsOf(root: THREE.Object3D): Solid[] {
  const out: Solid[] = [];
  root.traverse((o) => {
    if (!isSolid(o)) return;
    const im = o as THREE.InstancedMesh;
    if (im.isInstancedMesh) {
      for (let i = 0; i < im.count; i++) out.push({ mesh: im, inst: i, box: boxOf(im, i, new THREE.Box3()), name: nameOf(im) });
    } else out.push({ mesh: o, inst: -1, box: boxOf(o, -1, new THREE.Box3()), name: nameOf(o) });
  });
  return out;
}

/** The resting objects: each with its world box (a group's box is the union of its solids) and its instance (-1 for a plain one). */
interface Item {
  obj: THREE.Object3D;
  inst: number;
  box: THREE.Box3;
  name: string;
}

/**
 * TEMPORARY: the GPU parts are being replaced by new reference-accurate graphics-card models, so nothing under a part whose
 * name matches this pattern is counted as an item. They still hold things up (the plates rest on them). REMOVE this filter
 * (excludedItem and its use in itemsOf) when the new GPU parts land.
 */
export const TEMP_EXCLUDED_ITEM = /gpu/i;

export function excludedItem(o: THREE.Object3D): boolean {
  for (let p: THREE.Object3D | null = o; p; p = p.parent) if (TEMP_EXCLUDED_ITEM.test(p.name)) return true;
  return false;
}

function itemsOf(root: THREE.Object3D, solids: Solid[]): Item[] {
  const out: Item[] = [];
  root.traverse((o) => {
    if (o.userData.rest !== true) return;
    if (excludedItem(o)) return; // TEMPORARY GPU filter (see TEMP_EXCLUDED_ITEM)
    if ((o as THREE.InstancedMesh).isInstancedMesh) {
      const im = o as THREE.InstancedMesh;
      for (let i = 0; i < im.count; i++) out.push({ obj: im, inst: i, box: boxOf(im, i, new THREE.Box3()), name: `${nameOf(im)}#${i}` });
      return;
    }
    const box = new THREE.Box3();
    if ((o as THREE.Mesh).isMesh) boxOf(o as THREE.Mesh, -1, box);
    else {
      // A group: the union of its own solids (the children that are solids; a child that is itself a rest is its own item too).
      for (const s of solids) if (isUnder(s.mesh, o)) box.union(s.box);
    }
    if (box.isEmpty()) return;
    out.push({ obj: o, inst: -1, box, name: nameOf(o) });
  });
  return out;
}

function isUnder(o: THREE.Object3D, anc: THREE.Object3D): boolean {
  for (let p: THREE.Object3D | null = o; p; p = p.parent) if (p === anc) return true;
  return false;
}

/** Is this solid the item itself or one of its descendants (never a support for it)? An ancestor is a support. */
function ownOf(s: Solid, it: Item): boolean {
  if (it.inst >= 0 && s.mesh === it.obj) return s.inst === it.inst;
  return isUnder(s.mesh, it.obj);
}

/** Is this solid part of the item (the item itself, its descendants, or its ancestors), so it is not an overlap? */
function self(s: Solid, it: Item): boolean {
  if (it.inst >= 0 && s.mesh === it.obj) return s.inst === it.inst;
  return related(s.mesh, it.obj);
}

/** The audit of a scene: its resting objects, floating and sinking. The scene's world matrices are updated here. */
export function groundAudit(root: THREE.Object3D): GroundReport {
  root.updateMatrixWorld(true);
  const solids = solidsOf(root);
  const items = itemsOf(root, solids);
  const ray = new THREE.Raycaster();
  const down = new THREE.Vector3(0, -1, 0);
  const findings: GroundFinding[] = [];
  for (const it of items) {
    const b = it.box;
    const ex = b.max.x - b.min.x;
    const ez = b.max.z - b.min.z;
    const samples: [number, number][] = [
      [(b.min.x + b.max.x) / 2, (b.min.z + b.max.z) / 2],
      [b.min.x + INSET * ex, b.min.z + INSET * ez],
      [b.max.x - INSET * ex, b.min.z + INSET * ez],
      [b.min.x + INSET * ex, b.max.z - INSET * ez],
      [b.max.x - INSET * ex, b.max.z - INSET * ez],
    ];
    // Floating and sinking below the ground, sample by sample: the worst is reported.
    let worstGap = 0;
    let worstAt: [number, number, number] = [0, b.min.y, 0];
    let worstAgainst = 'ground';
    for (const [x, z] of samples) {
      const cands = solids.filter((s) => !ownOf(s, it) && s.box.min.x <= x && x <= s.box.max.x && s.box.min.z <= z && z <= s.box.max.z);
      const meshes = [...new Set(cands.map((s) => s.mesh))];
      ray.set(new THREE.Vector3(x, b.min.y + SEEK, z), down);
      ray.far = Infinity;
      let top = -Infinity; // the highest surface under the bottom point; none is the ground (y = 0)
      let against = 'ground';
      for (const hit of ray.intersectObjects(meshes, false)) {
        if (it.inst >= 0 && hit.object === it.obj && hit.instanceId === it.inst) continue;
        if (it.inst < 0 && isUnder(hit.object, it.obj)) continue; // its own parts; the parent's top IS a support
        if (hit.point.y > b.min.y + GROUND_TOL) continue; // inside or above the item's bottom: an overlap, checked below
        if (hit.point.y > top) {
          top = hit.point.y;
          against = nameOf(hit.object);
        }
      }
      if (top === -Infinity) top = 0;
      const gap = b.min.y - top;
      if (gap > worstGap) {
        worstGap = gap;
        worstAt = [x, b.min.y, z];
        worstAgainst = against;
      }
    }
    if (worstGap > GROUND_TOL) findings.push({ item: it.name, kind: 'floating', amount: worstGap, at: worstAt, against: worstAgainst });
    // Sinking below the ground.
    if (b.min.y < -GROUND_TOL) findings.push({ item: it.name, kind: 'sinking', amount: -b.min.y, at: [b.min.x, b.min.y, b.min.z], against: 'ground' });
    // Overlap with a solid that is not part of the item: every axis deeper than the tolerance.
    let worstOverlap = 0;
    let overlapAt: [number, number, number] = [0, 0, 0];
    let overlapWith = '';
    for (const s of solids) {
      if (self(s, it)) continue;
      const o = s.box;
      if (o.min.y > b.min.y + GROUND_TOL) continue; // only what is below: its bottom lies at or under this item's bottom
      const dx = Math.min(b.max.x, o.max.x) - Math.max(b.min.x, o.min.x);
      const dy = Math.min(b.max.y, o.max.y) - Math.max(b.min.y, o.min.y);
      const dz = Math.min(b.max.z, o.max.z) - Math.max(b.min.z, o.min.z);
      if (dx <= GROUND_TOL || dy <= GROUND_TOL || dz <= GROUND_TOL) continue;
      const depth = Math.min(dx, dy, dz);
      if (depth > worstOverlap) {
        worstOverlap = depth;
        overlapAt = [(Math.max(b.min.x, o.min.x) + Math.min(b.max.x, o.max.x)) / 2, Math.max(b.min.y, o.min.y), (Math.max(b.min.z, o.min.z) + Math.min(b.max.z, o.max.z)) / 2];
        overlapWith = s.name;
      }
    }
    if (worstOverlap > GROUND_TOL) findings.push({ item: it.name, kind: 'sinking', amount: worstOverlap, at: overlapAt, against: overlapWith });
  }
  return { items: items.length, findings };
}

/** The plate's thickness at least: its top is the slot's top, its underside rests on the highest support under it. */
export const PLATE = 0.2;
/**
 * The upward faces of a root's solids (not decals, not glows), in the root's frame. Built before the bake (the baked meshes
 * are merged and not ray-pickable).
 */
export function supportField(root: THREE.Object3D, cell = FIELD_CELL): SupportField {
  root.updateMatrixWorld(true);
  const f: SupportField = { cell, tris: [], cells: new Map() };
  const add = (p: THREE.Vector3, q: THREE.Vector3, r: THREE.Vector3): void => {
    const t = f.tris.length / 9;
    f.tris.push(p.x, p.y, p.z, q.x, q.y, q.z, r.x, r.y, r.z);
    const i0 = Math.floor(Math.min(p.x, q.x, r.x) / cell);
    const i1 = Math.floor(Math.max(p.x, q.x, r.x) / cell);
    const j0 = Math.floor(Math.min(p.z, q.z, r.z) / cell);
    const j1 = Math.floor(Math.max(p.z, q.z, r.z) / cell);
    for (let i = i0; i <= i1; i++) {
      for (let j = j0; j <= j1; j++) {
        const k = keyOf(i, j);
        const bucket = f.cells.get(k);
        if (bucket) bucket.push(t);
        else f.cells.set(k, [t]);
      }
    }
  };
  const a = new THREE.Vector3();
  const b = new THREE.Vector3();
  const c = new THREE.Vector3();
  const e1 = new THREE.Vector3();
  const e2 = new THREE.Vector3();
  const n = new THREE.Vector3();
  root.traverse((o) => {
    if (!isSolid(o)) return;
    const mesh = o as THREE.Mesh;
    const pos = mesh.geometry.getAttribute('position');
    if (!pos) return;
    const index = mesh.geometry.index;
    const count = index ? index.count : pos.count;
    const mats: THREE.Matrix4[] = [];
    if ((mesh as THREE.InstancedMesh).isInstancedMesh) {
      const im = mesh as THREE.InstancedMesh;
      for (let k = 0; k < im.count; k++) {
        const m = new THREE.Matrix4();
        im.getMatrixAt(k, m);
        mats.push(new THREE.Matrix4().multiplyMatrices(im.matrixWorld, m));
      }
    } else mats.push(mesh.matrixWorld);
    for (const W of mats) {
      for (let t = 0; t + 2 < count; t += 3) {
        const ia = index ? index.getX(t) : t;
        const ib = index ? index.getX(t + 1) : t + 1;
        const ic = index ? index.getX(t + 2) : t + 2;
        a.fromBufferAttribute(pos, ia).applyMatrix4(W);
        b.fromBufferAttribute(pos, ib).applyMatrix4(W);
        c.fromBufferAttribute(pos, ic).applyMatrix4(W);
        e1.subVectors(b, a);
        e2.subVectors(c, a);
        n.crossVectors(e1, e2);
        const len = n.length();
        if (len < 1e-12 || n.y <= 0.5 * len) continue; // only upward faces hold things up
        add(a, b, c);
      }
    }
  });
  return f;
}

