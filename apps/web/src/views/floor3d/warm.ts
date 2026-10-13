// The plaza's budgeted work and its caches. A plaza queues its heavy builds (the exterior, the board, the nav grid)
// as generators; the frame loop runs them a few milliseconds at a time, so no frame carries a whole build. Built
// board models and layouts are cached, so a board that comes back is not rebuilt. The generators here yield between
// their steps; the sync wrappers (buildModel, layoutOnBoard, navScene) stay for the lab and the tests.

import * as THREE from 'three';
import type { Agent } from '../../api';
import { buildModelSteps, type Built, type Model, type Params } from './hardware';
import { layoutOnBoard, navBoardOf, traceGraph, type BoardLayout } from './board-layout';
import { ROOM_CATALOG, roomLayout } from './rooms';
import { buildGridSteps } from './nav-grid';
import { plateList, roomPlan, terraceId, type NavScene, type RoomPlan } from './nav-scene';
import type { NavBoard, NavTerrace } from './nav-types';

const textWidths = new Map<string, number>();
/** A label's width in the context's current font, measured once per font and string (a plaque's text repeats across builds). */
export function textWidth(g: CanvasRenderingContext2D, text: string): number {
  const key = `${g.font}|${text}`;
  let w = textWidths.get(key);
  if (w === undefined) {
    w = g.measureText(text).width;
    textWidths.set(key, w);
    if (textWidths.size > 512) textWidths.delete(textWidths.keys().next().value!);
  }
  return w;
}

/** Frees a tree's geometry and materials (the plaza's own, never the shared ones of a pooled build: see releaseBuild). */
export function disposeTree(obj: THREE.Object3D): void {
  obj.traverse((o) => {
    const mesh = o as THREE.Mesh;
    mesh.geometry?.dispose();
    const mats = Array.isArray(mesh.material) ? mesh.material : mesh.material ? [mesh.material] : [];
    for (const m of mats) {
      // Every texture the material holds (colour, normal, emissive, roughness, gradient, ...), not only its colour map:
      // a texture left behind keeps its GPU memory. A shared one is simply uploaded again on its next use.
      for (const v of Object.values(m)) if (v instanceof THREE.Texture) v.dispose();
      m.dispose();
    }
  });
}

type Job = Generator<void, void, void>;

/**
 * A plaza's named jobs, run in order. `run` spends a frame's budget on them: each job goes on until its next yield,
 * and the budget is checked after each yield. A name replaces its queued job (the newest build wins).
 */
export class JobQueue {
  private jobs: { name: string; gen: Job }[] = [];
  private completed = 0; // jobs run to their end (the loading card's progress)

  get busy(): boolean {
    return this.jobs.length > 0;
  }

  /** The share of the queued work done, 0..1: the jobs finished over the jobs finished or still queued. */
  get progress(): number {
    const total = this.completed + this.jobs.length;
    return total === 0 ? 1 : this.completed / total;
  }

  has(name: string): boolean {
    return this.jobs.some((j) => j.name === name);
  }

  replace(name: string, gen: Job): void {
    this.jobs = this.jobs.filter((j) => j.name !== name);
    this.jobs.push({ name, gen });
  }

  cancel(name: string): void {
    this.jobs = this.jobs.filter((j) => j.name !== name);
  }

  clear(): void {
    this.jobs = [];
  }

  /** Runs jobs until the budget (ms) is spent. */
  run(budgetMs: number): void {
    const until = performance.now() + budgetMs;
    while (this.jobs.length > 0) {
      const head = this.jobs[0]!;
      const step = head.gen.next();
      // A job may replace itself or others while it runs: only the head that is still first is removed.
      if (step.done && this.jobs[0] === head) {
        this.jobs.shift();
        this.completed++;
      }
      if (performance.now() >= until) break;
    }
  }

  /** Runs one named job to its end now: for a moment that cannot wait for frames (the ENTER handoff). */
  finish(name: string): void {
    const job = this.jobs.find((j) => j.name === name);
    if (!job) return;
    while (!job.gen.next().done) {
      // run to the end
    }
    this.jobs = this.jobs.filter((j) => j !== job);
    this.completed++;
  }
}

/** The nav grid and room plans of a board's layout, a room plan per step (the same values as navScene in nav-scene.ts). */
export function* navSteps(board: Model, params: Params, layout: BoardLayout, machineId: string | null): Generator<void, NavScene, void> {
  const graph = traceGraph(board);
  // As navScene (nav-scene.ts): the board at the layout's world scale.
  const navBoard: NavBoard = navBoardOf(board, params, layout.pcbTop, plateList(board, layout), layout.boardScale);
  const plans: RoomPlan[] = [];
  const terraces: NavTerrace[] = [];
  for (let i = 0; i < layout.districts.length; i++) {
    const d = layout.districts[i]!;
    const plan = roomPlan(d, i, machineId, layout, graph);
    plans.push(plan);
    terraces.push({
      id: terraceId(i),
      cx: d.cx,
      cz: d.cy,
      half: Math.min(d.w, d.d) / 2,
      hw: d.w / 2,
      hd: d.d / 2,
      round: d.shape === 'round',
      y: d.y,
      turn: plan.turn,
      w: plan.w,
      d: plan.d,
      door: plan.door,
      layout: roomLayout(d.role, { w: plan.w, d: plan.d, door: plan.door, seed: plan.seed, agents: plan.agents }, ROOM_CATALOG),
      catalog: ROOM_CATALOG,
    });
    yield;
  }
  yield;
  // The grid, a chunk a step (buildGridSteps yields per chunk; its value is the grid).
  const grid = yield* stepsOf(buildGridSteps(navBoard, terraces));
  return { grid, terraces, plans };
}

/** Passes a generator's yields through as steps, and returns its value. */
function* stepsOf<T>(gen: Generator<number, T, void>): Generator<void, T, void> {
  for (;;) {
    const s = gen.next();
    if (s.done) return s.value;
    yield;
  }
}

const LAYOUTS_MAX = 6;
const layouts = new Map<string, BoardLayout>();

/**
 * The board's layout for a roster, cached by board and roster (sorted cid:tier:role). The layout reads only those
 * fields, so an equal roster lays out the same board the same way.
 */
function layoutKey(boardKey: string, list: Agent[]): string {
  return `${boardKey}|${list.map((a) => `${a.cid}:${a.tier}:${a.role ?? ''}`).sort().join(',')}`;
}

/** The cached layout of a board and roster, or null: a miss costs nothing here (layoutFor computes it, in a job). */
export function layoutCached(boardKey: string, list: Agent[]): BoardLayout | null {
  return layouts.get(layoutKey(boardKey, list)) ?? null;
}

export function layoutFor(boardKey: string, list: Agent[], board: Model, params: Params): BoardLayout {
  const key = layoutKey(boardKey, list);
  const hit = layouts.get(key);
  if (hit) return hit;
  const laid = layoutOnBoard(list, board, params);
  layouts.set(key, laid);
  if (layouts.size > LAYOUTS_MAX) layouts.delete(layouts.keys().next().value!);
  return laid;
}

const POOL_MAX = 3;
/** Built board models not in use, oldest first. A pooled model is out of every scene; its geometry stays until evicted. */
const pool: { key: string; built: Built }[] = [];

/** A board's built model: from the pool when one is idle for the key, else built a part at a time (buildModelSteps). */
export function* takeBuild(key: string, model: Model, params: Params): Generator<void, Built, void> {
  for (let i = pool.length - 1; i >= 0; i--) {
    if (pool[i]!.key === key) return pool.splice(i, 1)[0]!.built;
  }
  return yield* buildModelSteps(model, params);
}

/** Returns a built model to the pool once no scene shows it; the pool disposes its oldest past POOL_MAX. */
export function releaseBuild(key: string, built: Built): void {
  built.group.removeFromParent();
  pool.push({ key, built });
  while (pool.length > POOL_MAX) disposeTree(pool.shift()!.built.group);
}

/**
 * The shaders of a scene, a material a step: each program is linked in its own step (renderer.compile is synchronous
 * here), so no frame carries them all. A step compiles one material's first mesh against the scene (its lights and fog);
 * the scene is not changed, so a frame may draw between steps.
 */
export function* compileSteps(renderer: THREE.WebGLRenderer, scene: THREE.Scene, camera: THREE.Camera, extra: THREE.Mesh[] = []): Generator<void, void, void> {
  // One representative per program: a material's program also depends on its geometry (uv, colour, skinning, morphs),
  // so a material shared by meshes of different geometry is compiled once for each combination.
  const reps = new Map<string, THREE.Mesh>();
  const add = (m: THREE.Mesh): void => {
    if (!m.isMesh || Array.isArray(m.material)) return;
    const attrs = Object.keys(m.geometry.attributes).sort().join(',');
    const morphs = Object.keys(m.geometry.morphAttributes ?? {}).length;
    const key = `${m.material.uuid}|${attrs}|${(m as THREE.SkinnedMesh).isSkinnedMesh ? 'skin' : ''}|${morphs}`;
    if (!reps.has(key)) reps.set(key, m);
  };
  scene.traverse((o) => add(o as THREE.Mesh));
  for (const m of extra) add(m); // materials the scene swaps in later (ENTER's ground): compiled here, not in the sequence
  for (const rep of reps.values()) {
    // compile(object, camera, scene): the object's program with the scene's lights and fog, the scene itself untouched.
    const shown = rep.visible;
    rep.visible = true;
    renderer.compile(rep, camera, scene);
    // compile builds the program, but the driver links it on first use (the first draw): reading its uniforms here
    // forces the link (getProgramInfoLog) now, in this step, not in the first frame that draws the material.
    const prog = (renderer as unknown as { properties: { get(m: THREE.Material): { currentProgram?: { getUniforms?: () => unknown } } } }).properties.get(rep.material as THREE.Material)?.currentProgram;
    prog?.getUniforms?.();
    rep.visible = shown;
    yield;
  }
  // The flight's own view: every hidden object shown at once (the shadow casters that a part hides until ENTER), one
  // render with the scissor empty, then restored: the depth programs link here, not when the flight shows the object.
  const hidden: THREE.Object3D[] = [];
  scene.traverse((o) => {
    if (!o.visible) hidden.push(o);
  });
  for (const o of hidden) o.visible = true;
  const scissorAll = renderer.getScissorTest();
  renderer.setScissorTest(true);
  renderer.setScissor(0, 0, 0, 0);
  renderer.shadowMap.needsUpdate = true; // the shadow maps draw now (their depth programs link here), not on the flight's cadence
  renderer.render(scene, camera);
  renderer.setScissorTest(scissorAll);
  for (const o of hidden) o.visible = false;
  yield;
  // The programs a material first draws with in the view (a shadow's depth, a sprite's): one top-level part rendered a
  // step, the others hidden for it, the scissor empty so nothing is painted (a live canvas stays as it is). The links fall
  // in these steps, not in the ENTER sequence.
  const parts = scene.children.filter((c) => !(c as THREE.Light).isLight);
  for (const part of parts) {
    const shown = parts.map((p) => p.visible);
    for (const p of parts) p.visible = p === part;
    const scissor = renderer.getScissorTest();
    renderer.setScissorTest(true);
    renderer.setScissor(0, 0, 0, 0); // nothing is painted: the draws only link their programs
    renderer.shadowMap.needsUpdate = true;
    renderer.render(scene, camera);
    renderer.setScissorTest(scissor); // a frame drawn between steps is whole
    parts.forEach((p, i) => {
      p.visible = shown[i]!;
    });
    yield;
  }
}
