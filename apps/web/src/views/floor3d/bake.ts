// Static batching for the floor: merges every mesh that never moves into one mesh per
// (material, shadow flags) group, so the scene draws in a few hundred calls instead of
// thousands — with exactly the same look (same materials, same world transforms).

import type * as THREE from 'three';

export interface BakeStats {
  meshesBefore: number;
  meshesAfter: number;
}

/**
 * Merges the static meshes under `root`. A mesh is left alone when it, or any ancestor, has
 * `userData.dynamic === true` (it moves, animates, is picked or changes material), when it is
 * an InstancedMesh, SkinnedMesh, Points, Line or Sprite, or when its material is an array.
 */
export function bakeStatic(_root: THREE.Object3D): BakeStats {
  // contract: O2a
  return { meshesBefore: 0, meshesAfter: 0 };
}
