// Agent portraits: each agent's own 3D robot, posed in a little photo studio and photographed —
// toon-shaded and inked like on the Floor, on a night-sky starburst in the agent's colour. One small
// offscreen renderer takes every picture; pictures are cached as data URLs per agent and look.

import * as THREE from 'three';
import { makeRobot, ROBOT_PALETTE, speciesOf, type RobotPose, type RobotState } from './robot';

const SIZE = 384;
const POSES: RobotPose[] = ['wave', 'proud', 'think', 'cheer'];
const cache = new Map<string, string>();
let studio: { renderer: THREE.WebGLRenderer; scene: THREE.Scene; camera: THREE.PerspectiveCamera; backdrop: THREE.MeshBasicMaterial } | null | undefined;

function openStudio(): typeof studio {
  if (studio !== undefined) return studio;
  try {
    const canvas = document.createElement('canvas');
    if (!(canvas.getContext('webgl2') ?? canvas.getContext('webgl'))) return (studio = null);
    const renderer = new THREE.WebGLRenderer({ antialias: true, preserveDrawingBuffer: true });
    renderer.debug.checkShaderErrors = false; // the info log is read only when a shader fails
    renderer.setSize(SIZE, SIZE, false);
    renderer.shadowMap.enabled = true;
    const scene = new THREE.Scene();
    scene.add(new THREE.HemisphereLight(0xeaf4ff, 0x8a7a5a, 1.3));
    const key = new THREE.DirectionalLight(0xfff1d6, 2.6);
    key.position.set(2.5, 4, 3);
    key.castShadow = true;
    key.shadow.mapSize.set(512, 512);
    scene.add(key);
    // The backdrop: a big card behind the robot, painted per portrait.
    const backdrop = new THREE.MeshBasicMaterial({ toneMapped: false });
    const card = new THREE.Mesh(new THREE.PlaneGeometry(9, 9), backdrop);
    card.position.set(0, 1.2, -2.2);
    scene.add(card);
    // A floor disc that catches the robot's shadow.
    const floor = new THREE.Mesh(new THREE.CircleGeometry(1.4, 48), new THREE.ShadowMaterial({ opacity: 0.25 }));
    floor.rotation.x = -Math.PI / 2;
    floor.receiveShadow = true;
    scene.add(floor);
    const camera = new THREE.PerspectiveCamera(30, 1, 0.1, 50);
    camera.position.set(1.0, 1.25, 3.4);
    camera.lookAt(0, 0.72, 0);
    return (studio = { renderer, scene, camera, backdrop });
  } catch {
    return (studio = null);
  }
}

/** A starburst in the agent's colour on the night sky, drawn once per hue. */
const bursts = new Map<number, THREE.CanvasTexture>();
function sunburst(hue: number): THREE.CanvasTexture {
  const hit = bursts.get(hue);
  if (hit) return hit;
  const cv = document.createElement('canvas');
  cv.width = cv.height = 512;
  const g = cv.getContext('2d')!;
  g.fillStyle = `hsl(${hue} 45% 16%)`;
  g.fillRect(0, 0, 512, 512);
  g.fillStyle = `hsl(${hue} 55% 22%)`;
  const rays = 18;
  for (let i = 0; i < rays; i++) {
    const a0 = (i / rays) * Math.PI * 2;
    const a1 = a0 + Math.PI / rays;
    g.beginPath();
    g.moveTo(256, 300);
    g.lineTo(256 + Math.cos(a0) * 800, 300 + Math.sin(a0) * 800);
    g.lineTo(256 + Math.cos(a1) * 800, 300 + Math.sin(a1) * 800);
    g.closePath();
    g.fill();
  }
  const glow = g.createRadialGradient(256, 300, 10, 256, 300, 260);
  glow.addColorStop(0, `hsla(${hue}, 90%, 65%, 0.75)`);
  glow.addColorStop(1, `hsla(${hue}, 90%, 65%, 0)`);
  g.fillStyle = glow;
  g.fillRect(0, 0, 512, 512);
  // A sprinkle of stars.
  g.fillStyle = 'rgba(230,238,255,0.8)';
  for (let i = 0; i < 40; i++) {
    const x = (i * 197 + hue * 7) % 512;
    const y = (i * 131 + hue * 3) % 260;
    g.fillRect(x, y, i % 5 === 0 ? 3 : 2, i % 5 === 0 ? 3 : 2);
  }
  const tex = new THREE.CanvasTexture(cv);
  tex.colorSpace = THREE.SRGBColorSpace;
  bursts.set(hue, tex);
  return tex;
}

export interface PortraitSpec {
  cid: number;
  /** The district (role) hue, for the robot's front panel. */
  roleHue: number;
  /** The agent's own hue: its shell, and the backdrop. */
  bodyHue: number;
  orchestrator: boolean;
  state: RobotState;
  /** The agent record's species (`speciesOf`: the cid's deal when missing). */
  species?: string | null;
}

/** The agent's portrait as a PNG data URL, or null where WebGL is unavailable. */
export function portrait(spec: PortraitSpec): string | null {
  const key = `${spec.cid}|${spec.roleHue}|${Math.round(spec.bodyHue)}|${spec.orchestrator}|${spec.state === 'offline'}|${speciesOf(spec.species, spec.cid)}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const st = openStudio();
  if (!st) return null;
  const robot = makeRobot(spec.roleHue, spec.orchestrator, ROBOT_PALETTE, spec.bodyHue, speciesOf(spec.species, spec.cid));
  robot.setState(spec.state === 'offline' ? 'offline' : 'idle');
  robot.pose(POSES[spec.cid % POSES.length]!);
  robot.group.rotation.y = -0.35;
  robot.group.traverse((o) => (o.castShadow = true));
  if (spec.orchestrator) robot.group.scale.multiplyScalar(0.85); // fit the bigger frame
  st.backdrop.map = sunburst(Math.round(spec.bodyHue));
  st.backdrop.needsUpdate = true;
  st.scene.add(robot.group);
  st.renderer.render(st.scene, st.camera);
  const url = st.renderer.domElement.toDataURL('image/png');
  st.scene.remove(robot.group);
  robot.dispose();
  cache.set(key, url);
  return url;
}
