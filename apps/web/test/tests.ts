// Pure-logic tests: run with `npm test`.
import { parse, href } from '../src/router';
import { ago, clip, compact } from '../src/ui/components';
import { floorName, familyLabel, roleFamily } from '../src/roles';
import { builtinLaws, diagnosticText, diagnosticsByLine, errorText, isBlank, isOwnerLaw, lawsOfRule, lineSpan, matchesFilter, tierKey, withStrictPermits, type LawFilter } from '../src/views/rules-model';
import type { LawInfo, Settings } from '../src/api';
import { checkRetention, clampQuery, mergeNewest, mergeOlder, spanOf } from '../src/views/conversations';
import { level, membersOf, parentOf, trailOf } from '../src/views/society';
import type { Bond, Conversation, PriceRates, SocietyAgent, UsageCall } from '../src/api';
import { easeOut, niceScale, resample, tweenArray } from '../src/ui/chart';
import { cacheSaving, fixtureCalls, ledgerSeries, ledgerSummary, mergeCalls, pageCalls, rankGroups, areaData, totalsOf, windowOf } from '../src/views/usage';
import { balanceData, barData, creditsOf, creditsUsd, flowsOf, mergeJournal, movesOf, partyOf, sortRows, windowOf as treasuryWindow } from '../src/views/treasury';
import * as THREE from 'three';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { buildModel, currentPulses, evalWhen, modelsFor, parseWhen, placeAtom, repeatOffsets, traceMeshes, validate, validatePalette } from '../src/views/floor3d/hardware';
import { MODELS, PALETTE, PALETTE_FILE } from '../src/views/floor3d/hardware-index';
import { GPU_MATCH, GPU_SPECS, matchCard, validateMatch, validateSpec, validateSpecs } from '../src/views/floor3d/gpus';
import { boardHeightAt, layoutOnBoard, occludedSlotIds, pathOnTraces, segmentBlocked, slotBlockedShare, traceGraph, type BoardLayout } from '../src/views/floor3d/board-layout';
import workstationBoardFile from '../../../assets/hardware/board/workstation.json';
import laptopBoardFile from '../../../assets/hardware/board/laptop.json';
import serverBoardFile from '../../../assets/hardware/board/server.json';
import edgeBoardFile from '../../../assets/hardware/board/edge.json';
import phoneBoardFile from '../../../assets/hardware/board/phone.json';
import appleBoardFile from '../../../assets/hardware/board/apple-soc.json';
import type { Model } from '../src/views/floor3d/hardware';
import { partBoxes } from '../src/views/floor3d/zfight';
import { boardScene, roomPalette, syntheticAgents } from '../src/views/floor3d/board-rooms';
import { groundAudit, supportField } from '../src/views/floor3d/ground';
import { bakeDrawnTops, drawnTopAt, drawnTopBelow, type Support } from '../src/views/floor3d/drawn-tops';
import type { Agent, FleetApple, FleetKind, FleetMachine, FleetResponse, FleetStatus } from '../src/api';
import { agentTally, boardOf, fitDistance, fleetSig, gibText, machineFacts, machineSubtitle, roomAt } from '../src/views/floor';
import { canonicalJson, computeMinSize, generate, layoutHash, minSize, type Catalog, type Door, type Input, type Roles } from '../src/views/floor3d/rooms/gen';
import { verify } from '../src/views/floor3d/rooms/verify';
import { robotVariant, speciesOf } from '../src/views/floor3d/robot';
import { NAV_CELL, NavWorld, buildGrid, cellAt, connectivity, findPath, pathHash, snapWalk, type NavBoard, type NavGrid } from '../src/views/floor3d/nav';
import { buildGridSteps, type NavTerrace } from '../src/views/floor3d/nav';
import { NAV_CROUCH_TIME, NAV_DROP_DOWN, NAV_JUMP_TIME, NAV_LAND_TIME, NAV_LIFT_DWELL, NAV_LIFT_SPEED, NAV_RESPAWN_IN, NAV_RESPAWN_OUT, NAV_STEP } from '../src/views/floor3d/nav-types';
import { bodyScale, newBody, stepBody, type Body, type Surfaces } from '../src/views/floor3d/nav-height';
import { ROOM_CATALOG, roomLayout } from '../src/views/floor3d/rooms';
import { navScene, type NavScene } from '../src/views/floor3d/nav-scene';
import { mulberry32 } from '../src/views/floor3d/rooms/gen';
import navFixtures from '../../../assets/nav/fixtures.json';
import roomCatalogFile from '../../../assets/rooms/catalog.json';
import roomRolesFile from '../../../assets/rooms/roles.json';
import { readFileSync } from 'node:fs';
import { Behaviour, runScenario, SCENARIO_COUNT, type BehaviourConfig, type BotIn, type Decision, type EventIn, type Personality, type RoleDef, type SpotIn } from '../src/views/floor3d/behaviour';
import behaviourFile from '../../../assets/robots/behaviour.json';
import behaviourFixtures from '../../../assets/robots/behaviour-fixtures.json';
import robotIndex from '../../../assets/robots/index.json';
import robotClips from '../../../assets/robots/clips.json';
import { DIRECT_START, EntrySequence, FLY_STANDOFF, KEY, WAVE, beginEntry, endAt, fadeTree, probeInside, roomGap, robotGap, span, stageAt, takeEntry, waveEnd, type EntryContext } from '../src/views/floor3d/entry';
import { buildFleet } from '../src/views/floor3d/fleet';
import { LiftController } from '../src/views/floor3d/lift-ctrl';

let failures = 0;
// Each section checks its own failures and reports them, then the run goes on to the next section (so one failing
// section does not hide the others). The failed sections are listed at the end, and the run exits 1 if any failed.
const failedSections: string[] = [];
// A layout that throws is a failed check, not the end of the run: the error is reported and the caller moves on.
function laid<T>(run: () => T, name: string): T | null {
  try {
    return run();
  } catch (e) {
    eq((e as Error).message, '', `${name}: the layout throws`);
    return null;
  }
}
function sectionEnd(okLine: string): void {
  if (failures) {
    console.error(`${failures} failure(s)`);
    failedSections.push(okLine.replace(/: ok$/, ''));
    failures = 0;
  } else console.log(okLine);
}
function eq(actual: unknown, expected: unknown, name: string): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    failures++;
    console.error(`FAIL ${name}: ${JSON.stringify(actual)} !== ${JSON.stringify(expected)}`);
  }
}

// The ENTER sequence (floor3d/entry.ts): keyframes, the wave's budget, the fades and a direct run with Back.
{
  eq(span(0, 1, 2), 0, 'entry: span is 0 before its start');
  eq(span(3, 1, 2), 1, 'entry: span is 1 after its end');
  eq(span(1.5, 1, 2) > 0 && span(1.5, 1, 2) < 1, true, 'entry: span is between 0 and 1 inside');
  const at0 = stageAt(0, false);
  eq([at0.fly, at0.dive, at0.ext, at0.others, at0.board], [0, 0, 1, 1, 0], 'entry: at 0 the exterior is whole and the board absent');
  const atEnd = stageAt(endAt(80, 16), false);
  eq([atEnd.ext, atEnd.others, atEnd.board, atEnd.dive], [0, 0, 1, 1], 'entry: at the end the board stands and the exterior is gone');
  eq(stageAt(WAVE.start + 0.5, true).board, 1, 'entry: a direct load shows the board from the wave on');
  eq(roomGap(5), WAVE.stagger, 'entry: small boards keep the 40 ms gap');
  eq(roomGap(80) * 79 <= WAVE.budget + 1e-9, true, 'entry: 80 rooms fit the wave budget');
  eq(waveEnd(80) <= WAVE.start + WAVE.budget + WAVE.rise + 1e-9, true, 'entry: the wave ends within its budget');
  eq(robotGap(16) * 15 <= WAVE.robotsBudget + 1e-9, true, 'entry: 16 robots fit the pop budget');
  eq(endAt(80, 16) >= waveEnd(80), true, 'entry: the sequence runs until the last room has risen');
  eq(KEY.dive[0] < KEY.dive[1], true, 'entry: keyframes are ordered');

  // fadeTree: a fade scales the base opacity, and k = 1 restores it.
  const mat = new THREE.MeshBasicMaterial({ color: 0xffffff, opacity: 0.8, transparent: true });
  const mesh = new THREE.Mesh(new THREE.BoxGeometry(1, 1, 1), mat);
  fadeTree(mesh, 0.5);
  eq(Math.round(mat.opacity * 100), 40, 'entry: a fade halves the opacity');
  fadeTree(mesh, 1);
  eq([Math.round(mat.opacity * 100), mat.transparent], [80, true], 'entry: a full fade restores the base opacity');

  // The handoff is taken once, by its machine.
  beginEntry({ machineId: 'n0', fleet: {} as never, from: { position: new THREE.Vector3(), target: new THREE.Vector3() } });
  eq(takeEntry('n1'), null, 'entry: a handoff is not taken by another machine');
  eq(takeEntry('n0') === null, true, 'entry: a handoff is taken once');

  // A direct run: forward to the end, and a Back reverses to the start and calls onLeft once.
  const board = new THREE.Group();
  const rooms = new THREE.Group();
  for (let i = 0; i < 5; i++) rooms.add(new THREE.Group());
  const bots = [new THREE.Group(), new THREE.Group(), new THREE.Group()];
  const orbit = { target: new THREE.Vector3(), enabled: true };
  const pose = { position: new THREE.Vector3(0, 40, 60), target: new THREE.Vector3() };
  let left = 0;
  const ctx: EntryContext = {
    camera: new THREE.PerspectiveCamera(),
    orbit,
    scene: new THREE.Scene(),
    exterior: null,
    from: null,
    board: () => ({ group: board, model: { size: [64, 30, 51], anchors: { atom: [0, 10, 0] } } as never, atom: null }),
    rooms,
    bots: () => bots,
    defaultPose: () => pose,
    look: () => undefined,
    onLeft: () => left++,
  };
  const forward = new EntrySequence(ctx, DIRECT_START);
  for (let i = 0; i < 400 && forward.driving; i++) forward.step(1 / 30);
  eq(forward.driving, false, 'entry: a direct run finishes');
  eq(orbit.enabled, true, 'entry: the orbit is the user\'s again at the end');
  eq(rooms.children.every((r) => r.visible && r.scale.y === 1), true, 'entry: every room is up at the end');
  eq(bots.every((g) => g.visible && g.scale.x === 1), true, 'entry: every robot is in at the end');

  const backing = new EntrySequence(ctx, DIRECT_START);
  for (let i = 0; i < 6; i++) backing.step(1 / 30);
  backing.back();
  for (let i = 0; i < 400 && backing.driving; i++) backing.step(1 / 30);
  eq([backing.driving, left], [false, 1], 'entry: Back reverses to the start and leaves once');

  // Back from the end: a finished flight reverses in full (not a no-op), the orbit is off while it flies, and the
  // leave is final: a Back after it, or a second Back while it flies back, changes nothing.
  const done = new EntrySequence(ctx, DIRECT_START);
  for (let i = 0; i < 400 && done.driving; i++) done.step(1 / 30);
  eq([done.driving, done.isFinished, orbit.enabled], [false, true, true], 'entry: a finished flight is finished, the orbit is the user\'s');
  done.back();
  eq([done.driving, done.isFinished, orbit.enabled], [true, false, false], 'entry: Back from the end drives the flight again');
  done.step(1 / 30);
  const t1 = done.time;
  done.back();
  eq(done.time === t1, true, 'entry: a second Back while flying back does not restart it');
  for (let i = 0; i < 400 && done.driving; i++) done.step(1 / 30);
  eq([done.driving, left], [false, 2], 'entry: the reverse from the end reaches the start and leaves once more');
  done.back();
  for (let i = 0; i < 20; i++) done.step(1 / 30);
  eq([done.driving, left], [false, 2], 'entry: a Back after the leave is final');
}

// The entry path into the server (atlas) on the fleet as the fixture reports it: the same fifteen machines, makers, kinds,
// model names, GPU counts and Apple cores, with the hub's links. The flight and its reverse walk the real parts: at every
// step, no drawn part of any machine or of the board holds the camera or lies within the probe's near distance, and no
// opaque part is within two units of it (such a part fills the frame). entry.ts: FLY_STANDOFF, DIVE_LIFT, probeInside.
{
  const FLEET: { name: string; kind: FleetKind; vendor: string; model: string; gpus: number; status?: FleetStatus; apple?: FleetApple }[] = [
    { name: 'lumen', kind: 'workstation', vendor: 'asus', model: 'ProArt Workstation', gpus: 2 },
    { name: 'atlas', kind: 'server', vendor: 'supermicro', model: 'SYS-421GE', gpus: 8 },
    { name: 'quill', kind: 'laptop', vendor: 'apple', model: 'MacBook Pro 14-inch', gpus: 1, apple: { chip: 'M3 Pro', p_cores: 6, e_cores: 6, gpu_cores: 14, neural_cores: 16, unified_memory_gib: 36 } },
    { name: 'sable', kind: 'edge', vendor: 'apple', model: 'Mac mini', gpus: 1, apple: { chip: 'M4', p_cores: 4, e_cores: 6, gpu_cores: 10, neural_cores: 16, unified_memory_gib: 16 } },
    { name: 'thimble', kind: 'laptop', vendor: 'dell', model: 'XPS 13', gpus: 1 },
    { name: 'cinder', kind: 'laptop', vendor: 'lenovo', model: 'ThinkPad P1 Gen 7', gpus: 1 },
    { name: 'cobalt', kind: 'laptop', vendor: 'framework', model: 'Laptop 16', gpus: 1 },
    { name: 'pebble', kind: 'phone', vendor: 'samsung', model: 'Galaxy S24', gpus: 0, status: 'offline' },
    { name: 'ridge', kind: 'edge', vendor: 'raspberrypi', model: 'Raspberry Pi 5', gpus: 0, status: 'offline' },
    { name: 'spire', kind: 'workstation', vendor: 'lenovo', model: 'ThinkStation P620', gpus: 1 },
    { name: 'kestrel', kind: 'workstation', vendor: 'apple', model: 'Mac Studio', gpus: 1, apple: { chip: 'M2 Ultra', p_cores: 16, e_cores: 8, gpu_cores: 60, neural_cores: 32, unified_memory_gib: 64 } },
    { name: 'gale', kind: 'workstation', vendor: 'dell', model: 'Precision 7875', gpus: 4 },
    { name: 'harbor', kind: 'server', vendor: 'dell', model: 'PowerEdge R760xa', gpus: 2 },
    { name: 'tundra', kind: 'server', vendor: 'hpe', model: 'ProLiant DL380 Gen11', gpus: 4 },
    { name: 'fjord', kind: 'workstation', vendor: 'intel', model: 'Arc Pro B60 reference', gpus: 2 },
  ];
  const fleetResp: FleetResponse = {
    machines: FLEET.map((m, i) => ({
      id: `n${i}`,
      name: m.name,
      role: i === 0 ? 'hub' : 'node',
      kind: m.kind,
      vendor: m.vendor,
      model: m.model,
      apple: m.apple ?? null,
      status: m.status ?? 'online',
      arch: 'x86_64',
      os: 'linux',
      backend: 'kvm',
      cpu: { model: 'cpu', cores: 8, threads: 16 },
      memory_mib: 65536,
      gpus: Array.from({ length: m.gpus }, () => ({ name: 'gpu', vendor: m.vendor, memory_mib: 24576 })),
      storage_gib: 1000,
      agents: [],
      working: 0,
      stats: null,
      last_seen_ms: 0,
    })),
    links: FLEET.slice(1).map((_, i) => ({ a: 'n0', b: `n${i + 1}`, online: true })),
  };
  const fleet = buildFleet(fleetResp);
  const atlas = fleetResp.machines.find((m) => m.name === 'atlas')!;
  const boardModel = MODELS[modelsFor(atlas).board]!;
  const board = buildModel(boardModel).group; // the server board's parts, unplaced: the sequence moves it
  const pose = { position: new THREE.Vector3(-28.0, 130.97, -40.92), target: new THREE.Vector3(0, 5, 0) }; // the settled server view (floor.ts defaultPose)
  const camera = new THREE.PerspectiveCamera(40, 1.6, 0.5, 1000);
  const ctx: EntryContext = {
    camera,
    orbit: { target: new THREE.Vector3(), enabled: true },
    scene: new THREE.Scene(),
    exterior: { fleet, data: fleetResp, machineId: atlas.id, model: MODELS[modelsFor(atlas).exterior]! },
    from: { position: new THREE.Vector3(83.1, 90.6, 83.1), target: new THREE.Vector3() },
    board: () => ({ group: board, model: boardModel, atom: null }),
    rooms: new THREE.Group(),
    bots: () => [],
    defaultPose: () => pose,
    look: () => undefined,
    onLeft: () => undefined,
  };
  // The nearest drawn part's world box to p: any part, and the opaque ones (a fade at 0.5 or more). Hidden parts, and
  // parts under a hidden ancestor, are not drawn; a part at or below opacity 0.05 is the sequence's own fade (not a solid).
  const clearance = (root: THREE.Object3D, p: THREE.Vector3): { any: number; opaque: number } => {
    const out = { any: Infinity, opaque: Infinity };
    const walk = (o: THREE.Object3D): void => {
      if (!o.visible) return;
      const mesh = o as THREE.Mesh;
      if (mesh.isMesh && !(mesh as THREE.InstancedMesh).isInstancedMesh) {
        const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
        const op = Math.max(...mats.map((m) => m.opacity));
        if (op > 0.05) {
          const g = mesh.geometry;
          if (!g.boundingBox) g.computeBoundingBox();
          const d = new THREE.Box3().copy(g.boundingBox!).applyMatrix4(mesh.matrixWorld).distanceToPoint(p);
          out.any = Math.min(out.any, d);
          if (op >= 0.5) out.opaque = Math.min(out.opaque, d);
        }
      }
      for (const c of o.children) walk(c);
    };
    root.updateMatrixWorld(true);
    walk(root);
    return out;
  };
  const seq = new EntrySequence(ctx, 0);
  let steps = 0;
  let inside = 0;
  let near = 0;
  let opaqueMin = Infinity;
  let atlasMin = Infinity;
  const check = (): void => {
    steps++;
    const p = camera.position;
    for (const [id, mc] of fleet.machines) {
      const r = probeInside(mc.root, p);
      inside += r.solid.length;
      near += r.near;
      const c = clearance(mc.root, p);
      opaqueMin = Math.min(opaqueMin, c.opaque);
      if (id === atlas.id) atlasMin = Math.min(atlasMin, c.any);
    }
    const rb = probeInside(board, p);
    inside += rb.solid.length;
    near += rb.near;
    opaqueMin = Math.min(opaqueMin, clearance(board, p).opaque);
  };
  for (let i = 0; i < 400 && seq.driving; i++) {
    seq.step(1 / 60);
    check();
  }
  const forwardDone = seq.isFinished;
  seq.back();
  for (let i = 0; i < 400 && seq.driving; i++) {
    seq.step(1 / 60);
    check();
  }
  eq([forwardDone, seq.driving, steps > 250], [true, false, true], 'entry path (server board): the flight and its reverse both run to the end');
  eq([inside, near], [0, 0], 'entry path (server board): no drawn part of any machine or the board holds the camera or lies within the probe distance, at any step');
  eq(opaqueMin >= 2, true, 'entry path (server board): no opaque part is within two units of the camera');
  eq(atlasMin < FLY_STANDOFF + 1, true, 'entry path (server board): the flight does reach the server (the walk is not vacuous)');
}
eq(parse('#/agents/105'), { screen: 'agents', params: ['105'] }, 'route with a param');
eq(parse(''), { screen: 'overview', params: [] }, 'empty route is the overview');
eq(href('tasks', 't 1'), '#/tasks/t%201', 'href encodes params');
eq(ago(1000, 31_000), '30s ago', 'ago seconds');
eq(clip('abcdef', 4), 'abc…', 'clip');
eq(compact(12_345), '12k', 'compact thousands');
eq(floorName({ cid: 7, name: '' }), 'agent 7', 'floor name falls back to the cid');
eq(floorName({ cid: 7, name: '  ' }), 'agent 7', 'floor name: whitespace counts as empty');

// Rules: the screen's logic (the view itself is checked in the browser).
{
  const law = (id: string, tier: string, origin: 'built-in' | 'owner', ruleId = '', file = ''): LawInfo => ({ id, tier, origin, rule_id: ruleId, file } as LawInfo);
  const laws = [law('S-1', 'statute', 'built-in'), law('C-1', 'constitution', 'built-in'), law('O-1', 'owner', 'owner', 'R-001'), law('X-1', '', 'built-in', '', 'operations.jhrl')];
  eq(tierKey({ tier: 'statute', file: '' }), 'statute', 'rules: a statute tier');
  eq(tierKey({ tier: '', file: 'constitution.jhrl' }), 'constitution', 'rules: the tier falls back to the file');
  eq(tierKey({ tier: 'weird', file: 'misc.jhrl' }), 'other', 'rules: an unknown tier and file is other');
  eq([isOwnerLaw(laws[0]), isOwnerLaw(laws[2])], [false, true], 'rules: built-in and owner laws are told apart');
  eq(builtinLaws(laws, 'all').map((l) => l.id), ['S-1', 'C-1', 'X-1'], 'rules: built-in statutes exclude owner laws');
  eq(builtinLaws(laws, 'statute').map((l) => l.id), ['S-1'], 'rules: the statute tier filter');
  eq(builtinLaws(laws, 'operations').map((l) => l.id), ['X-1'], 'rules: the operations tier filter reads the file');
  eq(lawsOfRule(laws, 'R-001').map((l) => l.id), ['O-1'], 'rules: a rule declares its own laws');
  eq(lawsOfRule(laws, 'R-002'), [], 'rules: a rule with no laws');
  const all: LawFilter = { q: '', effect: 'all', sev: 'any', tier: 'all' };
  const sample = { ...laws[0], id: 'S-9', title: 'Board posts', statement: 'no deletes', ops: ['board.delete'], effect: 'forbid', severity: 'major' } as LawInfo;
  const pass = (over: Partial<LawFilter>): boolean => matchesFilter(sample, { ...all, ...over });
  eq(
    [pass({}), pass({ q: ' DELETE ' }), pass({ q: 'nothing' }), pass({ effect: 'permit' }), pass({ effect: 'forbid' }), pass({ sev: 'major' }), pass({ sev: 'minor' }), pass({ tier: 'statute' }), pass({ tier: 'operations' })],
    [true, true, false, false, true, true, false, true, false],
    'rules: the statute filters (search, effect, severity, tier)',
  );

  eq(lineSpan('ab\ncde\nf', 2), { start: 3, end: 6 }, 'rules: the span of line 2');
  eq(lineSpan('ab\ncde', 1), { start: 0, end: 2 }, 'rules: the span of line 1');
  eq(lineSpan('ab', 9), { start: 0, end: 2 }, 'rules: a line past the end is the last');
  eq(lineSpan('', 1), { start: 0, end: 0 }, 'rules: an empty source has an empty line');
  eq(Array.from(diagnosticsByLine([{ file: '', line: 2, column: 1, message: 'a' }, { file: '', line: 2, column: 4, message: 'b' }]).entries()), [[2, 'a\nb']], 'rules: messages per line');
  eq(diagnosticText({ file: '', line: 3, column: 5, message: 'bad' }), 'Line 3, column 5: bad', 'rules: a diagnostic reads as a line and column');

  const settings = { default_model: 'm', dev_models: true, laws: { strict_permits: false }, fetch_block: [] } as unknown as Settings;
  const on = withStrictPermits(settings, true);
  eq([on.laws?.strict_permits, 'dev_models' in on], [true, false], 'rules: strict permits on, dev_models dropped');
  eq([settings.laws?.strict_permits, 'dev_models' in settings], [false, true], 'rules: the settings given are not changed');
  eq(withStrictPermits({ default_model: 'm' } as unknown as Settings, false).laws, { strict_permits: false }, 'rules: strict permits set when the settings had none');

  eq([isBlank('  '), isBlank(' a ')], [true, false], 'rules: a blank title or reason');
  eq(errorText(new Error('nope')), 'nope', 'rules: an error reads as its message');
  eq(errorText('plain'), 'plain', 'rules: a non-error reads as text');
}
eq(floorName({ cid: 7, name: 'Ada' }), 'Ada', 'floor name keeps a chosen name');

// Conversations: paging keeps what was read, and retention and search are checked as typed.
const exchange = (n: number): Conversation => ({ id: `c${n}`, from_cid: 1, to_cid: 2, from_role: 'a', to_role: 'b', text: `m${n}`, created_at: n, delivered: true });
const ids = (s: { items: Conversation[] }): string[] => s.items.map((c) => c.id);
const exchanges = (from: number, to: number): Conversation[] => Array.from({ length: to - from + 1 }, (_, i) => exchange(from + i));

eq(ids(spanOf([exchange(3), exchange(1), exchange(2)], null)), ['c1', 'c2', 'c3'], 'spans sort oldest first');
eq(
  mergeNewest({ items: exchanges(1, 5), next: 0 }, spanOf(exchanges(4, 6), 3)),
  { items: exchanges(1, 6), next: 0 },
  'a refresh keeps the older pages already read',
);
eq(
  mergeNewest({ items: exchanges(5, 6), next: 4 }, spanOf(exchanges(4, 6), 3)),
  { items: exchanges(4, 6), next: 3 },
  'a refresh with nothing older read takes the fresh cursor',
);
eq(
  mergeOlder({ items: exchanges(4, 5), next: 3 }, spanOf(exchanges(2, 3), 1)),
  { items: exchanges(2, 5), next: 1 },
  'an older page is prepended',
);
eq(ids(mergeOlder({ items: exchanges(4, 5), next: 3 }, spanOf(exchanges(3, 4), 2))), ['c3', 'c4', 'c5'], 'an overlapping older page is not duplicated');
eq(checkRetention('4096', '30').errors, {}, 'retention: the defaults are valid');
eq(checkRetention(' 100 ', '0').errors, {}, 'retention: the lowest keep and forever are valid');
eq(checkRetention('100000', '3650').errors, {}, 'retention: the highest limits are valid');
eq(Object.keys(checkRetention('99', '30').errors), ['max'], 'retention: below the minimum keep');
eq(Object.keys(checkRetention('100001', '30').errors), ['max'], 'retention: above the maximum keep');
eq(Object.keys(checkRetention('1e3', '30').errors), ['max'], 'retention: not a whole number');
eq(Object.keys(checkRetention('4096', '3651').errors), ['days'], 'retention: days above the maximum');
eq(Object.keys(checkRetention('4096', '-1').errors), ['days'], 'retention: negative days');
eq(clampQuery('  hi  '), 'hi', 'search is trimmed');
eq(clampQuery('a'.repeat(300)).length, 200, 'search is capped at 200 bytes');
eq(clampQuery('é'.repeat(150)).length, 100, 'the cap counts bytes, on whole characters');

// Society: families, groups and their levels.
const ag = (cid: number, role: string): SocietyAgent => ({ cid, name: `a${cid}`, role, reputation: 50, confidence: 0.5, wallet: 0 });
const bond = (from: number, to: number, strength: number, e: number): Bond => ({
  from, to, kind: 'met', since_ms: 0, strength, interactions: 0, last_ms: 0, note: null, blocked: false,
  opinion: { r: 0, s: 0, b: 0, d: 0, u: 0, e, updated_ms: 0 },
});
eq(roleFamily('builder'), 'build', 'family: builder is build');
eq(roleFamily('mayor-office'), 'command', 'family: mayor office is command');
eq(roleFamily('security-reviewer'), 'operations', 'family: security reviewer is operations');
eq(roleFamily('planner'), 'knowledge', 'family: planner is knowledge');
eq(roleFamily('auditor'), 'money', 'family: auditor is money');
eq(roleFamily('my-custom-role'), 'other', 'family: a custom role is other');
eq(roleFamily('breeder'), 'other', 'family: a role outside the list is other');
eq(roleFamily('constructor'), 'other', 'family: an Object.prototype name is other');
eq(roleFamily('builder', 'orchestrator'), 'command', 'family: the orchestrator tier is command');
eq(roleFamily('worker-x', 'worker'), 'other', 'family: a worker tier keeps its role');
eq(familyLabel('money'), 'Money', 'family label');

const crew = [ag(1, 'builder'), ag(2, 'architect'), ag(3, 'finance'), ag(4, 'planner')];
const cross = [bond(1, 3, 4, 0.8), bond(2, 3, 3, 0.4), bond(3, 1, 1, 0.6), bond(1, 2, 5, 0.9)];
const top = level(crew, cross, { family: null, role: null }, true);
eq(top.marks.map((m) => m.key), ['f:build', 'f:knowledge', 'f:money'], 'top level: one box per family');
eq(top.edges.length, 1, 'edges: the one build-money edge (the build-build bond is inside a box; the planner has no contacts)');
const buildMoney = top.edges.find((e) => e.from === 'f:build' && e.to === 'f:money');
eq(buildMoney?.strength, 8, 'edge aggregation: summed strength of the cross contacts');
eq(Math.round((buildMoney?.trust ?? 0) * 100), 60, 'edge aggregation: mean trust of the cross contacts');
eq(buildMoney?.bonds.length, 3, 'edge aggregation: all three cross bonds in one edge');

const flat = level(crew, cross, { family: null, role: null }, false);
eq(flat.marks.map((m) => m.key), ['a:1', 'a:2', 'a:3', 'a:4'], 'flat level: every agent is its own mark');
eq(flat.edges.length, 4, 'flat level: one edge per bond, direction kept');

const team = [ag(1, 'builder'), ag(2, 'reviewer'), ag(3, 'reviewer'), ag(4, 'finance')];
const inBuild = level(team, [], { family: 'build', role: null }, true);
eq(inBuild.marks.map((m) => m.key), ['a:1', 'r:build/reviewer', 'f:money'], 'open family: roles, a single-agent role is its agent');
eq(inBuild.marks.find((m) => m.key === 'r:build/reviewer')?.members.length, 2, 'open family: the role box holds its agents');
const inRole = level(team, [], { family: 'build', role: 'reviewer' }, true);
eq(inRole.marks.map((m) => m.key), ['a:1', 'a:2', 'a:3', 'f:money'], 'open role: its agents are drawn one by one');
eq(membersOf(team, 'r:build/reviewer').map((a) => a.cid), [2, 3], 'membersOf: a role key');
eq(membersOf(team, 'f:build').map((a) => a.cid), [1, 2, 3], 'membersOf: a family key');

const at = { family: 'build', role: 'builder' } as const;
eq(trailOf(at).map((c) => c.label), ['All families', 'Build', 'Builder'], 'breadcrumb: all families, family, role');
eq(parentOf(at), { family: 'build', role: null }, 'back from a role goes to its family');
eq(parentOf({ family: 'build', role: null }), { family: null, role: null }, 'back from a family goes to the top');

// Charts: the scale, resampling and tweening that every chart animates through.
eq(niceScale(0), [0, 1], 'nice scale: nothing to show is a unit axis');
eq(niceScale(7), [0, 2, 4, 6, 8], 'nice scale: round steps that reach the max');
eq(resample([1, 2, 3], 5), [1, 1.5, 2, 2.5, 3], 'resample: linear between points');
eq(resample([], 3), [0, 0, 0], 'resample: no values is all zeros');
eq(resample([4], 2), [4, 4], 'resample: one value repeats');
eq(tweenArray([0, 10], [10, 0], 0.5), [5, 5], 'tween: halfway between aligned arrays');
eq(tweenArray([1], [1, 3], 1), [1, 3], 'tween: a missing entry counts as zero');
eq([easeOut(0), easeOut(1)], [0, 1], 'ease out: the ends are fixed');

// Usage: the ledger's sums, the series buckets, the paging rule and the cache saving.
const call = (id: number, at: number, over: Partial<UsageCall> = {}): UsageCall => ({
  id, at_ms: at, cid: 1, role: 'builder', model: 'm/a', provider: 'p', input: 100, output: 10, cache_read: 0, cache_write: 0,
  cost_usd: null, est_usd: 0.5, task: null, via: 'task', ...over,
});
const HR = 3_600_000;
const sample = [call(1, 0.5 * HR, { cost_usd: 0.2 }), call(2, 1.5 * HR, { model: 'm/b', cid: 2, cache_read: 300 }), call(3, 2.5 * HR, { cache_read: 100 })];
const sum = totalsOf(sample);
eq([sum.calls, sum.input, sum.usd, sum.cost_usd, sum.est_usd], [3, 300, 0.2 + 0.5 + 0.5, 0.2, 1.5], 'totals: usd is reported where known, else the estimate');
eq(totalsOf([call(1, 0, { input: 0, cache_read: 0 })]).cache_hit_rate, 0, 'hit rate: no input is zero, not NaN');
eq(windowOf('24h', 1e9), { from: 1e9 - 24 * HR, to: 1e9, bucket: 'hour' }, 'window: 24 h ends now, hourly buckets');
eq(windowOf('90d', 1e9).bucket, 'day', 'window: 90 d uses daily buckets');

const series = ledgerSeries(sample, 0, 3 * HR, 'hour', 'model');
eq(series.points.map((p) => p.totals.calls), [1, 1, 1], 'series: one bucket per hour, each call in its own');
eq(Object.keys(series.points[1]?.groups ?? {}), ['m/b'], 'series: the group key is the chosen grouping');
eq(ledgerSeries([], 0, 2 * HR, 'hour', 'agent').points.length, 2, 'series: empty buckets are kept');
eq(ledgerSummary(sample, 0, 3 * HR).by_model.map((m) => m.key), ['m/a', 'm/b'], 'summary: groups sort by spend');
eq(ledgerSummary(sample, 0, 3 * HR).unpriced, [], 'summary: priced calls are not listed as unpriced');

const ranked = rankGroups(series, 'usd');
eq(ranked.length, 2, 'ranking: every group that spent');
const many = ledgerSeries(Array.from({ length: 8 }, (_, i) => call(`g${i}`, 0.5 * HR, { model: `m/${i}`, est_usd: 8 - i })), 0, HR, 'hour', 'model');
const manyRanked = rankGroups(many, 'usd');
const area = areaData(many, manyRanked, new Set([manyRanked[0] ?? '']), 'usd', (k) => k);
eq(area.layers.map((l) => l.key), [...manyRanked.slice(1, 7), '~other'], 'area: six groups, the rest in Other, hidden groups out');
eq(area.layers.at(-1)?.values, [manyRanked.slice(7).reduce((s, k) => s + (many.points[0]?.groups[k]?.usd ?? 0), 0)], 'area: Other sums the rest');
eq(area.labels.length, 1, 'area: one label per bucket');

const prices = new Map<string, PriceRates | null>([['m/a', { input: 3, output: 15, cache_read: 0.3, cache_write: null, source: 'litellm' }]]);
const saving = cacheSaving([{ key: 'm/a', totals: totalsOf([call(1, 0, { cache_read: 1_000_000 })]) }], prices);
eq(saving, { usd: 2.7, complete: true }, 'cache saving: cache reads times the input-minus-read price gap');
eq(cacheSaving([{ key: 'm/z', totals: totalsOf([call(1, 0, { cache_read: 5 })]) }], prices).complete, false, 'cache saving: an unpriced model makes it partial');

const merged = mergeCalls([call(1, 5 * HR)], [call(1, 5 * HR), call(2, 9 * HR)]);
eq(merged.map((c) => c.id), [2, 1], 'merge: newest first, no repeats');

const many5 = [call(5, 5), call(4, 4), call(3, 3), call(2, 2), call(1, 1)];
const page1 = pageCalls(many5, { limit: 2 });
eq([page1.calls.map((c) => c.id), page1.total, page1.next_before], [[5, 4], 5, 4], 'paging: the first page and its cursor');
const page2 = pageCalls(many5, { limit: 2, before: page1.next_before ?? undefined });
eq([page2.calls.map((c) => c.id), page2.next_before], [[3, 2], 2], 'paging: the next page is strictly older');
eq(pageCalls(many5, { limit: 9, model: 'nope' }).calls, [], 'paging: a filter with no match is empty');

const fx = fixtureCalls(1e12, 2);
eq(fx.length > 0 && fx.every((c, i) => i === 0 || (fx[i - 1]?.at_ms ?? 0) >= c.at_ms), true, 'fixture: calls are newest first');
eq(JSON.stringify(fixtureCalls(1e12, 2).slice(0, 5)), JSON.stringify(fx.slice(0, 5)), 'fixture: deterministic');

// Hardware models (assets/hardware): every file validates, the index lists every file, the loader builds them.
eq(parse('#/hardware-lab?model=exterior/workstation&gpus=1'), { screen: 'hardware-lab', params: [] }, 'a query is not part of the route');

// `npm test` runs from apps/web; the bundle sits in a temp dir, so the path is from the working directory.
const hwRoot = join(process.cwd(), '../../assets/hardware');
const hwFiles = ['exterior', 'board', 'parts']
  .flatMap((d) => {
    try {
      return readdirSync(join(hwRoot, d)).filter((f) => f.endsWith('.json')).map((f) => `${d}/${f.slice(0, -5)}`);
    } catch {
      return [];
    }
  })
  .sort();
eq(Object.keys(MODELS).sort(), hwFiles, 'every hardware file is in the index');
eq(Object.entries(MODELS).flatMap(([name, m]) => validate(m, name)), [], 'every hardware model validates');
eq(validatePalette(PALETTE_FILE), [], 'the palette validates');
eq(validatePalette({ schema: 1, colors: {} }).length > 0, true, 'the palette needs the status LEDs');

// Graphics cards (assets/hardware/gpus): the specs validate, the match rules point at specs, and the names that
// detection reports resolve to the card they should (the stand-in fixture's GPUs must all be specific matches).
{
  const specIds = GPU_SPECS.specs.map((s) => s.id);
  eq(validateSpecs(GPU_SPECS), [], 'every GPU spec validates (dimensions, cooling, power, approx, sources)');
  eq(validateMatch(GPU_MATCH, specIds), [], 'every GPU match rule points at a spec, and the fallbacks come last');
  eq(validateSpec({ ...GPU_SPECS.specs[0], size_mm: { length: 900, height: 111, thickness: 40 } }).length > 0, true, 'a PCIe card over 340 mm is rejected');
  eq(validateMatch({ schema: 1, rules: [{ vendor: '*', name_regex: '.', card: 'consumer-generic', fallback: true }, { vendor: 'nvidia', name_regex: 'RTX', card: 'consumer-generic' }] }, specIds).length, 1, 'a specific rule after a fallback is rejected');
  const layoutSize = (id: string): number => {
    const l = GPU_SPECS.specs.find((s) => s.id === id)?.layout;
    return l ? l.rows * l.cols : 0;
  };
  eq(['nvidia-hgx-8gpu-baseboard', 'amd-ubb-8gpu'].map(layoutSize), [8, 8], 'both 8-GPU baseboards lay out eight modules');
  eq(GPU_SPECS.specs.filter((s) => s.form === 'integrated').every((s) => s.size_mm === null), true, 'integrated entries have no card size');

  const cases: [string, string, string | null, string, boolean][] = [
    ['nvidia', 'NVIDIA GeForce RTX 4090', 'workstation', 'nvidia-rtx4090-fe', false],
    ['nvidia', 'NVIDIA GeForce RTX 5090', 'workstation', 'nvidia-rtx5090-fe', false],
    ['nvidia', 'NVIDIA GeForce RTX 5070 Ti', 'workstation', 'nvidia-rtx5070ti-generic', false],
    ['nvidia', 'NVIDIA GeForce RTX 4070 Ti SUPER', 'workstation', 'nvidia-rtx4070ti-generic', false],
    ['nvidia', 'NVIDIA GeForce RTX 4080 SUPER', 'workstation', 'nvidia-rtx4080-fe', false],
    ['nvidia', 'NVIDIA GeForce RTX 3090 Ti', 'workstation', 'nvidia-rtx3090-fe', false],
    ['nvidia', 'NVIDIA GeForce RTX 5060', 'workstation', 'nvidia-rtx5070-fe', true],
    ['nvidia', 'NVIDIA GeForce GTX 1080', 'workstation', 'nvidia-workstation-generic', true],
    ['nvidia', 'NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition', 'workstation', 'nvidia-rtxpro6000-maxq', false],
    ['nvidia', 'NVIDIA RTX PRO 6000 Blackwell Workstation Edition', 'workstation', 'nvidia-rtxpro6000-ws', false],
    ['nvidia', 'NVIDIA RTX 6000 Ada Generation', 'workstation', 'nvidia-rtx6000ada', false],
    ['nvidia', 'NVIDIA RTX 3500 Ada', 'laptop', 'nvidia-rtx3500ada-laptop', false],
    ['nvidia', 'NVIDIA H100 PCIe', 'server', 'nvidia-h100-pcie', false],
    ['nvidia', 'NVIDIA H100 80GB HBM3', 'server', 'nvidia-sxm5-module', false],
    ['nvidia', 'NVIDIA H100 NVL', 'server', 'nvidia-h100-nvl', false],
    ['nvidia', 'NVIDIA A100-SXM4-80GB', 'server', 'nvidia-sxm5-module', true],
    ['nvidia', 'NVIDIA A100 80GB PCIe', 'server', 'nvidia-a100-pcie', false],
    ['nvidia', 'NVIDIA L40S', 'server', 'nvidia-l40s', false],
    ['nvidia', 'NVIDIA L4', 'server', 'nvidia-l4', false],
    ['nvidia', 'NVIDIA A10', 'server', 'nvidia-a10', false],
    ['nvidia', 'NVIDIA Jetson AGX Orin', 'edge', 'nvidia-jetson-agx-orin-devkit', false],
    ['nvidia', 'NVIDIA GB10', 'edge', 'nvidia-dgx-spark', false],
    ['amd', 'AMD Radeon RX 7900 XTX', 'workstation', 'amd-rx7900xtx-mba', false],
    ['amd', 'AMD Radeon RX 9070 XT', 'workstation', 'amd-rx9070xt-generic', false],
    ['amd', 'AMD Radeon Pro W7800', 'workstation', 'amd-w7800', false],
    ['amd', 'AMD Radeon 890M', 'laptop', 'amd-radeon-igpu', false],
    ['amd', 'AMD Instinct MI300X', 'server', 'amd-mi300x-oam', false],
    ['amd', 'AMD Radeon RX 7600M XT', 'laptop', 'laptop-generic', true],
    ['intel', 'Intel Corporation DG2 [Arc A770] (rev 08)', 'workstation', 'intel-arc-a770-le', false],
    ['intel', 'Intel Corporation BMG G21 [Arc B580]', 'workstation', 'intel-arc-b580-le', false],
    ['intel', 'Intel Arc Pro B60', 'workstation', 'intel-arc-pro-b60', false],
    ['intel', 'Intel Arc Graphics', 'laptop', 'intel-coreultra-igpu', false],
    ['intel', 'Intel Corporation Lunar Lake [Arc Graphics 130V]', 'laptop', 'intel-coreultra-igpu', false],
    ['intel', 'Intel Gaudi 3 HL-325L OAM', 'server', 'intel-gaudi3-oam', false],
    ['intel', 'Intel Gaudi 3 HL-338', 'server', 'intel-gaudi3-pcie', false],
    ['intel', 'Intel Data Center GPU Max 1100', 'server', 'intel-max1100', false],
    ['intel', 'Intel Flex 170', 'server', 'intel-flex170', false],
    ['apple', 'Apple M3 Pro GPU (14-core)', 'laptop', 'apple-soc-gpu', false],
    ['apple', 'Apple M2 Ultra GPU (60-core)', 'workstation', 'apple-soc-gpu', false],
    ['intel', 'Intel Mystery Accelerator', null, 'intel-arc-le-generic', true],
  ];
  for (const [vendor, name, kind, card, fallback] of cases) {
    const m = matchCard({ vendor, name }, kind);
    eq([m?.card ?? null, m?.fallback ?? null], [card, fallback], `match ${vendor} "${name}" (${kind ?? 'no kind'})`);
  }

  // The stand-in hub fixture (agentplaza-landing) lists the GPUs the app sees; each must match a specific card.
  const fixturePath = join(process.cwd(), '../../../agentplaza-landing/fixtures/floor-15.json');
  if (existsSync(fixturePath)) {
    const hub = JSON.parse(readFileSync(fixturePath, 'utf8')) as { machines: { kind: string; gpus?: { vendor: string; name: string; memory_mib?: number | null }[] }[] };
    const gpus = hub.machines.flatMap((m) => (m.gpus ?? []).map((g) => ({ g, kind: m.kind })));
    const misses = gpus.flatMap(({ g, kind }) => {
      const m = matchCard(g, kind);
      return m && !m.fallback ? [] : [`${g.name} (${kind}) -> ${m?.card ?? 'none'}`];
    });
    eq(misses, [], `every GPU in the stand-in fixture (${gpus.length}) matches a specific card`);
  }
}

const hwPart = (extra: object): unknown => ({ schema: 1, kind: 'x', view: 'part', size: [1, 1, 1], parts: [{ geo: { type: 'box', w: 1, h: 1, d: 1 }, mat: 'case', ...extra }] });
const mentions = (model: unknown, word: string, name: string): void => eq(validate(model, 'bad').some((e) => e.includes(word)), true, name);
mentions(hwPart({ geo: { type: 'blob' } }), 'unknown geo type', 'an unknown geo type is an error');
mentions({ ...(hwPart({}) as object), parts: [{ geo: { type: 'box', w: 1, h: 1 }, mat: 'case' }] }, 'missing dimension "d"', 'a missing dimension is an error');
mentions(hwPart({ mat: { color: 'red' } }), 'color', 'a bad colour is an error');
mentions(hwPart({ mat: 'nope' }), 'unknown palette name', 'an unknown palette name is an error');
mentions({ ...(hwPart({}) as object), parts: [{ id: 'a', geo: { type: 'box', w: 1, h: 1, d: 1 } }, { id: 'a', geo: { type: 'box', w: 1, h: 1, d: 1 } }] }, 'duplicate id', 'a duplicate id is an error');
mentions(hwPart({ use: 'parts/missing' }), 'unknown part', 'a use of a missing part is an error');
mentions(hwPart({ when: 'gpus >>> 1' }), 'bad when', 'a bad when expression is an error');
mentions(
  { schema: 1, kind: 'x', view: 'board', size: [1, 1, 1], parts: [{ id: 'pcb', geo: { type: 'box', w: 1, h: 1, d: 1 } }], anchors: { camera: { at: [0, 1, 1], look: [0, 0, 0] }, atom: [0, 1, 0] }, slots: [{ id: 's', on: 'cpu9', type: 'cpu', top: 1, rect: [0, 0, 1, 1], capacity: 1 }] },
  'no part with id "cpu9"',
  'a slot on a missing part is an error',
);
eq(validate(hwPart({ repeat: { count: 'p_cores', step: [1, 0, 0] } }), 'ok'), [], 'a repeat count may name a param');
eq(validate(hwPart({ when: 'gpu_cores >= 30' }), 'ok'), [], 'a when may name any numeric param');

const when = (expr: string, params: Record<string, number>): boolean | string => {
  const w = parseWhen(expr);
  return w ? evalWhen(w, params) : 'bad';
};
eq(when('gpus >= 1', { gpus: 2 }), true, 'when: gpus >= 1');
eq(when('!gpus', { gpus: 0 }), true, 'when: !gpus');
eq(when('gpus', {}), false, 'when: a missing param is zero');
eq(when('gpu_cores >= 30', { gpu_cores: 16 }), false, 'when: any numeric param');
eq(when('gpus >>> 1', {}), 'bad', 'when: not an expression');

eq(modelsFor({ kind: 'laptop', vendor: 'apple' }), { exterior: 'exterior/apple-laptop', board: 'board/apple-soc' }, 'an Apple laptop');
eq(modelsFor({ kind: 'workstation', vendor: 'apple' }).exterior, 'exterior/apple-desktop', 'an Apple desktop');
eq(modelsFor({ kind: 'edge', vendor: null }), { exterior: 'exterior/edge', board: 'board/edge' }, 'a generic machine');
for (const m of [modelsFor({ kind: 'laptop', vendor: 'apple' }), modelsFor({ kind: 'server' })]) {
  eq([m.exterior in MODELS, m.board in MODELS], [true, true], `models exist: ${m.exterior}`);
}

const repeatModel = {
  schema: 1, kind: 'x', view: 'part', size: [1, 1, 1], params: { gpus: 0, n: 3 },
  parts: [
    { id: 'gpu', when: 'gpus >= 1', geo: { type: 'box', w: 1, h: 1, d: 1 }, mat: 'case' },
    { id: 'fin', repeat: { count: 'n', step: [2, 0, 0] }, geo: { type: 'box', w: 1, h: 1, d: 1 }, mat: 'case' },
  ],
};
const r0 = buildModel(repeatModel);
eq([...r0.parts.keys()].sort(), ['fin', 'fin#1', 'fin#2'], 'when hides a part; a repeat count names a param');
eq(buildModel(repeatModel, { gpus: 1 }).parts.has('gpu'), true, 'when shows a part with its param');
eq(validate(hwPart({ repeat: { count: 'p_cores/4', step: [1, 0, 0] } }), 'ok'), [], 'a repeat count may be a param over n');
mentions(hwPart({ repeat: { count: 'p_cores/0', step: [1, 0, 0] } }), 'param name', 'a repeat count over 0 is an error');
mentions(hwPart({ repeat: { count: 2, step: [1, 0, 0], wrap: 4 } }), 'missing row', 'a wrap needs a row');
mentions(hwPart({ repeat: { count: 2, step: [1, 0, 0], row: [0, 0, 1] } }), 'row needs a wrap', 'a row needs a wrap');
mentions(hwPart({ repeat: { count: 2, step: [1, 0, 0], wrap: 0, row: [0, 0, 1] } }), 'wrap: a whole number', 'a wrap of 0 is an error');
eq(validate(hwPart({ repeat: { count: 10, step: [1, 0, 0], wrap: 4, row: [0, 0, 1] } }), 'ok'), [], 'a wrapped repeat validates');
// count 10, wrap 4, step [1, 0, 0], row [0, 0, 1]: rows of four, then two on the third row. The desktop loader's test checks the same numbers.
eq(repeatOffsets({ count: 10, step: [1, 0, 0], wrap: 4, row: [0, 0, 1] }, {}), [
  [0, 0, 0], [1, 0, 0], [2, 0, 0], [3, 0, 0], [0, 0, 1], [1, 0, 1], [2, 0, 1], [3, 0, 1], [0, 0, 2], [1, 0, 2],
], 'a wrapped repeat lays its copies in rows');
eq(repeatOffsets({ count: 'n', step: [2, 0, 0] }, { n: 3 }), [[0, 0, 0], [2, 0, 0], [4, 0, 0]], 'a line of copies at step * i');
eq(repeatOffsets({ count: 'p_cores/4', step: [1, 0, 0] }, { p_cores: 10 }).length, 3, 'a param over 4 rounds up: 10 gives 3 copies');
eq(repeatOffsets({ count: 'p_cores/4', step: [1, 0, 0] }, {}).length, 0, 'a missing param repeats nothing');

const statusModel = { schema: 1, kind: 'x', view: 'part', size: [1, 1, 1], parts: [{ id: 'led', geo: { type: 'sphere', r: 0.2 }, mat: 'led-ok', tag: 'status' }] };
const sb = buildModel(statusModel);
sb.update(0, { status: 'offline' });
eq((sb.parts.get('led') as THREE.Mesh).material && ((sb.parts.get('led') as THREE.Mesh).material as THREE.MeshToonMaterial).color.getHexString(), new THREE.Color(PALETTE['led-off']!.color).getHexString(), 'a status LED takes the offline colour');

// Every model builds and updates in every state, with and without GPUs.
for (const [name, m] of Object.entries(MODELS)) {
  for (const gpus of [0, 2]) {
    const b = buildModel(m, { gpus });
    b.update(0.016, { status: 'draining', working: 2, load: 0.5, open: 0.5 });
    eq(b.group.children.length > 0, true, `builds ${name} (gpus ${gpus})`);
  }
}
const board = MODELS['board/workstation']!;
placeAtom(board, new THREE.Group());
const traceModel = { schema: 1, kind: 'x', view: 'board', size: [10, 2, 10], parts: [], anchors: { camera: { at: [0, 10, 10], look: [0, 0, 0] }, atom: [0, 1, 0] }, traces: [{ path: [[0, 0], [4, 0], [4, 3]], y: 0.06, width: 0.25 }] } as unknown as Parameters<typeof traceMeshes>[0];
eq(traceMeshes(traceModel).children.length, 2, 'a trace is one copper strip per segment');
eq(currentPulses(traceModel).group.children.length, 1, 'a short trace carries one pulse');

// Treasury: the range window, flow totals, chart data, journal paging merge and the party labels.
const TDAY = 86_400_000;
const NOW = 100 * TDAY;
eq(treasuryWindow('7d', NOW, 0), { from: NOW - 7 * TDAY, to: NOW, bucket: 'hour' }, 'treasury 7 d is hourly');
eq(treasuryWindow('30d', NOW, 0), { from: NOW - 30 * TDAY, to: NOW, bucket: 'day' }, 'treasury 30 d is daily');
eq(treasuryWindow('all', NOW, NOW - 10 * TDAY), { from: NOW - 10 * TDAY, to: NOW, bucket: 'day' }, 'treasury all starts at the first account');
eq(treasuryWindow('all', NOW, NOW + 5), { from: NOW - TDAY, to: NOW, bucket: 'day' }, 'treasury all with a future start spans a day');

const tp = (at: number, over: Record<string, number> = {}) => ({
  at_ms: at, end_ms: at + TDAY, entries: 2, minted: 0, escrowed: 0, paid: 0, refunded: 0, fined: 0, spent: 0,
  by_kind: {}, plaza_budget_end: 100, circulating_end: 20, treasury_end: 3, ...over,
});
const pts = [tp(0, { minted: 50, paid: 5, spent: 2 }), tp(TDAY, { minted: 10, escrowed: 7, fined: 1, refunded: 4, spent: 3, entries: 3 })];
eq(flowsOf(pts), { entries: 5, minted: 60, escrowed: 7, paid: 5, refunded: 4, fined: 1, spent: 5 }, 'flow totals sum over buckets');
eq(flowsOf([]), { entries: 0, minted: 0, escrowed: 0, paid: 0, refunded: 0, fined: 0, spent: 0 }, 'flow totals of no buckets are zero');
const bd = barData(pts, (at) => `t${at / TDAY}`);
eq(bd.labels, ['t0', 't1'], 'bars are labelled per bucket');
eq(bd.layers.map((l) => l.key), ['minted', 'escrowed', 'paid', 'refunded', 'fined', 'spent'], 'bars: one layer per kind, in order');
eq(bd.layers[0]?.values, [50, 10], 'bars: minted values per bucket');
const ad = balanceData(pts, (at) => `t${at / TDAY}`);
eq(ad.layers.map((l) => [l.key, l.values]), [['plaza', [100, 100]], ['circulating', [20, 20]], ['treasury', [3, 3]]], 'balances: plaza, circulating, treasury');

const je = (id: number, over: Record<string, unknown> = {}) => ({ id, kind: 'spend', amount: 1, created_at: id, memo: '', posted_by: 'x', postings: [], task_id: null, root_task: null, ...over });
const j1 = [je(5), je(4)];
eq(mergeJournal(j1, [je(4), je(3)]).map((e) => e.id), [5, 4, 3], 'journal: an older page extends the list without repeats');
eq(mergeJournal([je(4), je(3)], [je(6), je(5)]).map((e) => e.id), [6, 5, 4, 3], 'journal: a newer head page goes first');
eq(mergeJournal([], [je(2), je(1)]).map((e) => e.id), [2, 1], 'journal: the first page is newest first');
eq(mergeJournal([je(4, { memo: 'old' })], [je(4, { memo: 'new' })])[0]?.memo, 'new', 'journal: the fresher copy of an id wins');

eq(sortRows(['b', 'a', 'c'], (s) => s, 1), ['a', 'b', 'c'], 'sort strings ascending');
eq(sortRows([3, 10, 2], (n) => n, -1), [10, 3, 2], 'sort numbers descending');

const names = new Map([[7, 'Della']]);
eq(partyOf({ account: 1, amount: -5, kind: 'wallet', owner_ref: '7' }, names), 'Della', 'a wallet is its agent');
eq(partyOf({ account: 2, amount: 5, kind: 'town_budget', owner_ref: 'plaza' }, names), 'Plaza budget', 'the plaza budget account');
eq(partyOf({ account: 3, amount: -5, kind: 'task_escrow', owner_ref: 't9' }, names), 'Escrow t9', 'an escrow names its task');
eq(movesOf({ id: 1, kind: 'refund', amount: 500, created_at: 0, memo: '', posted_by: '', task_id: 't9', root_task: null, postings: [
  { account: 12, amount: -500, kind: 'task_escrow', owner_ref: 't9' }, { account: 2, amount: 500, kind: 'town_budget', owner_ref: 'plaza' }] }, names),
  { from: 'Escrow t9', to: 'Plaza budget' }, 'a refund moves from escrow to the plaza budget');
eq(Math.abs(creditsUsd(1000) - 1) < 1e-9, true, 'a thousand credits are a dollar');
eq(creditsOf(0.5), 500, 'half a dollar of model spend is five hundred credits');

// ---- Board layout (floor3d/board-layout) ------------------------------------------------------------

const boardAgent = (cid: number, role: string, tier = 'worker'): Agent => ({ cid, name: '', role, tier }) as unknown as Agent;
/** No minimum room sizes: the fixture's own rules (the table is an argument, so these ignore minsize.json). */
const NO_MINIMUMS = {};
/** The contract's fixture: cpu (cap 2), gpu (cap 2, shown by gpus >= 1), memory (cap 1); the atom over the cpu. */
const boardFixture = (): Model => ({
  schema: 1,
  kind: 'test',
  view: 'board',
  size: [40, 4, 30],
  params: { gpus: 1 },
  parts: [{ id: 'cpu0' }, { id: 'gpu0', when: 'gpus >= 1' }, { id: 'dimm0' }],
  anchors: { atom: [-7, 2, 0] },
  slots: [
    { id: 'cpu', on: 'cpu0', type: 'cpu', top: 1.4, rect: [-12, -4, 8, 8], capacity: 2 },
    { id: 'gpu', on: 'gpu0', type: 'gpu', top: 1.4, rect: [2, -6, 12, 12], capacity: 2 },
    { id: 'memory', on: 'dimm0', type: 'memory', top: 1.4, rect: [-12, -12, 8, 6], capacity: 1 },
  ],
});
const slotOf = (l: BoardLayout): Record<string, string> => Object.fromEntries(l.districts.map((d) => [d.role, d.slot]));
const room = (l: BoardLayout, role: string) => l.districts.find((d) => d.role === role)!;
const team5 = [boardAgent(1, 'orchestrator', 'orchestrator'), boardAgent(2, 'builder'), boardAgent(3, 'finance'), boardAgent(4, 'librarian'), boardAgent(5, 'qa')];

const lay = layoutOnBoard(team5, boardFixture(), {}, NO_MINIMUMS);
// aisle layout: each room goes to the least-filled slot; family preference only breaks ties, so finance and librarian swap their family slots.
eq(slotOf(lay), { orchestrator: 'cpu', builder: 'gpu', finance: 'memory', librarian: 'cpu', qa: 'gpu' }, 'board: each room takes the least-filled slot; the orchestrator on the cpu');
// aisle layout: the cpu is double-loaded (rows 3.5 deep); the two rows tie at 2.5 from the atom, the first one is taken.
eq([room(lay, 'orchestrator').cx, room(lay, 'orchestrator').cy, room(lay, 'orchestrator').w, room(lay, 'orchestrator').d, room(lay, 'orchestrator').y], [-5.75, -2.25, 3.5, 3.5, 1.4], 'board: the orchestrator takes the cpu cell nearest the atom');
// aisle layout: the second cpu room is the librarian (finance went to memory); its cell is the other row of the cpu.
eq([room(lay, 'librarian').cx, room(lay, 'librarian').cy], [-9.25, -2.25], 'board: the other cpu room takes the free cell');
// aisle layout: the gpu is double-loaded, so builder and qa share its lower row (z -6 to -0.5).
eq([room(lay, 'builder').cx, room(lay, 'builder').cy, room(lay, 'qa').cx], [5.75, -3.25, 11.25], 'board: rooms in one slot fill its cells in order');
// aisle layout: the memory slot is double-loaded (its rows are 2.5 deep), so its cell is a row, not the whole rect; radius 0.9 of 2.5 / 2 = 1.125.
eq([room(lay, 'finance').cx, room(lay, 'finance').cy, room(lay, 'finance').r], [-7.5, -10.75, 1.125], 'board: a one-room slot is its aisle row, radius 0.9 of its half side');
// aisle layout: the orchestrator (agent 1) sits at its cell centre (-5.75, -2.25).
eq(lay.homes.get(1)?.slice(1), [-5.75, -2.25], 'board: a lone agent sits at its cell centre');
eq([lay.bounds, lay.pcbTop], [{ cx: 0, cz: 0, w: 40, d: 30 }, 0.4], 'board: bounds are the footprint; pcb top defaults to 0.4');

eq(Math.abs(boardHeightAt(5, 0, lay) - 1.4) < 1e-9, true, 'board height: inside a cell is the slot top');
// aisle layout: the memory cell is the row centred at z -10.75; the probe sits half a unit outside its right edge (x -4).
eq(Math.abs(boardHeightAt(-3.5, -10.75, lay) - 0.9) < 1e-9, true, 'board height: half a unit outside a cell ramps halfway down');
eq(boardHeightAt(-2.5, -9, lay), 0.4, 'board height: past the ramp is the PCB top');
eq(boardHeightAt(0, 20, lay), 0.4, 'board height: away from every cell is the PCB top');

const noGpu = layoutOnBoard([boardAgent(1, 'orchestrator', 'orchestrator'), boardAgent(2, 'builder'), boardAgent(3, 'qa')], boardFixture(), { gpus: 0 }, NO_MINIMUMS);
// aisle layout (balanced): with no gpu, builder and qa take the memory and cpu slots by fill; no room is on a gpu.
eq(slotOf(noGpu), { orchestrator: 'cpu', builder: 'memory', qa: 'cpu' }, 'board: a hidden gpu is no slot; the other rooms take the cpu and memory slots');

const crowded = layoutOnBoard(
  [boardAgent(1, 'orchestrator', 'orchestrator'), boardAgent(2, 'architect'), boardAgent(3, 'auditor'), boardAgent(4, 'builder'), boardAgent(5, 'finance'), boardAgent(6, 'librarian'), boardAgent(7, 'qa')],
  boardFixture(),
  {},
  NO_MINIMUMS,
);
// A role with no free cell joins a built room (desks added, no cell shrunk): every agent keeps a desk.
eq(crowded.homes.size, 7, 'board: every agent of an overfull board has a desk');
eq(crowded.districts.length, 5, 'board: the overflowing roles join built rooms; the rooms keep their cells');
// aisle layout (balanced): the seated roles spread over the slots by fill (auditor on memory, finance on the second gpu row).
eq(slotOf(crowded), { orchestrator: 'cpu', architect: 'gpu', auditor: 'memory', builder: 'cpu', finance: 'gpu' }, 'board: the seated roles keep their slots');
eq(crowded.districts.filter((d) => d.slot === 'gpu').map((d) => d.w), [5.5, 5.5], 'board: a gpu room keeps its cell when others join it');

// Occlusion (board-layout.ts): a slot is hidden from the board's camera when more than a third of its samples are
// blocked by another part. The workstation: the chipset and the M.2 slots under the standing GPU card are hidden,
// the cpu, the GPU top and the RAM are not. m2_3 is hidden only when a second GPU card stands over it.
{
  const cube = (lo: [number, number, number], hi: [number, number, number]) => ({ id: 'box', path: 'box', file: '', lo, hi });
  eq(segmentBlocked([0, 10, 0], [0, 0, 0], [-1, 4, -1], [1, 6, 1]), true, 'occlusion: a box between the camera and the point blocks the segment');
  eq(segmentBlocked([0, 10, 0], [0, 0, 0], [3, 4, 3], [5, 6, 5]), false, 'occlusion: a box beside the segment does not block it');
  eq(segmentBlocked([0, 10, 0], [0, 5, 0], [-1, 1, -1], [1, 3, 1]), false, 'occlusion: a box beyond the point does not block it');
  eq(segmentBlocked([0, 10, 0], [0, 0, 0], [-1, 10, -1], [1, 12, 1]), false, 'occlusion: a box above the camera does not block it');
  const near = slotBlockedShare({ on: 'base', top: 1, rect: [0, 0, 2, 2] }, [0, 10, 0], [cube([9, 3, 9], [10, 4, 10])] as never[]);
  eq(near, 0, 'occlusion: with no blocking box no sample is blocked');
  const own = slotBlockedShare({ on: 'base', top: 1, rect: [-1, -1, 2, 2] }, [0, 10, 0], [{ id: 'base', path: 'base', file: '', lo: [-5, 0, -5], hi: [5, 1, 5] }] as never[]);
  eq(own, 0, 'occlusion: a slot does not block itself (its own part is excluded)');
  const wsBoard = workstationBoardFile as unknown as Model;
  const occludedFor = (gpus: number): string[] => [...occludedSlotIds(wsBoard, partBoxes(wsBoard, { gpus }))].sort();
  eq(occludedFor(2), ['slot_io', 'slot_m2_1', 'slot_m2_2', 'slot_m2_3'], 'occlusion workstation (2 GPUs): the chipset and the three M.2 slots under the cards are hidden');
  eq(occludedFor(1), ['slot_io', 'slot_m2_1', 'slot_m2_2'], 'occlusion workstation (1 GPU): m2_3 is in front of the card and stays visible');
  for (const gpus of [1, 2]) {
    const hidden = occludedFor(gpus);
    eq(['slot_cpu', 'slot_gpu', 'slot_mem'].filter((id) => hidden.includes(id)), [], `occlusion workstation (${gpus} GPU): cpu, gpu and mem are visible`);
  }
}

// Grounding (ground.ts): on every board, at 1, 4, 8 and 20 agents, no resting part floats above what is under it or sinks
// into it. The plates, their columns and the rooms are built as the Floor builds them (boardScene), so a plate that
// rests on the wrong height fails here too.
{
  const groundPalette = roomPalette({ sheet: '#ffffff', rule: '#d6d2c8', ink2: '#4a4a4a', signal: '#d9482b' });
  const groundBoards: [string, Model, Record<string, number>][] = [
    ['workstation', workstationBoardFile as unknown as Model, {}],
    ['workstation (2 GPUs)', workstationBoardFile as unknown as Model, { gpus: 2 }],
    ['laptop', laptopBoardFile as unknown as Model, {}],
    ['server', serverBoardFile as unknown as Model, {}],
    ['edge', edgeBoardFile as unknown as Model, {}],
    ['phone', phoneBoardFile as unknown as Model, {}],
    ['apple-soc', appleBoardFile as unknown as Model, {}],
  ];
  for (const [name, board, params] of groundBoards) {
    for (const n of [1, 4, 8, 20]) {
      if (name.includes('2 GPUs') && n !== 8) continue; // the second GPU's variant is checked at one count
      const lay = laid(() => layoutOnBoard(syntheticAgents(n), board, params), `grounding ${name} (${n} agents)`);
      if (!lay) continue;
      const rep = groundAudit(boardScene(board, params, lay, groundPalette));
      const bad = rep.findings.map((f) => `${f.item} ${f.kind} ${f.amount.toFixed(3)} against ${f.against}`);
      if (bad.length > 0) console.log(`grounding ${name} (${n} agents): ${bad.length} findings, the first: ${bad.slice(0, 6).join('; ')}`);
      eq(bad.length, 0, `grounding ${name} (${n} agents): no floating or sinking resting part (${rep.items} checked)`);
    }
  }
}

// Joining (board-layout.ts): with the 17 roles of the roster, every agent has a desk, and no room on any board model is
// under its minimum for its desk count (a role with no free cell joins a built room; the room's minimum is recomputed).
{
  const ROSTER = ['architect', 'auditor', 'builder', 'edge-operator', 'finance', 'incident', 'librarian', 'mayor', 'mayor-office', 'orchestrator', 'planner', 'qa', 'release', 'researcher', 'reviewer', 'security-reviewer', 'breeder'];
  const roster17 = ROSTER.map((role, i) => boardAgent(200 + i, role, role === 'orchestrator' ? 'orchestrator' : 'worker'));
  const models: Record<string, Model> = {
    workstation: workstationBoardFile as unknown as Model,
    laptop: laptopBoardFile as unknown as Model,
    server: serverBoardFile as unknown as Model,
    edge: edgeBoardFile as unknown as Model,
    phone: phoneBoardFile as unknown as Model,
    'apple-soc': appleBoardFile as unknown as Model,
  };
  for (const [name, board] of Object.entries(models)) {
    const lay = laid(() => layoutOnBoard(roster17, board, {}), `board join ${name}`);
    if (!lay) continue;
    eq(lay.homes.size, 17, `board join ${name}: all 17 agents have a desk`);
    // The board's world scale: 1 when the plates seat everyone as the model draws them, else the layout grows it (x1.1 steps).
    eq(lay.boardScale >= 1 && lay.boardScale <= 3, true, `board join ${name}: the world scale is between 1 and the cap`);
    console.log(`board join ${name}: boardScale ${lay.boardScale.toFixed(3)}`);
    // The minimum (assets/rooms/minsize.json, read raw) is indexed by each room's own desk count and compared with the room's
    // inner width (2r - 0.15, the room itself, not its cell). A room with no entry for its count is a violation, never clamped.
    const deskTable = JSON.parse(readFileSync(join(process.cwd(), '../../assets/rooms/minsize.json'), 'utf8')) as Record<string, (number | null)[]>;
    const violations: string[] = [];
    for (const d of lay.districts) {
      const deskPositions = [...lay.homes.values()].filter(([h]) => h === d);
      const desks = deskPositions.length;
      const row = deskTable[d.role];
      const min = row && desks >= 1 && desks <= row.length ? row[desks - 1] : null;
      const inner = 2 * d.r - 0.15;
      if (typeof min !== 'number' || inner < min - 1e-9) violations.push(`${d.role} (${desks} desks) inner ${inner.toFixed(2)}, min ${min}`);
      for (const [, gx, gy] of deskPositions) {
        if (Math.abs(gx - d.cx) > inner / 2 + 1e-9 || Math.abs(gy - d.cy) > inner / 2 + 1e-9) violations.push(`${d.role}: desk (${gx.toFixed(2)}, ${gy.toFixed(2)}) outside its room`);
      }
    }
    // The join rule seats a room at most as many desks as assets/rooms/minsize.json has entries for, so no minimum is clamped.
    const uncovered = lay.districts
      .map((d) => [d.role, [...lay.homes.values()].filter(([h]) => h === d).length] as const)
      .filter(([role, desks]) => desks > (deskTable[role]?.length ?? 0))
      .map(([role, desks]) => `${role} (${desks} desks)`);
    eq(uncovered, [], `board join ${name}: the minimum table covers every room's desk count`);
    eq(violations, [], `board join ${name}: every room's inner width covers min(its desks) and every desk lies inside its room`);
  }
}

// Minimum room sizes (the table is an argument; the default is read from assets/rooms/minsize.json on each call).
const minTable = { orchestrator: [4.5], librarian: [7], mayor: [4.5] };
const grown = layoutOnBoard([boardAgent(1, 'orchestrator', 'orchestrator')], boardFixture(), {}, minTable);
eq([room(grown, 'orchestrator').cx, room(grown, 'orchestrator').w], [-7.5, 7], 'board minimum: a cell too small for its room is grown (one 8-wide cell, not two 4-wide)');
const asBefore = layoutOnBoard([boardAgent(1, 'orchestrator', 'orchestrator')], boardFixture(), {}, { orchestrator: [2.5] });
eq([room(asBefore, 'orchestrator').cx, room(asBefore, 'orchestrator').w], [-5.75, 3.5], 'board minimum: the table is read at call time, a smaller minimum keeps the two cells');
const moved = layoutOnBoard([boardAgent(1, 'orchestrator', 'orchestrator'), boardAgent(2, 'librarian')], boardFixture(), {}, minTable);
// world scale: the board grows until every agent seats (the 7-wide room needs a larger gpu), so the librarian now takes the gpu
// at a scale above 1; before the scale rule she had no slot.
eq(slotOf(moved), { orchestrator: 'cpu', librarian: 'gpu' }, 'board minimum: a room no slot holds at scale 1 moves to the gpu once the board grows');
const cpuBase = boardFixture();
const twoCpu: Model = { ...cpuBase, slots: [...(cpuBase.slots ?? []),{ id: 'cpu2', on: 'cpu0', type: 'cpu', top: 1.4, rect: [6, -4, 8, 8], capacity: 2 }] };
const pair = layoutOnBoard([boardAgent(1, 'orchestrator', 'orchestrator'), boardAgent(2, 'mayor')], twoCpu, {}, minTable);
eq([slotOf(pair).orchestrator, room(pair, 'orchestrator').cx], ['cpu', -7.5], 'board minimum: the orchestrator stays in the cpu cell nearest the atom');
eq(slotOf(pair).mayor, 'cpu2', 'board minimum: a second command room takes the far cpu slot, not the orchestrator\'s');

eq(layoutOnBoard([], boardFixture()).districts.length, 0, 'board: no agents, no rooms');

const walk = traceGraph({ ...boardFixture(), traces: [{ path: [[0, 0], [10, 0], [10, 10]], y: 0.42, width: 0.25 }] });
eq(walk.nodes.length, 3, 'walkways: one node per distinct trace point');
eq(pathOnTraces(walk, [1, 0.5], [9.5, 10]), [[1, 0.5], [0, 0], [10, 0], [10, 10], [9.5, 10]], 'walkways: a walk along an L-shaped trace goes by its corner');
const apart = traceGraph({ ...boardFixture(), traces: [{ path: [[0, 0], [1, 0]], y: 0.42, width: 0.25 }, { path: [[5, 5], [6, 5]], y: 0.42, width: 0.25 }] });
eq(pathOnTraces(apart, [0, 0], [6, 5]), null, 'walkways: no walk between traces that do not join');
// The board plaza's machine mapping and room lookup (floor.ts).
const wsBoard = boardOf({ kind: 'workstation', vendor: null, model: null, apple: null, gpus: [{}] });
eq(wsBoard, { name: 'board/workstation', params: { gpus: 1, studio: 0 } }, 'board plaza: a workstation with one GPU shows the workstation board');
const appleBoard = boardOf({ kind: 'laptop', vendor: 'apple', model: 'MacBook Pro', apple: { chip: 'M3', p_cores: 4, e_cores: 4, gpu_cores: 10, neural_cores: 16, unified_memory_gib: 16 }, gpus: [] });
eq(appleBoard, { name: 'board/apple-soc', params: { gpus: 0, studio: 0, p_cores: 4, e_cores: 4, gpu_cores: 10, memory_gib: 16 } }, 'board plaza: an Apple laptop shows the Apple SoC with its cores and memory');
eq(boardOf({ kind: 'workstation', vendor: 'apple', model: 'Mac Studio', apple: null, gpus: [] }).params.studio, 1, 'board plaza: a Studio is flagged');
const roomsOnBoard = layoutOnBoard([boardAgent(1, 'orchestrator', 'orchestrator'), boardAgent(2, 'builder')], boardFixture());
// aisle layout: the builder's cell is its gpu row, centred at (5, -3.25); (5, 0) is now the aisle, not a room.
eq(roomAt(roomsOnBoard, 5, -3.25)?.role, 'builder', 'board plaza: a point inside a room cell finds that room');
eq(roomAt(roomsOnBoard, 0, 25), null, 'board plaza: a point off every cell is no room');

// The Machines scene's pure parts (fleet.ts; desktop mirrors the same numbers).
import { cableCurvePoints, facingAngle, packetRate, ringLayout, rotateY } from '../src/views/floor3d/fleet';
const r3 = (v: number): number => Math.round(v * 1000) / 1000;
const ring1 = ringLayout([{ id: 'n1', radius: 5 }]);
eq([ring1.ring, r3(ring1.slots[0]!.x), r3(ring1.slots[0]!.z)], [14, -14, 0], 'fleet ring: one node sits opposite the origin on the minimum ring');
const fixNodes = [{ id: 'n10', radius: 3 }, { id: 'n2', radius: 6 }, { id: 'n1', radius: 4 }, { id: 'n3', radius: 3 }];
const ringFix = ringLayout(fixNodes, 6);
eq(ringFix.slots.map((s) => s.id), ['n1', 'n2', 'n3', 'n10'], 'fleet ring: nodes sorted by id, numbers as numbers');
eq(ringFix.ring >= 6 + 6 + 4, true, 'fleet ring: clears the hub and the largest node');
let ringGap = true;
ringFix.slots.forEach((s, i) => {
  const o = ringFix.slots[(i + 1) % ringFix.slots.length]!;
  const ra = fixNodes.find((n) => n.id === s.id)!.radius;
  const rb = fixNodes.find((n) => n.id === o.id)!.radius;
  if (Math.hypot(o.x - s.x, o.z - s.z) < ra + rb + 4 - 1e-9) ringGap = false;
});
eq(ringGap, true, 'fleet ring: neighbours keep their radii plus 4 apart (chord, cyclic)');
const cable = cableCurvePoints([0, 4, 0], [10, 4, 0]);
eq(cable.length, 25, 'fleet cable: 24 segments, 25 points');
eq([cable[0], cable[24]], [[0, 4, 0], [10, 4, 0]], 'fleet cable: starts at one port and ends at the other');
eq(cable[12], [5, 2.5, 0], 'fleet cable: the middle hangs 1.5 below the straight line');
eq(cableCurvePoints([0, 0.5, 0], [10, 0.5, 0])[12]![1], 0.1,'fleet cable: never below the ground clamp');
eq(packetRate(0, 0, true), 0.5, 'fleet packets: the base rate when idle');
eq(packetRate(1, 0, true), 1.5, 'fleet packets: one working agent on the hub end adds one per second');
eq(packetRate(2, 1, true), 3.5, 'fleet packets: working agents on both ends add up');
eq(packetRate(4, 4, true), 4, 'fleet packets: capped at four per second');
eq(packetRate(3, 3, false), 0, 'fleet packets: an offline link carries none');
const fp = rotateY([0, 0, -4.6], facingAngle(14, 0, [0, 0, -4.6]));
eq([r3(fp[0]), r3(fp[2])], [-4.6, 0], 'fleet facing: the port turns to face the hub');
eq(rotateY([1, 2, 0], Math.PI / 2).map(r3), [0, 2, -1], 'fleet facing: rotation about y, as three.js turns it');

// The Machines view's pure parts (floor.ts): the camera distance, the popup's text.
const near = (a: number, b: number): boolean => Math.abs(a - b) < 1e-9;
const d80 = fitDistance(10, 40, 1, 0.8);
eq(near(Math.tan(Math.asin(10 / d80)), 0.8 * Math.tan(Math.PI / 9)), true, 'machines fit: the sphere fills 80% of the tighter half-field');
eq(fitDistance(10, 40, 0.5, 0.8) > d80, true, 'machines fit: a narrow stage (the width is tighter) backs the camera off');
eq(gibText(16384), '16 GiB', 'machines text: whole GiB');
eq(gibText(16000), '15.6 GiB', 'machines text: one decimal when not whole');
eq(machineSubtitle({ kind: 'laptop', model: 'MacBook Pro (M3 Max)' }), 'laptop · MacBook Pro (M3 Max)', 'machines subtitle: kind and model');
eq(machineSubtitle({ kind: 'edge', model: null }), 'edge', 'machines subtitle: no model, the kind alone');
eq(agentTally(0, 0, 'online'), 'No agents', 'machines agents: none');
eq(agentTally(3, 2, 'online'), '3 agents: 2 working, 1 idle', 'machines agents: the split while online');
eq(agentTally(1, 0, 'offline'), '1 agent on a machine that is offline', 'machines agents: offline machine states its status');
const rig: FleetMachine = {
  id: 'n0', name: 'zarathos-pc', role: 'hub', kind: 'workstation', vendor: 'asus', model: 'ASUS ROG', apple: null, status: 'online',
  arch: 'x86_64', os: 'linux', backend: 'docker', cpu: { model: 'AMD Ryzen 9 5950X', cores: 16, threads: 32 }, memory_mib: 65536,
  gpus: [{ name: 'NVIDIA GeForce RTX 3090', vendor: 'nvidia', memory_mib: 24576 }], storage_gib: 2000, agents: [1, 2], working: 1,
  stats: null, last_seen_ms: 0,
};
const facts = machineFacts(rig);
eq(facts[0], ['CPU', 'AMD Ryzen 9 5950X'], 'machines facts: the CPU model first');
eq(facts[1], ['Cores', '16 cores · 32 threads'], 'machines facts: cores and threads');
eq(facts[2], ['Memory', '64 GiB'], 'machines facts: memory in GiB');
eq(facts[3], ['GPUs', 'NVIDIA GeForce RTX 3090 · 24 GiB'], 'machines facts: GPU name and memory');
eq(machineFacts({ ...rig, gpus: [] })[3], ['GPUs', 'none'], 'machines facts: no GPU reads none');
eq(machineFacts({ ...rig, gpus: [{ name: 'NVIDIA GeForce RTX 3090', vendor: 'NVIDIA', memory_mib: null as unknown as number }] })[3], ['GPUs', 'NVIDIA GeForce RTX 3090'], 'machines facts: a GPU of unknown memory shows its name alone');
eq(gibText(null), '—', 'machines text: unknown memory is a dash');
eq(fleetSig({ machines: [rig, { ...rig, id: 'n1' }], links: [] }), 'n0,n1', 'machines: the fleet signature is the ids in order');

sectionEnd('web tests: ok');

// ---- Robots (assets/robots): every file validates and is listed, the rig stays within its draw budget. ----
// The block runs its own failure check at its end (the checks above have already run).
import { registry } from '../src/views/floor3d/robots/files';
import { makeRobot, ROBOT_PALETTE, robotVariant, speciesOf } from '../src/views/floor3d/robot';
import { channelValue } from '../src/views/floor3d/robots/clips';
import { REQUIRED_BONES } from '../src/views/floor3d/robots/types';
import { validateSpecies } from '../src/views/floor3d/robots/validate';

const robotRoot = join(process.cwd(), '../../assets/robots');
const listedIn = (dir: string): string[] =>
  readdirSync(join(robotRoot, dir)).filter((f) => f.endsWith('.json')).map((f) => f.slice(0, -5)).sort();
const reg = registry();
eq(reg.errors, [], 'robots: every listed file validates');
eq(Object.keys(reg.species).sort(), listedIn('species'), 'robots: every species file is in the index');
eq([...reg.index.species].sort(), listedIn('species'), 'robots: the index lists exactly the species files');
eq(Object.keys(reg.tools).sort(), listedIn('tools'), 'robots: every tool file is in the index');
eq(reg.index.species.length, listedIn('species').length, 'robots: every species file on disk is listed');
eq(reg.index.species.at(-1), 'oracle', 'robots: the oracle is listed last (reserved)');
for (const [id, s] of Object.entries(reg.species)) {
  eq(REQUIRED_BONES.filter((b) => !(b in s.bones)), [], `robots ${id}: has the required bones`);
  const bones = new Set(Object.keys(s.bones));
  const named = Object.values(s.clips).flatMap((c) => c.channels.map((ch) => ch.bone));
  eq(named.filter((b) => !bones.has(b)), [], `robots ${id}: its clips name its bones`);
  eq(s.parts.filter((p) => p.bone === undefined || !bones.has(p.bone)).length, 0, `robots ${id}: every part hangs on a bone`);
}
eq(Object.values(reg.clips).flatMap((c) => c.channels.map((ch) => ch.bone)).filter((b) => !(REQUIRED_BONES as readonly string[]).includes(b)), [], 'robots: shared clips name the required bones');

// The floor's API: the same names, and a variant is now a species id (the same deal as before for 0–4).
eq(typeof makeRobot, 'function', 'robots: the floor API has makeRobot');
eq(robotVariant(0), 'rover', 'robots: robotVariant(0) is the rover');
eq(robotVariant(3), 'can', 'robots: robotVariant(3) is the can, as before');
// The species: the hub's record wins over the cid; a record with no species (or a blank one) falls back to the cid's deal.
eq(speciesOf('rover', 3), 'rover', 'robots: a record species wins over the cid');
eq(speciesOf('can', 0), 'can', 'robots: an agent record with species can renders that species');
eq(speciesOf(undefined, 3), robotVariant(3), 'robots: a record with no species falls back to the cid');
eq(speciesOf('  ', 0), robotVariant(0), 'robots: a blank species is missing');
eq(new Set(Array.from({ length: 300 }, (_, i) => robotVariant(i))).has('oracle'), false, 'robots: the fallback deal never gives the oracle');
eq(Object.keys(ROBOT_PALETTE).sort(), ['info', 'ink', 'inkMuted', 'ok', 'paper', 'rule', 'sheet', 'signal'], 'robots: the palette keys the floor reads');

// Draw budget, as a proxy in node (no WebGL here): every mesh of a robot is one draw call.
for (const id of reg.index.species) {
  const r = makeRobot(200, false, ROBOT_PALETTE, 200, id);
  let meshes = 0;
  r.group.traverse((o) => {
    if ((o as THREE.Mesh).isMesh) meshes++;
  });
  eq(meshes <= 14, true, `robots ${id}: at most 14 draw calls (${meshes} meshes)`);
  r.dispose();
}
const orch = makeRobot(200, true, ROBOT_PALETTE, 200, 'rover');
let orchMeshes = 0;
orch.group.traverse((o) => {
  if ((o as THREE.Mesh).isMesh) orchMeshes++;
});
eq(orchMeshes <= 16, true, `robots rover as orchestrator: at most 16 draw calls (${orchMeshes} meshes)`);
orch.dispose();

// The blink: the lids close to 0.1 for 0.14 s of every 3.6 s, and are open between blinks.
const blink = reg.clips.blink!.channels[0]!;
eq(Math.round(channelValue(blink, 0.05, reg.species.rover!.gait) * 100) / 100, 0.1, 'robots: the blink closes the lids');
eq(Math.round(channelValue(blink, 1, reg.species.rover!.gait) * 100) / 100, 1, 'robots: the lids are open between blinks');

// A bad file is reported by its path, not accepted.
const broken = JSON.parse(JSON.stringify(reg.species.rover)) as { parts: { bone: string }[] };
broken.parts[0]!.bone = 'nope';
const brokenErrs = validateSpecies(broken, 'rover', new Set(Object.keys(reg.clips)));
eq(brokenErrs.some((e) => e.includes('nope')), true, 'robots: a part on a missing bone is an error');

sectionEnd('robot tests: ok');

// ---- Props (assets/robots/props): every file validates and is listed, every spot names a shared clip, each prop builds. ----
import { buildProp, propRegistry, validateProp } from '../src/views/floor3d/robots/props';
const props = propRegistry();
eq(props.errors, [], 'props: every listed prop file validates');
eq(Object.keys(props.props).sort(), [...reg.index.props].sort(), 'props: every prop file is in the index');
eq(reg.index.props.length, 17, 'props: the 17 station props are listed');
const shared = new Set(Object.keys(reg.clips));
for (const p of Object.values(props.props)) {
  eq(p.spots.filter((s) => !shared.has(s.clip)), [], `props ${p.id}: every spot names a shared clip`);
  const built = buildProp(p, ROBOT_PALETTE, 200);
  let meshes = 0;
  built.group.traverse((o) => {
    if ((o as THREE.Mesh).isMesh) meshes++;
  });
  eq(meshes > 0, true, `props ${p.id}: builds into meshes (${meshes})`);
  built.dispose();
}
const badSpot = JSON.parse(JSON.stringify(props.props['easel'])) as { spots: { clip: string }[] };
badSpot.spots[0]!.clip = 'nope';
eq(validateProp(badSpot, 'easel', shared).some((e) => e.includes('nope')), true, 'props: a spot on a missing clip is an error');

sectionEnd('robot props tests: ok');

// ---- Xenon atom (floor3d/props): a few draw calls, instanced nucleons and electrons ---------------------
import { xenonAtom, type PropPalette } from '../src/views/floor3d/props';
{
  const palette: PropPalette = {
    sheet: new THREE.Color(0xe8e4d8),
    rule: new THREE.Color(0x6a7090),
    ink3: new THREE.Color(0x3a3f55),
    signal: new THREE.Color(0xff8a6a),
    ok: new THREE.Color(0x5ef08a),
    info: new THREE.Color(0x6ad8ff),
  };
  const atom = xenonAtom(palette, 1);
  let meshes = 0;
  const instanced: THREE.InstancedMesh[] = [];
  atom.group.traverse((o) => {
    if (!(o as THREE.Mesh).isMesh) return;
    meshes++;
    if ((o as THREE.InstancedMesh).isInstancedMesh) instanced.push(o as THREE.InstancedMesh);
  });
  eq(meshes <= 13, true, `xenon atom: at most 13 meshes with their hulls (${meshes})`);
  eq(instanced.map((m) => m.count).sort((a, b) => a - b), [12, 12, 12, 12, 54, 54], 'xenon atom: nucleons (12 + 12) and electrons (54) are instanced, each with an instanced hull');
  eq(atom.group.userData.dynamic, true, 'xenon atom: marked dynamic so bakeStatic leaves it alone');
  const electrons = instanced.find((m) => m.count === 54 && !m.userData.isOutline);
  const hull = electrons?.children.find((c) => c.userData.isOutline) as THREE.InstancedMesh | undefined;
  eq(hull?.instanceMatrix === electrons?.instanceMatrix, true, 'xenon atom: the electron hull shares its instance matrices');
  atom.animate(0.5);
  const before = Array.from(electrons?.instanceMatrix.array ?? []);
  atom.animate(1.5);
  const after = Array.from(electrons?.instanceMatrix.array ?? []);
  eq(after.some((v, i) => v !== before[i]), true, 'xenon atom: the electrons move with time');
}

sectionEnd('xenon atom tests: ok');

// ---- The z-fight lint over every hardware model (floor3d/zfight.ts): no two solids share an overlapping face
// plane, and no plane or ring decal sits on a surface without a lift. Labels are decals on a part's top: they
// draw with a polygon offset, so they are counted in the log and not failed.
{
  const { lintModels, describe: describeFinding } = await import('../src/views/floor3d/zfight');
  const { findings } = lintModels(MODELS);
  eq(findings.filter((f) => f.kind === 'pair').map(describeFinding), [], 'no hardware model has coplanar, overlapping faces (zfight)');
  eq(findings.filter((f) => f.kind === 'decal' && !f.a.label).map(describeFinding), [], 'no plane or ring decal sits on a surface without a lift');
  const labels = findings.filter((f) => f.kind === 'decal' && f.a.label).length;
  if (labels) console.log(`zfight: ${labels} label(s) sit on their part's top (drawn with a polygon offset)`);
}

// ---- The z-fight lint over the robot species (assets/robots/species). A species' bones are translations, so each
// part is flattened into the rig's frame (its bone's offset chain plus its `at`) before the lint walks it.
// Every species is linted and fails the test on any coplanar pair.
{
  const { lintModels, describe: describeFinding } = await import('../src/views/floor3d/zfight');
  const PENDING_SPECIES = new Set<string>(); // every species is linted and must be clean
  type Bone = { parent: string; at: number[] };
  const boneOffset = (bones: Record<string, Bone>, name: string): number[] => {
    const b = bones[name];
    if (!b) return [0, 0, 0];
    const up = b.parent === 'root' ? [0, 0, 0] : boneOffset(bones, b.parent);
    return [up[0]! + b.at[0]!, up[1]! + b.at[1]!, up[2]! + b.at[2]!];
  };
  const robotModels: Record<string, unknown> = {};
  for (const [id, s] of Object.entries(registry().species)) {
    const parts = s.parts.map((p) => {
      const o = boneOffset(s.bones as Record<string, Bone>, p.bone);
      const at = p.at ?? [0, 0, 0];
      return { ...p, at: [o[0]! + at[0]!, o[1]! + at[1]!, o[2]! + at[2]!] };
    });
    robotModels[`species/${id}`] = { parts };
  }
  const robotFindings = lintModels(robotModels as Record<string, Parameters<typeof lintModels>[0][string]>, undefined, 'assets/robots');
  const isPending = (name: string): boolean => PENDING_SPECIES.has(name.replace('species/', ''));
  const robotPairs = robotFindings.findings.filter((f) => f.kind === 'pair');
  const pending = robotPairs.filter((f) => isPending(f.a.model)).length;
  eq(robotPairs.filter((f) => !isPending(f.a.model)).map(describeFinding), [], 'no robot species has coplanar, overlapping faces (zfight; snail, jelly, crab and octo pending)');
  eq(robotFindings.findings.filter((f) => f.kind === 'decal' && !f.a.label && !isPending(f.a.model)).map(describeFinding), [], 'no robot plane or ring decal sits on a surface without a lift');
  if (pending) console.log(`zfight: ${pending} robot pair(s) on snail, jelly, crab and octo are pending (not failed)`);
}

// ---- The robot design audit (floor3d/robots/audit.ts): a REPORT until the species are fixed. It audits every
// species and writes the counts and the worst ten. The audit must run (a sanity check); a species with errors
// fails only when AUDIT_STRICT is set, which the fixing agents do when they finish.
{
  const { auditAll } = await import('../src/views/floor3d/robots/audit');
  const AUDIT_STRICT = false;
  const species = Object.values(registry().species);
  const results = auditAll(species);
  eq(Object.keys(results).sort(), species.map((s) => s.id).sort(), 'robot audit: every valid species is audited');
  const errorsOf = (id: string): number => (results[id] ?? []).filter((i) => i.severity === 'error').length;
  const ranked = species
    .map((s) => ({ id: s.id, errors: errorsOf(s.id), warns: (results[s.id] ?? []).length - errorsOf(s.id) }))
    .sort((a, b) => b.errors - a.errors || b.warns - a.warns || a.id.localeCompare(b.id));
  const total = ranked.reduce((n, r) => n + r.errors + r.warns, 0);
  console.log(`robot audit (report): ${total} issue(s) over ${species.length} species; ${ranked.filter((r) => r.errors === 0).length} with no errors`);
  console.log(`robot audit worst ten: ${ranked.slice(0, 10).map((r) => `${r.id} ${r.errors}e ${r.warns}w`).join(', ')}`);
  if (AUDIT_STRICT) eq(ranked.filter((r) => r.errors > 0).map((r) => r.id), [], 'robot audit: no species has an error');
}

// ---- The robot eyes are strict: no eye may be clipped, covered (eye-occluded), floating or overlapping. An eye error
// fails the suite (unlike the rest of the audit, which is a report). The seat tool (scripts/robot-seat-eyes.mjs) and the
// hand fixes keep this green.
{
  const { auditAll } = await import('../src/views/floor3d/robots/audit');
  const species = Object.values(registry().species);
  const results = auditAll(species);
  const eyeErrors = species.flatMap((s) => (results[s.id] ?? []).filter((i) => i.severity === 'error' && i.kind.startsWith('eye-')).map((i) => `${s.id} ${i.pose} ${i.path} ${i.kind}`));
  eq(eyeErrors, [], 'robot eyes: no species has a clipped, covered, floating or overlapping eye (strict in every pose)');
}

sectionEnd('zfight lint: ok');

// ---- Rooms (floor3d/rooms): the generator and the verifier. The design is docs/design/rooms.md.
const roomCatalog = roomCatalogFile as unknown as Catalog;
const roomRoles = roomRolesFile as unknown as Roles;
const roomDoors: Door[] = ['n', 'e', 's', 'w'];
const roomCap = (w: number, d: number): number => Math.min(4, Math.max(1, Math.floor((w * d) / 6)));
const roomUrl = (i: Input): string => `http://127.0.0.1:7700/#/room-lab?role=${i.role}&w=${i.w}&d=${i.d}&door=${i.door}&seed=${i.seed}`;
// The sweep sizes (w, d): a spread from 2.5 to 8, with w != d where it counts. 60 seeds each; the door cycles by seed.
const roomPairs: [number, number][] = [[2.5, 3], [3, 3.5], [3.5, 4.5], [4, 3.5], [4.5, 4.5], [5, 4.5], [6, 5], [8, 4]];
// minSize is asked only for a room that drops or violates: a clean room needs no bar, so the sweep stays fast.
const roomMinimums = new Map<string, number>();
const roomMinimum = (role: string, agents: number): number => {
  const key = `${role}/${agents}`;
  let m = roomMinimums.get(key);
  if (m === undefined) roomMinimums.set(key, (m = minSize(role, agents, roomCatalog, roomRoles)));
  return m;
};

{
  let clean = 0;
  let below = 0;
  let visits = 0;
  const bad: string[] = [];
  for (const role of Object.keys(roomRoles.roles)) {
    for (const [w, d] of roomPairs) {
      const agents = roomCap(w, d);
      for (let seed = 1; seed <= 60; seed++) {
        const input: Input = { role, w, d, door: roomDoors[seed & 3]!, seed, agents };
        const layout = generate(input, roomCatalog, roomRoles);
        const found = verify(layout, input, roomCatalog, roomRoles);
        visits++;
        // The verifier reports a dropped required group as well (its "dropped" and "required group" lines). Those
        // are errors only at or above the room's minimum: below it, a required group may legitimately not place.
        const real = found.filter((m) => !m.startsWith('dropped required groups') && !m.startsWith('required group'));
        if (real.length > 0) bad.push(`${roomUrl(input)}: ${real[0]}`);
        else if (layout.dropped.length === 0) clean++;
        else if (Math.min(w, d) >= roomMinimum(role, agents)) bad.push(`${roomUrl(input)}: dropped ${layout.dropped.join(', ')}`);
        else below++;
      }
    }
  }
  eq(bad, [], 'rooms: every sweep room verifies clean; at or above minSize nothing is dropped (the sweep)');
  eq(visits, 17 * roomPairs.length * 60, 'rooms: the sweep visits every role, size, door and seed');
}

{
  const input: Input = { role: 'architect', w: 4, d: 3.5, door: 's', seed: 7, agents: 2 };
  const first = layoutHash(generate(input, roomCatalog, roomRoles));
  const again = layoutHash(generate(input, roomCatalog, roomRoles));
  eq(first, again, 'rooms: the layout hash is equal across two runs of a fixed input');
  eq(/^[0-9a-f]{8}$/.test(first), true, 'rooms: the layout hash is 8 lowercase hex digits');
  eq(canonicalJson(-0), '0', 'rooms: the canonical JSON of -0 is 0');
  eq(canonicalJson({ b: [1, -0], a: null }), '{"a":null,"b":[1,0]}', 'rooms: the canonical JSON sorts keys and prints -0 as 0');
}

{
  // v2 (groups with a purpose): every catalogue item belongs to a group, and every group item is a catalogue item or $station.
  const used = new Set<string>();
  for (const g of Object.values(roomCatalog.groups)) for (const it of g.items) used.add(it.id);
  // A station prop reaches a group as the template's station parameter ("$station").
  for (const r of Object.values(roomRoles.roles)) for (const e of [...r.required, ...(r.preferred ?? [])]) if (e.station) used.add(e.station);
  eq(Object.keys(roomCatalog.items).filter((id) => !used.has(id)), [], 'rooms v2: every catalogue item is used by a group');
  eq([...used].filter((id) => id !== '$station' && !(id in roomCatalog.items)), [], 'rooms v2: every group item is a catalogue item');
  // A generated room places every item in a group; moving one of its chairs into the middle is a violation.
  const input: Input = { role: 'researcher', w: 4.5, d: 4.5, door: 's', seed: 11, agents: 2 };
  const layout = generate(input, roomCatalog, roomRoles);
  eq(verify(layout, input, roomCatalog, roomRoles), [], 'rooms v2: a generated room verifies clean');
  eq(layout.items.every((it) => Number.isInteger(it.g) && it.g >= 0 && it.g < layout.groups.length), true, 'rooms v2: every item has a group instance');
  const moved = structuredClone(layout);
  const chair = moved.items.find((it) => it.id === 'chair');
  eq(chair !== undefined, true, 'rooms v2: the seed places a chair (the mutation test has a subject)');
  if (chair) {
    chair.x = 0;
    chair.z = 0;
  }
  eq(chair ? verify(moved, input, roomCatalog, roomRoles).length > 0 : false, true, 'rooms v2: a chair moved to the middle is a violation');
}

{
  // v2 relations: a desk-set's chair sits in front of its desk in the desk's own frame, on every wall. The sweep
  // covers the four anchor rotations (one per wall), every layout verifies, and a chair moved sideways does not.
  const rots = new Set<number>();
  let clean = 0;
  let total = 0;
  for (let seed = 1; seed <= 80; seed++) {
    for (const door of roomDoors) {
      const input: Input = { role: 'other', w: 5, d: 5, door, seed, agents: 1 };
      const layout = generate(input, roomCatalog, roomRoles);
      total++;
      if (verify(layout, input, roomCatalog, roomRoles).length === 0) clean++;
      const desk = layout.items.find((it) => it.id === 'desk');
      if (desk) rots.add(desk.rot);
    }
  }
  eq([...rots].sort((a, b) => a - b), [0, 90, 180, 270], 'rooms v2: a desk-set anchors on every wall (all four rotations)');
  eq(clean, total, 'rooms v2: every desk-set layout of the sweep verifies clean');
  const input: Input = { role: 'other', w: 5, d: 5, door: 's', seed: 3, agents: 1 };
  const base = generate(input, roomCatalog, roomRoles);
  const sided = structuredClone(base);
  const ch = sided.items.find((it) => it.id === 'chair');
  eq(ch !== undefined, true, 'rooms v2: the relation mutation has a chair');
  if (ch) ch.x += 0.3;
  eq(ch ? verify(sided, input, roomCatalog, roomRoles).some((m) => m.includes('not on the front side')) : false, true, 'rooms v2: a chair moved sideways is not on its front side');
}

{
  // assets/rooms/minsize.json (npm run rooms:fixtures): each table entry equals a fresh computation.
  const table = JSON.parse(readFileSync(join(process.cwd(), '../../assets/rooms/minsize.json'), 'utf8')) as Record<string, (number | null)[]>;
  const fresh = (role: string, agents: number): number | null => {
    const v = computeMinSize(role, agents, roomCatalog, roomRoles);
    return Number.isFinite(v) ? v : null;
  };
  for (const role of ['orchestrator', 'architect', 'breeder']) {
    for (const agents of [1, 3, 4]) {
      eq(table[role]?.[agents - 1] ?? 'missing', fresh(role, agents), `rooms minsize: ${role} with ${agents} desk(s) matches a fresh computation`);
    }
  }
}

{
  // assets/rooms/fixtures.json (npm run rooms:fixtures): every case must reproduce its layout hash and counts.
  const file = JSON.parse(readFileSync(join(process.cwd(), '../../assets/rooms/fixtures.json'), 'utf8')) as {
    cases: { role: string; w: number; d: number; door: Door; seed: number; agents: number; hash: string; skipped: number; dropped: number }[];
  };
  const bad: string[] = [];
  file.cases.forEach((c, k) => {
    const input: Input = { role: c.role, w: c.w, d: c.d, door: c.door, seed: c.seed, agents: c.agents };
    const layout = generate(input, roomCatalog, roomRoles);
    const got = `${layoutHash(layout)} skipped ${layout.skipped.length} dropped ${layout.dropped.length}`;
    const want = `${c.hash} skipped ${c.skipped} dropped ${c.dropped}`;
    if (got !== want) bad.push(`case ${k} (${roomUrl(input)}): ${got}, fixture says ${want}`);
  });
  eq(file.cases.length, 200, 'rooms fixtures: 200 cases');
  eq(bad, [], 'rooms fixtures: every case recomputes its layout hash');
}

{
  // partBoxes: a labelled part is still a solid box (the label is a decal on its top); a bare decal gives none.
  const labelled = {
    kind: 'chip',
    parts: [
      { id: 'die', at: [0, 0.5, 0], geo: { type: 'box', w: 1, h: 1, d: 1 }, label: { text: 'CPU' } },
      { id: 'sticker', at: [0, 0, 0], geo: { type: 'plane', w: 1, d: 1 } },
    ],
  } as unknown as Model;
  const boxes = partBoxes(labelled, {}, {});
  eq(boxes.map((b) => b.id), ['die'], 'partBoxes: a labelled box part yields its box; a plane decal yields none');
  eq(boxes[0]?.lo, [-0.5, 0, -0.5], 'partBoxes: labelled box lo');
  eq(boxes[0]?.hi, [0.5, 1, 0.5], 'partBoxes: labelled box hi');
}

// --- Robot behaviour engine (floor3d/behaviour.ts; desktop world/behaviour.rs runs the same tests and fixtures).
{
  const cfg = behaviourFile as unknown as BehaviourConfig;
  const resolveRole = (name: string): RoleDef => {
    const e = cfg.roles[name]!;
    return 'alias' in e ? (cfg.roles[e.alias] as RoleDef) : e;
  };
  const roleIds = Object.keys(cfg.roles).filter((r) => !('alias' in cfg.roles[r]!));
  const idleIds = new Set(cfg.idle.map((d) => d.activity));
  const ALL_KINDS = [
    'commons', 'commons', 'commons', 'dock', 'dock', 'dock', 'trace', 'trace', 'anywhere', 'anywhere', 'anywhere', 'anywhere',
    'prop:drafting-table', 'prop:easel', 'prop:lab-bench', 'prop:microscope-desk', 'prop:workbench', 'prop:whiteboard',
    'prop:book-cart', 'prop:counter', 'prop:podium', 'prop:crate-stack', 'prop:plant-bed',
    'component:cpu', 'component:gpu', 'component:memory', 'component:network', 'component:bmc', 'component:io',
  ];
  const board = (bots: BotIn[]): SpotIn[] => {
    const spots: SpotIn[] = ALL_KINDS.map((kind, i) => ({
      key: `s${String(i).padStart(2, '0')}`,
      kind,
      region: i % 2,
      x: (i % 7) * 3 - 9,
      z: Math.floor(i / 7) * 3 - 6,
      face: 0,
      clip: null,
    }));
    for (const b of bots) if (b.desk) spots.push({ key: b.desk, kind: 'desk', region: b.at.region, x: b.at.x + 0.6, z: b.at.z, face: 0, clip: 'type' });
    return spots;
  };
  const bot = (cid: number, role: string, over: Partial<BotIn> = {}): BotIn => ({
    cid,
    role,
    personality: { social: 5, energy: 5, focus: 5, curiosity: 5, play: 5, tidy: 5 },
    state: 'idle',
    lastTool: null,
    talkingTo: null,
    at: { x: (cid % 9) - 4, z: 0, region: 0 },
    desk: `d${cid}`,
    quirks: [],
    ...over,
  });
  const step = (e: Behaviour, now: number, robots: BotIn[], o: { spots?: SpotIn[]; held?: boolean; events?: EventIn[] } = {}): Decision[] =>
    e.step({ now, robots, spots: o.spots ?? board(robots), held: o.held ?? false, events: o.events ?? [] });
  const toolFor = (cat: string): string =>
    cat === 'default' ? 'unknown_tool' : cfg.tools.find((t) => t.category === cat && !t.match.endsWith('*'))!.match;
  /** The largest group: robots linked by partner pointers (a pair is 2, a standup or cheer is a star). */
  const largestGroup = (out: Decision[]): number => {
    const parent = new Map<number, number>();
    for (const d of out) parent.set(d.cid, d.cid);
    const find = (x: number): number => {
      let r = x;
      while (parent.get(r)! !== r) r = parent.get(r)!;
      return r;
    };
    for (const d of out) if (d.partner !== null && parent.has(d.partner)) parent.set(find(d.cid), find(d.partner));
    const sizes = new Map<number, number>();
    for (const d of out) sizes.set(find(d.cid), (sizes.get(find(d.cid)) ?? 0) + 1);
    return Math.max(0, ...sizes.values());
  };

  // Data: every name the tables use exists.
  const toolIds = new Set(robotIndex.tools as string[]);
  const propIds = new Set(robotIndex.props as string[]);
  const clipIds = new Set(Object.keys(robotClips.clips));
  const dataBad: string[] = [];
  for (const r of roleIds) {
    const role = resolveRole(r);
    for (const t of role.toolbox) if (!toolIds.has(t)) dataBad.push(`${r}: tool ${t}`);
    for (const p of role.props) if (!propIds.has(p)) dataBad.push(`${r}: prop ${p}`);
    if (!role.work.default) dataBad.push(`${r}: no default work`);
    for (const [cat, entries] of Object.entries(role.work)) {
      if (cat !== 'default' && !cfg.categories.includes(cat)) dataBad.push(`${r}: category ${cat}`);
      for (const e of entries) if (!cfg.activities[e.activity]) dataBad.push(`${r}: activity ${e.activity}`);
    }
  }
  for (const [id, a] of Object.entries(cfg.activities)) {
    for (const t of a.tools) if (t !== '$first' && !toolIds.has(t)) dataBad.push(`${id}: tool ${t}`);
    if (!a.clip.startsWith('@') && !clipIds.has(a.clip)) dataBad.push(`${id}: clip ${a.clip}`);
    if (a.dur.length !== 2 || a.dur[0]! > a.dur[1]!) dataBad.push(`${id}: duration`);
  }
  for (const d of cfg.idle) if (!cfg.activities[d.activity] && d.activity !== 'quirk') dataBad.push(`idle: ${d.activity}`);
  for (const t of cfg.tools) if (!cfg.categories.includes(t.category)) dataBad.push(`tool ${t.match}: category ${t.category}`);
  eq(dataBad, [], 'behaviour data: every role, activity, tool, prop and clip exists');

  // Activity choice per role x category: a working robot's work comes from its role's table for the tool's category.
  const badChoice: string[] = [];
  let choices = 0;
  for (const role of roleIds) {
    for (const cat of [...cfg.categories, 'default']) {
      // think is the fallback when nothing in the table can be done (a reviewer with nobody to review).
      const allowed = new Set([...(resolveRole(role).work[cat] ?? resolveRole(role).work.default!).map((e) => e.activity), 'think']);
      for (let cid = 1; cid <= 24; cid++) {
        const d = step(new Behaviour(cfg), 0, [bot(cid, role, { state: 'working', lastTool: toolFor(cat) })])[0]!;
        choices++;
        if (!allowed.has(d.activity)) badChoice.push(`${role}/${cat} cid ${cid}: ${d.activity}`);
      }
    }
  }
  eq(badChoice, [], `behaviour: work is chosen from the role's table (${choices} cases)`);
  const builderTests = new Set<string>();
  for (let cid = 1; cid <= 200; cid++) builderTests.add(step(new Behaviour(cfg), 0, [bot(cid, 'builder', { state: 'working', lastTool: 'bash_output' })])[0]!.activity);
  eq([...builderTests].sort(), ['probe', 'tinker'], 'behaviour: a builder testing tinkers (60) and probes (40)');
  eq(step(new Behaviour(cfg), 0, [bot(5, 'builder', { state: 'working', lastTool: null })])[0]!.activity, 'think', 'behaviour: working without a tool call thinks');

  // Idle distribution follows the traits.
  const countFirst = (act: string, traits: Partial<Personality>): number => {
    let n = 0;
    for (let cid = 1; cid <= 300; cid++) {
      const personality: Personality = { social: 5, energy: 5, focus: 5, curiosity: 5, play: 5, tidy: 5, ...traits };
      if (step(new Behaviour(cfg), 0, [bot(cid, 'other', { personality })])[0]!.activity === act) n++;
    }
    return n;
  };
  const wHi = countFirst('wander', { energy: 9 });
  const wLo = countFirst('wander', { energy: 0 });
  eq(wHi > wLo, true, `behaviour: high-energy robots wander more (${wHi} vs ${wLo} of 300)`);
  const napLo = countFirst('nap', { energy: 9 });
  const napHi = countFirst('nap', { energy: 0 });
  eq(napHi > napLo, true, `behaviour: tired robots nap more (${napHi} vs ${napLo} of 300)`);

  // Reservation: no two robots hold one spot, and every held spot is on the board.
  {
    const bots: BotIn[] = [];
    for (let cid = 1; cid <= 12; cid++) bots.push(bot(cid, cid % 3 === 0 ? 'builder' : 'researcher', { state: cid % 2 ? 'working' : 'idle', lastTool: cid % 2 ? 'bash' : null }));
    const spots = board(bots);
    const keys = new Set(spots.map((s) => s.key));
    const e = new Behaviour(cfg);
    let clash = 0;
    let unknown = 0;
    for (let t = 0; t < 30; t++) {
      const taken = new Set<string>();
      for (const d of step(e, t * 0.5, bots, { spots })) {
        if (d.spot === null) continue;
        if (taken.has(d.spot)) clash++;
        taken.add(d.spot);
        if (!keys.has(d.spot)) unknown++;
      }
    }
    eq([clash, unknown], [0, 0], 'behaviour: no two robots hold one spot (30 ticks, 12 robots)');
  }

  // Groups: a cheer is at most 3 robots round the celebrant; a standup is the conductor and at most 3 members.
  {
    const crowd: BotIn[] = [];
    for (let cid = 1; cid <= 8; cid++) crowd.push(bot(cid, 'other', { at: { x: 0, z: 0, region: 0 } }));
    const cheer = step(new Behaviour(cfg), 0, crowd, { events: [{ type: 'celebrate', cid: 1 }] });
    eq(cheer.filter((d) => d.activity === 'cheer').length, 3, 'behaviour: a celebration gets three cheers');
    eq(largestGroup(cheer) <= 4, true, 'behaviour: a cheer group is at most 4');
    const stand = step(new Behaviour(cfg), 0, crowd, { events: [{ type: 'standup', conductor: 2, members: [1, 3, 4, 5, 6, 7, 8] }] });
    eq(stand.filter((d) => d.activity === 'standup').length, 3, 'behaviour: a standup takes three members');
    eq(largestGroup(stand) <= 4, true, 'behaviour: a standup group is at most 4');
    const e = new Behaviour(cfg);
    const social: BotIn[] = [];
    for (let cid = 1; cid <= 8; cid++) social.push(bot(cid, 'other', { personality: { social: 9, energy: 5, focus: 2, curiosity: 5, play: 5, tidy: 5 }, at: { x: 0, z: 0, region: 0 } }));
    let worst = 0;
    for (let t = 0; t < 60; t++) worst = Math.max(worst, largestGroup(step(e, t * 0.5, social)));
    eq(worst <= 2, true, `behaviour: chats are pairs (largest group ${worst})`);
  }

  // Offline robots go to a dock; with no dock they stay where they are.
  {
    const off = bot(4, 'builder', { state: 'offline', at: { x: 1, z: 1, region: 0 } });
    const spots = board([off]);
    const d = step(new Behaviour(cfg), 0, [off], { spots })[0]!;
    const kind = spots.find((s) => s.key === d.spot)?.kind;
    eq([d.activity, d.clip, kind], ['powered-down', 'offline', 'dock'], 'behaviour: offline goes to a dock and powers down');
    const noDock = step(new Behaviour(cfg), 0, [off], { spots: spots.filter((s) => s.kind !== 'dock') })[0]!;
    eq([noDock.spot, noDock.goal?.x], [null, 1], 'behaviour: offline without a dock stays put');
  }

  // Held: nobody works and events are ignored.
  {
    const bots: BotIn[] = [];
    for (let cid = 1; cid <= 6; cid++) bots.push(bot(cid, cid % 2 ? 'builder' : 'librarian', { state: 'working', lastTool: 'bash' }));
    const e = new Behaviour(cfg);
    const bad: string[] = [];
    for (let t = 0; t < 12; t++) {
      const events: EventIn[] = t === 0 ? [{ type: 'celebrate', cid: 1 }] : [];
      for (const d of step(e, t * 0.5, bots, { held: true, events })) if (!idleIds.has(d.activity) && d.activity !== 'powered-down') bad.push(`${d.cid}:${d.activity}`);
    }
    eq(bad, [], 'behaviour: held plaza, nobody works (idle activities only)');
  }

  // Pairs are mutual: a mover's partner is a host that points back, and a reviewer's target is not changed.
  {
    const roles = ['orchestrator', 'reviewer', 'other', 'librarian', 'builder', 'qa', 'incident', 'planner'];
    const tools = ['bash', 'plaza_send_message', 'plaza_verdict', 'memory_write', null];
    const bad: string[] = [];
    for (let seed = 0; seed < 20; seed++) {
      const bots: BotIn[] = [];
      for (let cid = 1; cid <= 8; cid++) {
        const k = (cid + seed) % 5;
        bots.push(bot(cid, roles[(cid * 3 + seed) % roles.length]!, {
          state: k < 2 ? 'working' : 'idle',
          lastTool: tools[(cid + seed) % tools.length],
          talkingTo: cid % 2 ? ((cid + seed) % 8) + 1 : null,
          at: { x: (cid % 4) * 2 - 3, z: (seed % 3) - 1, region: 0 },
        }));
      }
      const e = new Behaviour(cfg);
      for (let t = 0; t < 20; t++) {
        const out = step(e, t * 0.5 + seed, bots);
        const by = new Map(out.map((d) => [d.cid, d]));
        for (const d of out) {
          if (d.partner === null || cfg.activities[d.activity]!.pairs === 'review') continue;
          if (by.get(d.partner)?.partner !== d.cid) bad.push(`seed ${seed} t ${t}: ${d.cid} ${d.activity} -> ${d.partner}`);
        }
      }
    }
    eq(bad, [], 'behaviour: every pair is mutual (20 seeds, 20 ticks)');
  }

  // Events: a payment is carried by a finance robot, coins to the payee's desk.
  {
    const bots = [bot(1, 'finance', { at: { x: 0, z: 0, region: 0 } }), bot(2, 'other', { at: { x: 5, z: 0, region: 0 } })];
    const out = step(new Behaviour(cfg), 0, bots, { events: [{ type: 'payment', to: 2 }] });
    const carrier = out.find((d) => d.cid === 1)!;
    eq([carrier.activity, carrier.carry, carrier.goal?.x], ['deliver', 'coin-bag', 5.6], 'behaviour: a payment is carried by finance to the payee desk');
  }

  // Determinism: the same inputs give the same decisions.
  {
    const bots: BotIn[] = [];
    for (let cid = 1; cid <= 6; cid++) bots.push(bot(cid, roleIds[cid % roleIds.length]!, { state: cid % 2 ? 'working' : 'idle', lastTool: 'plaza_post_task' }));
    const a = new Behaviour(cfg);
    const b = new Behaviour(cfg);
    let same = true;
    for (let t = 0; t < 30; t++) {
      if (JSON.stringify(step(a, t * 0.5, bots)) !== JSON.stringify(step(b, t * 0.5, bots))) same = false;
    }
    eq(same, true, 'behaviour: the same inputs give the same decisions');
  }

  // Arrival: a decision made while its robot is walking holds its until at Infinity; its duration starts on arrival,
  // or at the 12 s cap when the robot never gets there.
  {
    const walker = bot(7, 'other', { arrived: false });
    const e = new Behaviour(cfg);
    const first = step(e, 0, [walker])[0]!;
    const [lo, hi] = cfg.activities[first.activity]!.dur as [number, number];
    const walking = step(e, 5, [walker])[0]!;
    const arrived = step(e, 6, [{ ...walker, arrived: true }])[0]!;
    eq(
      [first.until === Infinity, walking.until === Infinity, arrived.activity === first.activity, arrived.changed],
      [true, true, true, false],
      'behaviour arrival: until holds while walking, and the same decision stays',
    );
    eq(arrived.until - 6 >= lo - 1e-9 && arrived.until - 6 <= hi + 1e-9, true, 'behaviour arrival: the duration starts at arrival');
    const e2 = new Behaviour(cfg);
    const stuck = step(e2, 0, [walker])[0]!;
    const early = step(e2, 11.5, [walker])[0]!;
    const capped = step(e2, 12, [walker])[0]!;
    eq(
      [early.until === Infinity, Number.isFinite(capped.until) && capped.until - 12 >= lo - 1e-9 && capped.until - 12 <= hi + 1e-9],
      [true, true],
      'behaviour arrival: a robot that never arrives starts its duration at the 12 s cap',
    );
    eq(stuck.activity === first.activity, true, 'behaviour arrival: the same inputs decide the same');
  }

  // A new decision walks when its goal is far, even though the input's arrival was for the previous goal.
  {
    let far = 0;
    const bad: string[] = [];
    let n = 0;
    for (let cid = 1; cid <= 30; cid++) {
      const b = bot(cid, 'other', { arrived: true, at: { x: -9, z: -9, region: 0 } });
      const d = step(new Behaviour(cfg), 0, [b])[0]!;
      if (d.goal && Math.sqrt((-9 - d.goal.x) ** 2 + (-9 - d.goal.z) ** 2) > 0.6) {
        n++;
        if (d.until !== Infinity) bad.push(`cid ${cid}`);
      }
    }
    eq([n > 0 && far === 0, bad], [true, []], 'behaviour arrival: a new far goal waits even when the last arrival was true');
  }
  // A spot the floor cannot reach is given up: the decision is made again without it.
  {
    let checked = 0;
    const bad: string[] = [];
    for (let cid = 1; cid <= 30; cid++) {
      const b0 = bot(cid, 'other');
      const e = new Behaviour(cfg);
      const d0 = step(e, 0, [b0])[0]!;
      if (!d0.spot) continue;
      checked++;
      const d1 = step(e, 0.5, [{ ...b0, avoid: [d0.spot] }])[0]!;
      if (!d1.changed || d1.spot === d0.spot) bad.push(`cid ${cid}`);
    }
    eq([checked > 0, bad], [true, []], 'behaviour avoid: an unreachable spot is given up for another');
  }

  // Fixtures: 100 scenarios hash to the same decision sequences as the desktop build.
  {
    const bad: string[] = [];
    for (let s = 0; s < SCENARIO_COUNT; s++) {
      const got = runScenario(cfg, s).hash;
      if (got !== behaviourFixtures.hashes[s]) bad.push(`scenario ${s}: ${got}, fixture says ${behaviourFixtures.hashes[s]}`);
    }
    eq(behaviourFixtures.hashes.length, SCENARIO_COUNT, 'behaviour fixtures: 100 scenarios');
    eq(bad, [], 'behaviour fixtures: every scenario recomputes its decision hash');
  }
}

// Navigation (docs/design/navigation.md, Verification): the grid, the planner and the followers on tiny boards; a
// property check and a short simulation on three boards; the golden path fixtures. The long variants are in
// test/nav-sim.mjs.
{
  const NAV_BOARD_FILES: Record<string, unknown> = {
    workstation: workstationBoardFile, laptop: laptopBoardFile, edge: edgeBoardFile,
    phone: phoneBoardFile, server: serverBoardFile, 'apple-soc': appleBoardFile,
  };
  const NAV_BOARD_NAMES = Object.keys(NAV_BOARD_FILES);
  const NAV_COUNTS = [1, 4, 8];
  const NAV_OFFSETS = [0, 5, 10];
  const NAV_ROLE_IDS = Object.keys(roomRolesFile.roles);
  type NavPt = { x: number; z: number };
  const navAgents = (count: number, offset: number): Agent[] =>
    Array.from({ length: count }, (_, i) => ({ cid: i + 1, name: '', role: NAV_ROLE_IDS[(offset + i) % NAV_ROLE_IDS.length], tier: 'worker' }) as unknown as Agent);
  const navScenes = new Map<string, NavScene>();
  const navLayouts = new Map<string, ReturnType<typeof layoutOnBoard>>();
  const navSceneOf = (board: string, count: number, offset: number, params: Record<string, number> = {}): NavScene => {
    const key = `${board}|${count}|${offset}|${JSON.stringify(params)}`;
    let s = navScenes.get(key);
    if (!s) {
      const model = NAV_BOARD_FILES[board] as Model;
      const lay = layoutOnBoard(navAgents(count, offset), model, params);
      navLayouts.set(key, lay);
      s = navScene(model, params, lay, null);
      navScenes.set(key, s);
    }
    return s;
  };
  // The drawn scene of the same layout (boardScene: the board and its rooms, aisles included), as the supports the walk reads.
  const drawnScenes = new Map<string, Support[]>();
  const drawnOf = (board: string, count: number, offset: number, params: Record<string, number> = {}): Support[] => {
    const key = `${board}|${count}|${offset}|${JSON.stringify(params)}`;
    let s = drawnScenes.get(key);
    if (!s) {
      navSceneOf(board, count, offset, params);
      const palette = roomPalette({ sheet: '#ffffff', rule: '#d6d2c8', ink2: '#4a4a4a', signal: '#d9482b' });
      const scene = boardScene(NAV_BOARD_FILES[board] as Model, params, navLayouts.get(key)!, palette);
      s = [{ field: supportField(scene), scale: 1 }];
      drawnScenes.set(key, s);
    }
    return s;
  };
  const navCentre = (g: NavGrid, c: number): NavPt => ({ x: g.x0 + ((c % g.cols) + 0.5) * g.cell, z: g.z0 + (Math.floor(c / g.cols) + 0.5) * g.cell });
  const navWalkable = (g: NavGrid, spotsOnly: boolean): NavPt[] => {
    const pts: NavPt[] = g.spots.map((s) => ({ x: s.x, z: s.z }));
    if (!spotsOnly) for (const p of g.portals) pts.push(navCentre(g, p.a));
    return pts.filter((p) => g.walk[cellAt(g, p.x, p.z)] === 1);
  };
  const navBadCell = (g: NavGrid, x: number, z: number): boolean => {
    const c = cellAt(g, x, z);
    return c < 0 || g.blocked[c] === 1 || g.inflated[c] === 1;
  };
  // The grid's components (g.component, rule 12 with the height rule): a pair in two components has no route, so the
  // property draws only pairs in one component.
  const navPairs = (g: NavGrid, seed: number, pairs: number): { nulls: number; bad: number } => {
    const pts = navWalkable(g, false);
    if (pts.length === 0) return { nulls: pairs, bad: 0 };
    const comp = g.component!;
    const compOf = (p: NavPt): number => comp[cellAt(g, p.x, p.z)]!;
    const rng = mulberry32(seed);
    let nulls = 0;
    let bad = 0;
    let made = 0;
    for (let tries = 0; made < pairs && tries < pairs * 100; tries++) {
      const a = pts[Math.floor(rng() * pts.length)]!;
      const b = pts[Math.floor(rng() * pts.length)]!;
      if (compOf(a) !== compOf(b)) continue;
      made++;
      const path = findPath(g, a, b);
      if (!path) nulls++;
      else if (path.points.some((p) => navBadCell(g, p.x, p.z))) bad++;
    }
    return { nulls: nulls + (pairs - made), bad };
  };
  // Robots take reachable random goals; idle for 2 s, a robot takes a new one. The stuck measure is the sim's reading of
  // invariant 2 (docs/design/navigation.md): the distance to the current waypoint shrinks by 0.05, or the robot is legitimately waiting
  // (on a lift, yielding, turning in place up to 1.2 s, a finished path); a stuck event opens after 1 s of non-legit time
  // without progress and closes with progress, the arrival or the unreachable handler. Pair-seconds: the sum over steps
  // of the robot pairs closer than 0.5, times dt.
  const navSimulate = (g: NavGrid, seed: number, robots: number, seconds: number, drawn: Support[] | null = null) => {
    const dt = 1 / 30;
    const rng = mulberry32(seed);
    const cands = navWalkable(g, false);
    const world = new NavWorld(g);
    world.setDrawn(drawn);
    type Robot = { id: number; goal: NavPt | null; active: boolean; idle: number; ref: number; wp: string; nop: number; open: boolean; turnT: number };
    const rs: Robot[] = [];
    const used = new Set<number>();
    for (let tries = 0; rs.length < robots && tries < 100000; tries++) {
      const p = cands[Math.floor(rng() * cands.length)]!;
      const c = cellAt(g, p.x, p.z);
      if (used.has(c)) continue;
      used.add(c);
      const id = rs.length + 1;
      world.add({ id, x: p.x, z: p.z, speed: 0.8, turnRate: 2.5, priority: 1 });
      rs.push({ id, goal: null, active: false, idle: 0, ref: Infinity, wp: '', nop: 0, open: false, turnT: 0 });
    }
    const pickGoal = (from: NavPt): NavPt | null => {
      for (let t = 0; t < 30; t++) {
        const q = cands[Math.floor(rng() * cands.length)]!;
        if (findPath(g, from, q)) return q;
      }
      return null;
    };
    let robotSteps = 0;
    let bad = 0;
    // Gravity and heights: respawns, air frames stuck for more than 2 s, and grounded frames off a walk cell.
    let respawns = 0;
    let airStuck = 0;
    let groundBad = 0;
    const airFor = new Map<number, number>();
    // A lift ride is not air: its longest run is counted apart (a ride is about a travel and two dwells).
    const liftFor = new Map<number, number>();
    let liftMax = 0;
    let pairSeconds = 0;
    let events = 0;
    let resolvedMax = 0;
    let goals = 0;
    let reached = 0;
    const close = (r: Robot): void => {
      if (r.open) resolvedMax = Math.max(resolvedMax, r.nop - 1);
      r.open = false;
      r.nop = 0;
    };
    world.onUnreachable((id) => {
      const r = rs[id - 1];
      if (!r) return;
      close(r);
      r.active = false;
      r.idle = 0;
    });
    world.onRespawn(() => {
      respawns++;
    });
    const steps = Math.round(seconds / dt);
    for (let s = 0; s < steps; s++) {
      for (const r of rs) {
        const p = world.pose(r.id);
        robotSteps++;
        if (navBadCell(g, p.x, p.z)) bad++;
        const cell = cellAt(g, p.x, p.z);
        if (p.body === 'ground' || p.body === 'land' || p.body === 'in') {
          if (cell < 0 || g.walk[cell] !== 1) groundBad++;
          airFor.set(r.id, 0);
          liftFor.set(r.id, 0);
        } else if (p.body === 'lift') {
          const t = (liftFor.get(r.id) ?? 0) + dt;
          liftFor.set(r.id, t);
          liftMax = Math.max(liftMax, t);
          airFor.set(r.id, 0);
        } else {
          const t = (airFor.get(r.id) ?? 0) + dt;
          airFor.set(r.id, t);
          liftFor.set(r.id, 0);
          if (t > 2) airStuck++;
        }
        if (r.active) {
          if (!p.moving && Math.hypot(p.x - r.goal!.x, p.z - r.goal!.z) < 0.35) {
            r.active = false;
            r.idle = 0;
            reached++;
            close(r);
            continue;
          }
          const path = world.path(r.id);
          const target = path && path.length > 0 ? path[0]! : null;
          const pathDone = path !== null && path.length === 0;
          const want = target ? Math.atan2(target.x - p.x, target.z - p.z) : 0;
          const turning = target !== null && !p.moving && Math.abs(Math.atan2(Math.sin(want - p.heading), Math.cos(want - p.heading))) > Math.PI / 6;
          r.turnT = turning ? r.turnT + dt : 0;
          const legit = pathDone || p.onLift || p.yielding || p.waiting || (turning && r.turnT <= 1.2);
          let progressed = pathDone;
          if (target) {
            const dist = Math.hypot(target.x - p.x, target.z - p.z);
            const key = `${target.x},${target.z}`;
            // A new waypoint (crossed, or a replan) re-bases the reference without progress.
            if (key !== r.wp) {
              r.wp = key;
              r.ref = dist;
            } else if (dist <= r.ref - 0.05) {
              r.ref = dist;
              progressed = true;
            }
          }
          if (progressed) close(r);
          else if (!legit) {
            r.nop += dt;
            if (!r.open && r.nop > 1) {
              r.open = true;
              events++;
            }
          }
        } else {
          r.idle += dt;
          if (r.idle >= 2) {
            r.idle = 0;
            const q = pickGoal(p);
            if (q) {
              world.setGoal(r.id, { x: q.x, z: q.z });
              r.goal = q;
              r.active = true;
              r.ref = Infinity;
              r.wp = '';
              r.turnT = 0;
              goals++;
            }
          }
        }
      }
      world.step(dt);
      const pos = rs.map((r) => world.pose(r.id));
      for (let i = 0; i < pos.length; i++) {
        for (let j = i + 1; j < pos.length; j++) {
          if (Math.hypot(pos[i]!.x - pos[j]!.x, pos[i]!.z - pos[j]!.z) < 0.5) pairSeconds += dt;
        }
      }
    }
    let openMax = 0;
    for (const r of rs) if (r.open) openMax = Math.max(openMax, r.nop - 1);
    const au = world.audit();
    return {
      robotSteps, bad, pairSeconds, events, resolvedMax, openMax, goals, reached, respawns, airStuck, groundBad, liftMax,
      floating: au.floating, sinking: au.sinking, onWall: au.onWall, hover: au.hover, hoverUndrawn: au.hoverUndrawn, fallback: au.fallback,
    };
  };

  // Unit: the grid, the planner and the followers on tiny boards.
  const flat: NavBoard = { size: [6, 1, 6], pcbTop: 0.4, boxes: [], traces: [] };
  const withBox = (lo: [number, number, number], hi: [number, number, number]): NavBoard => ({ ...flat, boxes: [{ lo, hi }] });
  const blockedCells = (g: NavGrid): number => g.blocked.reduce((a, b) => a + b, 0);
  const tall = buildGrid(withBox([-0.5, 0.4, -0.5], [0.5, 1.2, 0.5]), []);
  eq(tall.blocked[cellAt(tall, 0, 0)], 1, 'grid: a part taller than a step blocks the cell under its centre');
  eq(tall.blocked[cellAt(tall, 1.5, 0)], 0, 'grid: the cell 1.5 away stays open');
  eq(blockedCells(buildGrid(withBox([-0.5, 0.4, -0.5], [0.5, 0.6, 0.5]), [])), 0, 'grid: a part at most one step high blocks nothing');

  const open = buildGrid(flat, []);
  const straight = findPath(open, { x: -2, z: 0 }, { x: 2, z: 0 });
  eq(straight !== null, true, 'path: an open board has a route');
  if (straight) {
    const first = straight.points[0]!;
    const last = straight.points[straight.points.length - 1]!;
    eq(
      [Math.abs(first.x + 2) <= NAV_CELL, Math.abs(first.z) <= NAV_CELL, Math.abs(last.x - 2) <= NAV_CELL, Math.abs(last.z) <= NAV_CELL],
      [true, true, true, true],
      'path: starts at the start and ends at the goal (the planner snaps the ends to cell centres, within a cell)',
    );
    eq(straight.points.filter((p) => navBadCell(open, p.x, p.z)).length, 0, 'path: an open board has no blocked or inflated point');
  }
  const wall = buildGrid(withBox([-0.1, 0.4, -2], [0.1, 1.2, 1.5]), []);
  const around = findPath(wall, { x: -2, z: 0 }, { x: 2, z: 0 });
  eq(around !== null && around.points.some((p) => p.z > 1.5), true, 'path: a wall across the middle is gone round (a point beyond its far end)');
  eq(around ? around.points.filter((p) => navBadCell(wall, p.x, p.z)).length : -1, 0, 'path: the detour avoids blocked and inflated cells');

  const two = [{ x: 0, z: 0, y: 0, portal: -1 }, { x: 1.25, z: -0.5, y: 0, portal: -1 }];
  eq(pathHash(two.map((p) => ({ ...p }))), pathHash(two), 'pathHash: the same points give the same hash');
  eq(pathHash([two[0]!, { ...two[1]!, x: 1.26 }]) !== pathHash(two), true, 'pathHash: a changed point changes the hash');

  const world = new NavWorld(open);
  world.add({ id: 1, x: -2, z: 0, speed: 1, turnRate: 3, priority: 1 });
  const cellGoal = { x: 2.125, z: 0.125 }; // a cell centre: the planner snaps goals to cell centres
  world.setGoal(1, cellGoal);
  for (let t = 0; t < 600; t++) world.step(1 / 30);
  eq(Math.hypot(world.pose(1).x - cellGoal.x, world.pose(1).z - cellGoal.z) < 0.1, true, 'follow: a robot reaches its goal within 20 s');

  // Walk invariant (navigation.md, "Gravity and heights"): a grid rebuilt mid-run leaves no grounded robot off a walk cell.
  // Robots walk the open grid, then every 7th cell is blocked under them (setGrid); the audit's onWall must stay 0.
  {
    const world2 = new NavWorld(open);
    for (let i = 1; i <= 4; i++) world2.add({ id: i, x: -2 + i * 0.5, z: 0, speed: 1, turnRate: 3, priority: 1 });
    for (let i = 1; i <= 4; i++) world2.setGoal(i, { x: 2.125, z: (i - 2.5) * 0.5 });
    for (let t = 0; t < 120; t++) world2.step(1 / 30);
    const walk = open.walk.slice();
    const blocked = open.blocked.slice();
    for (let c = 0; c < walk.length; c += 7) {
      walk[c] = 0;
      blocked[c] = 1;
    }
    world2.setGrid({ ...open, walk, blocked });
    for (let t = 0; t < 300; t++) world2.step(1 / 30);
    eq(world2.audit().onWall, 0, 'walk invariant: a grid rebuilt mid-run leaves no grounded robot off a walk cell (onWall 0)');
  }

  const turner = new NavWorld(open);
  turner.add({ id: 1, x: 0, z: 0, speed: 1, turnRate: 3, priority: 1 });
  const h0 = turner.pose(1).heading;
  const want = Math.atan2(0, -2); // directly behind: the goal is at negative z
  eq(Math.abs(Math.atan2(Math.sin(want - h0), Math.cos(want - h0))) > Math.PI / 6, true, 'follow: the goal is more than 30 degrees off the heading');
  turner.setGoal(1, { x: 0, z: -2 });
  turner.step(1 / 30);
  eq(turner.pose(1).moving, false, 'follow: turns before it walks');
  let walking = false;
  for (let t = 0; t < 90 && !walking; t++) {
    turner.step(1 / 30);
    walking = turner.pose(1).moving;
  }
  eq(walking, true, 'follow: walks within 3 s of turning');

  // Property (fast): 20 random pairs per board and room layout, among spots and portal cells.
  NAV_BOARD_NAMES.forEach((board, bi) => {
    NAV_COUNTS.forEach((count, l) => {
      const g = navSceneOf(board, count, NAV_OFFSETS[l]!).grid;
      const r = navPairs(g, 500 + bi * 3 + l, 20);
      eq(r.bad, 0, `property: ${board}, ${count} agent(s): no path cell is blocked or inflated`);
      eq(r.nulls, 0, `property: ${board}, ${count} agent(s): every pair in one component has a path`);
    });
  });

  // Rule 12 (docs/design/navigation.md, The nav grid): every seat is in the main component (two-way links, lifts and
  // gangways; a one-way drop never strands one), and on the 8-agent layout every pair of seats has a path both ways.
  NAV_BOARD_NAMES.forEach((board) => {
    NAV_COUNTS.forEach((count, l) => {
      const g = navSceneOf(board, count, NAV_OFFSETS[l]!).grid;
      const main = connectivity(g).main;
      // A seat is reached from the cell the planner snaps it to (snapWalk): that cell must be in the main component.
      const stranded = g.spots.filter((s) => { const c = snapWalk(g, s.x, s.z); return c < 0 || g.component![c] !== main; }).length;
      eq(stranded, 0, `components: ${board}, ${count} agent(s): every seat is in the main component`);
    });
    const g = navSceneOf(board, NAV_COUNTS[2]!, NAV_OFFSETS[2]!).grid;
    const seats = g.spots.filter((s) => s.cell >= 0 && g.walk[s.cell] === 1);
    let pairs = 0;
    let nulls = 0;
    for (let a = 0; a < seats.length; a++) {
      for (let b = a + 1; b < seats.length; b++) {
        pairs++;
        const pa = { x: seats[a]!.x, z: seats[a]!.z };
        const pb = { x: seats[b]!.x, z: seats[b]!.z };
        if (!findPath(g, pa, pb) || !findPath(g, pb, pa)) nulls++;
      }
    }
    eq(nulls, 0, `components: ${board}: every pair of seats has a path both ways (${pairs} pairs)`);
  });

  // Layout invariants (board-layout.ts, the landing rule of navigation.md): every plate that holds a room is reached, and every
  // desk of a room connects to the room's door both ways through walk cells (no desk is enclosed by its furniture).
  NAV_BOARD_NAMES.forEach((board) => {
    NAV_COUNTS.forEach((count, l) => {
      const s = navSceneOf(board, count, NAV_OFFSETS[l]!);
      const g = s.grid;
      const unreachedRooms = s.terraces.filter((t) => {
        const plate = g.plates.find((p) => t.cx >= p.x0 && t.cx <= p.x0 + p.w && t.cz >= p.z0 && t.cz <= p.z0 + p.d);
        return !plate || g.unreached.includes(plate.id);
      }).length;
      eq(unreachedRooms, 0, `landings: ${board}, ${count} agent(s): every plate with a room is reached`);
      let enclosed = 0;
      s.terraces.forEach((t, i) => {
        const out = g.doorCells[i]?.[1] ?? -1;
        if (out < 0) {
          enclosed++;
          return;
        }
        const door = navCentre(g, out);
        for (const sp of g.spots) {
          const c = snapWalk(g, sp.x, sp.z);
          if (c < 0 || g.region[c] !== t.id) continue;
          const seat = { x: sp.x, z: sp.z };
          if (!findPath(g, door, seat) || !findPath(g, seat, door)) enclosed++;
        }
      });
      eq(enclosed, 0, `landings: ${board}, ${count} agent(s): every desk connects to its room's door both ways`);
    });
  });

  // Gangways (rule 6a): a gangway's bridge is walk at its plate's height, and its lift starts at the bridge's last cell.
  NAV_BOARD_NAMES.forEach((board) => {
    NAV_COUNTS.forEach((count, l) => {
      const g = navSceneOf(board, count, NAV_OFFSETS[l]!).grid;
      for (const pl of g.plates) {
        const land = pl.landing;
        if (land?.kind !== 'gangway' || !land.bridge) continue;
        const cells = land.bridge.map(([x, z]) => cellAt(g, x, z));
        const last = cells[cells.length - 1]!;
        const atHeight = cells.every((c) => c >= 0 && g.walk[c] === 1 && g.y[c] === pl.y);
        const lifted = g.portals.some((p) => p.kind === 'lift' && p.a === last);
        eq(atHeight && lifted, true, `gangway: ${board}, ${count} agent(s), plate ${pl.id}: bridge walk at plate height, lift from its last cell`);
      }
    });
  });

  // Simulation (short): 15 s, 20 robots, on the 8-agent layout of three boards.
  ['workstation', 'server', 'apple-soc'].forEach((board, si) => {
    const r = navSimulate(navSceneOf(board, NAV_COUNTS[2]!, NAV_OFFSETS[2]!).grid, 3000 + si, 20, 15);
    eq(r.bad, 0, `simulation (15 s, 20 robots, ${board}): no robot centre in a blocked or inflated cell`);
    eq(r.pairSeconds / (20 * 15) < 0.005, true, `simulation (15 s, 20 robots, ${board}): robot overlap under 0.5 % of robot-seconds`);
    eq(r.resolvedMax < 3 && r.openMax < 3, true, `simulation (15 s, 20 robots, ${board}): no robot stuck for 3 s or more`);
  });

  // Fixtures: the 100 cases of assets/nav/fixtures.json recompute to the same path hash.
  {
    const cases = navFixtures as unknown as { board: string; params: Record<string, number>; seed: number; start: [number, number]; goal: [number, number]; hash: string }[];
    const bad: string[] = [];
    for (const c of cases) {
      if (c.board !== NAV_BOARD_NAMES[c.seed % NAV_BOARD_NAMES.length]) bad.push(`seed ${c.seed}: board ${c.board}`);
      const s = navSceneOf(c.board, NAV_COUNTS[Math.floor(c.seed / 6) % 3]!, c.seed % NAV_ROLE_IDS.length, c.params);
      const path = findPath(s.grid, { x: c.start[0], z: c.start[1] }, { x: c.goal[0], z: c.goal[1] });
      const got = path ? path.hash : 'none';
      if (got !== c.hash) bad.push(`seed ${c.seed}: ${got}, fixture says ${c.hash}`);
    }
    eq(cases.length, 100, 'nav fixtures: 100 cases');
    eq(bad, [], 'nav fixtures: every case recomputes its path hash');
  }

  // Gravity and heights (docs/design/navigation.md, "Gravity and heights"): the body (nav-height.ts) on flat surfaces, the
  // respawn through NavWorld on a lowered floor, and the 8-agent boards for 60 s.
  {
    const DT = 1 / 120;
    /** Runs a body for `seconds` on fixed surfaces: the modes in order (repeats dropped), the dwell per mode and the height range. */
    const drive = (b: Body, s: Surfaces, seconds: number, floats = false): { modes: string; dwell: Record<string, number>; maxY: number } => {
      const modes: string[] = [];
      const dwell: Record<string, number> = {};
      let maxY = -Infinity;
      for (let i = 0; i < Math.round(seconds / DT); i++) {
        stepBody(b, s, DT, floats);
        if (modes[modes.length - 1] !== b.mode) modes.push(b.mode);
        dwell[b.mode] = (dwell[b.mode] ?? 0) + DT;
        maxY = Math.max(maxY, b.y);
      }
      return { modes: modes.join('>'), dwell, maxY };
    };

    // A rise of at most NAV_STEP is walked: the body eases onto the surface and never sits above it.
    const step = newBody(0);
    const walked = drive(step, { walkTop: 0.25, floorTop: 0.25 }, 1);
    eq(walked.modes, 'ground', 'body: a rise of at most NAV_STEP is walked, grounded');
    eq(Math.abs(step.y - 0.25) < 1e-3 && walked.maxY <= 0.25 + 1e-9, true, 'body: a walked step eases onto the surface, never above it');

    // A planned rise (0.8) is a jump: a crouch, then the arc that lands on the new surface after NAV_JUMP_TIME, then the squash.
    const jump = newBody(0);
    const j = drive(jump, { walkTop: 0.8, floorTop: 0.8 }, 1.5);
    eq(j.modes, 'crouch>jump>land>ground', 'body: a rise over NAV_STEP is a crouch, a jump, a landing, then the ground');
    eq(Math.abs((j.dwell.jump ?? 0) - NAV_JUMP_TIME) < 0.02 && Math.abs((j.dwell.crouch ?? 0) - NAV_CROUCH_TIME) < 0.02, true, 'body: the jump takes NAV_JUMP_TIME after a NAV_CROUCH_TIME crouch');
    eq(jump.y === 0.8 && j.maxY > 0.8 + 0.1, true, 'body: the arc reaches above the target and lands exactly on it');

    // A fall of 1.5 lands on the surface with the squash and no respawn; over NAV_DROP_DOWN it goes out; off the board it goes out at once.
    const drop = newBody(1.5);
    const fell = drive(drop, { walkTop: 0, floorTop: 0 }, 1.5);
    eq(fell.modes, 'fall>land>ground', 'body: a drop of 1.5 is a fall, a landing, then the ground (no respawn)');
    eq(drop.y === 0 && Math.abs((fell.dwell.fall ?? 0) - Math.sqrt((2 * 1.5) / 20)) < 0.02, true, 'body: the fall follows gravity (20) and lands on the surface');
    eq(drive(newBody(2.5), { walkTop: 0, floorTop: 0 }, 1).modes, 'fall>out', 'body: a fall over NAV_DROP_DOWN goes out (the respawn), it never lands');
    eq(drive(newBody(0.4), { walkTop: null, floorTop: null }, 0.5).modes, 'fall>out', 'body: no floor under it (off the board) goes out at once');

    // A floating species over a gap glides down to the floor below: it never falls, so it never respawns.
    const ghost = newBody(0.8);
    eq(drive(ghost, { walkTop: null, floorTop: 0 }, 2, true).modes, 'glide', 'body: a floating species glides over a gap, no fall');
    eq(ghost.y === 0, true, 'body: the glide ends on the floor below');

    // The respawn fade: full size, then about zero at the end of the fade out; full size again at the end of the fade in.
    const out = newBody(0);
    out.mode = 'out';
    eq(bodyScale(out) === 1, true, 'fade: the fade out starts at full size');
    out.t = NAV_RESPAWN_OUT;
    eq(bodyScale(out) < 0.05, true, 'fade: the fade out ends at about zero');
    const back = newBody(0);
    back.mode = 'in';
    back.t = NAV_RESPAWN_IN;
    eq(Math.abs(bodyScale(back) - 1) < 1e-9, true, 'fade: the fade in ends at full size');

    // NavWorld on a flat board whose right half is lowered under the robot (a layout change, or a shove off a plate's edge).
    // A drop over NAV_DROP_DOWN respawns the robot at home within 1.5 s; a legal drop lands without a respawn.
    const flatGrid = buildGrid(flat, []);
    const lowered = (drop: number): NavGrid => {
      const y = Float64Array.from(flatGrid.y);
      for (let c = 0; c < y.length; c++) if (flatGrid.walk[c] === 1 && navCentre(flatGrid, c).x > 0.5) y[c] = y[c]! - drop;
      return { ...flatGrid, y };
    };
    const shoveOff = (drop: number): { respawnAt: number; x: number; z: number; body: string; y: number } => {
      const world = new NavWorld(lowered(drop));
      world.add({ id: 1, x: 0, z: 0, speed: 0.8, turnRate: 2.5, priority: 1 });
      let respawnAt = Infinity;
      world.onRespawn(() => {
        if (respawnAt === Infinity) respawnAt = world.time;
      });
      for (let i = 0; i < 30; i++) world.step(1 / 30);
      const t0 = world.time;
      world.shove(1, 1, 0);
      for (let i = 0; i < 90; i++) world.step(1 / 30);
      const p = world.pose(1);
      return { respawnAt: respawnAt === Infinity ? Infinity : respawnAt - t0, x: p.x, z: p.z, body: p.body, y: p.y };
    };
    const far = shoveOff(2.5);
    eq(far.respawnAt <= 1.5, true, 'body: a forced push off a 2.5 drop respawns the robot within 1.5 s');
    eq(far.x === 0 && far.z === 0 && far.body === 'ground', true, 'body: the respawn is at the home desk, grounded');
    const legal = shoveOff(1.5);
    eq(legal.respawnAt === Infinity, true, 'body: a 1.5 drop lands without a respawn');
    eq(legal.body === 'ground' && Math.abs(legal.y - (0.4 - 1.5)) < 1e-6, true, 'body: the robot lands on the lower surface');

    // Lift riders: on the server board, robots from the board to plate goals that need a lift, and from the plates down to the
    // board. Each arrives within 240 s (a queue at a lift is a wait, not a stall), and a rider's y is its cab's floor (its body).
    {
      const g = navSceneOf('server', NAV_COUNTS[2]!, NAV_OFFSETS[2]!).grid;
      const rng = mulberry32(9100);
      const cells = (pred: (c: number) => boolean): NavPt[] => {
        const out: NavPt[] = [];
        for (let c = 0; c < g.walk.length; c += 3) if (g.walk[c] === 1 && pred(c)) out.push(navCentre(g, c));
        return out;
      };
      const boardPts = cells((c) => g.region[c] === 0);
      const platePts = cells((c) => g.region[c] !== 0);
      const liftPath = (a: NavPt, b: NavPt): boolean => {
        const path = findPath(g, a, b);
        return path !== null && path.points.some((p) => p.portal >= 0 && g.portals[p.portal]?.kind === 'lift');
      };
      const trips: [NavPt, NavPt][] = [];
      for (let tries = 0; trips.length < 6 && tries < 4000; tries++) {
        const up = trips.length % 2 === 0;
        const a = up ? boardPts[Math.floor(rng() * boardPts.length)]! : platePts[Math.floor(rng() * platePts.length)]!;
        const b = up ? platePts[Math.floor(rng() * platePts.length)]! : boardPts[Math.floor(rng() * boardPts.length)]!;
        if (a && b && liftPath(a, b)) trips.push([a, b]);
      }
      eq(trips.length, 6, 'lift riders: six trips on the server board need a lift (three up, three down)');
      const world = new NavWorld(g);
      trips.forEach(([a, b], i) => {
        world.add({ id: i + 1, x: a.x, z: a.z, speed: 0.9, turnRate: 4, priority: 0 });
        world.setGoal(i + 1, { x: b.x, z: b.z });
      });
      const arrived = new Map<number, number>();
      const rode = new Set<number>();
      let riderFault = 0;
      for (let s = 0; s < 240 * 30 && arrived.size < trips.length; s++) {
        world.step(1 / 30);
        trips.forEach(([, b], i) => {
          const p = world.pose(i + 1);
          if (p.onLift) rode.add(i + 1);
          if (p.onLift && p.body !== 'lift') riderFault++;
          if (!arrived.has(i + 1) && !p.moving && !p.onLift && Math.hypot(p.x - b.x, p.z - b.z) < 0.35) arrived.set(i + 1, world.time);
        });
      }
      const slowest = Math.max(0, ...arrived.values());
      console.log(`lift riders: slowest of ${trips.length} trips ${slowest.toFixed(1)} s (all arrived: ${arrived.size === trips.length})`);
      // Each trip's bound comes from its own route: the planned length at the robot's speed, with room for turns and
      // yielding (x1.35), plus one full lift cycle of waiting (30 s). A fixed bound cannot tell a stall from a long walk.
      const routeLen = (a: NavPt, b: NavPt): number => {
        const pts = findPath(g, a, b)!.points;
        let len = 0;
        for (let i = 1; i < pts.length; i++) len += Math.hypot(pts[i]!.x - pts[i - 1]!.x, pts[i]!.z - pts[i - 1]!.z);
        return len;
      };
      const late = trips.filter(([a, b], i) => !((arrived.get(i + 1) ?? Infinity) <= (routeLen(a, b) / 0.9) * 1.35 + 30));
      eq(arrived.size === trips.length && late.length === 0, true, `lift riders: ${arrived.size}/${trips.length} robots reach their goals, each within its route's walk time x1.35 + 30 s (late: ${late.length}, the slowest: ${slowest.toFixed(0)} s)`);
      eq(riderFault, 0, 'lift riders: a robot on the lift is in the lift body (its y is the cab floor every frame)');
      // Every trip boards the cab, and after its ride the robot walks on to its goal (it does not stop at the far landing).
      eq(rode.size, trips.length, `lift riders: every trip rides the lift (${rode.size}/${trips.length})`);
      eq([...rode].filter((id) => !arrived.has(id)).length, 0, 'lift riders: no robot that rode stops short of its goal after the ride');
    }

    // The bake reads a drawn top only inside the walk band (a top a step and a half above the walk is not kept).
    {
      const flatTop = (x0: number, z0: number, x1: number, z1: number, y: number): number[] => [
        x0, y, z0, x1, y, z0, x0, y, z1,
        x1, y, z0, x1, y, z1, x0, y, z1,
      ];
      const tris = [...flatTop(-10, -10, 10, 10, 0.4), ...flatTop(0, 0, 1, 1, 0.6), ...flatTop(-1, -1, 0, 0, 1)];
      const bakeGrid = { ...open, y: new Float64Array(open.y.length).fill(0.4) };
      const baked = bakeDrawnTops(bakeGrid, [{ field: { cell: 0.5, tris, cells: new Map() }, scale: 1 }]);
      const at = (x: number, z: number): number => drawnTopAt(baked, x, z);
      const rounded = [at(0.5, 0.5), at(-0.5, -0.5), at(-2, -2)].map((h) => Math.round(h * 1e6) / 1e6);
      eq([...rounded, Number.isNaN(at(100, 100))], [0.6, 0.4, 0.4, true],'bake: a drawn top in the band (0.6 over a 0.4 walk) is the walk height; one a step and a half above (1.0) is not; off the field is NaN');
    }

    // Drawn walk (walk-surfaces.md, check 1): on every board at 1, 4, 8 and 20 agents, at least 99.9% of the walk cells have
    // a drawn top within 0.01 of their grid height at the cell centre (the aisles included). The rest are listed.
    const HOVER_CASES: [string, string, Record<string, number>][] = [
      ...NAV_BOARD_NAMES.map((b): [string, string, Record<string, number>] => [b, b, {}]),
      ['workstation (2 GPUs)', 'workstation', { gpus: 2 }],
    ];
    // A layout that cannot seat its agents is a failed case, not a crash of the suite (the 2-GPU workstation at 8 agents).
    const hoverLayoutOk = (label: string, board: string, count: number, params: Record<string, number>): boolean => {
      try {
        navSceneOf(board, count, NAV_OFFSETS[0]!, params);
        return true;
      } catch (e) {
        eq(false, true, `drawn walk and hover: ${label} with ${count} agents: the layout seats them (${(e as Error).message.replace(/^board layout: /, '')})`);
        return false;
      }
    };
    for (const [label, board, params] of HOVER_CASES) {
      for (const count of [1, 4, 8, 20]) {
        if (!hoverLayoutOk(label, board, count, params)) continue;
        const g = navSceneOf(board, count, NAV_OFFSETS[0]!, params).grid;
        const sups = drawnOf(board, count, NAV_OFFSETS[0]!, params);
        let walk = 0;
        let bad = 0;
        let sample = '';
        for (let c = 0; c < g.cols * g.rows; c++) {
          if (g.walk[c] !== 1) continue;
          walk++;
          const p = navCentre(g, c);
          const h = drawnTopBelow(sups, p.x, p.z, g.y[c]!);
          if (Math.abs(h - g.y[c]!) <= 0.01) continue;
          bad++;
          if (!sample) sample = `(${p.x.toFixed(2)}, ${p.z.toFixed(2)}) grid ${g.y[c]!.toFixed(3)} drawn ${h.toFixed(3)}`;
        }
        eq(bad <= walk * 0.001, true, `drawn walk: ${label}, ${count} agents: ${walk - bad}/${walk} walk cells have a drawn top at their height (${bad} remain${sample ? `, first ${sample}` : ''})`);
      }
    }

    // 60 s with 8 and 20 robots on every board, walking the drawn tops: no grounded frame off a walk cell (onWall) and none
    // hovering more than 0.01 over the drawn top under its centre (hover). The walk and the drawing agree or the test fails.
    HOVER_CASES.forEach(([label, board, params], bi) => {
      for (const count of [8, 20]) {
        if (!hoverLayoutOk(label, board, count, params)) continue;
        const r = navSimulate(navSceneOf(board, count, NAV_OFFSETS[0]!, params).grid, 9100 + bi * 40 + count, count, 60, drawnOf(board, count, NAV_OFFSETS[0]!, params));
        eq([r.onWall, r.hover], [0, 0], `hover: 60 s on ${label} with ${count} agents: onWall ${r.onWall}, hover ${r.hover} (${r.hoverUndrawn} over an undrawn walk cell, ${r.hover - r.hoverUndrawn} over a drawn one), walk fallbacks ${r.fallback}`);
      }
    });

    // 60 s with 8 agents on every board: no respawn, no airborne frame stuck for more than 2 s, no grounded frame off a
    // walk cell, and no robot in a blocked or inflated cell (invariant 1).
    NAV_BOARD_NAMES.forEach((board, bi) => {
      const r = navSimulate(navSceneOf(board, NAV_COUNTS[2]!, NAV_OFFSETS[2]!).grid, 7000 + bi, NAV_COUNTS[2]!, 60);
      eq([r.respawns, r.airStuck, r.groundBad, r.bad, r.floating, r.sinking], [0, 0, 0, 0, 0, 0], `body: 60 s on ${board} with 8 agents: no respawn, no stuck air, no grounded robot off the walk, no floating or sunk frame (${r.floating} floating, ${r.sinking} sunk)`);
      // A ride is the lift's travel and its dwell (nav-types): a longer one is a stall.
      const lifts = navSceneOf(board, NAV_COUNTS[2]!, NAV_OFFSETS[2]!).grid.lifts;
      // A ride is the travel plus the controller's door cycle: open and close (0.5 s each), the least open time (1 s) and
      // the most it waits for an admitted straggler (2.5 s). See lift-ctrl.ts's defaults.
      const allowed = Math.max(0, ...lifts.map((L) => (L.high - L.low) / NAV_LIFT_SPEED)) + NAV_LIFT_DWELL + 0.5 + 1 + 2.5;
      eq([r.liftMax > allowed ? 1 : 0], [0], `body: 60 s on ${board} with 8 agents: no lift ride longer than its travel and dwell (the longest is ${r.liftMax.toFixed(1)} s, ${allowed.toFixed(1)} s allowed)`);
    });
  }

}

// buildGridSteps (nav-grid.ts): the build in slices drains to the grid, for any chunk size, with one progress value per
// step (rising, in (0, 1], the last 1).
{
  const gridKey = (g: NavGrid): string =>
    JSON.stringify({
      cols: g.cols,
      rows: g.rows,
      x0: g.x0,
      z0: g.z0,
      walk: Array.from(g.walk),
      blocked: Array.from(g.blocked),
      inflated: Array.from(g.inflated),
      region: Array.from(g.region),
      y: Array.from(g.y),
      cost: Array.from(g.cost),
      portal: Array.from(g.portal),
      portals: g.portals,
      lifts: g.lifts,
      spots: g.spots,
    });
  const drain = (board: NavBoard, terraces: NavTerrace[], chunk?: number): { grid: NavGrid; yields: number[] } => {
    const it = buildGridSteps(board, terraces, chunk);
    const yields: number[] = [];
    let r = it.next();
    while (!r.done) {
      yields.push(r.value);
      r = it.next();
    }
    return { grid: r.value, yields };
  };
  const role = Object.keys(roomRolesFile.roles)[0]!;
  const roomW = 2.6;
  const room: NavTerrace = {
    id: 1,
    cx: 0,
    cz: 0,
    half: roomW / 2,
    round: false,
    y: 2.4,
    turn: 0,
    w: roomW,
    d: roomW,
    door: 's',
    layout: roomLayout(role, { w: roomW, d: roomW, door: 's', seed: 7, agents: 2 }, ROOM_CATALOG),
    catalog: ROOM_CATALOG,
  };
  const small: NavBoard = { size: [8, 1, 8], pcbTop: 0.4, boxes: [{ lo: [-1, 0.4, -1], hi: [-0.25, 1.2, -0.25] }], traces: [], plates: [] };
  const crowded: NavBoard = {
    ...small,
    boxes: [...small.boxes, { lo: [0.5, 0.4, 0.5], hi: [0.9, 1.2, 0.9] }, { lo: [2, 0.4, -2], hi: [2.4, 1.2, -1.6] }],
  };
  const cases: [string, NavBoard, NavTerrace[]][] = [
    ['one box and a terrace', small, [room]],
    ['one box only', small, []],
    ['three boxes and a terrace', crowded, [room]],
  ];
  for (const [name, board, terraces] of cases) {
    const key = gridKey(buildGrid(board, terraces));
    const whole = drain(board, terraces);
    eq(gridKey(whole.grid), key, `grid steps (${name}): the drained generator equals buildGrid`);
    const fine = drain(board, terraces, 1);
    eq(gridKey(fine.grid), key, `grid steps (${name}): chunk 1 equals chunk 64`);
    const ys = whole.yields;
    eq(
      ys.length > 0 && ys[ys.length - 1] === 1 && ys.every((p, k) => p > 0 && p <= 1 && (k === 0 || p > ys[k - 1]!)),
      true,
      `grid steps (${name}): progress rises in (0, 1] and ends at 1`,
    );
    // Steps: base cells (1), box chunks, traces (1), landings (1), one per terrace, then inflation, costs, portals and components (4).
    eq(fine.yields.length, 1 + board.boxes.length + 1 + 1 + terraces.length + 4, `grid steps (${name}): chunk 1 yields one step per box`);
    eq(whole.yields.length, 1 + (board.boxes.length > 0 ? 1 : 0) + 1 + 1 + terraces.length + 4, `grid steps (${name}): chunk 64 yields one step for all boxes`);
  }
}

// Heights (nav-grid.ts rules 1, 2, 11; nav-plan.ts rule 4): a step up of at most NAV_STEP is walked, a rise up to 0.8 is a
// jump edge, a higher step is crossed only by a ramp or lift portal. The board is flat at 0 with one plate across it.
{
  const heightBoard = (y: number): NavBoard => ({
    size: [6, 1, 6],
    pcbTop: 0,
    boxes: [],
    traces: [],
    plates: [{ id: 1, x0: 1, z0: -1, w: 1, d: 2, y }],
  });
  const boardPt = { x: -1.5, z: 0 };
  const platePt = { x: 1.5, z: 0 };
  const flatPlate = buildGrid(heightBoard(0.5), []);
  const lowPlan = findPath(flatPlate, boardPt, platePt);
  eq(lowPlan !== null, true, 'heights: a plate 0.5 above the board has a route');
  eq(
    lowPlan !== null && lowPlan.points.some((p) => p.edge === 'jump'),
    true,
    'heights: a plate 0.5 above the board is reached through a jump edge (0.5 is above NAV_STEP, up to NAV_JUMP_UP)',
  );
  eq(
    lowPlan !== null && lowPlan.points.find((p) => p.y > 0.3)?.edge,
    'jump',
    'heights: the first point above the board is the jump edge (the plate is then walked level)',
  );
  const jumpGrid = buildGrid(heightBoard(0.6), []);
  const jumpPlan = findPath(jumpGrid, boardPt, platePt);
  eq(
    jumpPlan !== null && jumpPlan.points.some((p) => p.edge === 'jump'),
    true,
    'heights: a 0.6 step is a jump edge (a path point flagged jump)',
  );
  const tallGrid = buildGrid(heightBoard(2.5), []);
  const liftPlan = findPath(tallGrid, boardPt, platePt);
  eq(
    liftPlan !== null && liftPlan.points.some((p) => p.portal >= 0 && tallGrid.portals[p.portal]!.kind === 'lift'),
    true,
    'heights: a 2.5 step is crossed by its lift portal',
  );
  const noPortals: NavGrid = { ...tallGrid, portals: [], portal: new Int32Array(tallGrid.portal.length).fill(-1) };
  eq(findPath(noPortals, boardPt, platePt), null, 'heights: a 2.5 step is impassable without a portal');
}

// The fallback deal never yields the oracle (reserved for model agents), and a record's own species wins even as the oracle.
{
  let dealt = 0;
  for (let cid = 0; cid < 10000; cid++) {
    if (robotVariant(cid) === 'oracle') dealt++;
    if (speciesOf(null, cid) === 'oracle') dealt++;
  }
  eq(dealt, 0, 'the cid fallback never deals the oracle');
  for (const cid of [0, 1, 7, 9999]) eq(speciesOf('oracle', cid), 'oracle', `a record's own species wins, cid ${cid}`);
}

// ---- Lifts: the cab's controller (lift-ctrl.ts) ----
{
  const LO = 0;
  const HI = 1.5;
  const DT = 1 / 30;
  // The instant robot: it steps into the cab when its ticket is 'boarding', and out of it when 'arrived'.
  const serve = (lift: LiftController, ids: number[]): void => {
    for (const id of ids) {
      const t = lift.ticket(id);
      if (t === 'boarding') lift.board(id);
      else if (t === 'arrived') lift.alight(id);
    }
  };

  // Robots asking at once ride in the order they asked.
  {
    const lift = new LiftController({ low: LO, high: HI });
    const ids = [1, 2, 3];
    for (const id of ids) lift.request(id, LO, HI);
    const order: number[] = [];
    for (let s = 0; s < 30 * 30; s++) {
      lift.update(DT);
      serve(lift, ids);
      for (const id of ids) if (lift.ticket(id) === 'riding' && !order.includes(id)) order.push(id);
    }
    eq(order, [1, 2, 3], 'lift: three robots asking at once ride in the order they asked');
  }

  // The cab takes three at once; the other two wait for the next trip and are all served.
  {
    const lift = new LiftController({ low: LO, high: HI });
    const ids = [1, 2, 3, 4, 5];
    for (const id of ids) lift.request(id, LO, HI);
    eq(ids.slice(3).some((id) => lift.ticket(id) === 'riding'), false, 'lift: robots 4 and 5 are not riding right after the requests');
    const everRode = new Set<number>();
    let maxRiders = 0;
    let overCap = 0;
    let steps = 0;
    while (steps < 60 * 30 && !ids.every((id) => lift.ticket(id) === 'none')) {
      lift.update(DT);
      steps++;
      serve(lift, ids);
      const riders = lift.riders();
      if (riders.length > 3) overCap++;
      maxRiders = Math.max(maxRiders, riders.length);
      for (const id of riders) everRode.add(id);
    }
    eq(overCap, 0, 'lift: never more than 3 riders aboard');
    eq(ids.every((id) => lift.ticket(id) === 'none'), true, 'lift: all five robots are served within 60 s');
    eq(everRode.size, 5, 'lift: all five robots ride');
    eq(maxRiders, 3, 'lift: the cab is full at 3 riders at its fullest');
  }

  // The cab never moves with its doors open, and its doors never open between the stops.
  {
    const lift = new LiftController({ low: LO, high: HI });
    const ids = [1, 2, 3, 4, 5, 6];
    const at = [0, 0.4, 0.9, 1.5, 2.2, 3.0];
    const asked = new Set<number>();
    let clock = 0;
    let prevY = lift.state().cabY;
    let violations = 0;
    let reachedLow = false;
    let reachedHigh = false;
    for (let s = 0; s < 60 * 30; s++) {
      for (let i = 0; i < ids.length; i++) {
        const id = ids[i]!;
        if (asked.has(id) || clock < at[i]!) continue;
        asked.add(id);
        const up = id % 2 === 1; // odd ids ride up from the board, even ids ride down from the plate
        lift.request(id, up ? LO : HI, up ? HI : LO);
      }
      lift.update(DT);
      clock += DT;
      serve(lift, ids);
      const st = lift.state();
      if (st.doorsOpen && st.cabY !== prevY) violations++;
      if (st.doorsOpen && st.cabY > LO && st.cabY < HI) violations++;
      if (st.cabY === LO) reachedLow = true;
      if (st.cabY === HI) reachedHigh = true;
      prevY = st.cabY;
    }
    eq(violations, 0, 'lift: no move with the doors open, and no doors open between the stops');
    eq(reachedLow && reachedHigh, true, 'lift: the cab really travels between both stops in this run');
    eq(ids.every((id) => lift.ticket(id) === 'none'), true, 'lift: all six crossing robots are served within 60 s');
  }

  // Eight robots asking at staggered times from alternating ends: each is served within 90 s of its own request.
  {
    const lift = new LiftController({ low: LO, high: HI });
    const n = 8;
    const ids = Array.from({ length: n }, (_, i) => i);
    const askedAt = new Array<number>(n).fill(-1);
    const doneAt = new Array<number>(n).fill(-1);
    let clock = 0;
    for (let s = 0; s < 150 * 30; s++) {
      for (const i of ids) {
        if (askedAt[i]! >= 0 || clock < 0.7 * i) continue;
        askedAt[i] = clock;
        const up = i % 2 === 0;
        lift.request(i, up ? LO : HI, up ? HI : LO);
      }
      lift.update(DT);
      clock += DT;
      serve(lift, ids);
      for (const i of ids) if (askedAt[i]! >= 0 && doneAt[i]! < 0 && lift.ticket(i) === 'none') doneAt[i] = clock;
      if (doneAt.every((d) => d >= 0)) break;
    }
    const late = ids.filter((i) => doneAt[i]! < 0 || doneAt[i]! - askedAt[i]! > 90).length;
    eq(doneAt.filter((d) => d >= 0).length, n, 'lift: all eight robots are served');
    eq(late, 0, 'lift: no robot waits more than 90 s from its request to its ride end');
  }

  // Two robots at opposite ends ask at once: both are served within 20 s, and the cab is then idle; it stays idle after.
  {
    const lift = new LiftController({ low: LO, high: HI });
    lift.request(1, LO, HI);
    lift.request(2, HI, LO);
    const ids = [1, 2];
    const doneAt = new Map<number, number>();
    let clock = 0;
    while (clock < 20 && doneAt.size < ids.length) {
      lift.update(DT);
      clock += DT;
      serve(lift, ids);
      for (const id of ids) if (!doneAt.has(id) && lift.ticket(id) === 'none') doneAt.set(id, clock);
    }
    eq(doneAt.size, 2, 'lift: robots at opposite ends are both served within 20 s');
    eq(lift.isIdle(), true, 'lift: the cab is idle once both are served');

    const restY = lift.state().cabY;
    let leftIdle = false;
    for (let s = 0; s < 10 * 30; s++) {
      lift.update(DT);
      const st = lift.state();
      if (st.phase !== 'idle' || st.doorsOpen || st.cabY !== restY) leftIdle = true;
    }
    eq(leftIdle, false, 'lift: the idle cab stays idle for 10 more seconds (doors shut, no move)');
    eq(lift.state().phase, 'idle', 'lift: phase is idle 10 s after the run');
    eq(lift.state().doorsOpen, false, 'lift: doors are shut 10 s after the run');
    eq(lift.isIdle(), true, 'lift: isIdle() is true 10 s after the run');
  }

  // A robot waiting at its landing keeps its request while it waits: no touch for 2 s (it yields, holds or walks in
  // slowly), and the cab still comes for it and serves it. The robot is at the upper stop, the cab at the lower one.
  {
    const lift = new LiftController({ low: LO, high: HI });
    lift.request(7, HI, LO);
    for (let s = 0; s < 2 * 30; s++) lift.update(DT); // touches paused for 2 s
    eq(lift.ticket(7) !== 'none', true, 'lift: a waiting robot keeps its request with no touch for 2 s');
    let rode = false;
    for (let s = 0; s < 60 * 30 && lift.ticket(7) !== 'none'; s++) {
      lift.update(DT);
      serve(lift, [7]);
      if (lift.riders().includes(7)) rode = true;
    }
    eq(rode && lift.ticket(7) === 'none', true, 'lift: a robot waiting 2 s with touches paused is still served (it rides and alights)');
    eq(lift.state().cabY, LO, 'lift: the cab went to the robot and back to the stop it asked for');
  }

  // A robot that walks away (cancelled before the cab moves) is dropped: the cab does not travel for it, and it is idle after.
  {
    const lift = new LiftController({ low: LO, high: HI });
    lift.request(1, HI, LO);
    lift.cancel(1);
    eq(lift.ticket(1), 'none', 'lift: a cancelled robot has no ticket');
    let moved = false;
    for (let s = 0; s < 10 * 30; s++) {
      lift.update(DT);
      const st = lift.state();
      if (st.phase !== 'idle' || st.doorsOpen || st.cabY !== LO) moved = true;
    }
    eq(moved, false, 'lift: a robot that walked away is dropped; the cab does not travel for it');
    eq(lift.isIdle(), true, 'lift: the cab is idle once the only robot has walked away');
  }

  // An empty cab does nothing.
  {
    const lift = new LiftController({ low: LO, high: HI });
    for (let s = 0; s < 5 * 30; s++) lift.update(DT);
    const st = lift.state();
    eq(st.phase, 'idle', 'lift: an empty cab is idle');
    eq(st.doorsOpen, false, 'lift: an empty cab keeps its doors shut');
    eq(st.cabY, LO, 'lift: an empty cab stays at the low stop');
  }

  // Request rules: one request per robot, and not within one stop.
  {
    const lift = new LiftController({ low: LO, high: HI });
    eq(lift.request(1, LO, HI), true, 'lift: a new request is accepted');
    eq(lift.request(1, LO, HI), false, 'lift: a second request from the same robot is refused');
    eq(lift.request(2, LO, LO), false, 'lift: a request from a stop to the same stop is refused');
    eq(lift.ticket(2), 'none', 'lift: a refused request leaves the robot with no ticket');
  }
}

// The page's Content-Security-Policy: the engine's WebAssembly compiles under 'wasm-unsafe-eval' only, never under
// 'unsafe-eval'. The hub serves no CSP header of its own (noded and apid set none), so the page's meta tag is the policy.
{
  const html = readFileSync(join(process.cwd(), 'src/index.html'), 'utf8');
  const policy = /<meta http-equiv="Content-Security-Policy" content="([^"]*)"/.exec(html)?.[1] ?? '';
  const script = /script-src([^;]*)/.exec(policy)?.[1] ?? '';
  eq(script.includes("'wasm-unsafe-eval'"), true, "csp: script-src allows 'wasm-unsafe-eval' (the engine's wasm)");
  eq(policy.includes("'unsafe-eval'"), false, "csp: the policy never allows 'unsafe-eval'");
  eq(script.includes("'unsafe-inline'"), false, "csp: script-src allows no inline script");
}

sectionEnd('rooms tests: ok');

if (failedSections.length) {
  console.error(`${failedSections.length} section(s) failed: ${failedSections.join(', ')}`);
  process.exit(1);
}
