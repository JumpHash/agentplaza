// Static batching for the floor: merges every mesh that never moves into one mesh per compatible
// group, so the scene draws in a few hundred calls instead of thousands, with the same look.
//
// Toon and basic materials that differ only in `color` batch too: a group shares one material
// (a clone of its first member's, colour white, `vertexColors: true`) and each mesh's colour goes
// into a per-vertex `color` attribute. `material.color` is already stored in linear working space,
// so the attribute holds exactly the values the material would have multiplied in.

import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';

export interface BakeStats {
  meshesBefore: number;
  meshesAfter: number;
}

/**
 * Static batching is on unless the hash query says `nobatch=1` (`#/floor/n0?nobatch=1`): the same scene
 * drawn with every mesh separate, so an A/B comparison of the two shows whether batching changed the look.
 */
export function batchingOn(): boolean {
  if (typeof window === 'undefined') return true; // tests build scenes without a window
  const q = window.location.hash.split('?')[1] ?? '';
  return new URLSearchParams(q).get('nobatch') !== '1';
}

function hasDynamicAncestor(obj: THREE.Object3D): boolean {
  for (let o: THREE.Object3D | null = obj; o; o = o.parent) {
    if (o.userData.dynamic === true) return true;
  }
  return false;
}

function hasHiddenAncestor(obj: THREE.Object3D): boolean {
  for (let o: THREE.Object3D | null = obj; o; o = o.parent) {
    if (!o.visible) return true;
  }
  return false;
}

function isCandidate(obj: THREE.Object3D): obj is THREE.Mesh {
  if (!(obj instanceof THREE.Mesh)) return false;
  if (obj instanceof THREE.InstancedMesh || obj instanceof THREE.SkinnedMesh) return false;
  if (Array.isArray(obj.material) || !obj.geometry.attributes.position) return false;
  if (obj.onBeforeRender !== THREE.Object3D.prototype.onBeforeRender) return false;
  return !hasHiddenAncestor(obj) && !hasDynamicAncestor(obj);
}

/** Material properties that identify an instance rather than its look; they are left out of the key. */
const IGNORED = new Set(['uuid', 'id', 'name', 'type', 'color', 'userData', 'version', 'vertexColors']);

/**
 * The compatibility key of a colour-batchable material: every property except its colour, so two
 * materials with the same key differ only in `color`. Null when the material cannot batch: a shader,
 * a material with its own `userData` (a closure may animate it, so it is marked `live`), or any
 * property this key does not understand.
 */
export function colourKey(mat: THREE.Material): string | null {
  if (!(mat instanceof THREE.MeshToonMaterial) && !(mat instanceof THREE.MeshBasicMaterial)) return null;
  if (mat.vertexColors || Object.keys(mat.userData).length > 0) return null;
  const parts: string[] = [mat.type];
  const own = mat as unknown as Record<string, unknown>;
  for (const k of Object.keys(mat).sort()) {
    if (IGNORED.has(k) || k.startsWith('_')) continue;
    const v = own[k];
    if (v === null || v === undefined) parts.push(`${k}=`);
    else if (typeof v === 'number' || typeof v === 'string' || typeof v === 'boolean') parts.push(`${k}=${String(v)}`);
    else if (v instanceof THREE.Color) parts.push(`${k}=${v.r},${v.g},${v.b}`); // emissive and the like: exact linear values
    else if (v instanceof THREE.Texture) parts.push(`${k}=${v.uuid}`);
    else return null;
  }
  return parts.join('|');
}

interface Group {
  members: THREE.Mesh[];
  /** The colour key when the group batches by colour; null when its members share one material. */
  batch: string | null;
}

function groupKey(mesh: THREE.Mesh, batch: string | null, cell: string): string {
  const geo = mesh.geometry;
  // A colour-batched group writes its own `color` attribute, so the attribute set leaves it out.
  const attrs = Object.keys(geo.attributes)
    .filter((a) => batch === null || a !== 'color')
    .sort()
    .join(',');
  const material = batch !== null ? `colour:${batch}` : `material:${(mesh.material as THREE.Material).uuid}`;
  const detail = mesh.userData.detail ?? 0;
  return [
    material,
    mesh.castShadow,
    mesh.receiveShadow,
    mesh.renderOrder,
    detail, // a detail level shows or hides by distance as one mesh, so levels never merge
    detail !== 0 ? cell : '', // ...and a level's parts merge only within a cell, so its distance culling stays local
    mesh.userData.isOutline === true,
    attrs,
    geo.index ? 'indexed' : 'flat',
  ].join('|');
}

/**
 * Merges the static meshes under `root`. A mesh is left alone when it, or any ancestor, has
 * `userData.dynamic === true` (it moves, animates, is picked or changes material), when it is
 * an InstancedMesh, SkinnedMesh, Points, Line or Sprite, or when its material is an array.
 */
export interface BakeOptions {
  /**
   * Static casters cast from merged proxies on SHADOW_PROXY_LAYER instead of being drawn twice: the
   * shadow pass draws a few cell-sized caster meshes, and the main pass draws the same as before.
   * Needs routeShadowProxies() on the view's renderer, or the proxies never reach the shadow map.
   */
  shadowProxies?: boolean;
}

/** The layer the caster proxies live on: the main camera leaves it out, the shadow pass puts it in. */
export const SHADOW_PROXY_LAYER = 1;

/** Caster proxies merge per cell: this many cells along the root's largest dimension. */
const PROXY_CELLS = 3;

/**
 * A static caster whose shadow a position-only proxy can carry: opaque, unanimated, not a detail
 * level (those show and hide by distance) and not a material whose depth depends on alpha or a shader.
 */
function proxyEligible(m: THREE.Mesh): boolean {
  if (!m.castShadow || m.userData.detail !== undefined || m.customDepthMaterial) return false;
  const mat = m.material as THREE.Material;
  if (!(mat instanceof THREE.MeshToonMaterial || mat instanceof THREE.MeshBasicMaterial || mat instanceof THREE.MeshStandardMaterial || mat instanceof THREE.MeshLambertMaterial)) return false;
  return !mat.transparent && mat.opacity === 1 && mat.alphaTest === 0 && !mat.alphaMap && Object.keys(mat.userData).length === 0;
}

/**
 * Makes the view's shadow pass see the caster proxies: while the shadow map draws, the main camera
 * also takes SHADOW_PROXY_LAYER, so the proxies cast; the main pass then restores the mask and skips them.
 * Installed once per renderer; a no-op for views whose scene has no proxies.
 */
const routed = new WeakSet<THREE.WebGLRenderer>();
export function routeShadowProxies(renderer: THREE.WebGLRenderer): void {
  if (routed.has(renderer)) return;
  routed.add(renderer);
  const shadows = renderer.shadowMap;
  const draw = shadows.render.bind(shadows);
  shadows.render = (lights, scene, camera) => {
    const mask = camera.layers.mask;
    camera.layers.enable(SHADOW_PROXY_LAYER);
    try {
      draw(lights, scene, camera);
    } finally {
      camera.layers.mask = mask;
    }
  };
}

export function bakeStatic(root: THREE.Object3D, options: BakeOptions = {}): BakeStats {
  // contract: O2a
  root.updateMatrixWorld(true);

  let meshesBefore = 0;
  root.traverse((o) => {
    if (o instanceof THREE.Mesh) meshesBefore++;
  });
  if (!batchingOn()) return { meshesBefore, meshesAfter: meshesBefore }; // ?nobatch=1: the A/B reference, nothing merged

  // Baked geometry is expressed in root-local space, so the merged mesh can keep an identity
  // transform under root whatever root's own transform is.
  const toRootLocal = new THREE.Matrix4().copy(root.matrixWorld).invert();
  const scratch = new THREE.Matrix4();
  const centre = new THREE.Vector3();

  // Detail parts merge per cell: a cell is a sixth of the root's largest dimension, so a level's
  // meshes stay small enough for their distance culling to mean something.
  const rootBox = new THREE.Box3().setFromObject(root).applyMatrix4(toRootLocal);
  const size = rootBox.getSize(new THREE.Vector3());
  const cellSize = Math.max(size.x, size.y, size.z) / 6 || 1;
  const cellOf = (m: THREE.Mesh): string => {
    const geo = m.geometry;
    if (!geo.boundingBox) geo.computeBoundingBox();
    geo.boundingBox?.getCenter(centre);
    centre.applyMatrix4(scratch.multiplyMatrices(toRootLocal, m.matrixWorld));
    return [centre.x, centre.y, centre.z].map((v) => Math.floor(v / cellSize)).join(',');
  };

  // Shadow proxies: every eligible static caster's position goes into one merged caster per cell
  // and side on SHADOW_PROXY_LAYER; the caster itself stops casting, so the shadow pass draws the proxies.
  // The proxy meshes themselves are added after the groups below, so the merge never sees them.
  const proxyBuckets = new Map<string, { side: THREE.Side; shadowSide: THREE.Side | null; geos: THREE.BufferGeometry[] }>();
  if (options.shadowProxies === true) {
    const proxyCell = Math.max(size.x, size.y, size.z) / PROXY_CELLS || 1;
    const casters: THREE.Mesh[] = [];
    root.traverse((obj) => {
      if (isCandidate(obj) && proxyEligible(obj)) casters.push(obj);
    });
    for (const m of casters) {
      const geo = m.geometry.clone();
      for (const name of Object.keys(geo.attributes)) if (name !== 'position') geo.deleteAttribute(name);
      const flat = geo.index ? geo.toNonIndexed() : geo;
      if (flat !== geo) geo.dispose();
      flat.applyMatrix4(scratch.multiplyMatrices(toRootLocal, m.matrixWorld));
      flat.computeBoundingBox();
      flat.boundingBox?.getCenter(centre);
      const cell = [centre.x, centre.y, centre.z].map((v) => Math.floor(v / proxyCell)).join(',');
      const mat = m.material as THREE.Material;
      const key = `${cell}|${mat.side}|${mat.shadowSide ?? ''}`;
      const bucket = proxyBuckets.get(key);
      if (bucket) bucket.geos.push(flat);
      else proxyBuckets.set(key, { side: mat.side, shadowSide: mat.shadowSide, geos: [flat] });
      m.castShadow = false; // the proxy casts for it
    }
  }

  // Group candidates by a key that guarantees identical attribute sets and index layout, so
  // every geometry in a group can be merged directly.
  const groups = new Map<string, Group>();
  root.traverse((obj) => {
    if (!isCandidate(obj)) return;
    const batch = colourKey(obj.material as THREE.Material);
    const key = groupKey(obj, batch, (obj.userData.detail ?? 0) !== 0 ? cellOf(obj) : '');
    const g = groups.get(key);
    if (g) g.members.push(obj);
    else groups.set(key, { members: [obj], batch });
  });

  // One shared material per colour key, whatever its shadow flags or detail level.
  const batchMaterials = new Map<string, THREE.Material>();
  const batchMaterial = (key: string, source: THREE.Material): THREE.Material => {
    let m = batchMaterials.get(key);
    if (!m) {
      m = source.clone();
      (m as THREE.MeshBasicMaterial).color.setRGB(1, 1, 1); // the colour now comes from the vertices
      m.vertexColors = true;
      m.name = `batch:${source.name || source.type}`;
      batchMaterials.set(key, m);
    }
    return m;
  };

  for (const { members, batch } of groups.values()) {
    // A lone outline hull still merges: its parent is removed below, and it must not go with it.
    if (members.length < 2 && members[0]?.userData.isOutline !== true) continue;

    const clones = members.map((m) => {
      const geo = m.geometry.clone();
      geo.applyMatrix4(scratch.multiplyMatrices(toRootLocal, m.matrixWorld));
      if (batch !== null) {
        const c = (m.material as THREE.MeshBasicMaterial).color;
        const n = geo.getAttribute('position').count;
        const col = new Float32Array(n * 3);
        for (let i = 0; i < n; i++) col.set([c.r, c.g, c.b], i * 3);
        geo.setAttribute('color', new THREE.BufferAttribute(col, 3));
      }
      return geo;
    });
    const merged = mergeGeometries(clones, false);
    for (const c of clones) c.dispose();
    if (!merged) continue; // incompatible layouts: keep the originals as they are
    merged.computeBoundingSphere();

    const first = members[0];
    if (!first) continue;
    const source = first.material as THREE.Material;
    const material = batch !== null ? batchMaterial(batch, source) : source;
    const baked = new THREE.Mesh(merged, material);
    baked.name = `baked:${material.name || material.type}`;
    baked.castShadow = first.castShadow;
    baked.receiveShadow = first.receiveShadow;
    baked.renderOrder = first.renderOrder;
    baked.matrixAutoUpdate = false;
    if (first.userData.detail !== undefined) baked.userData.detail = first.userData.detail;
    if (first.userData.isOutline === true) baked.userData.isOutline = true;
    root.add(baked);
    baked.updateMatrixWorld(true);

    // Only references are dropped: the originals' geometries and materials stay intact.
    for (const m of members) m.removeFromParent();
  }

  for (const { side, shadowSide, geos } of proxyBuckets.values()) {
    const merged = mergeGeometries(geos, false);
    for (const g of geos) g.dispose();
    if (!merged) continue;
    merged.computeBoundingSphere();
    const proxyMat = new THREE.MeshBasicMaterial({ side });
    proxyMat.shadowSide = shadowSide;
    const proxy = new THREE.Mesh(merged, proxyMat);
    proxy.name = 'shadow-proxy';
    proxy.layers.set(SHADOW_PROXY_LAYER);
    proxy.castShadow = true;
    proxy.matrixAutoUpdate = false;
    root.add(proxy);
  }

  // Prune empty plain containers, children before parents.
  const containers: THREE.Object3D[] = [];
  root.traverse((o) => {
    if (o !== root) containers.push(o);
  });
  for (let i = containers.length - 1; i >= 0; i--) {
    const o = containers[i];
    if (!o) continue;
    if (
      o.children.length === 0 &&
      (o.type === 'Group' || o.type === 'Object3D') &&
      Object.keys(o.userData).length === 0
    ) {
      o.removeFromParent();
    }
  }

  let meshesAfter = 0;
  root.traverse((o) => {
    if (o instanceof THREE.Mesh) meshesAfter++;
  });

  return { meshesBefore, meshesAfter };
}
