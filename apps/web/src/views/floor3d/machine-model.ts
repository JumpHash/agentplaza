// Machines as 3D toon models: each kind of machine built from rounded boxes and cylinders in the
// plaza's palette, inked like the Floor, and photographed once in a small studio. It follows the
// agent portraits (portrait.ts): one offscreen renderer, a cached PNG data URL per picture, a key
// light and a shadow catcher. The backdrop is a plain cream card with a soft dotted floor.

import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import { outline, toonMat } from './toon';

export type MachineKind = 'server' | 'workstation' | 'edge' | 'apple';

type V = [number, number, number];

const W = 384;
const H = 288;
const CREAM = 0xf5efe0;
const SHEET = 0xfbf7ee;
const PAPER = 0xe8dfcb;
const INK = 0x2b2420;
const GLASS = 0x3d352d;
const SIGNAL = 0xd9482b;
const INFO = 0x2f6fb0;
const OK = 0x3fae73;
const OFF = 0xb3a993;

type Studio = { renderer: THREE.WebGLRenderer; scene: THREE.Scene; camera: THREE.PerspectiveCamera };
let studio: Studio | null | undefined;
const cache = new Map<string, string>();

/** A cream floor printed with soft ink dots: one tile of dots, repeated across the floor. */
function dottedFloor(): THREE.CanvasTexture {
  const cv = document.createElement('canvas');
  cv.width = cv.height = 64;
  const g = cv.getContext('2d')!;
  g.fillStyle = '#f5efe0';
  g.fillRect(0, 0, 64, 64);
  g.fillStyle = 'rgba(43, 36, 32, 0.22)';
  g.beginPath();
  g.arc(32, 32, 5, 0, Math.PI * 2);
  g.fill();
  const tex = new THREE.CanvasTexture(cv);
  tex.wrapS = THREE.RepeatWrapping;
  tex.wrapT = THREE.RepeatWrapping;
  tex.repeat.set(28, 28);
  tex.colorSpace = THREE.SRGBColorSpace;
  return tex;
}

function openStudio(): Studio | null {
  if (studio !== undefined) return studio;
  try {
    const canvas = document.createElement('canvas');
    if (!(canvas.getContext('webgl2') ?? canvas.getContext('webgl'))) return (studio = null);
    const renderer = new THREE.WebGLRenderer({ antialias: true, preserveDrawingBuffer: true });
    renderer.setPixelRatio(1);
    renderer.setSize(W, H, false);
    renderer.shadowMap.enabled = true;
    const scene = new THREE.Scene();
    scene.background = new THREE.Color(CREAM);
    scene.add(new THREE.HemisphereLight(0xfff8ec, 0x9c8f75, 1.4));
    const key = new THREE.DirectionalLight(0xfff1d6, 2.4);
    key.position.set(2.5, 4, 3);
    key.castShadow = true;
    key.shadow.mapSize.set(512, 512);
    scene.add(key);
    const floor = new THREE.Mesh(new THREE.PlaneGeometry(14, 14), new THREE.MeshBasicMaterial({ map: dottedFloor() }));
    floor.rotation.x = -Math.PI / 2;
    scene.add(floor);
    // The shadow catcher: a transparent floor just above the dots, so the model's shadow falls on them.
    const catcher = new THREE.Mesh(new THREE.PlaneGeometry(14, 14), new THREE.ShadowMaterial({ opacity: 0.2 }));
    catcher.rotation.x = -Math.PI / 2;
    catcher.position.y = 0.002;
    catcher.receiveShadow = true;
    scene.add(catcher);
    // A three-quarter view, a little above the machine.
    const camera = new THREE.PerspectiveCamera(30, W / H, 0.1, 50);
    camera.position.set(3.05, 1.9, 4.4);
    camera.lookAt(0.45, 0.8, 0);
    return (studio = { renderer, scene, camera });
  } catch {
    return (studio = null);
  }
}

function paint(color: number, glow = 0): THREE.MeshToonMaterial {
  return glow ? toonMat({ color, emissive: color, emissiveIntensity: glow }) : toonMat({ color });
}

/** A rounded box centred on `at`. */
function block(g: THREE.Group, size: V, radius: number, color: number, at: V, glow = 0): void {
  const m = new THREE.Mesh(new RoundedBoxGeometry(size[0], size[1], size[2], 3, radius), paint(color, glow));
  m.position.set(at[0], at[1], at[2]);
  g.add(m);
}

/** A cylinder centred on `at`, its axis along `axis`. */
function cylinder(g: THREE.Group, radius: number, length: number, color: number, at: V, axis: 'x' | 'y' | 'z', glow = 0): void {
  const m = new THREE.Mesh(new THREE.CylinderGeometry(radius, radius, length, 24), paint(color, glow));
  if (axis === 'x') m.rotation.z = Math.PI / 2;
  if (axis === 'z') m.rotation.x = Math.PI / 2;
  m.position.set(at[0], at[1], at[2]);
  g.add(m);
}

/** A ball: a lamp, an antenna tip. */
function ball(g: THREE.Group, radius: number, color: number, at: V, glow = 0): void {
  const m = new THREE.Mesh(new THREE.SphereGeometry(radius, 16, 12), paint(color, glow));
  m.position.set(at[0], at[1], at[2]);
  g.add(m);
}

/** An LED: lit green and glowing when `on`, a dull dot otherwise. */
function led(g: THREE.Group, at: V, on: boolean): void {
  ball(g, 0.028, on ? OK : OFF, at, on ? 1 : 0);
}

/** Tower case: a front panel with drive bays, a power button, a side window onto a GPU, and a monitor. */
function workstation(g: THREE.Group, gpu: boolean, active: boolean): void {
  block(g, [0.8, 1.5, 1.3], 0.08, SHEET, [0, 0.75, 0]);
  block(g, [0.62, 1.32, 0.03], 0.03, PAPER, [0, 0.76, 0.66]);
  block(g, [0.46, 0.1, 0.02], 0.008, INK, [0, 1.2, 0.69]);
  block(g, [0.46, 0.1, 0.02], 0.008, INK, [0, 1.05, 0.69]);
  for (const y of [0.5, 0.42, 0.34]) block(g, [0.36, 0.03, 0.02], 0.01, INK, [0, y, 0.69]);
  cylinder(g, 0.07, 0.04, active ? OK : OFF, [0.22, 0.2, 0.7], 'z', active ? 1 : 0);
  // Side window: a dark pane, with a GPU card behind it whose two fans glow faintly.
  block(g, [0.02, 0.95, 0.9], 0.01, GLASS, [0.41, 0.8, 0]);
  if (gpu) {
    block(g, [0.05, 0.3, 0.7], 0.012, SIGNAL, [0.45, 0.8, 0]);
    for (const z of [-0.18, 0.18]) cylinder(g, 0.11, 0.03, SIGNAL, [0.5, 0.8, z], 'x', 0.35);
  }
  // A small monitor beside the tower, its screen lit while the machine is busy.
  block(g, [0.95, 0.62, 0.07], 0.03, SHEET, [1.15, 0.95, 0]);
  block(g, [0.82, 0.5, 0.02], 0.01, active ? INFO : GLASS, [1.15, 0.95, 0.045], active ? 0.8 : 0);
  block(g, [0.12, 0.6, 0.05], 0.02, PAPER, [1.15, 0.335, -0.02]);
  block(g, [0.5, 0.03, 0.3], 0.012, SHEET, [1.15, 0.02, 0]);
}

/** A rack stack on a frame: three units with drive bays and LEDs, and a GPU card standing out of the top. */
function server(g: THREE.Group, gpu: boolean, active: boolean): void {
  block(g, [1.2, 0.06, 0.8], 0.02, INK, [0, 0.03, 0]);
  for (const x of [-0.52, 0.52]) for (const z of [-0.36, 0.36]) block(g, [0.06, 1.6, 0.06], 0.02, PAPER, [x, 0.83, z]);
  for (const y of [0.27, 0.63, 0.99]) {
    block(g, [0.9, 0.34, 0.7], 0.04, SHEET, [0, y, 0]);
    for (const x of [-0.3, -0.16, -0.02]) {
      block(g, [0.1, 0.2, 0.02], 0.008, PAPER, [x, y, 0.36]);
      block(g, [0.06, 0.02, 0.02], 0.004, INK, [x, y, 0.37]);
    }
    // The LEDs blink: the middle one is always dark.
    [0.22, 0.3, 0.38].forEach((x, i) => led(g, [x, y + 0.07, 0.37], active && i !== 1));
  }
  if (gpu) {
    block(g, [0.1, 0.5, 0.6], 0.02, SIGNAL, [0.1, 1.45, 0]);
    for (const z of [-0.15, 0.15]) cylinder(g, 0.12, 0.03, INK, [0.16, 1.45, z], 'x');
  }
}

/** A fanless box with fins down its flank, a front panel with LEDs and a port, and an antenna. */
function edge(g: THREE.Group, active: boolean): void {
  block(g, [0.9, 0.5, 0.7], 0.07, INFO, [0, 0.25, 0]);
  for (let i = 0; i < 6; i++) block(g, [0.07, 0.42, 0.025], 0.008, INFO, [0.48, 0.25, -0.27 + i * 0.11]);
  block(g, [0.5, 0.26, 0.02], 0.01, SHEET, [-0.1, 0.25, 0.36]);
  block(g, [0.12, 0.07, 0.02], 0.01, INK, [0.22, 0.25, 0.37]);
  led(g, [-0.3, 0.3, 0.37], active);
  led(g, [-0.22, 0.3, 0.37], active);
  cylinder(g, 0.022, 0.8, INK, [-0.28, 0.9, 0], 'y');
  ball(g, 0.07, SIGNAL, [-0.28, 1.36, 0]);
}

/** A squat rounded box with a front LED and a slot, like a Mac mini. */
function apple(g: THREE.Group, active: boolean): void {
  block(g, [0.95, 0.3, 0.95], 0.14, SHEET, [0, 0.15, 0]);
  block(g, [0.22, 0.02, 0.02], 0.008, INK, [-0.2, 0.15, 0.48]);
  led(g, [0.3, 0.15, 0.48], active);
}

function build(kind: MachineKind, gpu: boolean, active: boolean): THREE.Group {
  const g = new THREE.Group();
  if (kind === 'workstation') workstation(g, gpu, active);
  else if (kind === 'server') server(g, gpu, active);
  else if (kind === 'edge') edge(g, active);
  else apple(g, active);
  return g;
}

/** The machine's picture as a PNG data URL, or null where WebGL is unavailable. */
export function machineImage(kind: MachineKind, opts: { gpu: boolean; active: boolean }): string | null {
  const key = `${kind}|${opts.gpu}|${opts.active}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const st = openStudio();
  if (!st) return null;
  const model = build(kind, opts.gpu, opts.active);
  outline(model, 0.1);
  model.traverse((o) => {
    if (o.userData.isOutline) return;
    o.castShadow = true;
    o.receiveShadow = true;
  });
  st.scene.add(model);
  st.renderer.render(st.scene, st.camera);
  const url = st.renderer.domElement.toDataURL('image/png');
  st.scene.remove(model);
  model.traverse((o) => {
    const m = o as THREE.Mesh;
    if (!m.isMesh || m.userData.isOutline) return;
    m.geometry.dispose();
    (m.material as THREE.Material).dispose();
  });
  cache.set(key, url);
  return url;
}
