// Debug overlay for the robot navigation grid, shown on the floor when the URL has ?nav=1. It draws the cells by
// class (blocked red, inflated amber, trace or near cyan, other walkable green; plain PCB cells are left out), the
// ramp and lift portals, the lift rails, and for each robot its path, an amber ring while it waits and a violet
// ring while it yields. Read-only: it reads NavGrid and NavWorld and changes neither. The contracts are in
// nav-types.ts; the world's rules are in nav-agents.ts.

import * as THREE from 'three';
import { NAV_COST_OPEN, NAV_COST_TRACE, type NavGrid, type NavPoint } from './nav-types';
import type { NavWorld } from './nav-agents';

export interface NavOverlay {
  group: THREE.Group;
  /** Rebuilds the cell and portal layers for a grid (after the board or rooms change). */
  setGrid(grid: NavGrid): void;
  /** Per frame: robot paths, yield and wait markers from the world. ids: the robot ids to show. */
  update(world: NavWorld, ids: number[]): void;
  dispose(): void;
}

const COLOUR = {
  green: 0x3ee08f,
  cyan: 0x4cc9ff,
  red: 0xff4d6d,
  amber: 0xffb020,
  ramp: 0xff8c1a,
  lift: 0xff3cd6,
  path: 0xcfe3ff,
  violet: 0xb388ff,
} as const;

/** Cell quads sit this far above their surface; portal lines and lift markers this far; path lines this far. */
const CELL_LIFT = 0.03;
const PORTAL_LIFT = 0.08;
const PATH_LIFT = 0.1;
/** Rings float this far above the robot's surface. */
const RING_LIFT = 1.4;
const CELL_OPACITY = 0.35;
const MARKER_RADIUS = 0.05;
const RING_INNER = 0.2;
const RING_OUTER = 0.26;

interface PathEntry {
  line: THREE.Line;
  attr: THREE.BufferAttribute | null;
  arr: Float32Array;
}

/** Overlay objects are not dynamic and never outlined (the floor's userData convention). */
function tag(o: THREE.Object3D): void {
  o.userData.dynamic = false;
  o.userData.outline = false;
}

function attach(parent: THREE.Object3D, child: THREE.Object3D): void {
  tag(child);
  parent.add(child);
}

/** Centre of a cell by index (row-major: index = j * cols + i). */
function centreOf(grid: NavGrid, idx: number): { x: number; z: number } {
  const i = idx % grid.cols;
  const j = Math.floor(idx / grid.cols);
  return { x: grid.x0 + (i + 0.5) * grid.cell, z: grid.z0 + (j + 0.5) * grid.cell };
}

/** The colour of a cell, or -1 when it is not drawn (a plain PCB cell, or off the board). */
function cellColour(grid: NavGrid, idx: number): number {
  if ((grid.blocked[idx] ?? 0) === 1) return COLOUR.red;
  if ((grid.inflated[idx] ?? 0) === 1) return COLOUR.amber;
  if ((grid.walk[idx] ?? 0) === 0) return -1;
  const cost = grid.cost[idx] ?? NAV_COST_OPEN;
  if ((grid.region[idx] ?? 0) === 0 && cost === NAV_COST_OPEN) return -1;
  if (cost === NAV_COST_TRACE || cost > NAV_COST_OPEN) return COLOUR.cyan;
  return COLOUR.green;
}

function buildCells(grid: NavGrid): THREE.InstancedMesh {
  const picks: number[] = [];
  const colours: number[] = [];
  const total = grid.cols * grid.rows;
  for (let idx = 0; idx < total; idx++) {
    const c = cellColour(grid, idx);
    if (c >= 0) {
      picks.push(idx);
      colours.push(c);
    }
  }
  const geo = new THREE.PlaneGeometry(grid.cell, grid.cell);
  geo.rotateX(-Math.PI / 2);
  const mat = new THREE.MeshBasicMaterial({
    color: 0xffffff,
    transparent: true,
    opacity: CELL_OPACITY,
    depthWrite: false,
    depthTest: true,
    polygonOffset: true,
    polygonOffsetFactor: -1,
    polygonOffsetUnits: -1,
  });
  const mesh = new THREE.InstancedMesh(geo, mat, picks.length);
  const m = new THREE.Matrix4();
  const col = new THREE.Color();
  picks.forEach((idx, n) => {
    const c = centreOf(grid, idx);
    m.makeTranslation(c.x, (grid.y[idx] ?? 0) + CELL_LIFT, c.z);
    mesh.setMatrixAt(n, m);
    mesh.setColorAt(n, col.setHex(colours[n] ?? COLOUR.green));
  });
  mesh.instanceMatrix.needsUpdate = true;
  if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
  mesh.frustumCulled = false;
  return mesh;
}

/** Line segments from a flat array of (x, y, z) pairs, two points per segment. */
function segments(pos: Float32Array, colour: number): THREE.LineSegments {
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  return new THREE.LineSegments(geo, new THREE.LineBasicMaterial({ color: colour }));
}

/** One segment per portal of the kind, from cell a's centre to cell b's centre. */
function portalSegments(grid: NavGrid, kind: 'ramp' | 'lift', colour: number): THREE.LineSegments {
  const list = grid.portals.filter((p) => p.kind === kind);
  const pos = new Float32Array(list.length * 6);
  list.forEach((p, n) => {
    const a = centreOf(grid, p.a);
    const b = centreOf(grid, p.b);
    pos.set(
      [a.x, (grid.y[p.a] ?? 0) + PORTAL_LIFT, a.z, b.x, (grid.y[p.b] ?? 0) + PORTAL_LIFT, b.z],
      n * 6,
    );
  });
  return segments(pos, colour);
}

/** Fills a layer group with the grid-dependent objects: cells, portals, lift rails and lift front markers. */
function fillLayer(layer: THREE.Group, grid: NavGrid): void {
  attach(layer, buildCells(grid));
  attach(layer, portalSegments(grid, 'ramp', COLOUR.ramp));
  attach(layer, portalSegments(grid, 'lift', COLOUR.lift));

  const rails = new Float32Array(grid.lifts.length * 6);
  grid.lifts.forEach((L, n) => rails.set([L.x, L.low, L.z, L.x, L.high, L.z], n * 6));
  attach(layer, segments(rails, COLOUR.cyan));

  const markerGeo = new THREE.SphereGeometry(MARKER_RADIUS, 8, 6);
  const markerMat = new THREE.MeshBasicMaterial({ color: COLOUR.lift, toneMapped: false });
  for (const L of grid.lifts) {
    const c = centreOf(grid, L.front);
    const marker = new THREE.Mesh(markerGeo, markerMat);
    marker.position.set(c.x, (grid.y[L.front] ?? 0) + PORTAL_LIFT, c.z);
    attach(layer, marker);
  }
}

/** Disposes the geometry and materials of an object and everything under it. */
function disposeTree(o: THREE.Object3D): void {
  o.traverse((c) => {
    const m = c as THREE.Mesh;
    if (m.geometry) m.geometry.dispose();
    const mat = m.material as THREE.Material | THREE.Material[] | undefined;
    if (Array.isArray(mat)) mat.forEach((x) => x.dispose());
    else mat?.dispose();
  });
}

/** Writes a robot's path into its position buffer, reallocating the buffer when the point count changes. */
function writePath(e: PathEntry, pts: NavPoint[]): void {
  const n = pts.length;
  if (!e.attr || e.attr.count !== n) {
    e.line.geometry.dispose();
    e.arr = new Float32Array(n * 3);
    e.attr = new THREE.BufferAttribute(e.arr, 3);
    e.line.geometry.setAttribute('position', e.attr);
  }
  pts.forEach((p, k) => {
    e.arr[k * 3] = p.x;
    e.arr[k * 3 + 1] = p.y + PATH_LIFT;
    e.arr[k * 3 + 2] = p.z;
  });
  if (e.attr) e.attr.needsUpdate = true;
}

export function buildNavOverlay(grid: NavGrid): NavOverlay {
  const group = new THREE.Group();
  tag(group);
  group.userData.overlay = true;
  const layer = new THREE.Group();
  const pathGroup = new THREE.Group();
  const ringGroup = new THREE.Group();
  attach(group, layer);
  attach(group, pathGroup);
  attach(group, ringGroup);

  // Shared by every robot: one path material, two ring materials and one ring geometry, all made once.
  const pathMat = new THREE.LineBasicMaterial({ color: COLOUR.path });
  const amberMat = new THREE.MeshBasicMaterial({ color: COLOUR.amber, toneMapped: false });
  const violetMat = new THREE.MeshBasicMaterial({ color: COLOUR.violet, toneMapped: false });
  const ringGeo = new THREE.RingGeometry(RING_INNER, RING_OUTER, 24);
  ringGeo.rotateX(-Math.PI / 2);

  const pathMap = new Map<number, PathEntry>();
  const ringMap = new Map<number, THREE.Mesh>();

  fillLayer(layer, grid);

  const newPath = (): PathEntry => {
    const line = new THREE.Line(new THREE.BufferGeometry(), pathMat);
    line.frustumCulled = false;
    tag(line);
    return { line, attr: null, arr: new Float32Array(0) };
  };

  return {
    group,

    setGrid(next: NavGrid): void {
      disposeTree(layer);
      layer.clear();
      fillLayer(layer, next);
    },

    update(world: NavWorld, ids: number[]): void {
      // Robots that are gone release their objects.
      for (const [id, e] of Array.from(pathMap)) {
        if (world.has(id)) continue;
        pathGroup.remove(e.line);
        e.line.geometry.dispose();
        pathMap.delete(id);
      }
      for (const [id, r] of Array.from(ringMap)) {
        if (world.has(id)) continue;
        ringGroup.remove(r);
        ringMap.delete(id);
      }

      const shown = new Set<number>();
      for (const id of ids) {
        if (!world.has(id)) continue;
        shown.add(id);

        const pts = world.path(id);
        let e = pathMap.get(id);
        if (pts) {
          if (!e) {
            e = newPath();
            pathMap.set(id, e);
            attach(pathGroup, e.line);
          }
          e.line.visible = true;
          writePath(e, pts);
        } else if (e) {
          e.line.visible = false;
        }

        const pose = world.pose(id);
        const waiting = pose.waiting;
        const yielding = pose.yielding === true;
        let r = ringMap.get(id);
        if (!waiting && !yielding) {
          if (r) r.visible = false;
          continue;
        }
        if (!r) {
          r = new THREE.Mesh(ringGeo, amberMat);
          ringMap.set(id, r);
          attach(ringGroup, r);
        }
        r.material = waiting ? amberMat : violetMat;
        r.position.set(pose.x, pose.y + RING_LIFT, pose.z);
        r.visible = true;
      }

      // Robots not asked for this frame are hidden, not removed.
      pathMap.forEach((e, id) => {
        if (!shown.has(id)) e.line.visible = false;
      });
      ringMap.forEach((r, id) => {
        if (!shown.has(id)) r.visible = false;
      });
    },

    dispose(): void {
      disposeTree(layer);
      layer.clear();
      pathMap.forEach((e) => e.line.geometry.dispose());
      pathMap.clear();
      ringMap.clear();
      ringGeo.dispose();
      pathMat.dispose();
      amberMat.dispose();
      violetMat.dispose();
      pathGroup.clear();
      ringGroup.clear();
      group.clear();
    },
  };
}
