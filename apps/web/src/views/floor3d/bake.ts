// Static batching for the floor: merges every mesh that never moves into one mesh per
// (material, shadow flags) group, so the scene draws in a few hundred calls instead of
// thousands — with exactly the same look (same materials, same world transforms).

import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';

export interface BakeStats {
  meshesBefore: number;
  meshesAfter: number;
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

function groupKey(mesh: THREE.Mesh): string {
  const geo = mesh.geometry;
  return [
    (mesh.material as THREE.Material).uuid,
    mesh.castShadow,
    mesh.receiveShadow,
    mesh.renderOrder,
    Object.keys(geo.attributes).sort().join(','),
    geo.index ? 'indexed' : 'flat',
  ].join('|');
}

/**
 * Merges the static meshes under `root`. A mesh is left alone when it, or any ancestor, has
 * `userData.dynamic === true` (it moves, animates, is picked or changes material), when it is
 * an InstancedMesh, SkinnedMesh, Points, Line or Sprite, or when its material is an array.
 */
export function bakeStatic(root: THREE.Object3D): BakeStats {
  // contract: O2a
  root.updateMatrixWorld(true);

  let meshesBefore = 0;
  root.traverse((o) => {
    if (o instanceof THREE.Mesh) meshesBefore++;
  });

  // Group candidates by a key that guarantees identical attribute sets and index layout, so
  // every geometry in a group can be merged directly.
  const groups = new Map<string, THREE.Mesh[]>();
  root.traverse((obj) => {
    if (!isCandidate(obj)) return;
    const key = groupKey(obj);
    const list = groups.get(key);
    if (list) list.push(obj);
    else groups.set(key, [obj]);
  });

  // Baked geometry is expressed in root-local space, so the merged mesh can keep an identity
  // transform under root whatever root's own transform is.
  const toRootLocal = new THREE.Matrix4().copy(root.matrixWorld).invert();
  const scratch = new THREE.Matrix4();

  for (const members of groups.values()) {
    if (members.length < 2) continue;

    const clones = members.map((m) => {
      const geo = m.geometry.clone();
      geo.applyMatrix4(scratch.multiplyMatrices(toRootLocal, m.matrixWorld));
      return geo;
    });
    const merged = mergeGeometries(clones, false);
    for (const c of clones) c.dispose();
    if (!merged) continue; // incompatible layouts: keep the originals as they are
    merged.computeBoundingSphere();

    const first = members[0];
    if (!first) continue;
    const material = first.material as THREE.Material;
    const baked = new THREE.Mesh(merged, material);
    baked.name = `baked:${material.name || material.type}`;
    baked.castShadow = first.castShadow;
    baked.receiveShadow = first.receiveShadow;
    baked.renderOrder = first.renderOrder;
    baked.matrixAutoUpdate = false;
    root.add(baked);
    baked.updateMatrixWorld(true);

    // Only references are dropped: the originals' geometries and materials stay intact.
    for (const m of members) m.removeFromParent();
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
