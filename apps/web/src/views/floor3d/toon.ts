// The Floor's cartoon look: banded toon shading, ink outlines, a painted night sky with stars and a
// ringed planet. Every lit surface in the scene is a `toonMat`; `outline` gives a group the
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

/** A painted sky dome: banded from the zenith's colour down to the horizon's glow. */
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

/**
 * The night sky's furniture: stars scattered over the upper sky, a few bright coloured ones, and a
 * ringed planet low over the horizon. Unfogged, so they read through the haze. The field turns slowly.
 */
export function starfield(n: number, radius: number, rand: () => number): { group: THREE.Group; animate: (t: number) => void } {
  const group = new THREE.Group();
  const layer = (count: number, size: number, color: number, opacity: number): void => {
    const pos = new Float32Array(count * 3);
    for (let i = 0; i < count; i++) {
      // Uniform over the sky above 6 degrees of elevation.
      const a = rand() * Math.PI * 2;
      const y = 0.1 + rand() * 0.9;
      const ring = Math.sqrt(1 - y * y);
      pos.set([Math.cos(a) * ring * radius, y * radius, Math.sin(a) * ring * radius], i * 3);
    }
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.BufferAttribute(pos, 3));
    const mat = new THREE.PointsMaterial({ color, size, transparent: true, opacity, depthWrite: false, fog: false });
    group.add(new THREE.Points(geo, mat));
  };
  layer(n, 1.4, 0xdfe8ff, 0.85);
  layer(Math.round(n / 8), 2.6, 0x9fdcff, 0.95);
  layer(Math.round(n / 16), 2.4, 0xd9a8ff, 0.95);

  // A ringed planet low in the sky.
  const planet = new THREE.Group();
  const body = new THREE.Mesh(
    new THREE.SphereGeometry(18, 32, 20),
    toonMat({ color: 0x6b4fd8, emissive: 0x3a1f8f, emissiveIntensity: 0.6, fog: false }),
  );
  const band = new THREE.Mesh(
    new THREE.SphereGeometry(18.15, 32, 6, 0, Math.PI * 2, Math.PI * 0.42, Math.PI * 0.1),
    toonMat({ color: 0x9d7dff, emissive: 0x5a3bd0, emissiveIntensity: 0.6, fog: false }),
  );
  const ringGeo = new THREE.RingGeometry(24, 33, 64);
  ringGeo.rotateX(-Math.PI / 2);
  const ring = new THREE.Mesh(
    ringGeo,
    new THREE.MeshBasicMaterial({ color: 0x8fd8ff, transparent: true, opacity: 0.45, side: THREE.DoubleSide, depthWrite: false, fog: false }),
  );
  ring.userData.outline = false;
  planet.add(body, band, ring);
  planet.rotation.set(0.35, 0, 0.3);
  planet.position.set(-150, 70, -210);
  group.add(planet);
  outline(planet, 1);
  return {
    group,
    animate: (t: number) => {
      group.rotation.y = t * 0.004;
      planet.rotation.y = t * 0.05;
    },
  };
}
