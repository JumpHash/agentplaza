import { machineImage } from '../views/floor3d/machine-model';
// Small SVG illustrations of the plaza's machines: what each one is.
// Built with createElementNS only (no innerHTML). Line art: strokes follow
// currentColor, fills come from the civic-register tokens on :root.

const NS = 'http://www.w3.org/2000/svg';

export type DeviceKind = 'server' | 'workstation' | 'edge' | 'apple';

export interface DeviceOpts {
  /** Draw a small GPU card on the device. */
  gpu: boolean;
  /** VMs are running: LEDs lit/blinking. */
  active: boolean;
}

type Attrs = Record<string, string | number>;

function add(parent: Element, tag: string, attrs: Attrs = {}): SVGElement {
  const n = document.createElementNS(NS, tag);
  for (const k of Object.keys(attrs)) n.setAttribute(k, String(attrs[k]));
  parent.appendChild(n);
  return n as SVGElement;
}

/** A filled rectangle; the outline inherits the root stroke. */
function box(
  parent: Element,
  x: number,
  y: number,
  w: number,
  h: number,
  fill = 'var(--sheet)',
  extra: Attrs = {},
): SVGElement {
  return add(parent, 'rect', { x, y, width: w, height: h, fill, ...extra });
}

/** A row of LED dots; when active they glow --ok and the first one blinks. */
function leds(parent: Element, pts: [number, number][], active: boolean): void {
  pts.forEach(([cx, cy], i) => {
    const led = add(parent, 'circle', {
      cx,
      cy,
      r: 1.6,
      fill: active ? 'var(--ok)' : 'var(--paper)',
      'stroke-width': 1,
    });
    if (active && i === 0) {
      add(led, 'animate', {
        attributeName: 'opacity',
        values: '1;0.15;1',
        dur: '1.6s',
        repeatCount: 'indefinite',
      });
    }
  });
}

/** 2U rack chassis: two front bays with drive slots, vents and an LED row. */
function drawServer(p: Element, active: boolean): number {
  box(p, 8, 18, 68, 44);
  box(p, 13, 23, 40, 14, 'var(--paper)');
  for (const x of [17, 21, 25]) add(p, 'line', { x1: x, y1: 27, x2: x, y2: 33 });
  box(p, 13, 41, 40, 15, 'var(--paper)');
  for (const y of [45, 48, 51, 54]) {
    add(p, 'line', { x1: 17, y1: y, x2: 49, y2: y, 'stroke-width': 1 });
    add(p, 'line', { x1: 58, y1: y, x2: 73, y2: y, 'stroke-width': 1 });
  }
  leds(p, [[58, 30], [63, 30], [68, 30], [73, 30]], active);
  return 76;
}

/** Tower: front panel with drive bay, power button, LEDs, side vents. */
function drawWorkstation(p: Element, active: boolean): number {
  box(p, 26, 8, 42, 76);
  add(p, 'polygon', { points: '68,8 76,3 76,79 68,84', fill: 'var(--paper)' });
  for (const y of [30, 36, 42]) {
    add(p, 'line', { x1: 70, y1: y - 1.25, x2: 75, y2: y - 4.4, 'stroke-width': 1 });
  }
  box(p, 32, 14, 30, 18, 'var(--paper)');
  add(p, 'line', { x1: 36, y1: 22, x2: 58, y2: 22, 'stroke-width': 1 });
  add(p, 'line', { x1: 36, y1: 26, x2: 58, y2: 26, 'stroke-width': 1 });
  for (const y of [70, 73, 76, 79]) {
    add(p, 'line', { x1: 34, y1: y, x2: 62, y2: y, 'stroke-width': 1 });
  }
  add(p, 'circle', { cx: 52, cy: 60, r: 3.5, fill: 'var(--paper)' });
  leds(p, [[36, 60], [42, 60]], active);
  return 76;
}

/** Single-board computer: green board, CPU die, GPIO header, USB and ethernet. */
function drawEdge(p: Element, active: boolean): number {
  const green = '#5a9a78';
  box(p, 10, 30, 70, 40, 'var(--sheet)', { stroke: green });
  const holes: [number, number][] = [[14, 34], [76, 34], [14, 66], [76, 66]];
  holes.forEach(([cx, cy]) => {
    add(p, 'circle', { cx, cy, r: 1.5, stroke: green, 'stroke-width': 1 });
  });
  box(p, 30, 40, 20, 20, 'var(--paper)');
  add(p, 'rect', { x: 36, y: 46, width: 8, height: 8, 'stroke-width': 1 });
  for (let i = 0; i < 10; i++) {
    const cx = 52 + i * 2.2;
    add(p, 'circle', { cx, cy: 34, r: 0.9, 'stroke-width': 0.8 });
    add(p, 'circle', { cx, cy: 37, r: 0.9, 'stroke-width': 0.8 });
  }
  box(p, 69, 48, 11, 9, 'var(--paper)');
  box(p, 69, 58, 11, 9, 'var(--paper)');
  box(p, 54, 60, 13, 10, 'var(--paper)');
  leds(p, [[18, 62], [23, 62]], active);
  return 80;
}

/** Flat rounded box with a front light and a top highlight line. */
function drawApple(p: Element, active: boolean): number {
  box(p, 10, 30, 66, 30, 'var(--sheet)', { rx: 8 });
  add(p, 'line', { x1: 18, y1: 35, x2: 66, y2: 35, 'stroke-width': 1 });
  for (const y of [50, 53]) {
    add(p, 'line', { x1: 20, y1: y, x2: 50, y2: y, 'stroke-width': 1 });
  }
  leds(p, [[66, 45]], active);
  return 76;
}

const DRAW: Record<DeviceKind, (p: Element, active: boolean) => number> = {
  server: drawServer,
  workstation: drawWorkstation,
  edge: drawEdge,
  apple: drawApple,
};

/** A small GPU card beside the device, riser line joining it at y 46. */
function drawGpu(p: Element, riserX: number): void {
  const g = add(p, 'g');
  add(g, 'line', { x1: riserX, y1: 46, x2: 86, y2: 46 });
  box(g, 86, 36, 28, 24);
  add(g, 'circle', { cx: 98, cy: 46, r: 6 });
  add(g, 'circle', { cx: 98, cy: 46, r: 1.5 });
  box(g, 90, 56, 20, 3, 'var(--gold, #c9a227)', { stroke: 'none' });
}

/** An SVG drawing (viewBox 0 0 120 90) of a machine of `kind`. */
export function deviceSvg(kind: DeviceKind, opts: DeviceOpts): SVGSVGElement {
  const svg = document.createElementNS(NS, 'svg') as SVGSVGElement;
  svg.setAttribute('viewBox', '0 0 120 90');
  svg.setAttribute('class', 'device');
  svg.setAttribute('role', 'img');
  svg.setAttribute('aria-label', kind);
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', '1.5');
  const riserX = DRAW[kind](svg, opts.active);
  if (opts.gpu) drawGpu(svg, riserX);
  return svg;
}

/** The machine's picture on the Machines screen: a 3D toon model, or the line drawing without WebGL. */
export function deviceVisual(kind: DeviceKind, opts: DeviceOpts): HTMLElement | SVGSVGElement {
  const picture = machineImage(kind, opts);
  if (!picture) return deviceSvg(kind, opts);
  const img = document.createElement('img');
  img.className = 'device-model';
  img.src = picture;
  img.alt = kind;
  img.style.width = '220px';
  img.style.height = 'auto';
  img.style.display = 'block';
  return img;
}
