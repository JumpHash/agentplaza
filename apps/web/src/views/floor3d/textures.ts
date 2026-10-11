// Procedural textures for the Floor: each is drawn once on a 2D canvas, cached by key, and
// repeat-wrapped so a small tile covers a large surface. All are sRGB with 4x anisotropy; the
// materials that use them set their own colour and roughness.

import * as THREE from 'three';

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

/** Grass: speckled greens with a few dry patches. The ground plane repeats it every 4 units. */
export function grass(): THREE.CanvasTexture {
  return canvasTexture('grass', 256, 256, 50, (g, w, h, r) => {
    g.fillStyle = '#7a9d5c';
    g.fillRect(0, 0, w, h);
    for (let i = 0; i < 2200; i++) {
      g.fillStyle = `hsla(${80 + r() * 50}, ${35 + r() * 25}%, ${30 + r() * 26}%, ${0.35 + r() * 0.4})`;
      g.fillRect(r() * w, r() * h, 1 + r() * 2.2, (1 + r() * 2.2) * (1 + r()));
    }
    for (let i = 0; i < 40; i++) {
      g.fillStyle = `hsla(${50 + r() * 20}, 40%, 55%, 0.25)`;
      g.beginPath();
      g.arc(r() * w, r() * h, 4 + r() * 10, 0, Math.PI * 2);
      g.fill();
    }
  });
}

/** Offset stone pavers with mortar lines and a little colour variation per stone. */
export function paving(tint: THREE.Color, repeat = 4): THREE.CanvasTexture {
  return canvasTexture(`paving:${css(tint)}:${repeat}`, 256, 256, repeat, (g, w, h, r) => {
    const rows = 4;
    const rowH = h / rows;
    const stoneW = w / 3;
    g.fillStyle = css(tint.clone().multiplyScalar(0.7));
    g.fillRect(0, 0, w, h);
    for (let row = 0; row < rows; row++) {
      const shift = (row % 2) * (stoneW / 2);
      for (let col = -1; col <= 3; col++) {
        g.fillStyle = css(tint.clone().multiplyScalar(0.88 + r() * 0.16));
        g.fillRect(col * stoneW + shift + 2, row * rowH + 2, stoneW - 4, rowH - 4);
      }
    }
    for (let i = 0; i < 160; i++) {
      g.fillStyle = `rgba(40,34,28,${0.05 + r() * 0.1})`;
      g.fillRect(r() * w, r() * h, 1 + r() * 2, 1 + r() * 2);
    }
  });
}

/** A district's floor tiles, tinted by its role hue, with a subtle grout. */
export function tiles(hue: number, repeat = 3): THREE.CanvasTexture {
  return canvasTexture(`tiles:${Math.round(hue)}:${repeat}`, 256, 256, repeat, (g, w, h, r) => {
    const grout = new THREE.Color().setHSL(hue / 360, 0.18, 0.74, THREE.SRGBColorSpace);
    const base = new THREE.Color().setHSL(hue / 360, 0.3, 0.86, THREE.SRGBColorSpace);
    g.fillStyle = css(grout);
    g.fillRect(0, 0, w, h);
    const n = 4;
    const t = w / n;
    for (let j = 0; j < n; j++) {
      for (let i = 0; i < n; i++) {
        g.fillStyle = css(base.clone().multiplyScalar(0.96 + r() * 0.08));
        g.fillRect(i * t + 2, j * t + 2, t - 4, t - 4);
      }
    }
  });
}

/** Warm wooden planks with grain and seams. */
export function wood(): THREE.CanvasTexture {
  return canvasTexture('wood', 256, 256, 1, (g, w, h, r) => {
    const planks = 4;
    const ph = h / planks;
    for (let p = 0; p < planks; p++) {
      g.fillStyle = `hsl(${26 + r() * 6}, ${40 + r() * 12}%, ${40 + r() * 10}%)`;
      g.fillRect(0, p * ph, w, ph);
      for (let k = 0; k < 14; k++) {
        const y = p * ph + r() * ph;
        g.strokeStyle = `hsla(26, 45%, 25%, ${0.15 + r() * 0.2})`;
        g.lineWidth = 1 + r();
        g.beginPath();
        g.moveTo(0, y);
        g.bezierCurveTo(w * 0.3, y + (r() - 0.5) * 6, w * 0.7, y + (r() - 0.5) * 6, w, y);
        g.stroke();
      }
      g.fillStyle = 'rgba(40,25,12,0.55)';
      g.fillRect(0, p * ph, w, 2);
    }
  });
}

/** Brushed metal: fine horizontal streaks over a cool grey. */
export function metal(): THREE.CanvasTexture {
  return canvasTexture('metal', 128, 128, 2, (g, w, h, r) => {
    g.fillStyle = '#9aa0a8';
    g.fillRect(0, 0, w, h);
    for (let i = 0; i < 260; i++) {
      g.strokeStyle = r() < 0.5 ? 'rgba(255,255,255,0.18)' : 'rgba(40,44,52,0.18)';
      const y = r() * h;
      g.beginPath();
      g.moveTo(0, y);
      g.lineTo(w, y + (r() - 0.5) * 1.5);
      g.stroke();
    }
  });
}

/** Polished stone: the tint with faint, soft veining. */
export function polish(tint: THREE.Color): THREE.CanvasTexture {
  return canvasTexture(`polish:${css(tint)}`, 256, 256, 2, (g, w, h, r) => {
    g.fillStyle = css(tint);
    g.fillRect(0, 0, w, h);
    for (let i = 0; i < 26; i++) {
      g.strokeStyle = `rgba(90,80,70,${0.05 + r() * 0.08})`;
      g.lineWidth = 0.6 + r() * 1.4;
      let x = r() * w;
      let y = r() * h;
      g.beginPath();
      g.moveTo(x, y);
      for (let k = 0; k < 4; k++) {
        x += (r() - 0.5) * 90;
        y += (r() - 0.5) * 60;
        g.lineTo(x, y);
      }
      g.stroke();
    }
  });
}

/** Water: a turquoise gradient with pale ripple streaks. Scrolled over time by the fountain. */
export function water(): THREE.CanvasTexture {
  return canvasTexture('water', 256, 256, 2, (g, w, h, r) => {
    const grad = g.createLinearGradient(0, 0, 0, h);
    grad.addColorStop(0, '#7fd3da');
    grad.addColorStop(1, '#2f8fa6');
    g.fillStyle = grad;
    g.fillRect(0, 0, w, h);
    for (let i = 0; i < 60; i++) {
      const x = r() * w;
      const y = r() * h;
      g.strokeStyle = `rgba(255,255,255,${0.12 + r() * 0.25})`;
      g.lineWidth = 1 + r() * 2;
      g.beginPath();
      g.moveTo(x, y);
      g.quadraticCurveTo(x + 12 + r() * 12, y - 4, x + 28 + r() * 20, y);
      g.stroke();
    }
  });
}

/** A slab's sides: a darker band of its district hue, so the tiled top stands out. */
export function slabSide(hue: number): THREE.Material {
  const key = `side:${Math.round(hue)}`;
  const hit = materials.get(key);
  if (hit) return hit;
  const m = new THREE.MeshStandardMaterial({
    color: new THREE.Color().setHSL(hue / 360, 0.25, 0.68, THREE.SRGBColorSpace),
    roughness: 0.95,
  });
  materials.set(key, m);
  return m;
}
