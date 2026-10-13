// Validation of the robot files (schema 1). Each validator returns the list of errors, each named by
// its path in the file ("parts[3].geo: ..."); an empty list is a valid file. Tests and the loader use them.

import {
  EYE_SHAPES, FX_KINDS, GEO_TYPES, GRIPS, LOCOMOTIONS, MATS, PERSONALITY, PROP_RE, REQUIRED_BONES, WAVES,
  type Clip, type Vec3,
} from './types';

type Errs = string[];
type Obj = Record<string, unknown>;

const GAIT_KEYS: readonly string[] = ['speed', 'freq', 'bob', 'sway', 'lean'];
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v);
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const isVec3 = (v: unknown): v is Vec3 => Array.isArray(v) && v.length === 3 && v.every(isNum);
const isHex = (v: unknown): boolean => typeof v === 'string' && /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.test(v);
const isStr = (v: unknown): v is string => typeof v === 'string' && v.length > 0;
const isId = (v: unknown): v is string => typeof v === 'string' && /^[a-z][a-z0-9-]*(\.[a-z0-9-]+)*$/.test(v);
const inList = (list: readonly string[], v: unknown): boolean => typeof v === 'string' && list.includes(v);

/** Keys outside `allowed` are errors: a typo in a field name must not pass silently. */
function unknownKeys(o: Obj, allowed: readonly string[], path: string, errs: Errs): void {
  for (const k of Object.keys(o)) if (!allowed.includes(k)) errs.push(`${path}.${k}: unknown field`);
}

// Geometry: the fields each type needs (the hardware format's, plus the optional segment counts).
const GEO_REQ: Record<string, readonly string[]> = {
  box: ['w', 'h', 'd'], rbox: ['w', 'h', 'd', 'r'], cyl: ['rt', 'rb', 'h'], cone: ['r', 'h'], sphere: ['r'],
  torus: ['r', 'tube'], plane: ['w', 'd'], ring: ['ri', 'ro'], shape: ['h', 'points'],
};
const GEO_KEYS = ['type', 'w', 'h', 'd', 'r', 'rt', 'rb', 'ri', 'ro', 'tube', 'arc', 'seg', 'half', 'points'];

function checkGeo(g: unknown, path: string, errs: Errs): void {
  if (!isObj(g)) return void errs.push(`${path}: a geometry object is required`);
  if (!inList(GEO_TYPES, g.type)) return void errs.push(`${path}.type: one of ${GEO_TYPES.join(', ')}`);
  unknownKeys(g, GEO_KEYS, path, errs);
  for (const k of GEO_REQ[g.type as string] ?? []) {
    if (k === 'points') {
      if (!Array.isArray(g.points) || g.points.length < 3) errs.push(`${path}.points: at least 3 [x, z] points`);
    } else if (!isNum(g[k])) errs.push(`${path}.${k}: a number is required for ${g.type}`);
  }
  if (g.seg !== undefined && (!Array.isArray(g.seg) || g.seg.length === 0 || g.seg.length > 2 || !g.seg.every((n) => Number.isInteger(n) && (n as number) >= 1 && (n as number) <= 64))) {
    errs.push(`${path}.seg: 1 or 2 whole numbers from 1 to 64`);
  }
  if (g.half !== undefined && (g.type !== 'sphere' || g.half !== 'top')) errs.push(`${path}.half: only "top", on a sphere`);
}

function checkMat(m: unknown, path: string, errs: Errs): void {
  if (typeof m === 'string') {
    if (!inList(MATS, m)) errs.push(`${path}: unknown material "${m}" (one of ${MATS.join(', ')})`);
    return;
  }
  if (!isObj(m) || !isHex(m.color)) return void errs.push(`${path}: a material name, or an inline material with a hex color`);
  unknownKeys(m, ['color', 'emissive', 'glow', 'opacity', 'flat'], path, errs);
  if (m.emissive !== undefined && !isHex(m.emissive)) errs.push(`${path}.emissive: a hex color`);
}

/** A part. `bones` names the bones a part may hang on; null for a tool (its parts have no bone). */
function checkPart(p: unknown, path: string, bones: Set<string> | null, errs: Errs): void {
  if (!isObj(p)) return void errs.push(`${path}: a part object is required`);
  unknownKeys(p, ['bone', 'geo', 'mat', 'at', 'rot', 'scale', 'outline', 'name', 'asym'], path, errs);
  if (bones === null) {
    if (p.bone !== undefined) errs.push(`${path}.bone: a tool's parts have no bone`);
  } else if (!isStr(p.bone) || !bones.has(p.bone)) errs.push(`${path}.bone: "${String(p.bone)}" is not a bone of this species`);
  checkGeo(p.geo, `${path}.geo`, errs);
  checkMat(p.mat, `${path}.mat`, errs);
  if (p.at !== undefined && !isVec3(p.at)) errs.push(`${path}.at: [x, y, z]`);
  if (p.rot !== undefined && !isVec3(p.rot)) errs.push(`${path}.rot: [x, y, z] in degrees`);
  if (p.scale !== undefined && !isVec3(p.scale) && !isNum(p.scale)) errs.push(`${path}.scale: a number or [x, y, z]`);
  if (p.outline !== undefined && typeof p.outline !== 'boolean') errs.push(`${path}.outline: true or false`);
}

/** A clip's channels. `bones` is the set they may name: a species' bones, or the required ones for a shared clip. */
export function validateClip(c: unknown, path: string, bones: Set<string>, gaitKeys: readonly string[] | null): Errs {
  const errs: Errs = [];
  if (!isObj(c)) return [`${path}: a clip object is required`];
  unknownKeys(c, ['loop', 'dur', 'channels', 'fx'], path, errs);
  if (c.loop !== undefined && typeof c.loop !== 'boolean') errs.push(`${path}.loop: true or false`);
  if (c.loop === false && !isNum(c.dur)) errs.push(`${path}.dur: a one-shot needs its length in seconds`);
  if (!Array.isArray(c.channels)) return [...errs, `${path}.channels: a list`];
  const num = (v: unknown): boolean => isNum(v) || (typeof v === 'string' && gaitKeys !== null && /^gait\.[a-z]+$/.test(v));
  c.channels.forEach((ch: unknown, i) => {
    const cp = `${path}.channels[${i}]`;
    if (!isObj(ch)) return void errs.push(`${cp}: a channel object`);
    unknownKeys(ch, ['bone', 'prop', 'base', 'amp', 'freq', 'phase', 'wave', 'duty', 'env', 'snap', 'rate'], cp, errs);
    if (!isStr(ch.bone) || !bones.has(ch.bone)) errs.push(`${cp}.bone: "${String(ch.bone)}" is not a bone here`);
    if (typeof ch.prop !== 'string' || !PROP_RE.test(ch.prop)) errs.push(`${cp}.prop: rot.x|y|z, pos.x|y|z, scale or lid`);
    for (const k of ['base', 'amp', 'freq']) if (ch[k] !== undefined && !num(ch[k])) errs.push(`${cp}.${k}: a number${gaitKeys ? ' or "gait.<name>"' : ''}`);
    if (ch.wave !== undefined && !inList(WAVES, ch.wave)) errs.push(`${cp}.wave: one of ${WAVES.join(', ')}`);
    if (ch.phase !== undefined && !isNum(ch.phase)) errs.push(`${cp}.phase: a number`);
    if (ch.duty !== undefined && (!isNum(ch.duty) || ch.duty < 0 || ch.duty > 1)) errs.push(`${cp}.duty: 0 to 1`);
    if (ch.env !== undefined && ch.env !== 'in-out') errs.push(`${cp}.env: "in-out"`);
    if (ch.snap !== undefined && typeof ch.snap !== 'boolean') errs.push(`${cp}.snap: true or false`);
    if (ch.rate !== undefined && !isNum(ch.rate)) errs.push(`${cp}.rate: a number (rad/s)`);
    if (ch.rate !== undefined && ch.prop !== undefined && !String(ch.prop).startsWith('rot.')) errs.push(`${cp}.rate: only for rot props`);
  });
  if (c.fx !== undefined) {
    if (!Array.isArray(c.fx)) errs.push(`${path}.fx: a list`);
    else c.fx.forEach((f: unknown, i) => {
      const fp = `${path}.fx[${i}]`;
      if (!isObj(f) || !inList(FX_KINDS, f.kind)) return void errs.push(`${fp}.kind: one of ${FX_KINDS.join(', ')}`);
      if (!isStr(f.at)) errs.push(`${fp}.at: an anchor (head, gripR, gripL, body, carry, or a bone)`);
      if (f.every !== undefined && (!isNum(f.every) || f.every <= 0)) errs.push(`${fp}.every: seconds, above 0`);
    });
  }
  return errs;
}

/** A shared clip file (clips.json): its clips name the required bones only. */
export function validateClips(file: unknown): Errs {
  if (!isObj(file) || file.schema !== 1 || !isObj(file.clips)) return ['clips: { schema: 1, clips: {...} }'];
  const bones = new Set<string>(REQUIRED_BONES);
  return Object.entries(file.clips).flatMap(([id, c]) => (isId(id) ? validateClip(c, `clips.${id}`, bones, GAIT_KEYS) : [`clips.${id}: a clip id`]));
}

/** A species (species/<id>.json): its bones, parts, eyes, motion, personality and clips. */
export function validateSpecies(file: unknown, name: string, shared: ReadonlySet<string>): Errs {
  const errs: Errs = [];
  if (!isObj(file)) return [`${name}: a JSON object`];
  unknownKeys(file, ['schema', 'id', 'name', 'blurb', 'accent', 'height', 'label_top', 'locomotion', 'gait', 'bones', 'parts', 'eyes', 'spin', 'feet', 'floats', 'personality', 'quirks', 'clips', 'overrides', 'orchestrator'], name, errs);
  if (file.schema !== 1) errs.push(`${name}.schema: 1`);
  if (file.id !== name) errs.push(`${name}.id: "${String(file.id)}" must match the file name "${name}"`);
  if (!isStr(file.name)) errs.push(`${name}.name: a nickname`);
  if (typeof file.blurb !== 'string') errs.push(`${name}.blurb: a line of text`);
  if (!isHex(file.accent)) errs.push(`${name}.accent: a hex color`);
  if (!isNum(file.height) || file.height <= 0) errs.push(`${name}.height: a positive number`);
  if (!isNum(file.label_top)) errs.push(`${name}.label_top: a height`);
  if (!inList(LOCOMOTIONS, file.locomotion)) errs.push(`${name}.locomotion: one of ${LOCOMOTIONS.join(', ')}`);
  if (typeof file.floats !== 'boolean') errs.push(`${name}.floats: true or false`);

  const gait = file.gait;
  if (!isObj(gait)) errs.push(`${name}.gait: an object`);
  else {
    for (const k of ['speed', 'freq', 'bob', 'sway', 'lean']) if (!isNum(gait[k])) errs.push(`${name}.gait.${k}: a number`);
    if (typeof gait.sideways !== 'boolean') errs.push(`${name}.gait.sideways: true or false`);
  }

  // Bones: the required set, every parent a known bone (or "root"), and no cycles.
  const bones = new Set<string>();
  if (!isObj(file.bones)) errs.push(`${name}.bones: an object`);
  else {
    const entries = Object.entries(file.bones);
    for (const [b, def] of entries) {
      if (!isObj(def) || !isVec3(def.at)) errs.push(`${name}.bones.${b}: { parent, at: [x, y, z] }`);
      bones.add(b);
    }
    for (const req of REQUIRED_BONES) if (!bones.has(req)) errs.push(`${name}.bones: the required bone "${req}" is missing`);
    for (const [b, def] of entries) {
      if (!isObj(def)) continue;
      let p: unknown = def.parent;
      for (let depth = 0; p !== 'root'; depth++) {
        if (!isStr(p) || !bones.has(p) || depth > entries.length) {
          errs.push(`${name}.bones.${b}.parent: "${String(p)}" is not a bone (or "root"), or a cycle`);
          break;
        }
        p = (file.bones as Record<string, Obj>)[p]?.parent;
      }
    }
  }
  const boneSet = bones;

  if (!Array.isArray(file.parts)) errs.push(`${name}.parts: a list`);
  else file.parts.forEach((p, i) => checkPart(p, `${name}.parts[${i}]`, boneSet, errs));

  if (!Array.isArray(file.eyes)) errs.push(`${name}.eyes: a list`);
  else file.eyes.forEach((e: unknown, i) => {
    const ep = `${name}.eyes[${i}]`;
    if (!isObj(e)) return void errs.push(`${ep}: an eye object`);
    unknownKeys(e, ['bone', 'at', 'r', 'pupil', 'shape', 'glass', 'stretch', 'asym'], ep, errs);
    if (!isStr(e.bone) || !boneSet.has(e.bone)) errs.push(`${ep}.bone: not a bone of this species`);
    if (!isVec3(e.at)) errs.push(`${ep}.at: [x, y, z]`);
    if (!isNum(e.pupil) || e.pupil <= 0) errs.push(`${ep}.pupil: a radius`);
    if (!inList(EYE_SHAPES, e.shape)) errs.push(`${ep}.shape: one of ${EYE_SHAPES.join(', ')}`);
    if (e.glass !== undefined && (typeof e.glass !== 'boolean' || (e.glass === true && !isNum(e.r)))) errs.push(`${ep}.glass: true or false (true needs r)`);
    if (e.r !== undefined && !isNum(e.r)) errs.push(`${ep}.r: a radius`);
    if (e.stretch !== undefined && !isNum(e.stretch)) errs.push(`${ep}.stretch: a number`);
  });

  if (!Array.isArray(file.spin)) errs.push(`${name}.spin: a list`);
  else file.spin.forEach((s: unknown, i) => {
    const sp = `${name}.spin[${i}]`;
    if (!isObj(s) || !isStr(s.bone) || !boneSet.has(s.bone)) return void errs.push(`${sp}.bone: not a bone of this species`);
    if (!inList(['x', 'y', 'z'], s.axis)) errs.push(`${sp}.axis: x, y or z`);
    if (!isNum(s.walk) || !isNum(s.idle)) errs.push(`${sp}: walk and idle are rad/s numbers`);
    if (s.offline !== undefined && !isNum(s.offline)) errs.push(`${sp}.offline: a number`);
  });

  if (!Array.isArray(file.feet) || !file.feet.every((b) => isStr(b) && boneSet.has(b))) errs.push(`${name}.feet: a list of bones`);

  const pers = file.personality;
  if (!isObj(pers)) errs.push(`${name}.personality: an object`);
  else for (const k of PERSONALITY) if (!Number.isInteger(pers[k]) || (pers[k] as number) < 0 || (pers[k] as number) > 9) errs.push(`${name}.personality.${k}: a whole number 0 to 9`);

  const clips: Record<string, Clip> = isObj(file.clips) ? (file.clips as Record<string, Clip>) : {};
  if (!isObj(file.clips)) errs.push(`${name}.clips: an object`);
  else for (const [id, c] of Object.entries(file.clips)) {
    if (!isId(id)) errs.push(`${name}.clips.${id}: a clip id`);
    errs.push(...validateClip(c, `${name}.clips.${id}`, boneSet, Object.keys(isObj(gait) ? gait : {})));
  }

  if (!Array.isArray(file.quirks)) errs.push(`${name}.quirks: a list`);
  else file.quirks.forEach((q: unknown, i) => {
    if (!isObj(q) || !isStr(q.clip) || !isNum(q.weight)) return void errs.push(`${name}.quirks[${i}]: { clip, weight }`);
    if (!(q.clip in clips) && !shared.has(q.clip)) errs.push(`${name}.quirks[${i}].clip: "${q.clip}" is not a clip`);
  });

  if (!isObj(file.overrides)) errs.push(`${name}.overrides: an object`);
  else for (const [key, own] of Object.entries(file.overrides)) {
    if (!shared.has(key)) errs.push(`${name}.overrides.${key}: not a shared clip`);
    if (!isStr(own) || !(own in clips)) errs.push(`${name}.overrides.${key}: "${String(own)}" is not a clip of this species`);
  }

  if (file.orchestrator !== undefined) {
    if (!isObj(file.orchestrator) || !Array.isArray(file.orchestrator.parts)) errs.push(`${name}.orchestrator: { parts: [...] }`);
    else file.orchestrator.parts.forEach((p: unknown, i) => checkPart(p, `${name}.orchestrator.parts[${i}]`, boneSet, errs));
  }
  return errs;
}

/** A tool (tools/<id>.json): parts at its origin, held at the grip (one, two or carry). */
export function validateTool(file: unknown, name: string): Errs {
  const errs: Errs = [];
  if (!isObj(file)) return [`${name}: a JSON object`];
  unknownKeys(file, ['schema', 'id', 'name', 'grip', 'aim', 'parts', 'tip'], name, errs);
  if (file.aim !== undefined && !inList(['forward', 'down', 'up'], file.aim)) errs.push(`${name}.aim: forward, down or up`);
  if (file.schema !== 1) errs.push(`${name}.schema: 1`);
  if (file.id !== name) errs.push(`${name}.id: must match the file name "${name}"`);
  if (!isStr(file.name)) errs.push(`${name}.name: a name`);
  if (!inList(GRIPS, file.grip)) errs.push(`${name}.grip: one of ${GRIPS.join(', ')}`);
  if (!isVec3(file.tip)) errs.push(`${name}.tip: [x, y, z]`);
  if (!Array.isArray(file.parts) || file.parts.length === 0) errs.push(`${name}.parts: a non-empty list`);
  else file.parts.forEach((p, i) => checkPart(p, `${name}.parts[${i}]`, null, errs));
  return errs;
}

/** The index (index.json): the species, tools and props, in order. */
export function validateIndex(file: unknown): Errs {
  if (!isObj(file) || file.schema !== 1) return ['index: { schema: 1, species, tools, props }'];
  const errs: Errs = [];
  for (const k of ['species', 'tools', 'props'] as const) {
    const list = file[k];
    if (!Array.isArray(list) || !list.every(isId)) errs.push(`index.${k}: a list of ids`);
    else if (new Set(list).size !== list.length) errs.push(`index.${k}: ids must be unique`);
  }
  return errs;
}

/** The palette (palette.json): the eight fixed robot colours. */
export function validatePalette(file: unknown): Errs {
  if (!isObj(file)) return ['palette: a JSON object'];
  const keys = ['ink', 'sheet', 'paper', 'rule', 'signal', 'ok', 'info', 'inkMuted'];
  const errs = keys.filter((k) => !isHex(file[k])).map((k) => `palette.${k}: a hex color`);
  return errs;
}
