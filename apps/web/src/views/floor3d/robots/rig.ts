// The robot rig: a species is baked once (its parts merged per bone), and each robot gets its own
// bone tree, a few materials and its own draw calls. Draw calls per robot: one matte mesh per bone,
// one hull (the ink line) per bone that has inked parts, one mesh per solo material (a light, the
// thruster, a clear part) and one pupil mesh per eye row. Parts whose colour depends on the robot
// (shell, shell2, panel) are a per-vertex slot, so one shared toon material serves every bone.

import * as THREE from 'three';
import { mergeGeometries, mergeVertices } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { toonMat } from '../toon';
import { eyeGeometries, nonIndexed, partGeometry } from './geometry';
import type { InlineMat, MatRef, Palette, Part, Species, Tool, Vec3 } from './types';

/** The slot of a part's colour: 0 a fixed palette colour (highlighted), 1–3 the robot's own hues, 4 a fixed colour that never glows. */
const SLOT: Record<string, number> = { shell: 1, shell2: 2, panel: 3 };
const FIXED = ['paper', 'trim', 'ink', 'signal', 'accent', 'glass'];
/** The solo materials: one mesh each, per robot (eye, light, thruster) or shared (clear, inline). */
const SOLO = ['eye', 'light', 'thruster', 'clear'];
/** A part smaller than this (its bounding radius) is not inked, as in toon.ts. */
const INK_MIN = 0.03;

export interface BoneGeo {
  matte: THREE.BufferGeometry | null;
  hull: THREE.BufferGeometry | null;
  solo: { key: string; geometry: THREE.BufferGeometry; inline?: InlineMat }[];
  /** One pupil mesh per eye row: its pivot is `y` (the blink scales about it). */
  pupils: { y: number; geometry: THREE.BufferGeometry }[];
}

/** A baked species (or a tool): the geometry of each bone, shared by every robot of it. */
export interface SpeciesGeo {
  bones: Map<string, BoneGeo>;
  /** The eyes' glass sockets are in the matte (they are not inked); this is for the count only. */
  eyeCount: number;
}

/** Robot materials: one set per robot (its hues and its eye state). */
export interface RobotMats {
  matte: THREE.MeshToonMaterial;
  eye: THREE.MeshToonMaterial;
  light: THREE.MeshToonMaterial;
  thruster: THREE.MeshBasicMaterial;
  uniforms: { uShell: { value: THREE.Color }; uShell2: { value: THREE.Color }; uPanel: { value: THREE.Color } };
}

const SLOT_VERT = 'attribute float slot;\nvarying float vSlot;\n';
const SLOT_FRAG = 'uniform vec3 uShell;\nuniform vec3 uShell2;\nuniform vec3 uPanel;\nvarying float vSlot;\n';

/** Per-robot hues: the shell (the agent's own), its darker shade, and the district's panel. */
export function robotMats(palette: Palette, hue: number, bodyHue: number): RobotMats {
  const uniforms = {
    uShell: { value: new THREE.Color().setHSL(bodyHue / 360, 0.62, 0.62, THREE.SRGBColorSpace) },
    uShell2: { value: new THREE.Color().setHSL(bodyHue / 360, 0.6, 0.46, THREE.SRGBColorSpace) },
    uPanel: { value: new THREE.Color().setHSL(hue / 360, 0.55, 0.78, THREE.SRGBColorSpace) },
  };
  const matte = toonMat({ vertexColors: true, color: 0xffffff });
  matte.onBeforeCompile = (shader) => {
    Object.assign(shader.uniforms, uniforms);
    shader.vertexShader = shader.vertexShader
      .replace('#include <common>', `#include <common>\n${SLOT_VERT}`)
      .replace('#include <begin_vertex>', '#include <begin_vertex>\nvSlot = slot;');
    shader.fragmentShader = shader.fragmentShader
      .replace('#include <common>', `#include <common>\n${SLOT_FRAG}`)
      .replace('#include <color_fragment>', '#include <color_fragment>\nif (vSlot > 0.5 && vSlot < 3.5) diffuseColor.rgb = vSlot < 1.5 ? uShell : (vSlot < 2.5 ? uShell2 : uPanel);')
      .replace('#include <emissivemap_fragment>', '#include <emissivemap_fragment>\nif (vSlot > 3.5) totalEmissiveRadiance = vec3(0.0);');
  };
  matte.customProgramCacheKey = () => 'apz-robot-matte';
  return {
    matte,
    eye: toonMat({ color: palette.ok, emissive: palette.ok, emissiveIntensity: 1.2 }),
    light: toonMat({ color: palette.signal, emissive: palette.signal, emissiveIntensity: 1.2 }),
    thruster: new THREE.MeshBasicMaterial({ color: palette.info, toneMapped: false }),
    uniforms,
  };
}

const sharedSolo = new Map<string, THREE.Material>();
/** The clear and inline materials are shared by every robot of the species that names them. */
function sharedMat(key: string, inline?: InlineMat): THREE.Material {
  const hit = sharedSolo.get(key);
  if (hit) return hit;
  const m = inline
    ? toonMat({ color: inline.color, emissive: inline.emissive ?? '#000000', emissiveIntensity: inline.glow ?? (inline.emissive ? 1 : 0), transparent: (inline.opacity ?? 1) < 1, opacity: inline.opacity ?? 1, depthWrite: (inline.opacity ?? 1) >= 1 })
    : toonMat({ color: '#ffffff', transparent: true, opacity: 0.5, depthWrite: false });
  sharedSolo.set(key, m);
  return m;
}
export function soloMaterial(key: string, mats: RobotMats, inline?: InlineMat): THREE.Material {
  if (key === 'eye') return mats.eye;
  if (key === 'light') return mats.light;
  if (key === 'thruster') return mats.thruster;
  return sharedMat(key, inline);
}

/** Vertex attributes for the matte: position, normal, colour, slot (the colour is linear, from the palette). */
function matteAttrs(geo: THREE.BufferGeometry, colour: THREE.Color, slot: number): THREE.BufferGeometry {
  const n = geo.getAttribute('position').count;
  const col = new Float32Array(n * 3);
  const sl = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    col[i * 3] = colour.r;
    col[i * 3 + 1] = colour.g;
    col[i * 3 + 2] = colour.b;
    sl[i] = slot;
  }
  geo.setAttribute('color', new THREE.BufferAttribute(col, 3));
  geo.setAttribute('slot', new THREE.BufferAttribute(sl, 1));
  return geo;
}

function fixedColour(name: string, palette: Palette, accent: string): THREE.Color {
  const hex = name === 'paper' ? palette.paper : name === 'trim' ? palette.rule : name === 'signal' ? palette.signal : name === 'accent' ? accent : palette.ink;
  return new THREE.Color(hex);
}

interface Acc {
  matte: THREE.BufferGeometry[];
  hull: THREE.BufferGeometry[];
  solo: Map<string, { geos: THREE.BufferGeometry[]; inline?: InlineMat }>;
  pupils: Map<number, THREE.BufferGeometry[]>;
}

function accOf(accs: Map<string, Acc>, bone: string): Acc {
  let a = accs.get(bone);
  if (!a) {
    a = { matte: [], hull: [], solo: new Map(), pupils: new Map() };
    accs.set(bone, a);
  }
  return a;
}

/** Bakes parts into the accumulators of their bones (`root` for a tool's parts). */
function bake(parts: Part[], palette: Palette, accent: string, accs: Map<string, Acc>, boneOf: (p: Part) => string): void {
  for (const p of parts) {
    const { geometry, radius } = partGeometry(p);
    const a = accOf(accs, boneOf(p));
    const mat: MatRef = p.mat;
    if (typeof mat === 'string' && (mat in SLOT || FIXED.includes(mat))) {
      const slot = SLOT[mat] ?? (mat === 'glass' ? 4 : 0);
      const colour = mat in SLOT ? new THREE.Color(1, 1, 1) : fixedColour(mat, palette, accent);
      a.matte.push(matteAttrs(geometry, colour, slot));
      const inked = p.outline ?? mat !== 'ink';
      if (inked && radius >= INK_MIN) a.hull.push(hullPositions(geometry));
    } else {
      const key = typeof mat === 'string' ? mat : `inline:${JSON.stringify(mat)}`;
      if (typeof mat === 'string' && !SOLO.includes(mat)) throw new Error(`robot material "${mat}" is not a known name`);
      const slotEntry = a.solo.get(key) ?? { geos: [] as THREE.BufferGeometry[], inline: typeof mat === 'string' ? undefined : mat };
      slotEntry.geos.push(geometry);
      a.solo.set(key, slotEntry);
    }
  }
}

/** A part's position-only copy for the hull (welded and normals made later, one per bone). */
function hullPositions(geometry: THREE.BufferGeometry): THREE.BufferGeometry {
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', geometry.getAttribute('position'));
  return g;
}

/** Merges a list of geometries (same attributes), or null when empty. */
function merge(list: THREE.BufferGeometry[]): THREE.BufferGeometry | null {
  if (list.length === 0) return null;
  return list.length === 1 ? list[0]! : mergeGeometries(list, false);
}

/** The ink line of a bone: one welded hull over its inked parts (smooth normals, as in toon.ts). */
function hullOf(list: THREE.BufferGeometry[]): THREE.BufferGeometry | null {
  const all = merge(list);
  if (!all) return null;
  const welded = mergeVertices(all, 1e-3);
  welded.computeVertexNormals();
  return welded;
}

/** Eyes: their glass sockets join the matte (fixed colour, no glow), their pupils a pupil mesh per row. */
function bakeEyes(species: Species, accs: Map<string, Acc>, palette: Palette): number {
  let n = 0;
  for (const e of species.eyes) {
    n++;
    const { pupil, glass } = eyeGeometries(e);
    const a = accOf(accs, e.bone);
    if (glass) a.matte.push(matteAttrs(nonIndexed(glass), fixedColour('glass', palette, species.accent), 4));
    const rows = a.pupils;
    const list = rows.get(e.at[1]) ?? [];
    list.push(nonIndexed(pupil));
    rows.set(e.at[1], list);
  }
  return n;
}

/** Species geometry: built once per (species, orchestrator). */
export function bakeSpecies(species: Species, palette: Palette, orchestrator: boolean): SpeciesGeo {
  const accs = new Map<string, Acc>();
  bake(species.parts, palette, species.accent, accs, (p) => p.bone ?? 'root');
  if (orchestrator && species.orchestrator) bake(species.orchestrator.parts, palette, species.accent, accs, (p) => p.bone ?? 'root');
  const eyeCount = bakeEyes(species, accs, palette);
  const bones = new Map<string, BoneGeo>();
  for (const [name, a] of accs) bones.set(name, toBoneGeo(a));
  return { bones, eyeCount };
}

function toBoneGeo(a: Acc): BoneGeo {
  return {
    matte: merge(a.matte),
    hull: hullOf(a.hull),
    solo: [...a.solo].map(([key, e]) => ({ key, geometry: merge(e.geos)!, inline: e.inline })),
    pupils: [...a.pupils].map(([y, list]) => {
      const g = merge(list)!;
      g.translate(0, -y, 0); // the row's pivot is its own height
      return { y, geometry: g };
    }),
  };
}

/** A tool or carried item, baked as a single bone; its parts use the robot's materials and its own colours. */
export function bakeTool(tool: Tool, palette: Palette): BoneGeo {
  const accs = new Map<string, Acc>();
  bake(tool.parts, palette, palette.signal, accs, () => 'root');
  const a = accs.get('root');
  return a ? toBoneGeo(a) : { matte: null, hull: null, solo: [], pupils: [] };
}

// The ink line: the same back-face hull as toon.ts's outline.
const OUTLINE_VS = /* glsl */ `
uniform float thickness;
#include <common>
#include <fog_pars_vertex>
void main() {
  vec4 mvPosition = modelViewMatrix * vec4(position, 1.0);
  vec3 n = normalize(normalMatrix * normal);
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
let inkMaterial: THREE.ShaderMaterial | null = null;
function inkMat(): THREE.ShaderMaterial {
  if (inkMaterial) return inkMaterial;
  inkMaterial = new THREE.ShaderMaterial({
    uniforms: THREE.UniformsUtils.merge([THREE.UniformsLib.fog, { thickness: { value: 0.024 }, color: { value: new THREE.Color(0x2b2420) } }]),
    vertexShader: OUTLINE_VS,
    fragmentShader: OUTLINE_FS,
    side: THREE.BackSide,
    fog: true,
  });
  return inkMaterial;
}

/** One robot's instance of a baked species: its bone tree, meshes, and the pupils to blink. */
export interface RigInstance {
  bones: Map<string, THREE.Group>;
  /** The rest offsets of each bone (its `at`), so a clip's pos offsets are relative to it. */
  rest: Map<string, Vec3>;
  pupils: THREE.Mesh[];
  /** Every mesh a robot owns (for dispose). */
  owned: THREE.Mesh[];
  /** Adds a bone's own geometry (a tool, a carried item) under a bone. */
  attach(bone: string, geo: BoneGeo): THREE.Object3D;
  dispose(): void;
}

function meshFor(geometry: THREE.BufferGeometry, material: THREE.Material, castShadow: boolean): THREE.Mesh {
  const m = new THREE.Mesh(geometry, material);
  m.castShadow = castShadow;
  m.receiveShadow = castShadow;
  return m;
}

/** The meshes of one bone's geometry, under `parent`. Pushes the robot's owned meshes and pupils. */
function addBoneGeo(parent: THREE.Object3D, geo: BoneGeo, mats: RobotMats, owned: THREE.Mesh[], pupils: THREE.Mesh[]): void {
  if (geo.matte) {
    const m = meshFor(geo.matte, mats.matte, true);
    parent.add(m);
    owned.push(m);
  }
  if (geo.hull) {
    const h = meshFor(geo.hull, inkMat(), false);
    h.raycast = () => {};
    h.userData.isOutline = true;
    parent.add(h);
    owned.push(h);
  }
  for (const s of geo.solo) {
    const m = meshFor(s.geometry, soloMaterial(s.key, mats, s.inline), s.key === 'eye' || s.key === 'light' ? false : true);
    parent.add(m);
    owned.push(m);
  }
  for (const p of geo.pupils) {
    const m = meshFor(p.geometry, mats.eye, false);
    m.position.set(0, p.y, 0);
    parent.add(m);
    owned.push(m);
    pupils.push(m);
  }
}

/**
 * A robot's instance: bones as groups (each at its `at`, under its parent), meshes from the baked
 * species, and the robot's own materials. `root` is the species group's root.
 */
export function instantiate(species: Species, geo: SpeciesGeo, mats: RobotMats, root: THREE.Group): RigInstance {
  const bones = new Map<string, THREE.Group>();
  const rest = new Map<string, Vec3>();
  const owned: THREE.Mesh[] = [];
  const pupils: THREE.Mesh[] = [];
  const make = (name: string): THREE.Group => {
    const hit = bones.get(name);
    if (hit) return hit;
    const def = species.bones[name]!;
    const g = new THREE.Group();
    g.name = name;
    g.position.set(...def.at);
    rest.set(name, [def.at[0], def.at[1], def.at[2]]);
    bones.set(name, g);
    (def.parent === 'root' ? root : make(def.parent)).add(g);
    return g;
  };
  for (const name of Object.keys(species.bones)) make(name);
  for (const [name, bg] of geo.bones) {
    if (name === 'root') addBoneGeo(root, bg, mats, owned, pupils);
    else addBoneGeo(bones.get(name)!, bg, mats, owned, pupils);
  }
  return {
    bones,
    rest,
    pupils,
    owned,
    attach(bone: string, bg: BoneGeo): THREE.Object3D {
      const holder = new THREE.Group();
      holder.name = `tool:${bone}`;
      bones.get(bone)!.add(holder);
      addBoneGeo(holder, bg, mats, owned, pupils);
      return holder;
    },
    dispose(): void {
      for (const m of owned) {
        m.removeFromParent();
        // Shared geometries are kept by their cache; the robot's own materials go with it.
      }
      mats.matte.dispose();
      mats.eye.dispose();
      mats.light.dispose();
      mats.thruster.dispose();
    },
  };
}

