// The Floor's cartoon look: banded toon shading, ink outlines, a painted sky with drifting clouds,
// and wind-swayed grass. Every lit surface in the scene is a `toonMat`; `outline` gives a group the
// inked silhouette of a cartoon.

import * as THREE from 'three';
import { mergeVertices } from 'three/examples/jsm/utils/BufferGeometryUtils.js';

const INK = new THREE.Color(0x2b2420);

let ramp: THREE.DataTexture | null = null;

/** Three light bands (shadow, mid, lit) with hard steps between them: the toon ramp. */
function gradientMap(): THREE.DataTexture {
  if (ramp) return ramp;
  const steps = [90, 170, 255];
  const data = new Uint8Array(steps.length * 4);
  steps.forEach((v, i) => data.set([v, v, v, 255], i * 4));
  ramp = new THREE.DataTexture(data, steps.length, 1, THREE.RGBAFormat);
  ramp.minFilter = THREE.NearestFilter;
  ramp.magFilter = THREE.NearestFilter;
  ramp.generateMipmaps = false;
  ramp.needsUpdate = true;
  return ramp;
}

/** The parameters a standard material would take; roughness and metalness are accepted and ignored. */
export type ToonParams = THREE.MeshToonMaterialParameters & { roughness?: number; metalness?: number };

/** A toon material: flat colour bands under the sun, with the plaza's shared ramp. */
export function toonMat(params: ToonParams = {}): THREE.MeshToonMaterial {
  const { roughness: _r, metalness: _m, ...rest } = params;
  return new THREE.MeshToonMaterial({ gradientMap: gradientMap(), ...rest });
}

// ---- Outlines --------------------------------------------------------------------------------

const OUTLINE_VS = /* glsl */ `
uniform float thickness;
#include <common>
#include <fog_pars_vertex>
void main() {
  vec4 mvPosition = modelViewMatrix * vec4(position, 1.0);
  vec3 n = normalize(normalMatrix * normal);
  // A constant screen-ish width: thicker the farther away, so outlines read at every zoom.
  mvPosition.xyz += n * thickness * clamp(-mvPosition.z * 0.06, 0.7, 4.0);
  gl_Position = projectionMatrix * mvPosition;
  #include <fog_vertex>
}`;
const OUTLINE_FS = /* glsl */ `
uniform vec3 color;
#include <common>
#include <fog_pars_fragment>
void main() {
  gl_FragColor = vec4(color, 1.0);
  #include <fog_fragment>
}`;

let outlineMaterial: THREE.ShaderMaterial | null = null;
function outlineMat(): THREE.ShaderMaterial {
  if (outlineMaterial) return outlineMaterial;
  outlineMaterial = new THREE.ShaderMaterial({
    uniforms: THREE.UniformsUtils.merge([THREE.UniformsLib.fog, { thickness: { value: 0.024 }, color: { value: INK } }]),
    vertexShader: OUTLINE_VS,
    fragmentShader: OUTLINE_FS,
    side: THREE.BackSide,
    fog: true,
  });
  return outlineMaterial;
}

/** Smooth normals for the hull: welded vertices, so a box's outline has no cracks at its corners. */
const hulls = new WeakMap<THREE.BufferGeometry, THREE.BufferGeometry>();
function hullOf(geo: THREE.BufferGeometry): THREE.BufferGeometry {
  const hit = hulls.get(geo);
  if (hit) return hit;
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', geo.getAttribute('position'));
  if (geo.index) g.setIndex(geo.index);
  const welded = mergeVertices(g, 1e-3);
  welded.computeVertexNormals();
  hulls.set(geo, welded);
  return welded;
}

/**
 * Inks every opaque mesh under `root` with a back-face hull: the cartoon outline. Meshes smaller
 * than `minSize` (flower heads, rivets) and transparent or unlit ones are left alone.
 */
export function outline(root: THREE.Object3D, minSize = 0.12): void {
  const meshes: THREE.Mesh[] = [];
  root.traverse((o) => {
    const m = o as THREE.Mesh;
    if (!m.isMesh || m.userData.outline === false || m.userData.isOutline) return;
    if (m.children.some((c) => c.userData.isOutline)) return; // already inked
    const mat = (Array.isArray(m.material) ? m.material[0] : m.material) as THREE.Material | undefined;
    if (!mat || mat.transparent || (mat as THREE.MeshBasicMaterial).isMeshBasicMaterial) return;
    const geo = m.geometry;
    if (!geo.boundingSphere) geo.computeBoundingSphere();
    if ((geo.boundingSphere?.radius ?? 0) < minSize) return;
    meshes.push(m);
  });
  for (const m of meshes) {
    const hull = new THREE.Mesh(hullOf(m.geometry), outlineMat());
    hull.userData.isOutline = true;
    hull.castShadow = false;
    hull.receiveShadow = false;
    hull.raycast = () => {};
    m.add(hull);
  }
}

// ---- Sky -------------------------------------------------------------------------------------

const SKY_VS = /* glsl */ `
varying vec3 vDir;
void main() {
  vDir = normalize(position);
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  gl_Position.z = gl_Position.w; // always at the far plane
}`;
const SKY_FS = /* glsl */ `
uniform vec3 top;
uniform vec3 horizon;
uniform vec3 ground;
varying vec3 vDir;
void main() {
  float h = vDir.y;
  // Hard-edged bands near the horizon, the way a painted backdrop does it.
  float t = smoothstep(0.0, 0.55, h);
  t = floor(t * 6.0 + 0.5) / 6.0;
  vec3 c = mix(horizon, top, t);
  c = mix(ground, c, smoothstep(-0.05, 0.02, h));
  gl_FragColor = vec4(c, 1.0);
}`;

/** A painted sky dome: banded blue to a warm horizon. */
export function sky(top: THREE.Color, horizon: THREE.Color, ground: THREE.Color): THREE.Mesh {
  const mat = new THREE.ShaderMaterial({
    uniforms: { top: { value: top }, horizon: { value: horizon }, ground: { value: ground } },
    vertexShader: SKY_VS,
    fragmentShader: SKY_FS,
    side: THREE.BackSide,
    depthWrite: false,
  });
  const dome = new THREE.Mesh(new THREE.SphereGeometry(400, 32, 16), mat);
  dome.renderOrder = -1;
  dome.frustumCulled = false;
  return dome;
}

/** Puffy cartoon clouds drifting in a slow ring overhead. Returns the group and its per-frame step. */
export function clouds(n: number, radius: number, rand: () => number): { group: THREE.Group; animate: (t: number) => void } {
  const group = new THREE.Group();
  const mat = toonMat({ color: 0xffffff, emissive: 0xdde6f0, emissiveIntensity: 0.35 });
  const puff = new THREE.SphereGeometry(1, 16, 12);
  for (let i = 0; i < n; i++) {
    const c = new THREE.Group();
    const lobes = 4 + Math.floor(rand() * 3);
    for (let k = 0; k < lobes; k++) {
      const s = 1.6 + rand() * 1.6;
      const m = new THREE.Mesh(puff, mat);
      m.scale.set(s * 1.3, s * 0.8, s);
      m.position.set((k - lobes / 2) * 1.9 + rand(), rand() * 0.8, (rand() - 0.5) * 2);
      c.add(m);
    }
    const a = (i / n) * Math.PI * 2 + rand() * 0.4;
    const r = radius * (0.7 + rand() * 0.5);
    c.position.set(Math.cos(a) * r, 48 + rand() * 14, Math.sin(a) * r);
    c.rotation.y = rand() * Math.PI;
    group.add(c);
  }
  outline(group, 0);
  return { group, animate: (t: number) => (group.rotation.y = t * 0.006) };
}

// ---- Grass -----------------------------------------------------------------------------------

/**
 * Tufts of grass instanced over the meadow, swaying in the wind (a vertex shader bends each blade
 * by its height). `keep(x, z)` says whether a tuft may grow at a spot.
 */
export function grassTufts(count: number, reach: number, keep: (x: number, z: number) => boolean, rand: () => number): { mesh: THREE.InstancedMesh; animate: (t: number) => void } {
  // One tuft: three crossed blades, each a thin triangle.
  const pos: number[] = [];
  for (let b = 0; b < 3; b++) {
    const a = (b / 3) * Math.PI + 0.3;
    const dx = Math.cos(a) * 0.07;
    const dz = Math.sin(a) * 0.07;
    const lean = (b - 1) * 0.06;
    pos.push(-dx, 0, -dz, dx, 0, dz, lean, 0.32 + b * 0.05, 0);
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  geo.computeVertexNormals();
  const mat = toonMat({ color: 0xffffff, side: THREE.DoubleSide });
  const uniforms = { uTime: { value: 0 } };
  mat.onBeforeCompile = (shader) => {
    shader.uniforms.uTime = uniforms.uTime;
    shader.vertexShader = shader.vertexShader
      .replace('#include <common>', '#include <common>\nuniform float uTime;')
      .replace(
        '#include <begin_vertex>',
        `#include <begin_vertex>
        vec4 wp = instanceMatrix * vec4(0.0, 0.0, 0.0, 1.0);
        float sway = sin(uTime * 1.7 + wp.x * 0.35 + wp.z * 0.25) * 0.5 + sin(uTime * 3.1 + wp.x) * 0.15;
        transformed.x += sway * position.y * 0.45;
        transformed.z += sway * position.y * 0.2;`,
      );
    // Paler tips: a little gradient up each blade.
    shader.vertexShader = shader.vertexShader.replace('#include <color_vertex>', '#include <color_vertex>\nvTipShade = position.y;');
    shader.vertexShader = shader.vertexShader.replace('#include <common>', '#include <common>\nvarying float vTipShade;');
    shader.fragmentShader = shader.fragmentShader
      .replace('#include <common>', '#include <common>\nvarying float vTipShade;')
      .replace('#include <color_fragment>', '#include <color_fragment>\ndiffuseColor.rgb *= 0.75 + vTipShade * 1.1;');
  };
  const mesh = new THREE.InstancedMesh(geo, mat, count);
  const m = new THREE.Matrix4();
  const q = new THREE.Quaternion();
  const up = new THREE.Vector3(0, 1, 0);
  const tint = new THREE.Color();
  let n = 0;
  for (let i = 0; i < count * 4 && n < count; i++) {
    const x = (rand() * 2 - 1) * reach;
    const z = (rand() * 2 - 1) * reach;
    if (Math.hypot(x, z) > reach || !keep(x, z)) continue;
    const s = 0.7 + rand() * 0.9;
    q.setFromAxisAngle(up, rand() * Math.PI * 2);
    m.compose(new THREE.Vector3(x, 0, z), q, new THREE.Vector3(s, s, s));
    mesh.setMatrixAt(n, m);
    mesh.setColorAt(n, tint.setHSL(0.24 + rand() * 0.07, 0.5 + rand() * 0.2, 0.42 + rand() * 0.14));
    n++;
  }
  mesh.count = n;
  mesh.receiveShadow = true;
  mesh.instanceMatrix.needsUpdate = true;
  if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
  mesh.userData.outline = false;
  return { mesh, animate: (t: number) => (uniforms.uTime.value = t) };
}
