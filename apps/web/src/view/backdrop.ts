// The backdrop: the picture behind the console's pages, drawn by the shader the desktop uses (crates/apz-theme/src/
// backdrop.wgsl). The build packs that shader for the browser: backdrop.wgsl for WebGPU, and backdrop.vert.glsl and
// backdrop.frag.glsl (naga's GLSL ES 3.00) for WebGL2. Without either, the same pattern is drawn with CSS gradients. The
// uniform block comes from the theme engine, so the picture is the engine's maths on the palette in force.

import '../styles/backdrop.css';
import { loadTheme, type BackdropPoint, type BackdropSize, type ThemeWasm } from '../theme/wasm';
import { BACKGROUNDS, BUILTIN_THEMES, DEFAULT_THEME_ID, TOKENS, current, tokenValue } from '../ui/theme';

/** The most frames a moving backdrop is drawn in a second (the shader's MAX_FPS). */
const MAX_FPS = 30;
const FRAME_MS = 1000 / MAX_FPS;
/** The uniform block: ten vec4s, 40 floats, 160 bytes. */
const UNIFORM_BYTES = 160;
/** The share of the device pixel ratio the picture is drawn at: it is soft, so half is enough. */
const RESOLUTION = 0.5;
/** Colour text a palette may hold: #rrggbb or #rrggbbaa. */
const HEX = /^#(?:[0-9a-f]{6}|[0-9a-f]{8})$/i;
/** WebGPU's buffer usage bits, written out so the build needs no WebGPU typings. */
const GPU_UNIFORM = 0x40;
const GPU_COPY_DST = 0x08;

/** The WebGPU objects are typed loosely: the TypeScript lib in this build has no WebGPU typings. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Gpu = any;

/** What the backdrop draws: the engine's background spec and the palette's colours by token name. */
interface Spec {
  bg: unknown;
  tokens: Record<string, string>;
}

/** One way of putting the picture on the canvas. */
interface Renderer {
  draw(uniforms: Float32Array): void;
  dispose(): void;
  /** Resolves when the GPU device is lost; WebGL reports its loss on the canvas instead. */
  readonly lost?: Promise<unknown>;
}

export interface Backdrop {
  /** Draws the given background spec on the given palette, until the next appearance change. */
  update(bg: unknown, tokens: Record<string, string>): void;
  /** Stops drawing, removes the canvas and frees the GPU objects. */
  stop(): void;
}

/** The backdrop running in each host: a host has at most one. */
const running = new WeakMap<HTMLElement, Backdrop>();

function makeCanvas(): HTMLCanvasElement {
  const canvas = document.createElement('canvas');
  canvas.className = 'apz-backdrop';
  canvas.setAttribute('aria-hidden', 'true');
  return canvas;
}

/** The colours of the palette in force, read from the root's tokens; only the colours that are plain hex. */
function paletteTokens(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const t of TOKENS) {
    const value = tokenValue(t.name);
    if (HEX.test(value)) out[t.name] = value;
  }
  return out;
}

/** The background spec and palette of the appearance in force; null while the palette is not applied yet. */
function derivedSpec(): Spec | null {
  const palette = document.documentElement.dataset.palette ?? DEFAULT_THEME_ID;
  const theme = BUILTIN_THEMES.find((t) => t.id === palette) ?? BUILTIN_THEMES.find((t) => t.id === DEFAULT_THEME_ID);
  const ref: unknown = current().appearance.background ?? theme?.background;
  let bg: unknown = ref;
  if (!(ref !== null && typeof ref === 'object' && 'pattern' in ref)) {
    // A preset id: the built-in set carries each preset's spec, so the entry itself is the spec.
    const id = typeof ref === 'string' ? ref : (ref as { id?: unknown } | null | undefined)?.id;
    bg = BACKGROUNDS.find((b) => b.id === id) ?? {};
  }
  const tokens = paletteTokens();
  if (!tokens.paper || !tokens.pattern || !tokens.signal) return null;
  return { bg, tokens };
}


/** WebGPU: the shader module, a uniform buffer and one draw of three vertices. Null when no adapter answers. */
async function createWebGPU(canvas: HTMLCanvasElement, code: string): Promise<Renderer | null> {
  const gpu: Gpu = (navigator as Navigator & { gpu?: Gpu }).gpu;
  if (!gpu) return null;
  const adapter: Gpu = await gpu.requestAdapter();
  if (!adapter) return null;
  const device: Gpu = await adapter.requestDevice();
  const context: Gpu = canvas.getContext('webgpu');
  if (!context) {
    device.destroy();
    return null;
  }
  const format: string = gpu.getPreferredCanvasFormat();
  context.configure({ device, format, alphaMode: 'opaque' });
  const module: Gpu = device.createShaderModule({ code });
  const pipeline: Gpu = device.createRenderPipeline({
    layout: 'auto',
    vertex: { module, entryPoint: 'vs_main' },
    fragment: { module, entryPoint: 'fs_main', targets: [{ format }] },
    primitive: { topology: 'triangle-list' },
  });
  const buffer: Gpu = device.createBuffer({ size: UNIFORM_BYTES, usage: GPU_UNIFORM | GPU_COPY_DST });
  const group: Gpu = device.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: [{ binding: 0, resource: { buffer } }],
  });
  return {
    lost: device.lost,
    draw(uniforms) {
      device.queue.writeBuffer(buffer, 0, uniforms);
      const encoder: Gpu = device.createCommandEncoder();
      const pass: Gpu = encoder.beginRenderPass({
        colorAttachments: [
          {
            view: context.getCurrentTexture().createView(),
            loadOp: 'clear',
            storeOp: 'store',
            clearValue: { r: 0, g: 0, b: 0, a: 1 },
          },
        ],
      });
      pass.setPipeline(pipeline);
      pass.setBindGroup(0, group);
      pass.draw(3);
      pass.end();
      device.queue.submit([encoder.finish()]);
    },
    dispose() {
      buffer.destroy();
      device.destroy();
    },
  };
}

function compile(gl: WebGL2RenderingContext, type: number, source: string): WebGLShader | null {
  const shader = gl.createShader(type);
  if (!shader) return null;
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (gl.getShaderParameter(shader, gl.COMPILE_STATUS)) return shader;
  console.warn('The backdrop shader does not compile.', gl.getShaderInfoLog(shader));
  gl.deleteShader(shader);
  return null;
}

function linkProgram(gl: WebGL2RenderingContext, vertSource: string, fragSource: string): WebGLProgram | null {
  const vs = compile(gl, gl.VERTEX_SHADER, vertSource);
  const fs = compile(gl, gl.FRAGMENT_SHADER, fragSource);
  if (!vs || !fs) return null;
  const program = gl.createProgram();
  if (!program) return null;
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);
  if (gl.getProgramParameter(program, gl.LINK_STATUS)) return program;
  console.warn('The backdrop program does not link.', gl.getProgramInfoLog(program));
  gl.deleteProgram(program);
  return null;
}

/**
 * WebGL2: the GLSL that naga wrote, one program, and the uniform block bound to binding point 0. The block's name is
 * the first `uniform` name in the fragment source (naga writes the block there). Null when the program does not build.
 */
function createWebGL2(canvas: HTMLCanvasElement, vertSource: string, fragSource: string): Renderer | null {
  const gl = canvas.getContext('webgl2', { alpha: false, antialias: false, depth: false, stencil: false });
  if (!gl) return null;
  const program = linkProgram(gl, vertSource, fragSource);
  if (!program) return null;
  const block = /uniform\s+(\w+)/.exec(fragSource)?.[1] ?? '';
  const index = gl.getUniformBlockIndex(program, block);
  if (index === gl.INVALID_INDEX) return null;
  gl.uniformBlockBinding(program, index, 0);
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.UNIFORM_BUFFER, buffer);
  gl.bufferData(gl.UNIFORM_BUFFER, UNIFORM_BYTES, gl.DYNAMIC_DRAW);
  gl.bindBufferBase(gl.UNIFORM_BUFFER, 0, buffer);
  // The triangle takes its corners from gl_VertexID, so the only state it needs is an empty vertex array.
  gl.bindVertexArray(gl.createVertexArray());
  return {
    draw(uniforms) {
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.useProgram(program);
      gl.bindBuffer(gl.UNIFORM_BUFFER, buffer);
      gl.bufferSubData(gl.UNIFORM_BUFFER, 0, uniforms);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    },
    dispose() {
      gl.getExtension('WEBGL_lose_context')?.loseContext();
    },
  };
}

async function fetchText(name: string): Promise<string | null> {
  try {
    const res = await fetch(name);
    return res.ok ? await res.text() : null;
  } catch {
    return null;
  }
}

/**
 * Starts the backdrop in `host` (the page's body): a fixed canvas behind the shell, drawn by WebGPU, else WebGL2, else
 * with CSS gradients. It draws at half the device pixel ratio, at most 30 frames a second, and only while the spec moves
 * or the pointer light is on; otherwise once after each change. It pauses while the page is hidden, and draws one still
 * frame when the user prefers reduced motion. It follows the appearance (the `apz:appearance` event) and the window size.
 */
export function startBackdrop(host: HTMLElement): Backdrop {
  const existing = running.get(host);
  if (existing !== undefined) return existing;
  const root = document.documentElement;
  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)');
  const t0 = performance.now();
  let canvas = makeCanvas();
  let engine: ThemeWasm | null = null;
  let renderer: Renderer | null = null;
  let spec: Spec | null = null;
  let pointer: BackdropPoint | null = null;
  let frame = 0;
  let dirty = true;
  let lastDraw = -Infinity;
  let stopped = false;
  let css = false;
  let scale = RESOLUTION;
  let glSource: { vert: string; frag: string } | null = null;

  function fit(): void {
    scale = RESOLUTION * (window.devicePixelRatio || 1);
    canvas.width = Math.max(1, Math.round(window.innerWidth * scale));
    canvas.height = Math.max(1, Math.round(window.innerHeight * scale));
  }

  function pointerLight(): boolean {
    return (spec?.bg as { pointer?: unknown } | null | undefined)?.pointer === true;
  }

  function moves(): boolean {
    return engine !== null && spec !== null && !reduced.matches && engine.backdropAnimates(spec.bg);
  }

  function schedule(): void {
    if (frame === 0 && !stopped && !document.hidden) frame = requestAnimationFrame(tick);
  }

  function paint(now: number): void {
    if (engine === null || renderer === null || spec === null) return;
    const time = reduced.matches ? 0 : (now - t0) / 1000;
    const size: BackdropSize = { w: canvas.width, h: canvas.height, scale, time };
    try {
      renderer.draw(engine.backdropUniforms(spec.bg, spec.tokens, size, pointer));
    } catch (err) {
      // A lost device or a refused spec: the page keeps the same pattern in CSS.
      console.warn('The backdrop could not be drawn; the pattern is drawn with CSS.', err);
      renderer.dispose();
      renderer = null;
      applyCss();
    }
  }

  function tick(now: number): void {
    frame = 0;
    if (stopped || document.hidden || renderer === null || spec === null) return;
    const animated = moves();
    if (!dirty && !animated) return;
    // Every draw is at least one frame apart, moving or not, so pointer moves coalesce into at most 30 draws a second.
    if (now - lastDraw < FRAME_MS) {
      schedule();
      return;
    }
    dirty = false;
    lastDraw = now;
    paint(now);
    if (animated) schedule();
  }

  // Without a GPU path the shell's ambient layer (ui/shell.ts, the prototype's CSS fields, pattern and grain) draws
  // the background: the canvas goes and data-backdrop is cleared so that layer shows.
  function applyCss(): void {
    css = true;
    canvas.remove();
    delete root.dataset.backdrop;
  }

  function clearCss(): void {
    css = false;
    root.dataset.backdrop = '';
  }

  /** The renderer is lost for good (its device or context): it is freed and the pattern is drawn with CSS. */
  function loseRenderer(r: Renderer): void {
    if (stopped || renderer !== r) return;
    renderer = null;
    r.dispose();
    console.warn('The backdrop lost its renderer; the pattern is drawn with CSS.');
    applyCss();
  }

  function watchLoss(r: Renderer): void {
    r.lost?.then(() => loseRenderer(r));
  }

  function onGlLost(e: Event): void {
    // Without preventDefault the browser never restores the context.
    e.preventDefault();
    if (renderer !== null) loseRenderer(renderer);
  }

  function onGlRestored(): void {
    if (stopped || renderer !== null || glSource === null) return;
    let r: Renderer | null = null;
    try {
      r = createWebGL2(canvas, glSource.vert, glSource.frag);
    } catch {
      r = null;
    }
    if (r === null) return;
    renderer = r;
    clearCss();
    if (!canvas.isConnected) host.append(canvas);
    fit();
    dirty = true;
    schedule();
  }

  function refresh(): void {
    if (css) applyCss();
    else {
      dirty = true;
      schedule();
    }
  }

  function setSpec(bg: unknown, tokens: Record<string, string>): void {
    spec = { bg, tokens };
    refresh();
  }

  function onAppearance(): void {
    const next = derivedSpec();
    if (next !== null) setSpec(next.bg, next.tokens);
  }

  function onResize(): void {
    fit();
    dirty = true;
    schedule();
  }

  function onPointer(e: PointerEvent): void {
    pointer = { x: e.clientX, y: e.clientY };
    if (pointerLight()) {
      dirty = true;
      schedule();
    }
  }

  function onPointerLeave(): void {
    pointer = null;
    if (pointerLight()) {
      dirty = true;
      schedule();
    }
  }

  function onVisibility(): void {
    if (document.hidden) {
      cancelAnimationFrame(frame);
      frame = 0;
    } else {
      dirty = true;
      schedule();
    }
  }

  function onMotion(): void {
    dirty = true;
    schedule();
  }

  async function init(): Promise<void> {
    try {
      engine = await loadTheme();
    } catch (err) {
      console.warn('The backdrop has no theme engine, so the pattern is drawn with CSS.', err);
    }
    if (stopped) return;
    if (engine !== null) {
      const [wgsl, vert, frag] = await Promise.all([
        fetchText('backdrop.wgsl'),
        fetchText('backdrop.vert.glsl'),
        fetchText('backdrop.frag.glsl'),
      ]);
      if (stopped) return;
      if (wgsl !== null) {
        try {
          renderer = await createWebGPU(canvas, wgsl);
        } catch {
          renderer = null;
        }
        // stop() may have run while the device was requested: free what was made and apply no CSS.
        if (stopped) {
          renderer?.dispose();
          renderer = null;
          return;
        }
        if (renderer !== null) watchLoss(renderer);
      }
      if (renderer === null && vert !== null && frag !== null) {
        // A WebGPU canvas cannot take a WebGL2 context, so WebGL2 gets a fresh canvas in the same place.
        const next = makeCanvas();
        canvas.replaceWith(next);
        canvas = next;
        glSource = { vert, frag };
        try {
          renderer = createWebGL2(canvas, vert, frag);
        } catch {
          renderer = null;
        }
        if (renderer !== null) {
          canvas.addEventListener('webglcontextlost', onGlLost);
          canvas.addEventListener('webglcontextrestored', onGlRestored);
        }
      }
    }
    fit();
    if (renderer === null) applyCss();
    if (spec === null) {
      const first = derivedSpec();
      if (first !== null) spec = first;
    }
    refresh();
  }

  host.append(canvas);
  root.dataset.backdrop = '';
  fit();
  window.addEventListener('resize', onResize);
  window.addEventListener('apz:appearance', onAppearance);
  window.addEventListener('pointermove', onPointer, { passive: true });
  window.addEventListener('blur', onPointerLeave);
  root.addEventListener('pointerleave', onPointerLeave);
  document.addEventListener('visibilitychange', onVisibility);
  reduced.addEventListener('change', onMotion);
  void init();

  const handle: Backdrop = {
    update(bg, tokens) {
      setSpec(bg, tokens);
    },
    stop() {
      if (stopped) return;
      stopped = true;
      if (running.get(host) === handle) running.delete(host);
      cancelAnimationFrame(frame);
      frame = 0;
      window.removeEventListener('resize', onResize);
      window.removeEventListener('apz:appearance', onAppearance);
      window.removeEventListener('pointermove', onPointer);
      window.removeEventListener('blur', onPointerLeave);
      root.removeEventListener('pointerleave', onPointerLeave);
      document.removeEventListener('visibilitychange', onVisibility);
      reduced.removeEventListener('change', onMotion);
      canvas.removeEventListener('webglcontextlost', onGlLost);
      canvas.removeEventListener('webglcontextrestored', onGlRestored);
      renderer?.dispose();
      renderer = null;
      canvas.remove();
      clearCss();
      delete root.dataset.backdrop;
    },
  };
  running.set(host, handle);
  return handle;
}
