// The 3D views' performance meter: fps, CPU frame time and draw counts (renderer.info), an overlay
// shown with ?perf=1 in the hash query or localStorage.apzPerf = '1', window.__apzPerf() for headless
// runs, the adaptive resolution (the pixel ratio drops toward 1 while frames are slow and recovers when
// they are fast) and the render gate (a hidden tab or an offscreen canvas does not draw).

import * as THREE from 'three';

/** The most pixels a view draws per CSS pixel; the adaptive resolution moves between 1 and this. */
export const PIXEL_RATIO_CAP = 1.5;
/** Average frame time (ms) above which the resolution drops, and below which it recovers. */
const SLOW_MS = 18;
const FAST_MS = 10;
/** The shadow map is redrawn every this many frames (its casters move, so it is not frozen). */
export const SHADOW_EVERY = 3;

export interface PerfSnapshot {
  view: string;
  fps: number;
  frameMs: number;
  calls: number;
  triangles: number;
  geometries: number;
  textures: number;
  programs: number;
  pixelRatio: number;
  drawing: boolean;
}

/** The pixel ratio a view starts from: the device's, capped at `max` (the plaza's cap unless a view says otherwise). */
export function startPixelRatio(max: number = PIXEL_RATIO_CAP): number {
  return Math.min(window.devicePixelRatio || 1, max);
}

function perfRequested(): boolean {
  const q = window.location.hash.split('?')[1] ?? '';
  if (new URLSearchParams(q).get('perf') === '1') return true;
  try {
    return window.localStorage.getItem('apzPerf') === '1';
  } catch {
    return false;
  }
}

export interface Meter {
  /** True when this frame should run its updates and draw; false for a hidden tab or an offscreen canvas. */
  ready(): boolean;
  /**
   * Draws the scene, then records the frame's numbers and adapts the resolution. `shadows` says whether this frame
   * redraws the shadow map (a view that draws on change passes it); when omitted, the shadow map is redrawn every
   * SHADOW_EVERY frames.
   */
  render(scene: THREE.Scene, camera: THREE.Camera, shadows?: boolean): void;
  /** Stops the overlay and the watchers. */
  dispose(): void;
}

/** The snapshot of the view on screen (the last meter created and not disposed). */
let active: (() => PerfSnapshot) | null = null;

/** Sets window.__apzPerf(): the numbers of the view on screen, for headless runs. A no-op without a window (tests). */
function expose(): void {
  if (typeof window === 'undefined') return;
  (window as unknown as { __apzPerf: () => PerfSnapshot | null }).__apzPerf = () => (active ? active() : null);
}

/**
 * A meter for one renderer. `view` names it in the snapshot; `host` is the element the overlay goes in
 * (ignored unless the overlay is on).
 */
export function createMeter(renderer: THREE.WebGLRenderer, view: string, host: HTMLElement | null): Meter {
  renderer.shadowMap.autoUpdate = false; // drawn on a cadence by render() below
  // The view starts at its full ratio (already capped by startPixelRatio); the adaptive step never goes above it.
  let pixelRatio = renderer.getPixelRatio();
  const cap = pixelRatio;
  let frameStart = 0;
  let frameNo = 0;
  let ema = 0;
  let sinceChange = 0;
  let fpsFrames = 0;
  let fpsSince = 0;
  let fps = 0;
  let frameMs = 0;
  let drawing = true;
  let onScreen = true;
  let hidden = document.hidden;
  const onVisibility = (): void => {
    hidden = document.hidden;
  };
  document.addEventListener('visibilitychange', onVisibility);
  const canvas = renderer.domElement;
  const io =
    typeof IntersectionObserver === 'undefined'
      ? null
      : new IntersectionObserver((entries) => {
          onScreen = entries.some((e) => e.isIntersecting);
        });
  io?.observe(canvas);

  let overlay: HTMLElement | null = null;
  if (host && perfRequested()) {
    overlay = document.createElement('pre');
    overlay.className = 'perf-overlay';
    overlay.style.cssText =
      'position:absolute;left:8px;top:8px;margin:0;padding:6px 8px;z-index:5;pointer-events:none;' +
      'font:11px/1.35 ui-monospace,Menlo,Consolas,monospace;color:#e8eadf;background:rgba(10,12,28,0.72);border-radius:6px';
    host.appendChild(overlay);
  }
  let overlayAt = 0;

  const snapshot = (): PerfSnapshot => {
    const info = renderer.info;
    return {
      view,
      fps,
      frameMs: Math.round(frameMs * 100) / 100,
      calls: info.render.calls,
      triangles: info.render.triangles,
      geometries: info.memory.geometries,
      textures: info.memory.textures,
      programs: info.programs?.length ?? 0, // the linked programs (the headless ENTER check counts them)
      pixelRatio,
      drawing,
    };
  };
  active = snapshot;
  expose();

  return {
    ready(): boolean {
      frameStart = performance.now();
      drawing = !hidden && onScreen;
      return drawing;
    },
    render(scene, camera, shadows?: boolean): void {
      frameNo++;
      (window as unknown as { __apzScene?: THREE.Scene }).__apzScene = scene; // for headless inspection
      renderer.shadowMap.needsUpdate = renderer.shadowMap.enabled && (shadows ?? frameNo % SHADOW_EVERY === 0);
      renderer.render(scene, camera);
      const now = performance.now();
      const ms = now - frameStart;
      frameMs = frameMs === 0 ? ms : frameMs + (ms - frameMs) * 0.1;
      // Adaptive resolution: one step at a time, at most once a second, from the average frame time.
      ema = ema === 0 ? ms : ema + (ms - ema) * 0.05;
      sinceChange++;
      if (sinceChange > 60) {
        const next =
          ema > SLOW_MS && pixelRatio > 1 ? Math.max(1, pixelRatio - 0.25) : ema < FAST_MS && pixelRatio < cap ? Math.min(cap, pixelRatio + 0.25) : pixelRatio;
        if (next !== pixelRatio) {
          pixelRatio = next;
          renderer.setPixelRatio(pixelRatio); // keeps the drawing buffer's size from the last setSize
          sinceChange = 0;
          ema = 0;
        }
      }
      fpsFrames++;
      if (now - fpsSince >= 1000) {
        fps = Math.round((fpsFrames * 1000) / (now - fpsSince));
        fpsFrames = 0;
        fpsSince = now;
      }
      if (overlay && now - overlayAt > 500) {
        overlayAt = now;
        const s = snapshot();
        overlay.textContent =
          `${view}  ${s.fps} fps  ${s.frameMs.toFixed(1)} ms cpu\n` +
          `calls ${s.calls}  tris ${s.triangles}\n` +
          `geometries ${s.geometries}  textures ${s.textures}\n` +
          `pixel ratio ${s.pixelRatio}${s.drawing ? '' : '  (not drawing)'}`;
      }
    },
    dispose(): void {
      document.removeEventListener('visibilitychange', onVisibility);
      io?.disconnect();
      overlay?.remove();
      if (active === snapshot) active = null;
    },
  };
}
