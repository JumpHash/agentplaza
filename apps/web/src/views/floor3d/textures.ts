// Procedural cartoon textures for the Floor: each is drawn once on a 2D canvas, cached by key, and
// repeat-wrapped so a small tile covers a large surface. All are sRGB with 4x anisotropy; the
// materials that use them set their own colour and roughness.

import * as THREE from 'three';
import { toonMat } from './toon';

/** A seeded random source, so a texture drawn from the same key is always the same picture. */
export type Rand = () => number;

type Draw = (g: CanvasRenderingContext2D, w: number, h: number, rand: Rand) => void;

const textures = new Map<string, THREE.CanvasTexture>();
const materials = new Map<string, THREE.Material>();

/** mulberry32: a small, fast, seeded generator. */
export function rng(seed: number): Rand {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function seedOf(key: string): number {
  let h = 2166136261;
  for (let i = 0; i < key.length; i++) h = Math.imul(h ^ key.charCodeAt(i), 16777619);
  return h >>> 0;
}

/** A repeat-wrapped canvas texture, drawn once per key and cached. */
export function canvasTexture(key: string, w: number, h: number, repeat: number, draw: Draw): THREE.CanvasTexture {
  const hit = textures.get(key);
  if (hit) return hit;
  const cv = document.createElement('canvas');
  cv.width = w;
  cv.height = h;
  const g = cv.getContext('2d');
  if (g) draw(g, w, h, rng(seedOf(key)));
  const tex = new THREE.CanvasTexture(cv);
  tex.wrapS = THREE.RepeatWrapping;
  tex.wrapT = THREE.RepeatWrapping;
  tex.repeat.set(repeat, repeat);
  tex.colorSpace = THREE.SRGBColorSpace;
  tex.anisotropy = 4;
  textures.set(key, tex);
  return tex;
}

const css = (c: THREE.Color): string => c.getStyle();

/** Rounded rectangle path. */
function rrect(g: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
  g.beginPath();
  g.moveTo(x + r, y);
  g.arcTo(x + w, y, x + w, y + h, r);
  g.arcTo(x + w, y + h, x, y + h, r);
  g.arcTo(x, y + h, x, y, r);
  g.arcTo(x, y, x + w, y, r);
  g.closePath();
}

/** Grass: a flat cartoon meadow — soft lighter patches and little inked tufts. */
export function grass(): THREE.CanvasTexture {
  return canvasTexture('grass', 512, 512, 24, (g, w, h, r) => {
    g.fillStyle = '#86c45e';
    g.fillRect(0, 0, w, h);
    // Big soft patches in two close greens.
    for (let i = 0; i < 14; i++) {
      g.fillStyle = r() < 0.5 ? '#8fcb66' : '#7fbc58';
      g.beginPath();
      g.ellipse(r() * w, r() * h, 30 + r() * 60, 20 + r() * 40, r() * Math.PI, 0, Math.PI * 2);
      g.fill();
    }
    // Tufts: three-stroke "v"s, the classic cartoon grass mark.
    g.lineCap = 'round';
    for (let i = 0; i < 90; i++) {
      const x = r() * w;
      const y = r() * h;
      const s = 4 + r() * 5;
      g.strokeStyle = r() < 0.7 ? 'rgba(70,128,52,0.55)' : 'rgba(196,232,150,0.7)';
      g.lineWidth = 1.6;
      g.beginPath();
      g.moveTo(x - s * 0.6, y - s);
      g.lineTo(x, y);
      g.lineTo(x + s * 0.1, y - s * 1.3);
      g.moveTo(x, y);
      g.lineTo(x + s * 0.7, y - s * 0.9);
      g.stroke();
    }
    // A few tiny flowers.
    for (let i = 0; i < 12; i++) {
      g.fillStyle = ['#fff6d8', '#ffd25e', '#ff9db5'][Math.floor(r() * 3)]!;
      g.beginPath();
      g.arc(r() * w, r() * h, 2 + r() * 1.5, 0, Math.PI * 2);
      g.fill();
    }
  });
}

/** Cobblestones: rounded stones, each with a highlight and an inked edge, set in dark grout. */
export function paving(tint: THREE.Color, repeat = 4): THREE.CanvasTexture {
  return canvasTexture(`paving:${css(tint)}:${repeat}`, 256, 256, repeat, (g, w, h, r) => {
    const rows = 5;
    const rowH = h / rows;
    const stoneW = w / 4;
    g.fillStyle = css(tint.clone().multiplyScalar(0.55));
    g.fillRect(0, 0, w, h);
    for (let row = 0; row < rows; row++) {
      const shift = (row % 2) * (stoneW / 2);
      for (let col = -1; col <= 4; col++) {
        const x = col * stoneW + shift + 3 + r() * 2;
        const y = row * rowH + 3 + r() * 2;
        const sw = stoneW - 7;
        const sh = rowH - 7;
        rrect(g, x, y, sw, sh, 9);
        g.fillStyle = css(tint.clone().multiplyScalar(0.92 + r() * 0.14));
        g.fill();
        g.lineWidth = 2;
        g.strokeStyle = 'rgba(43,36,32,0.55)';
        g.stroke();
        rrect(g, x + 5, y + 4, sw * 0.45, 5, 2.5);
        g.fillStyle = 'rgba(255,255,255,0.45)';
        g.fill();
      }
    }
  });
}

/** A district's floor: big glazed tiles in its role hue, a highlight on each and inked grout. */
export function tiles(hue: number, repeat = 3): THREE.CanvasTexture {
  return canvasTexture(`tiles:${Math.round(hue)}:${repeat}`, 256, 256, repeat, (g, w, h, r) => {
    const grout = new THREE.Color().setHSL(hue / 360, 0.28, 0.66, THREE.SRGBColorSpace);
    const base = new THREE.Color().setHSL(hue / 360, 0.5, 0.84, THREE.SRGBColorSpace);
    g.fillStyle = css(grout);
    g.fillRect(0, 0, w, h);
    const n = 4;
    const t = w / n;
    for (let j = 0; j < n; j++) {
      for (let i = 0; i < n; i++) {
        const alt = (i + j) % 2 === 0 ? 1 : 0.9;
        rrect(g, i * t + 3, j * t + 3, t - 6, t - 6, 6);
        g.fillStyle = css(base.clone().multiplyScalar(alt * (0.97 + r() * 0.06)));
        g.fill();
        g.beginPath();
        g.moveTo(i * t + 10, j * t + 14);
        g.lineTo(i * t + 22, j * t + 9);
        g.strokeStyle = 'rgba(255,255,255,0.6)';
        g.lineWidth = 3;
        g.lineCap = 'round';
        g.stroke();
      }
    }
  });
}

/** Cartoon planks: warm, flat bands with inked seams and a few knots. */
export function wood(): THREE.CanvasTexture {
  return canvasTexture('wood', 256, 256, 1, (g, w, h, r) => {
    const planks = 4;
    const ph = h / planks;
    for (let p = 0; p < planks; p++) {
      g.fillStyle = `hsl(${28 + r() * 6}, ${55 + r() * 10}%, ${50 + r() * 8}%)`;
      g.fillRect(0, p * ph, w, ph);
      g.fillStyle = 'rgba(255,255,255,0.18)';
      g.fillRect(0, p * ph + 4, w, 5);
      for (let k = 0; k < 2; k++) {
        g.strokeStyle = 'rgba(90,50,20,0.45)';
        g.lineWidth = 2;
        g.beginPath();
        g.ellipse(r() * w, p * ph + ph / 2, 7 + r() * 5, 3, 0, 0, Math.PI * 2);
        g.stroke();
      }
      g.fillStyle = 'rgba(60,32,14,0.8)';
      g.fillRect(0, p * ph, w, 3);
    }
  });
}

/** Painted metal: a flat cool grey with one bright stripe. */
export function metal(): THREE.CanvasTexture {
  return canvasTexture('metal', 64, 64, 1, (g, w, h) => {
    g.fillStyle = '#8f9bab';
    g.fillRect(0, 0, w, h);
    g.fillStyle = 'rgba(255,255,255,0.35)';
    g.fillRect(0, h * 0.2, w, h * 0.12);
  });
}

/** Polished stone: the tint, a little warmer, with a few soft cartoon sparkles. */
export function polish(tint: THREE.Color): THREE.CanvasTexture {
  return canvasTexture(`polish:${css(tint)}`, 256, 256, 2, (g, w, h, r) => {
    g.fillStyle = css(tint);
    g.fillRect(0, 0, w, h);
    for (let i = 0; i < 14; i++) {
      g.fillStyle = 'rgba(255,255,255,0.35)';
      rrect(g, r() * w, r() * h, 18 + r() * 30, 5, 2.5);
      g.fill();
    }
    for (let i = 0; i < 10; i++) {
      g.fillStyle = 'rgba(120,100,80,0.12)';
      g.beginPath();
      g.arc(r() * w, r() * h, 8 + r() * 14, 0, Math.PI * 2);
      g.fill();
    }
  });
}

/** Water: flat cartoon teal with white wave squiggles. Scrolled over time by the fountain. */
export function water(): THREE.CanvasTexture {
  return canvasTexture('water', 256, 256, 2, (g, w, h, r) => {
    g.fillStyle = '#3cc0d6';
    g.fillRect(0, 0, w, h);
    g.fillStyle = '#5fd3e3';
    for (let i = 0; i < 10; i++) {
      g.beginPath();
      g.ellipse(r() * w, r() * h, 30 + r() * 30, 12 + r() * 10, 0, 0, Math.PI * 2);
      g.fill();
    }
    g.lineCap = 'round';
    for (let i = 0; i < 26; i++) {
      const x = r() * w;
      const y = r() * h;
      g.strokeStyle = 'rgba(255,255,255,0.85)';
      g.lineWidth = 3;
      g.beginPath();
      g.moveTo(x, y);
      g.quadraticCurveTo(x + 8, y - 6, x + 16, y);
      g.quadraticCurveTo(x + 24, y + 6, x + 32, y);
      g.stroke();
    }
  });
}

/** A slab's sides: a darker band of its district hue, so the tiled top stands out. */
export function slabSide(hue: number): THREE.Material {
  const key = `side:${Math.round(hue)}`;
  const hit = materials.get(key);
  if (hit) return hit;
  const m = toonMat({ color: new THREE.Color().setHSL(hue / 360, 0.35, 0.66, THREE.SRGBColorSpace) });
  materials.set(key, m);
  return m;
}
