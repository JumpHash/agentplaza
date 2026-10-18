// Pure-logic tests: run with `npm test`.
import { parse, href, hrefIn, movedTo, setLinkScope } from '../src/router';
import { inScope, isLive, liveProjects, onBench } from '../src/scope';
import { machinesRoute, viewHash } from '../src/views/machines';
import { parseStyle, STYLES } from '../src/views/machine-style';
import { finiteValues, gpuCardLine, latestValue, metricValues, parseGpuCards, vmRows } from '../src/views/machine-view-model';
import { classify, label, type RichCtx, type Span } from '../src/ui/rich';
import { monogramOf, personOf, toneOf } from '../src/ui/embed-model';
import richVectors from '../../../crates/apz-rich/tests/vectors.json';
import { TOUR_STEPS, WIZARD_STEPS, fill, keysSet, moveStep, readFlag, writeFlag } from '../src/onboarding/flow';
import { ago, clip, compact, isTextValue } from '../src/ui/components';
import { floorName, familyLabel, roleFamily } from '../src/roles';
import { builtinLaws, diagnosticText, diagnosticsByLine, errorText, isBlank, isOwnerLaw, lawsOfRule, lineSpan, matchesFilter, tierKey, withStrictPermits, type LawFilter } from '../src/views/rules-model';
import type { LawInfo, Settings } from '../src/api';
import { approvalPath } from '../src/api';
import { checkRetention, clampQuery, mergeNewest, mergeOlder, spanOf } from '../src/views/conversations';
import { level, membersOf, parentOf, trailOf } from '../src/views/society';
import type { Bond, Conversation, SocietyAgent } from '../src/api';
import { easeOut, niceScale, resample, tweenArray } from '../src/ui/chart';
import { balanceData, barData, creditsOf, creditsUsd, flowsOf, mergeJournal, movesOf, partyOf, sortRows, windowOf as treasuryWindow } from '../src/views/treasury';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import type { Agent, FleetApple, FleetKind, FleetMachine, FleetResponse, FleetStatus, IdeWorkspaceInfo } from '../src/api';
import { agentTally, gibText, machineFacts, machineSubtitle } from '../src/views/floor';
import { acpSnippets, hubOrigin, setupSnippets } from '../src/views/hub-mcp-snippets';
import { DEFAULT_ENGINES, ENGINES, toggleEngine } from '../src/views/engines-model';
import { ENV_NAME_RE, limitsProblem, matchingPreset, ownRowProblem, specsOfBag } from '../src/views/mcp-common';
import { wrapIde, type IdeGlue } from '../src/ide/wasm';
import { wrapGraph, type GraphGlue, type Layout } from '../src/graph/wasm';
import { cardLine, compose, flowSegments, listed, looseGraph, needsMe, pathOf, planCards, shortSpan, stepText, waitsByRole, waitsForOwner, workState, type WorkState } from '../src/views/work-model';
import type { GraphTask } from '../src/api';
import { activitySplit, approvalIdIn, authorHue, buildChat, currentTask, filterPalette, formatMs, matchScore, mentionsOf, rowStatus, stepLabel, type PaletteItem } from '../src/ide/view';
import { groupByProject, matchingRows, projectOf, sharedNames, sidebarGroups, startAgent } from '../src/ide/groups';
import type { Entry, Project, Task, StationFacts, StationLease, StationSettings, StationView, RecordingDefaults, MachineNode } from '../src/api';
import { mentionAt, mentionOptions } from '../src/board/owner';
import { applyBuffer, badgeOf, diffMark, dirsOf, fuzzyScore, isViewed, languageFor, lineCounts, onRemote, openBuffer, parentDir, parseViewed, rankPaths, stationBadgeOf, viewedKey, withViewed, groupWorkspaces, osLabel, pickerLabel, stationCrumb } from '../src/ide/model';
import type { FileChange } from '../src/ide/types';
import { CENTRE_MIN, clampSize, closeSheets, defaultLayout, fitSize, isNarrow, LEFT_SIZE, leftShows, leftViewFor, openRight, originFallback, parseLayout, RIGHT_SIZE, rightShows, rightTabFor, SHUT, serializeLayout, toggleLeft, toggleRight, hasDiff, hasTerminal, centreViewsFor, rightViewsFor, runsFor } from '../src/ide/layout';
import { MAX_TERMINALS, attachFrame, base64ToBytes, bytesToBase64, clearSession, inputFrame, killFrame, nextOffset, openFrame, parseServerFrame, plainText, readSession, reconnectDelay, resizeFrame, sessionKey, splitInput, storedTerminalCount, terminalSocketUrl, writeSession, type KeyValueStore } from '../src/views/ide-terminal-model';
import { auditLabel, codeChip, countdown, displaysText, formLabel, gpuLine, gpuSummary, joinCommand, leaseCount, leaseStateChip, nameProblem, osText, pictureNote, ramText, readSettings, accountBadge, accountText, settingsForm, sortStations, stationSummary, statusChip, takeoverText, type SettingsForm, actionFrame, activeAction, backoffMs, bytesText, buttonAction, clockText, controlSocketUrl, crumbsOf, dollarsPerHour, defaultsForm, frameAt, frameTime, frameTimes, framePoint, inputButton, joinPath, keyAction, moveAction, parentDir, parseControlText, parseFrame, parseScreenText, pendingPaths, rateText, readDefaults, readRate, recordingSpan, scrollNotches, screenSocketUrl, sessionChoices, takeoverBanner, treeOf, uploadProblem, wheelAction, actionText, type KeyEventLike } from '../src/views/workstations-model';
import { isStationNode, leaseCounts, stationBadge, stationLine, stationLook, stationMachine, stationThumbnailPath } from '../src/views/station-machine';
import { lookOfSpec, sizeLine, specKey, VIEWS, type PictureSpec } from '../src/views/machine-picture';
import { VM_FORM_DEFAULTS, backendFor, backendHint, imageProblem, macosOn, nodeChoices, nodeFactsLine, nodeOptionLabel, nodeVmFacts, pciProblem, readVmForm, sortVms, vmBody, vmGpuText, vmNameProblem, vmOsText, vmProblem, vmStateChip, wholeIn, type NodeVmFacts, type VmForm } from '../src/views/workstations-vm-model';
import type { NodeView, StationVm } from '../src/api';
import { api, e2bApi, egressApi } from '../src/api';
import { statusPill } from '../src/views/agent-list';
import { sleepRoles, wholeInRange } from '../src/views/models-policy';
import { defaultRuleForm, listOf, methodsText, rolesText, ruleBody, ruleProblem, type RuleForm } from '../src/views/egress-credentials';
import { e2bSetupText, latestBuild, memoryText, sandboxActions, sandboxTime, templateFailure, templateTags, type E2bBuild, type E2bTemplate } from '../src/views/e2b';
import { movableNodes, moveText } from '../src/views/agent-detail';
import { parseVolumeList, volumeNameOk } from '../src/views/volumes';
import { checkpointsKey, treeKey, type TreeKeyInput } from '../src/views/ide-render';
import type { Entry as TreeEntry } from '../src/ide/types';
import { format, nice, timeLabel } from '../src/metrics/format';
import type { Unit } from '../src/metrics/model';
import formatGolden from '../../../crates/apz-metrics-view/tests/format_golden.json';
import { splitTreemap } from '../src/metrics/figures';
import type { TreeItem } from '../src/metrics/model';
import { bandPath, bucketStarts, stackedPaths, thin, xLabelIndexes } from '../src/metrics/charts';
import { appendPage, wrapsAtSpaces } from '../src/metrics/bodies';
import { DEFAULT_STATE, LeaseKeeper, narrowTileColumns, parseHash, queryOf, toHash, togglePin, togglePinnedScope, visit, type MetricsState } from '../src/metrics/state';
import type { LiveLease, Saved } from '../src/metrics/model';

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


eq(parse('#/agents/105'), { screen: 'agents', params: ['105'], project: null }, 'route with a param');
eq(parse(''), { screen: 'overview', params: [], project: null }, 'empty route is the overview');
// A project's route carries it first; the screen and its params follow as before.
eq(parse('#/p/pr4/agents/105'), { screen: 'agents', params: ['105'], project: 'pr4' }, 'a scoped route names its project');
eq(parse('#/p/pr4'), { screen: 'overview', params: [], project: 'pr4' }, "a project's front door is its overview");
eq(parse('#/p'), { screen: 'p', params: [], project: null }, 'a lone p is a screen name, not a scope');
eq(hrefIn('pr4', 'tasks', 't 1'), '#/p/pr4/tasks/t%201', 'a scoped link');
eq(hrefIn(null, 'tasks'), '#/tasks', 'All projects is the plain route');
setLinkScope('pr7');
eq(href('work', 'pr7'), '#/p/pr7/work/pr7', 'links carry the scope of the route on screen');
setLinkScope(null);
eq(href('work'), '#/work', 'and none under All projects');
eq(href('tasks', 't 1'), '#/tasks/t%201', 'href encodes params');
eq(ago(1000, 31_000), '30s ago', 'ago seconds');
eq(clip('abcdef', 4), 'abc…', 'clip');
eq(compact(12_345), '12k', 'compact thousands');
// A stat value with letters is text (a model id keeps its size class); figures and the dash placeholder are not.
eq(isTextValue('claude-sonnet-5'), true, 'a model id is a text stat value');
eq(isTextValue(12), false, 'a numeric stat value is a figure');
eq(isTextValue('1,204'), false, 'a formatted figure is a figure');
eq(isTextValue('—'), false, 'the dash placeholder keeps the figure size');
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

// Hardware models (assets/hardware): every file validates, the index lists every file, the loader builds them.
eq(parse('#/hardware-lab?model=hub&lod=1'), { screen: 'hardware-lab', params: [], project: null }, 'a query is not part of the route');
eq(parse('#/p/pr2/work?task=t3').project, 'pr2', 'a scoped route keeps its project before a query');

// ---- The project scope (scope.ts) ----
{
  const live = { archived: false, stage: 'implementation' as const };
  eq(isLive(live), true, 'a project at work is live');
  eq(isLive({ archived: true, stage: 'implementation' }), false, 'an archived one is not');
  eq(isLive({ archived: false, stage: 'delivered' }), false, 'nor a delivered one');
  const p = (id: string, created_ms: number, extra: Partial<Project> = {}) => ({ id, created_ms, archived: false, stage: 'implementation', ...extra }) as Project;
  eq(
    liveProjects([p('pr1', 1), p('pr2', 3), p('pr3', 2, { archived: true }), p('pr4', 4, { stage: 'delivered' })]).map((x) => x.id),
    ['pr2', 'pr1'],
    'the switcher offers the live projects, newest first',
  );
  const agent = (project: string | null, tier: 'worker' | 'orchestrator' = 'worker', kind = 'llm') => ({ project, tier, kind }) as unknown as Agent;
  eq(inScope(agent('pr1'), 'pr1'), true, "a project's agent is in its scope");
  eq(inScope(agent('pr2'), 'pr1'), false, "another project's is not");
  eq(inScope(agent(null), 'pr1'), false, 'nor a bench agent');
  eq(inScope(agent(null, 'orchestrator'), 'pr1'), true, 'the orchestrator works on every project');
  eq(inScope(agent('pr2'), null), true, 'All projects shows everyone');
  eq(onBench(agent(null)), true, 'no project is the bench');
  eq(onBench(agent(null, 'orchestrator')), false, 'the orchestrator is never on the bench');
  eq(onBench(agent(null, 'worker', 'model')), false, 'a model agent takes no work, so it is not on the bench');
  eq(onBench(agent('pr1')), false, "a project's agent is not on the bench");
}

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

// The Machines view's pure parts (floor-popup.ts): the popup's text.
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

sectionEnd('web tests: ok');

// The page's Content-Security-Policy: the engine's WebAssembly compiles under 'wasm-unsafe-eval' only, never under
// 'unsafe-eval'. The hub serves no CSP header of its own (noded and apid set none), so the page's meta tag is the policy.
{
  const html = readFileSync(join(process.cwd(), 'src/index.html'), 'utf8');
  const policy = /<meta http-equiv="Content-Security-Policy" content="([^"]*)"/.exec(html)?.[1] ?? '';
  const script = /script-src([^;]*)/.exec(policy)?.[1] ?? '';
  eq(script.includes("'wasm-unsafe-eval'"), true, "csp: script-src allows 'wasm-unsafe-eval' (the engine's wasm)");
  eq(policy.includes("'unsafe-eval'"), false, "csp: the policy never allows 'unsafe-eval'");
  eq(script.includes("'unsafe-inline'"), false, "csp: script-src allows no inline script");
  // A workstation's picture and a recording's frames load from object URLs (blob:), as data: and https: already did.
  const directive = (name: string): string => new RegExp(`${name}([^;]*)`).exec(policy)?.[1] ?? '';
  eq(
    [directive('img-src').includes('blob:'), directive('media-src').includes('blob:'), directive('img-src').includes('data:'), directive('img-src').includes('https:')],
    [true, true, true, true],
    'csp: images and media may be blob: object URLs, data: and https: kept',
  );
}

// The hub-as-a-server tab's setup text: the Claude Code command carries the URL and the bearer header, and the
// desktop block runs the stdio bridge against the hub's origin, never with the token in it.
{
  const url = 'https://hub.example.test/mcp';
  const s = setupSnippets(url, 'apz_mcp_test');
  eq(s.claudeCode, 'claude mcp add --transport http agentplaza https://hub.example.test/mcp --header "Authorization: Bearer apz_mcp_test"', 'hub mcp: the Claude Code command carries the URL and the header');
  eq(hubOrigin(url), 'https://hub.example.test', 'hub mcp: the origin is the URL without its /mcp path');
  eq(JSON.parse(s.desktop).mcpServers.agentplaza.args, ['mcp', 'serve', '--hub', 'https://hub.example.test'], 'hub mcp: the desktop block serves the hub origin');
  eq(s.desktopLogin, 'agentplaza mcp login --hub https://hub.example.test', 'hub mcp: the login line names the hub origin');
  eq(s.desktop.includes('apz_mcp_test'), false, 'hub mcp: the desktop block never carries the token');
  eq(s.generic, 'URL: https://hub.example.test/mcp\nAuthorization: Bearer apz_mcp_test', 'hub mcp: the generic setup is the URL and the header');
}
{
  // Open mode: no token, so no header and no login line.
  const o = setupSnippets('https://hub.example.test/mcp', null);
  eq(o.claudeCode, 'claude mcp add --transport http agentplaza https://hub.example.test/mcp', 'hub mcp: open mode drops the header');
  eq(o.desktopLogin, null, 'hub mcp: open mode has no login line');
  eq(o.generic, 'URL: https://hub.example.test/mcp', 'hub mcp: open mode generic setup is the URL alone');
}
{
  // The editor setup text: Zed's settings carry the agent server, the named form pins an agent, and none holds a token.
  const a = acpSnippets();
  eq(JSON.parse(a.zed), { agent_servers: { AgentPlaza: { command: 'agentplaza', args: ['acp'] } } }, 'acp editors: the Zed settings start the agent server');
  eq(JSON.parse(a.zedAgent).agent_servers.AgentPlaza.args, ['acp', '--agent', '<name>'], 'acp editors: the named Zed form pins an agent');
  eq(a.jetbrains, 'Command: agentplaza\nArguments: acp', 'acp editors: the JetBrains command and its arguments');
  eq(a.neovim, 'adapters = { acp = { agentplaza = { command = "agentplaza", args = { "acp" } } } }', 'acp editors: the CodeCompanion adapter');
  eq(Object.values(a).some((t) => /Bearer|apz_/.test(t)), false, 'acp editors: no token in the editor text');
}
sectionEnd('hub mcp tests: ok');

// The coding engines: switching one on or off keeps the canonical order, and the last engine stays on.
{
  eq(ENGINES.map((e) => e.value), ['claude', 'codex', 'opencode', 'gemini', 'qwen', 'kilo', 'cline', 'copilot', 'goose', 'kimi', 'auggie', 'vibe'], 'engines: the twelve engines in order');
  eq(ENGINES.find((e) => e.value === 'goose')?.label, 'Goose', 'engines: Goose is labelled');
  eq(ENGINES.find((e) => e.value === 'kimi')?.label, 'Kimi Code', 'engines: Kimi Code is labelled');
  eq(ENGINES.find((e) => e.value === 'auggie')?.label, 'Auggie', 'engines: Auggie is labelled');
  eq(ENGINES.find((e) => e.value === 'cline')?.label, 'Cline', 'engines: Cline is labelled');
  eq(toggleEngine(['claude'], 'cline', true), ['claude', 'cline'], 'engines: Cline switches on last');
  eq(ENGINES.find((e) => e.value === 'gemini')?.label, 'Gemini CLI', 'engines: Gemini CLI is labelled');
  eq(ENGINES.find((e) => e.value === 'qwen')?.label, 'Qwen Code', 'engines: Qwen Code is labelled');
  eq(ENGINES.find((e) => e.value === 'kilo')?.label, 'Kilo Code', 'engines: Kilo Code is labelled');
  eq(DEFAULT_ENGINES, ['claude', 'codex', 'opencode'], 'engines: Gemini CLI, Qwen Code, Kilo Code and Cline are off by default');
  eq(toggleEngine(['claude'], 'gemini', true), ['claude', 'gemini'], 'engines: Gemini CLI switches on after the others');
  eq(toggleEngine(['claude'], 'kilo', true), ['claude', 'kilo'], 'engines: Kilo Code switches on last');
  eq(toggleEngine(['claude', 'codex'], 'codex', false), ['claude'], 'engines: switching one off leaves the rest');
  eq(toggleEngine(['claude'], 'claude', false), ['claude'], 'engines: the last engine cannot be switched off');
  eq(toggleEngine(['codex'], 'claude', true), ['claude', 'codex'], 'engines: switching on keeps the canonical order');
  eq(toggleEngine(['claude', 'opencode'], 'codex', true), ['claude', 'codex', 'opencode'], 'engines: the middle engine goes between');
}

// The MCP wizard's pure helpers: the limit bounds the hub checks, the own-row names, and matching a server to its
// preset by transport (not by id).
{
  const ok = { timeout_s: 60, max_output_kb: 256, idle_stop_min: 10 };
  eq(limitsProblem(ok), '', 'mcp limits: the defaults are in bounds');
  eq(limitsProblem({ ...ok, timeout_s: 0 }), 'Timeout: enter a whole number from 1 to 600.', 'mcp limits: a timeout below 1 is named');
  eq(limitsProblem({ ...ok, max_output_kb: 4097 }), 'Output size: enter a whole number from 1 to 4096.', 'mcp limits: an output size above 4096 is named');
  eq(limitsProblem({ ...ok, idle_stop_min: 1.5 }), 'Idle stop: enter a whole number from 1 to 1440.', 'mcp limits: a fraction is refused');
  eq(limitsProblem({ ...ok, timeout_s: NaN }), 'Timeout: enter a whole number from 1 to 600.', 'mcp limits: an empty number (NaN) is refused');
  eq(limitsProblem({ timeout_s: 600, max_output_kb: 4096, idle_stop_min: 1440 }), '', 'mcp limits: the bounds themselves are allowed');
  eq(ENV_NAME_RE.test('GITHUB_TOKEN'), true, 'mcp own rows: an upper-case env name is accepted');
  eq(ENV_NAME_RE.test('github_token'), false, 'mcp own rows: a lower-case env name is refused');
  eq(ownRowProblem('GITHUB_TOKEN', false), '', 'mcp own rows: a valid env name has no problem');
  eq(ownRowProblem('1TOKEN', false) !== '', true, 'mcp own rows: an env name starting with a digit is refused');
  eq(ownRowProblem('Authorization', true), '', 'mcp own rows: a header name is accepted');
  eq(ownRowProblem('Auth_Header', true) !== '', true, 'mcp own rows: an underscore in a header name is refused');
  const catalog = [
    { id: 'files', name: 'Files', description: '', transport: 'stdio' as const, image: 'ghcr.io/x/fs:1', command: [], url: null, env: [], headers: [], docs: '', status: '', note: '' },
    { id: 'search', name: 'Search', description: '', transport: 'http' as const, image: null, command: [], url: 'https://mcp.example.test/sse', env: [], headers: [], docs: '', status: '', note: '' },
  ];
  eq(matchingPreset({ kind: 'stdio', image: 'ghcr.io/x/fs:1' }, catalog)?.id, 'files', 'mcp match: a container matches its preset by image');
  eq(matchingPreset({ kind: 'http', url: 'https://mcp.example.test/sse' }, catalog)?.id, 'search', 'mcp match: a URL matches its preset by URL');
  eq(matchingPreset({ kind: 'stdio', image: 'ghcr.io/other/fs:1' }, catalog), undefined, 'mcp match: another image matches nothing');
  eq(specsOfBag({ TOKEN: { secret: 'GH_TOKEN' }, ROOT: { value: '/data' } }), [
    { name: 'TOKEN', secret: true, required: false, help: '', format: null },
    { name: 'ROOT', secret: false, required: false, help: '', format: null },
  ], 'mcp own specs: a bag becomes one spec per name, secret when it is a vault reference');
  eq(specsOfBag({ Authorization: { secret: 'WEB_KEY', format: 'Bearer {}' } })[0]?.format, 'Bearer {}', 'mcp own specs: a header format is kept');
}
sectionEnd('mcp wizard helper tests: ok');


// The first-run wizard and the tour: steps, copy filling, flags that survive a refused store.
eq(moveStep(0, -1, 4), 0, 'wizard: Back stops at the first step');
eq(moveStep(3, 1, 4), 3, 'wizard: Next stops at the last step');
eq(moveStep(1, 1, 8), 2, 'tour: Next moves one step');
eq(WIZARD_STEPS.length <= 5, true, 'wizard: at most five steps');
eq(WIZARD_STEPS[2]?.id, 'deps', 'wizard: the dependencies step is third, after the hub');
eq(TOUR_STEPS.length >= 6 && TOUR_STEPS.length <= 8, true, 'tour: six to eight steps');
eq(fill('{n} of {m} providers have a key', { n: 1, m: 3 }), '1 of 3 providers have a key', 'copy: placeholders fill');
eq(fill('{n} of {m}', { n: 2 }), '2 of {m}', 'copy: a missing value stays visible');
eq(keysSet([{ credential: 'missing' }, { credential: 'secret' }, { credential: 'env' }]), { n: 2, m: 3 }, 'keys: a missing credential does not count');
const memory = new Map<string, string>();
const fakeStore = { getItem: (k: string) => memory.get(k) ?? null, setItem: (k: string, v: string) => void memory.set(k, v) };
eq(readFlag(fakeStore, 'setup'), false, 'flags: unset at first');
writeFlag(fakeStore, 'setup');
eq(readFlag(fakeStore, 'setup'), true, 'flags: set after a write');
eq(readFlag(fakeStore, 'tour'), false, 'flags: each flag on its own key');
const refusing = { getItem: () => { throw new Error('denied'); }, setItem: () => { throw new Error('denied'); } };
eq(readFlag(refusing, 'setup'), false, 'flags: a refused read is not set');
writeFlag(refusing, 'setup'); // must not throw
eq(readFlag(null, 'tour'), false, 'flags: no store at all is not set');
sectionEnd('onboarding tests: ok');

// The IDE model's typed wrapper: the JSON in and out, over a glue that records what it is given. The harness cannot load
// the built wasm; apz-ide-web's own tests cover the JSON layer, and the wasm was checked by loading the built glue.
{
  const seen: string[] = [];
  const glue: IdeGlue = {
    default: async () => undefined,
    parse_unified: () => JSON.stringify({ files: [], stats: { files: 1, additions: 2, deletions: 0 } }),
    check_path: (p) => {
      if (p.includes('..')) throw new Error('path may not contain ..');
      return p;
    },
    check_read: (p) => p,
    check_write: (p) => p,
    check_dir: (p) => p,
    is_secret: (p) => p.endsWith('.pem'),
    initial_state: () => JSON.stringify({ workspace: null, open: [], active: null, panel: 'changes', base: 'work_start', review: false, filter_changed: false }),
    reduce: (state, action) => {
      seen.push(action);
      const next = JSON.parse(state) as { open: string[] };
      const a = JSON.parse(action) as { open?: string };
      if (a.open) next.open = [...next.open, a.open];
      return JSON.stringify(next);
    },
    children: (entries) => entries,
    changed_filter: () => '[]',
    parse_workspace_id: (s) => JSON.stringify(s),
    format_workspace_id: (json) => JSON.parse(json) as string,
  };
  const ide = wrapIde(glue);
  eq(ide.initialState().base, 'work_start', 'ide: the initial state is the default');
  eq(ide.reduce(ide.initialState(), { open: 'a.txt' }).open, ['a.txt'], 'ide: reduce parses the state it returns');
  eq(seen, ['{"open":"a.txt"}'], 'ide: reduce sends the action as JSON');
  eq(ide.parseUnified('x').stats, { files: 1, additions: 2, deletions: 0 }, 'ide: parseUnified returns the totals');
  eq(ide.isSecret('keys/a.pem'), true, 'ide: isSecret passes through');
  eq(ide.checkPath('src/a.ts'), 'src/a.ts', 'ide: checkPath returns the cleaned path');
  let refusal = '';
  try {
    ide.checkPath('../x');
  } catch (e) {
    refusal = (e as Error).message;
  }
  eq(refusal, 'path may not contain ..', 'ide: a refusal reaches the caller with its message');
  eq(ide.formatWorkspaceId(ide.parseWorkspaceId('agent:7')), 'agent:7', 'ide: a workspace id round trips');
  eq(ide.formatWorkspaceId(ide.parseWorkspaceId('station:w3')), 'station:w3', 'ide: a station id round trips');
  eq(ide.children([{ path: 'a', kind: 'file', size: 1, hash: 'h', binary: false }], '')[0]?.path, 'a', 'ide: children parses the entries');
}
sectionEnd('ide wrapper tests: ok');

// The IDE view's pure helpers (ide/model.ts): the language by extension, the buffer and conflict transitions, the
// workspace's state badge, the viewed marks' storage, the diff marks, quick open's ranking and the tree's paths.
{
  eq(languageFor('src/a.ts'), 'typescript', 'ide model: .ts is TypeScript');
  eq(languageFor('ui/App.TSX'), 'tsx', 'ide model: the extension is matched in any case');
  eq(languageFor('build.mjs'), 'javascript', 'ide model: .mjs is JavaScript');
  eq(languageFor('src/lib.rs'), 'rust', 'ide model: .rs is Rust');
  eq(languageFor('tool.py'), 'python', 'ide model: .py is Python');
  eq(languageFor('README.md'), 'markdown', 'ide model: .md is Markdown');
  eq(languageFor('x/data.json'), 'json', 'ide model: .json is JSON');
  eq(languageFor('site/index.htm'), 'html', 'ide model: .htm is HTML');
  eq(languageFor('s.css'), 'css', 'ide model: .css is CSS');
  eq(languageFor('Makefile'), 'plain', 'ide model: an extensionless file is plain');
  eq(languageFor('.gitignore'), 'plain', 'ide model: a dotfile is plain');
  eq(languageFor('dir.ts/readme'), 'plain', 'ide model: a dot in a directory name is not an extension');
  eq(languageFor('notes.txt'), 'plain', 'ide model: an unknown extension is plain');

  // Save, conflict, keep mine, reload.
  let b = openBuffer('a.txt', 'h1');
  eq([b.dirty, b.conflict], [false, null], 'ide buffer: a fresh buffer is clean');
  b = applyBuffer(b, { kind: 'edit' });
  eq(b.dirty, true, 'ide buffer: an edit makes it dirty');
  b = applyBuffer(b, { kind: 'save' });
  eq(b.saving, true, 'ide buffer: a save in flight is saving');
  b = applyBuffer(b, { kind: 'conflict', current: 'h2' });
  eq([b.base, b.conflict, b.dirty, b.saving], ['h1', 'h2', true, false], 'ide buffer: a 409 keeps the edits and records the hub hash');
  b = applyBuffer(b, { kind: 'keep_mine', current: 'h2' });
  eq([b.base, b.conflict, b.dirty], ['h2', null, true], 'ide buffer: keep mine re-bases on the hub copy and keeps the edits');
  b = applyBuffer(b, { kind: 'saved', hash: 'h3', edited: false });
  eq([b.base, b.dirty, b.conflict], ['h3', false, null], 'ide buffer: a clean save takes the new hash');

  let c = applyBuffer(openBuffer('a.txt', 'h1'), { kind: 'edit' });
  c = applyBuffer(c, { kind: 'conflict', current: 'h2' });
  c = applyBuffer(c, { kind: 'reload', hash: 'h2' });
  eq([c.base, c.dirty, c.conflict], ['h2', false, null], 'ide buffer: reload takes the hub copy and drops the edits');
  const typedDuringSave = applyBuffer(applyBuffer(openBuffer('a.txt', 'h1'), { kind: 'edit' }), { kind: 'saved', hash: 'h2', edited: true });
  eq(typedDuringSave.dirty, true, 'ide buffer: an edit made during a save stays dirty');

  // A remote change: clean reloads, edited conflicts, the same hash is nothing.
  const clean = openBuffer('a.txt', 'h1');
  eq(onRemote(clean, 'h1').action, 'none', 'ide remote: the hash already read is nothing to do');
  eq(onRemote(clean, 'h2').action, 'reload', 'ide remote: a clean buffer reloads');
  const dirty = applyBuffer(clean, { kind: 'edit' });
  const clash = onRemote(dirty, 'h2');
  eq([clash.action, clash.buffer.conflict, clash.buffer.dirty], ['conflict', 'h2', true], 'ide remote: an edited buffer goes to the conflict state');
  eq(onRemote(clash.buffer, 'h1').buffer.conflict, null, 'ide remote: the hub back at the read hash clears the conflict');

  eq([badgeOf({ conflict: true, agentWorking: true, saved: true }), badgeOf({ conflict: false, agentWorking: true, saved: true })], ['conflict', 'agent working'], 'ide badge: conflict, then agent working');
  eq([badgeOf({ conflict: false, agentWorking: false, saved: true }), badgeOf({ conflict: false, agentWorking: false, saved: false })], ['saved', 'idle'], 'ide badge: saved, then idle');
  // A workstation's badge is its status, never the agent working state (a workstation has no agent); conflict still comes first.
  eq(badgeOf({ conflict: false, agentWorking: true, saved: true, station: 'draining' }), 'draining', 'ide badge: a workstation shows its status, not the agent working');
  eq([badgeOf({ conflict: true, agentWorking: false, saved: false, station: 'online' }), badgeOf({ conflict: false, agentWorking: false, saved: false, station: 'offline' })], ['conflict', 'offline'], 'ide badge: a workstation’s conflict outranks its status');
  eq(badgeOf({ conflict: false, agentWorking: false, saved: true, station: null }), 'saved', 'ide badge: no station is the agent kind’s badge');
  eq(
    [
      stationBadgeOf({ online: true, draining: false, revoked: false }),
      stationBadgeOf({ online: true, draining: true, revoked: false }),
      stationBadgeOf({ online: false, draining: false, revoked: false }),
      stationBadgeOf({ online: true, draining: false, revoked: true }),
    ],
    ['online', 'draining', 'offline', 'offline'],
    'ide badge: a workstation is online, draining, or offline (a revoked one is offline)',
  );

  // Viewed marks: per workspace, malformed storage reads as none, a changed diff is no longer viewed.
  eq(viewedKey('agent:7'), 'apz.ide.viewed.agent:7', 'ide viewed: the key names the workspace');
  eq([parseViewed(null), parseViewed('not json'), parseViewed('[1]')], [{}, {}, {}], 'ide viewed: missing or malformed storage reads as no marks');
  eq(parseViewed('{"a.ts":"m1","b":2}'), { 'a.ts': 'm1' }, 'ide viewed: only string marks are kept');
  const marked = withViewed({}, 'a.ts', 'm1', true);
  eq([isViewed(marked, 'a.ts', 'm1'), isViewed(marked, 'a.ts', 'm2')], [true, false], 'ide viewed: a mark counts only for its own diff');
  eq(withViewed(marked, 'a.ts', 'm1', false), {}, 'ide viewed: unmarking removes the path');

  const hunkOf = (text: string): FileChange => ({
    path: 'a.ts',
    old_path: null,
    status: 'modified',
    binary: false,
    hunks: [
      {
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 1,
        header: '@@ -1 +1 @@',
        lines: [
          { kind: 'remove', old_no: 1, new_no: null, text: 'a' },
          { kind: 'add', old_no: null, new_no: 1, text },
        ],
      },
    ],
  });
  eq(diffMark(hunkOf('b')) === diffMark(hunkOf('b')), true, 'ide diff mark: the same diff has the same mark');
  eq(diffMark(hunkOf('b')) === diffMark(hunkOf('c')), false, 'ide diff mark: a changed diff has a new mark');
  eq(lineCounts(hunkOf('b')), { add: 1, del: 1 }, 'ide diff: the line counts of a file');

  // Quick open: every character in order; the file name and word starts rank first.
  eq([fuzzyScore('', 'x.ts'), fuzzyScore('zz', 'src/a.ts')], [0, null], 'ide quick open: the empty query matches, a missing character does not');
  eq(rankPaths(['src/app/main.ts', 'src/main.ts', 'docs/readme.md'], 'main'), ['src/main.ts', 'src/app/main.ts'], 'ide quick open: the shorter name ranks first and a miss is dropped');
  eq(rankPaths(['b.ts', 'a.ts'], ''), ['a.ts', 'b.ts'], 'ide quick open: ties sort by path');
  eq(rankPaths(['a/b/c.ts', 'abc.ts'], 'abc')[0], 'abc.ts', 'ide quick open: a run in the file name beats a scattered match');

  // The tree's paths.
  eq(dirsOf('a/b/c.txt'), ['', 'a', 'a/b'], 'ide tree: the directories above a file, root first');
  eq(dirsOf('x.ts'), [''], 'ide tree: a top-level file sits in the root');
  eq([parentDir('a/b/c.txt'), parentDir('x.ts')], ['a/b', ''], 'ide tree: a file’s directory');
}
sectionEnd('ide view model tests: ok');

{
  // The layout: sizes kept in range, the stored layout read back safely, panels fitted to the window, the narrow
  // breakpoint, the screen the Console button returns to, and the panel an openIde link names.
  eq([clampSize(900, 180, 520, 260), clampSize('x', 180, 520, 260), clampSize(10.6, 180, 520, 260)], [520, 260, 180], 'ide layout: a size is clamped, and a non-number is the default');
  eq(defaultLayout(), { left: 260, right: 380, leftOpen: true, rightOpen: true }, 'ide layout: both panels are open by default, whatever the window');
  eq(parseLayout(null), defaultLayout(), 'ide layout: nothing stored is the default');
  eq(parseLayout('{not json'), defaultLayout(), 'ide layout: corrupt storage is the default');
  eq(parseLayout('{"left":9999,"right":1,"leftOpen":false,"rightOpen":"yes"}'), { left: LEFT_SIZE.max, right: RIGHT_SIZE.min, leftOpen: false, rightOpen: true }, 'ide layout: out-of-range sizes are clamped and a non-boolean open flag is the default');
  const kept = { left: 333, right: 401, leftOpen: false, rightOpen: true };
  eq(parseLayout(serializeLayout(kept)), kept, 'ide layout: a stored layout reads back as it was saved');
  eq(
    [fitSize(900, 'left', 1000, 0), fitSize(900, 'right', 1000, 300), fitSize(900, 'right', 800, 400)],
    [LEFT_SIZE.max, 1000 - 44 - 300 - CENTRE_MIN, RIGHT_SIZE.min],
    'ide layout: a panel is fitted to the window, keeping the editor its minimum',
  );
  eq([isNarrow(899), isNarrow(900)], [true, false], 'ide layout: narrow is under 900px');
  eq(originFallback(null), '#/overview', 'ide origin: no console route is the overview');
  eq([originFallback('#/ide/agent:7'), originFallback('#/ide'), originFallback('#/ide?panel=changes')], ['#/overview', '#/overview', '#/overview'], 'ide origin: an IDE route is never the origin');
  eq(originFallback('#/agents/105'), '#/agents/105', 'ide origin: the console route the IDE was opened from');
  eq([leftViewFor('changes'), leftViewFor('agent'), rightTabFor('agent'), rightTabFor('changes')], ['changes', null, 'agent', null], 'ide layout: a link’s panel is a left view or a right tab');
}
sectionEnd('ide layout tests: ok');

{
  // The panels on a narrow window are view state: the drawer and the sheet are never saved. Only an explicit toggle of a
  // docked panel saves the owner's choice, and a window that grows back past the breakpoint shows that choice again.
  const open = { left: 260, right: 380, leftOpen: true, rightOpen: true };
  const stored = serializeLayout(open);
  eq([leftShows(open, SHUT, true), rightShows(open, SHUT, true)], [false, false], 'ide panels: a narrow window starts with the drawer and the sheet shut');
  const drawerOn = toggleLeft(open, SHUT, true);
  eq([drawerOn.save, drawerOn.layout === open, leftShows(drawerOn.layout, drawerOn.sheets, true)], [false, true, true], 'ide panels: opening the drawer on a narrow window saves nothing');
  const sheetOn = openRight(open, SHUT, true);
  eq([sheetOn.save, rightShows(sheetOn.layout, sheetOn.sheets, true)], [false, true], 'ide panels: a tab in the right panel on a mid window opens the sheet and saves nothing');
  eq(closeSheets(sheetOn.sheets, true, true), SHUT, 'ide panels: a pick, the scrim or Escape on a narrow window shuts the drawer and the sheet');
  eq(closeSheets({ drawer: true, sheet: true }, false, true), { drawer: true, sheet: false }, 'ide panels: a pick at a mid width shuts the sheet and leaves the drawer alone');
  eq(serializeLayout(open), stored, 'ide panels: the narrow actions leave the stored layout as it was');
  // A docked toggle flips the saved flag and saves it.
  const hidden = toggleLeft(open, SHUT, false);
  eq([hidden.save, hidden.layout.leftOpen, leftShows(hidden.layout, hidden.sheets, false)], [true, false, false], 'ide panels: a docked toggle hides the side panel and is saved');
  const rightHidden = toggleRight(open, SHUT, false);
  eq([rightHidden.save, rightHidden.layout.rightOpen, rightShows(rightHidden.layout, rightHidden.sheets, false)], [true, false, false], 'ide panels: a docked toggle hides the right panel and is saved');
  eq(toggleRight(open, SHUT, true).save, false, 'ide panels: a toggle of the right sheet on a mid window is view only');
  eq([openRight(rightHidden.layout, SHUT, false).save, openRight(open, SHUT, false).save], [true, false], 'ide panels: a right panel opened from shut is saved open, one already open is not saved again');
  // Growing the window: a closed drawer or sheet leaves no trace, so the saved choice shows again.
  eq([leftShows(open, SHUT, false), rightShows(open, SHUT, false)], [true, true], 'ide panels: growing the window restores the saved choice, open by default');
  eq(parseLayout(stored), open, 'ide panels: the stored layout reads back as the owner saved it');
}
sectionEnd('ide panels tests: ok');

{
  // The workspace kinds: the picker's groups in order (an empty group is left out), the labels, and the views a kind has.
  const listed: IdeWorkspaceInfo[] = [
    { id: 'repo:api@main', kind: 'repo', owner: 'api', name: 'api', writable: true, status: 'ok' },
    { id: 'station:w3', kind: 'station', owner: 'w3', name: 'studio', writable: true, status: 'online', os: 'macos' },
    { id: 'agent:7', kind: 'agent', owner: '7', name: 'Ada', writable: true, status: 'working' },
    { id: 'station:w4', kind: 'station', owner: 'w4', name: '', writable: true, status: 'online', os: 'linux' },
  ];
  eq(
    groupWorkspaces(listed).map((g) => [g.kind, g.label, g.items.map((w) => w.id)]),
    [
      ['agent', 'Agents', ['agent:7']],
      ['station', 'Workstations', ['station:w3', 'station:w4']],
      ['repo', 'Repos', ['repo:api@main']],
    ],
    'ide picker: the workspaces group as agents, workstations, repos, each keeping the list order',
  );
  eq(groupWorkspaces(listed.filter((w) => w.kind === 'repo')).map((g) => g.label), ['Repos'], 'ide picker: a kind with no workspaces has no group');
  eq([osLabel('macos'), osLabel('windows'), osLabel('linux'), osLabel(undefined), osLabel('plan9')], ['macOS', 'Windows', 'Linux', 'Unknown OS', 'plan9'], 'ide picker: a station OS reads as a short label');
  eq(
    [pickerLabel(listed[1]!), pickerLabel(listed[2]!), pickerLabel(listed[3]!), pickerLabel({ ...listed[0]!, writable: false })],
    ['Workstation · studio · macOS · ● online', 'Agent · Ada', 'Workstation · w4 · Linux · ● online', 'Repo · api (read-only)'],
    'ide picker: each kind labels its option, and a station shows its OS and status',
  );
  // What a selection has: an agent its chat, terminal, editor and diff; a workstation its terminal and editor (no chat, no
  // diff); a repository or no workspace the editor and diff. The right panel: an agent all five tabs; a workstation its files;
  // a repository or none the project's three.
  eq(centreViewsFor('agent', true), ['chat', 'terminal', 'editor', 'diff'], 'ide views: an agent has chat, terminal, editor and diff');
  eq(centreViewsFor('station', false), ['terminal', 'editor'], 'ide views: a workstation has its terminal and editor, no chat and no diff');
  eq([centreViewsFor('repo', false), centreViewsFor(null, false)], [['editor', 'diff'], ['editor', 'diff']], 'ide views: a repository and no selection have the editor and the diff');
  eq(centreViewsFor(null, true), ['chat', 'terminal', 'editor', 'diff'], 'ide views: an agent with no VM has every view');
  eq(rightViewsFor('agent', true), ['board', 'files', 'tasks', 'approvals', 'activity'], 'ide views: an agent has every right tab');
  eq(rightViewsFor('station', false), ['files'], 'ide views: a workstation has its files, with no project, transcript or approvals');
  eq([rightViewsFor('repo', false), rightViewsFor(null, false)], [['board', 'files', 'tasks'], ['board', 'files', 'tasks']], 'ide views: a repository and no selection have the board, files and tasks');
  eq([hasTerminal('agent', true), hasTerminal('station', false), hasTerminal('repo', false), hasTerminal(null, false)], [true, true, false, false], 'ide views: an agent and a workstation have a terminal, a repository has none');
  eq([hasDiff('agent'), hasDiff('repo'), hasDiff('station'), hasDiff(null)], [true, true, false, true], 'ide views: only a station has no diff');
  eq([runsFor('agent'), runsFor('station'), runsFor('repo')], [true, true, false], 'ide views: runs are for agents and stations, not repositories yet');
  eq(stationCrumb({ name: 'studio', owner: 'w3' }), ['Workstations', 'studio', 'work folder'], 'ide breadcrumb: a workstation is its group, its name and its work folder');
  eq(stationCrumb({ name: '', owner: 'w4' }), ['Workstations', 'w4', 'work folder'], 'ide breadcrumb: a workstation with no name shows its id');
}
sectionEnd('ide workspace kind tests: ok');
// The IDE terminal's logic (views/ide-terminal-model.ts): base64 over bytes, the socket's frames, the offset a re-attach
// resumes from, the reconnect schedule and the session storage. The view itself needs a DOM and runs in the browser.
{
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  // Base64 of bytes: multi-byte UTF-8 survives, and a long buffer crosses the internal chunks.
  const text = 'héllo — 日本語 🙂 é';
  eq(decoder.decode(base64ToBytes(bytesToBase64(encoder.encode(text)))), text, 'ide terminal base64: multi-byte UTF-8 survives the round trip');
  eq(bytesToBase64(encoder.encode('é')), 'w6k=', 'ide terminal base64: the UTF-8 bytes are encoded, not the code units');
  const big = new Uint8Array(100_000).map((_, i) => (i * 31) & 0xff);
  const back = base64ToBytes(bytesToBase64(big));
  eq(back.length === big.length && back.every((b, i) => b === big[i]), true, 'ide terminal base64: a long buffer round-trips');
  eq(splitInput(new Uint8Array(40_000), 16_384).map((chunk) => chunk.length), [16_384, 16_384, 7_232], 'ide terminal input: frames of at most one chunk');

  // The client frames.
  eq(JSON.parse(openFrame(80, 24)), { t: 'open', cols: 80, rows: 24 }, 'ide terminal frame: open without a cwd');
  eq(JSON.parse(openFrame(80, 24, 'src')), { t: 'open', cols: 80, rows: 24, cwd: 'src' }, 'ide terminal frame: open with a cwd');
  eq(JSON.parse(openFrame(80, 24, '')), { t: 'open', cols: 80, rows: 24 }, 'ide terminal frame: an empty cwd is left out');
  eq(JSON.parse(attachFrame('term-1', 12)), { t: 'attach', term: 'term-1', offset: 12 }, 'ide terminal frame: attach');
  eq(JSON.parse(inputFrame(encoder.encode('é'))), { t: 'in', data: 'w6k=' }, 'ide terminal frame: input is base64 of its bytes');
  eq(JSON.parse(resizeFrame(100, 30)), { t: 'resize', cols: 100, rows: 30 }, 'ide terminal frame: resize');
  eq(JSON.parse(killFrame()), { t: 'kill' }, 'ide terminal frame: kill');

  // The hub's frames.
  const out = parseServerFrame(JSON.stringify({ t: 'out', data: bytesToBase64(encoder.encode('日本')), offset: 5 }));
  eq(out && out.t === 'out' ? [decoder.decode(out.data), out.offset] : null, ['日本', 5], 'ide terminal frame: out carries its bytes and offset');
  eq(parseServerFrame('{"t":"opened","term":"term-3"}'), { t: 'opened', term: 'term-3' }, 'ide terminal frame: opened');
  eq(parseServerFrame('{"t":"lost","bytes":9}'), { t: 'lost', bytes: 9 }, 'ide terminal frame: lost');
  eq(parseServerFrame('{"t":"exit","code":null}'), { t: 'exit', code: null }, 'ide terminal frame: exit without a code');
  eq(parseServerFrame('{"t":"exit","code":2}'), { t: 'exit', code: 2 }, 'ide terminal frame: exit with a code');
  eq(parseServerFrame('{"t":"error","message":"no such terminal"}'), { t: 'error', message: 'no such terminal' }, 'ide terminal frame: error');
  eq(parseServerFrame('{"t":"pong"}'), { t: 'pong' }, 'ide terminal frame: pong');
  eq(
    [parseServerFrame('not json'), parseServerFrame('{"t":"nope"}'), parseServerFrame('{"t":"out","data":"!!","offset":0}'), parseServerFrame('{"t":"out","data":"","offset":-1}'), parseServerFrame('{"t":"opened"}')],
    [null, null, null, null, null],
    'ide terminal frame: a frame that is not of the protocol is ignored',
  );

  // Offset tracking: a re-attach resumes one past the last byte the view holds (a count of bytes, not characters).
  const wire = [
    { t: 'out', data: bytesToBase64(encoder.encode('héllo')), offset: 0 },
    { t: 'out', data: bytesToBase64(encoder.encode(' wörld')), offset: 6 },
  ].map((frame) => parseServerFrame(JSON.stringify(frame)));
  const last = wire[1];
  eq(last && last.t === 'out' ? nextOffset(last) : -1, 13, 'ide terminal offset: one past the last byte held');

  // The reconnect schedule: 1 s, 2 s, 5 s, then 10 s for good.
  eq([0, 1, 2, 3, 4, 50].map(reconnectDelay), [1_000, 2_000, 5_000, 10_000, 10_000, 10_000], 'ide terminal reconnect: 1 s, 2 s, 5 s, then 10 s');
  eq([reconnectDelay(-1), reconnectDelay(0.5)], [1_000, 1_000], 'ide terminal reconnect: a negative or fractional attempt is the first wait');
  eq(MAX_TERMINALS, 4, 'ide terminal: four terminals per workspace, the hub cap');

  // Session storage: one key per workspace and slot, and a storage that refuses is no session.
  const memory = new Map<string, string>();
  const store: KeyValueStore = {
    getItem: (key) => memory.get(key) ?? null,
    setItem: (key, value) => {
      memory.set(key, value);
    },
    removeItem: (key) => {
      memory.delete(key);
    },
  };
  const key = sessionKey('agent-1', '0');
  eq(new Set([sessionKey('agent-1', '0'), sessionKey('agent-2', '0'), sessionKey('agent-1', '1')]).size, 3, 'ide terminal session keys: one per workspace and slot');
  eq(sessionKey('a/b', '0').includes('/'), false, 'ide terminal session keys: the workspace name is encoded');
  eq(readSession(store, key), null, 'ide terminal session: nothing stored reads as null');
  writeSession(store, key, { term: 'term-1', offset: 42 });
  eq(readSession(store, key), { term: 'term-1', offset: 42 }, 'ide terminal session: a stored session reads back');
  eq(readSession(store, sessionKey('agent-1', '1')), null, 'ide terminal session: slots do not share a session');
  store.setItem(key, '{"term":"","offset":1}');
  eq(readSession(store, key), null, 'ide terminal session: an empty term is no session');
  store.setItem(key, 'not json');
  eq(readSession(store, key), null, 'ide terminal session: a malformed value is no session');
  clearSession(store, key);
  eq(readSession(store, key), null, 'ide terminal session: a cleared session reads as null');
  eq(readSession(undefined, key), null, 'ide terminal session: no storage at all is no session');
  const blocked: KeyValueStore = {
    getItem: () => {
      throw new Error('storage is blocked');
    },
    setItem: () => {
      throw new Error('storage is blocked');
    },
    removeItem: () => {
      throw new Error('storage is blocked');
    },
  };
  let threw = false;
  try {
    writeSession(blocked, key, { term: 'term-1', offset: 0 });
    clearSession(blocked, key);
  } catch {
    threw = true;
  }
  eq([readSession(blocked, key), threw], [null, false], 'ide terminal session: blocked storage is no session and no error');

  // The socket's address: wss on https, ws on http, and the workspace encoded.
  eq(terminalSocketUrl('agent 1/x', { protocol: 'https:', host: 'console.example' }), 'wss://console.example/v1/ide/terminal/agent%201%2Fx', 'ide terminal socket: wss on https, the workspace encoded');
  eq(terminalSocketUrl('agent-7', { protocol: 'http:', host: 'localhost:8080' }), 'ws://localhost:8080/v1/ide/terminal/agent-7', 'ide terminal socket: ws on http');

  // Notices: no control characters from the hub reach the terminal.
  eq(plainText('\u001b[31mred\u0007\n'), '[31mred', 'ide terminal notices: control characters are dropped');
}
sectionEnd('ide terminal tests: ok');

// Workstations: the labels, chips and the settings form's serialization the screen draws from.
const linuxFacts: StationFacts = {
  os: 'linux', os_version: '6.8', arch: 'x86_64', hostname: 'studio', form: 'desktop', cpus: 16, ram_mb: 65536,
  gpus: [{ vendor: 'nvidia', name: 'RTX 4090', vram_mb: 24576 }], displays: [{ id: 1, w: 2560, h: 1440, scale: 1 }],
  account: { mode: 'agent_user', name: 'agentplaza' }, apps: 120, virtual_displays: false, ui_tree: true,
};
const macFacts: StationFacts = {
  ...linuxFacts, os: 'macos', os_version: '15.2', form: 'mini', ram_mb: 16384, gpus: [], displays: [], account: { mode: 'owner', name: 'Zarathos' },
};
eq(osText(linuxFacts), 'Linux 6.8', 'workstation os: the label and the version');
eq(osText(macFacts), 'macOS 15.2', 'workstation os: macOS');
eq(formLabel('vm'), 'VM', 'workstation form: a VM');
eq([ramText(16384), ramText(8192), ramText(0)], ['16 GB RAM', '8.0 GB RAM', '—'], 'workstation memory: whole GiB from 10, one decimal below, none');
eq(gpuLine({ vendor: 'nvidia', name: 'RTX 4090', vram_mb: 24576 }), 'RTX 4090 · 24 GB', 'workstation gpu: its memory in GB');
eq(gpuLine({ vendor: 'apple', name: 'Apple M4', vram_mb: 0 }), 'Apple M4 · shared memory', 'workstation gpu: shared memory');
eq(gpuLine({ vendor: 'x', name: 'Small', vram_mb: 512 }), 'Small · 512 MB', 'workstation gpu: under a GB in MB');
eq([gpuSummary([]), gpuSummary(linuxFacts.gpus)], ['no GPU', 'RTX 4090'], 'workstation gpu summary');
eq([displaysText([]), displaysText(linuxFacts.displays)], ['none', '2560×1440'], 'workstation displays');
eq(stationSummary(linuxFacts), 'Linux 6.8 · Desktop · 64 GB RAM · RTX 4090', 'workstation row summary: OS, form, memory, GPUs');
eq(stationSummary(macFacts), 'macOS 15.2 · Mini · 16 GB RAM · no GPU', 'workstation row summary: no GPU in words');
eq(statusChip('draining'), { label: 'draining', tone: 'warn' }, 'workstation status chip: draining is a warning');
eq(statusChip('revoked'), { label: 'revoked', tone: 'danger' }, 'workstation status chip: revoked is a danger');
eq(accountBadge(macFacts.account), { text: 'agents act as Zarathos', tone: 'warn' }, 'workstation account badge: the owner\'s own account is flagged');
eq(accountBadge(linuxFacts.account), null, 'workstation account badge: a separate agent account is not flagged');
eq(accountText(linuxFacts.account), 'a separate agent account (agentplaza)', 'workstation account in words: the agent account');

const lease = (state: StationLease['state'], position: number | null = null): StationLease => ({
  id: 'l1', station: 'ws-1', agent: 7, task: null, kind: 'pixel', session: null, started_ms: 1, state, position,
});
eq(leaseCount([lease('queued', 1), lease('opening'), lease('active')]), { holding: 2, waiting: 1 }, 'workstation leases: holding and waiting');
eq([leaseStateChip(lease('queued', 2)), leaseStateChip(lease('queued'))], [{ label: 'queued #2', tone: 'warn' }, { label: 'queued', tone: 'warn' }], 'workstation lease chip: queued shows its place');
eq(leaseStateChip(lease('active')), { label: 'active', tone: 'ok' }, 'workstation lease chip: active');
eq(takeoverText('release_lease').includes('back to the queue'), true, 'workstation takeover: the release policy is explained');
eq(takeoverText('agent_waits').includes('resumes when you give the session back'), true, 'workstation takeover: the wait policy is explained');
eq(joinCommand('abc.def'), 'agentplaza workstation join abc.def', 'workstation join: the command for the machine');
eq(codeChip('used'), { label: 'Used', tone: 'info' }, 'workstation join code chip: used');
eq([countdown(754_000), countdown(-5)], ['12:34', '00:00'], 'workstation join code countdown: mm:ss, never negative');
eq([nameProblem('  '), nameProblem('a'.repeat(65)), nameProblem('ab\u0007'), nameProblem(' studio ')], ['Give the workstation a name.', 'The name is up to 64 characters.', 'The name has no control characters.', null], 'workstation name: the hub\'s rules');
eq(nameProblem('a'.repeat(64)), null, 'workstation name: 64 characters is the most');
eq([auditLabel('lease_granted'), auditLabel('something_new')], ['Lease granted', 'something new'], 'workstation audit: known kinds have words, unknown ones show as text');
const named = (name: string) => ({ name }) as StationView;
const input = [named('studio'), named('Alpha'), named('beta')];
eq([sortStations(input).map((s) => s.name), input.map((s) => s.name)], [['Alpha', 'beta', 'studio'], ['studio', 'Alpha', 'beta']], 'workstation list: name order, input untouched');

const defaults: StationSettings = { takeover: 'agent_waits', record: true, retention_days: 30, record_fps: 2, record_takeovers: true, work_folder: '~/AgentPlaza', snapshot_reset: false, rate_per_hour: 0 };
const form = settingsForm(defaults);
eq(form, { takeover: 'agent_waits', record: true, retention_days: '30', record_fps: '2', record_takeovers: true, work_folder: '~/AgentPlaza', snapshot_reset: false, rate_per_hour: '0' }, 'workstation settings form: the numbers as text');
eq(readSettings(form), { ok: true, settings: defaults }, 'workstation settings form: the defaults round-trip');
eq(readSettings({ ...form, work_folder: '  /srv/work  ', retention_days: ' 7 ', record_fps: '1' }), { ok: true, settings: { ...defaults, work_folder: '/srv/work', retention_days: 7, record_fps: 1 } }, 'workstation settings form: trimmed and parsed');
const edited: SettingsForm = { takeover: 'keep_working_pointer_paused', record: false, retention_days: '365', record_fps: '5', record_takeovers: false, work_folder: 'D:\\work', snapshot_reset: true, rate_per_hour: '1000000' };
eq(readSettings(edited), { ok: true, settings: { takeover: 'keep_working_pointer_paused', record: false, retention_days: 365, record_fps: 5, record_takeovers: false, work_folder: 'D:\\work', snapshot_reset: true, rate_per_hour: 1000000 } }, 'workstation settings form: every field, at the bounds');
const refused = (patch: Partial<SettingsForm>): boolean => readSettings({ ...form, ...patch }).ok;
eq(
  [
    refused({ retention_days: '0' }),
    refused({ retention_days: '1.5' }),
    refused({ retention_days: '3651' }),
    refused({ record_fps: '0' }),
    refused({ record_fps: '6' }),
    refused({ record_fps: '31' }),
    refused({ record_fps: '' }),
    refused({ work_folder: '   ' }),
    refused({ work_folder: 'a'.repeat(513) }),
    refused({ work_folder: 'a\tb' }),
    refused({ takeover: 'nope' as SettingsForm['takeover'] }),
    refused({ rate_per_hour: '' }),
    refused({ rate_per_hour: '1.5' }),
    refused({ rate_per_hour: '-1' }),
    refused({ rate_per_hour: '1000001' }),
  ],
  [false, false, false, false, false, false, false, false, false, false, false, false, false, false, false],
  'workstation settings form: every out-of-bounds value is refused',
);
eq(readSettings({ ...form, rate_per_hour: ' 2500 ' }), { ok: true, settings: { ...defaults, rate_per_hour: 2500 } }, 'workstation rate: whole credits, trimmed');
eq(readSettings({ ...form, retention_days: '0' }), { ok: false, error: 'Keep recordings for whole days, from 1 to 3650.' }, 'workstation settings form: the retention message');
sectionEnd('workstation tests: ok');

// ---- Workstation VMs: the New VM form's rules and body (docs/design/workstations.md section 10) ----
{
  const plazaNode: NodeVmFacts = { id: 'n1', name: 'studio', reported: true, backends: ['plaza'], qemu: false, apple: false, online: true };
  const qemuNode: NodeVmFacts = { id: 'n2', name: 'tower', reported: true, backends: ['plaza', 'qemu'], qemu: true, apple: false, online: true };
  const mac: NodeVmFacts = { id: 'n3', name: 'mini', reported: true, backends: ['plaza'], qemu: false, apple: true, online: true };
  const linux: VmForm = { ...VM_FORM_DEFAULTS, node: 'n1', name: 'desk-1' };
  const windows: VmForm = { ...linux, node: 'n2', os: 'windows', ram_mb: '8192', image: 'D:\\images\\win.img' };
  const macos: VmForm = { ...linux, node: 'n3', os: 'macos' };
  const problemOf = (form: VmForm, node: NodeVmFacts | undefined, macs = 0): string => vmProblem(form, node, macs) ?? '';

  for (const ok of ['desk-1', 'a', 'a'.repeat(63), 'x-1', ' desk-2 ']) eq(vmNameProblem(ok), null, `vm name ${ok}: a DNS label`);
  for (const bad of ['', '  ', 'Desk', '-a', 'a-', 'a_b', 'a b', 'a'.repeat(64)]) eq(vmNameProblem(bad) !== null, true, `vm name ${JSON.stringify(bad)}: refused`);
  eq([wholeIn('16', 1, 16), wholeIn(' 4 ', 1, 16), wholeIn('17', 1, 16), wholeIn('', 1, 16), wholeIn('1.5', 1, 16)], [16, 4, null, null, null], 'vm sizes: whole numbers in range only');
  eq([pciProblem('0000:01:00.0'), pciProblem('0000:0A:00.7')], [null, null], 'vm pci: a well-formed address');
  eq([pciProblem('01:00.0') !== null, pciProblem('0000:01:00.9') !== null], [true, true], 'vm pci: a short or a bad function is refused');
  eq([imageProblem('/srv/win.img'), imageProblem('D:\\images\\win.img')], [null, null], 'vm image: absolute or a drive path');
  eq([imageProblem('win.img') !== null, imageProblem('') !== null], [true, true], 'vm image: relative or empty is refused');

  eq(readVmForm(linux, plazaNode, 0), { ok: true, body: { node: 'n1', os: 'linux', name: 'desk-1', cpus: 4, ram_mb: 8192, disk_gb: 40, gpu: 'none', backend: 'auto' } }, 'vm body: a linux VM names only its sizes, GPU and backend');
  eq(vmBody({ ...linux, name: ' desk-1 ', image: '/unused' }).image, undefined, 'vm body: no image unless the VM is Windows');
  eq(vmBody({ ...linux, gpu: 'vfio', pci: ' 0000:01:00.0 ' }).gpu, 'vfio:0000:01:00.0', 'vm body: a passthrough GPU is vfio:<pci-id>');
  eq(readVmForm(windows, qemuNode, 0), { ok: true, body: { node: 'n2', os: 'windows', name: 'desk-1', cpus: 4, ram_mb: 8192, disk_gb: 40, gpu: 'none', backend: 'auto', image: 'D:\\images\\win.img' } }, 'vm body: a Windows VM carries its image');

  eq([problemOf({ ...linux, cpus: '0' }, plazaNode), problemOf({ ...linux, cpus: '17' }, plazaNode)].map((p) => p.includes('CPUs')), [true, true], 'vm cpus: 1 to 16');
  eq(problemOf({ ...linux, ram_mb: '1024' }, plazaNode).includes('Memory'), true, 'vm memory: at least 2048 MiB for a linux VM');
  eq(problemOf({ ...windows, ram_mb: '2048' }, qemuNode).includes('4096'), true, 'vm memory: 4096 MiB for Windows');
  eq(problemOf({ ...linux, disk_gb: '7' }, plazaNode).includes('Disk'), true, 'vm disk: at least 8 GiB');
  eq(problemOf({ ...windows, image: '' }, qemuNode).includes('needs the path'), true, 'vm windows: an image is required');
  eq(problemOf({ ...windows, image: 'win.img' }, qemuNode).includes('absolute'), true, 'vm windows: the image is an absolute path');
  eq(problemOf({ ...linux, gpu: 'vfio', pci: '01:00.0' }, qemuNode).includes('PCI'), true, 'vm gpu: a passthrough GPU needs its address');
  eq(problemOf(linux, undefined), 'Choose a node to run the VM on.', 'vm node: one must be chosen');

  eq(problemOf(macos, plazaNode).includes('Apple hardware'), true, 'vm macos: refused on a node that is not an Apple Mac');
  eq(readVmForm(macos, mac, 0), { ok: true, body: { node: 'n3', os: 'macos', name: 'desk-1', cpus: 4, ram_mb: 8192, disk_gb: 40, gpu: 'none', backend: 'auto' } }, 'vm macos: fine on Apple hardware, with plaza');
  eq(problemOf(macos, mac, 2).includes('At most 2'), true, 'vm macos: at most two per Mac');
  eq(problemOf(macos, mac, 1), '', 'vm macos: one more is fine');

  eq(backendFor({ os: 'linux', gpu: 'none', backend: 'auto' }, false), { ok: true, backend: 'plaza' }, 'vm backend: a linux VM is plaza by default');
  eq(backendFor({ os: 'windows', gpu: 'none', backend: 'auto' }, true), { ok: true, backend: 'qemu' }, 'vm backend: auto takes QEMU for Windows');
  eq(backendFor({ os: 'linux', gpu: 'vfio', backend: 'auto' }, true), { ok: true, backend: 'qemu' }, 'vm backend: auto takes QEMU for a passthrough GPU');
  eq(backendFor({ os: 'windows', gpu: 'none', backend: 'auto' }, false), { ok: false, error: 'This node has no QEMU/OVMF; install qemu and edk2-ovmf.' }, 'vm backend: no QEMU on the node is refused, with the install hint');
  eq(backendFor({ os: 'windows', gpu: 'none', backend: 'plaza' }, true), { ok: true, backend: 'plaza' }, 'vm backend: plaza as chosen');
  eq(backendFor({ os: 'macos', gpu: 'none', backend: 'qemu' }, true).ok, false, 'vm backend: a macOS VM is never QEMU');
  eq(backendHint({ os: 'linux', gpu: 'none', backend: 'auto' }, false), 'Runs on plaza: auto picks plaza for this guest.', 'vm backend hint: a linux VM');
  eq(backendHint({ os: 'windows', gpu: 'none', backend: 'auto' }, true), 'Runs on QEMU: auto picks QEMU for a Windows VM.', 'vm backend hint: a Windows VM');
  eq(backendHint({ os: 'windows', gpu: 'none', backend: 'plaza' }, true).includes('cannot run a Windows VM'), true, 'vm backend hint: plaza warns for Windows');

  const view = (over: Record<string, unknown>): NodeView => ({ id: 'n9', name: '', revoked: false, online: true, machine: null, ...over }) as unknown as NodeView;
  eq(nodeVmFacts(view({ name: 'tower', machine: { workstation_backends: ['plaza', 'qemu'] } })), { id: 'n9', name: 'tower', reported: true, backends: ['plaza', 'qemu'], qemu: true, apple: false, online: true }, 'vm node facts: the backends a node reports');
  eq(nodeVmFacts(view({ machine: { vendor: 'apple', apple: { chip: 'M4' }, workstation_backends: ['plaza'] } })).apple, true, 'vm node facts: Apple hardware by its vendor or its Apple details');
  eq(nodeVmFacts(view({ machine: { cpus: 8 } })), { id: 'n9', name: 'n9', reported: false, backends: [], qemu: false, apple: false, online: true }, 'vm node facts: an older node reports no backends');
  eq(nodeChoices([view({ id: 'n1', revoked: true }), view({ id: 'n2' })]).map((n) => n.id), ['n2'], 'vm nodes: a revoked node is not offered');
  eq(nodeFactsLine(undefined), 'Choose the node that runs the VM.', 'vm node line: none chosen');
  eq(nodeFactsLine(qemuNode), 'Backends: plaza, qemu · QEMU/OVMF: yes · Apple hardware: no', 'vm node line: backends, QEMU and Apple');
  eq(nodeFactsLine({ ...plazaNode, online: false }).includes('offline'), true, 'vm node line: an offline node says so');
  eq(nodeOptionLabel(qemuNode), 'tower (n2) · plaza, qemu · QEMU/OVMF', 'vm node option: its backends and QEMU');
  eq(nodeOptionLabel({ ...plazaNode, reported: false, backends: [], online: false }), 'studio (n1) · backends not reported · no QEMU/OVMF · offline', 'vm node option: an older node and offline');

  const vm = (over: Partial<StationVm>): StationVm => ({ id: 'v1', node: 'n1', vm: 'wv1', name: 'a', os: 'linux', cpus: 4, ram_mb: 8192, disk_gb: 40, gpu: 'none', image: null, backend: 'plaza', station: null, created_ms: 0, online: false, ...over });
  eq([vmStateChip(vm({ online: true, station: 'w1' })), vmStateChip(vm({ station: 'w1' })), vmStateChip(vm({}))], [{ label: 'online', tone: 'ok' }, { label: 'offline', tone: 'muted' }, { label: 'joining', tone: 'info' }], 'vm state: online, offline once enrolled, joining before');
  eq(sortVms([vm({ id: 'v2', name: 'beta' }), vm({ id: 'v1', name: 'alpha' })]).map((v) => v.id), ['v1', 'v2'], 'vm list: by name');
  eq(macosOn([vm({ os: 'macos' }), vm({ os: 'macos', node: 'n2' }), vm({ os: 'linux' })], 'n1'), 1, 'vm list: macOS VMs on one node');
  eq([vmOsText('macos'), vmGpuText('vfio:0000:01:00.0'), vmGpuText('none'), vmGpuText('virtual')], ['macOS', 'passthrough 0000:01:00.0', 'no GPU', 'virtual GPU'], 'vm labels: OS and GPU in words');
}
sectionEnd('workstation VM tests: ok');

// Workstations, live view and takeover, recordings and the work folder: the wire shapes and the mappings the pages draw from.
const frameBytes = (header: Record<string, unknown>, image: number[]): ArrayBuffer => {
  const head = new TextEncoder().encode(JSON.stringify(header));
  const out = new Uint8Array(4 + head.length + image.length);
  new DataView(out.buffer).setUint32(0, head.length, false);
  out.set(head, 4);
  out.set(image, 4 + head.length);
  return out.buffer;
};
const parsed = parseFrame(frameBytes({ w: 4, h: 2, format: 'jpeg', ts: 9, seq: 3 }, [1, 2, 3]));
eq(parsed && { ...parsed, image: Array.from(parsed.image) }, { w: 4, h: 2, format: 'jpeg', ts: 9, seq: 3, image: [1, 2, 3] }, 'live frame: the header and the image after it');
eq(
  [
    parseFrame(new ArrayBuffer(2)),
    parseFrame(frameBytes({ w: 4, h: 2, format: 'gif', ts: 9, seq: 3 }, [])),
    parseFrame(frameBytes({ w: 0, h: 2, format: 'png', ts: 9, seq: 3 }, [])),
    parseFrame(frameBytes({ w: 4, h: 2, format: 'png', ts: 9 }, [])),
    parseFrame(frameBytes({ w: 4, h: 2, format: 'png', ts: 9, seq: 1.5 }, [])),
  ].map((f) => f === null),
  [true, true, true, true, true],
  'live frame: anything that is not a frame of the shape is refused',
);
{
  // A header longer than the message is refused, not read past its end.
  const buf = new Uint8Array([0, 0, 1, 0, 123]).buffer;
  eq(parseFrame(buf), null, 'live frame: a header longer than the message is refused');
}
eq(parseScreenText('{"t":"started","session":4}'), { t: 'started', session: 4 }, 'live text: started names the session');
eq([parseScreenText('{"t":"ended","reason":"done"}'), parseScreenText('{"t":"pong"}'), parseScreenText('{"t":"nope"}'), parseScreenText('not json')], [{ t: 'ended', reason: 'done' }, { t: 'pong' }, null, null], 'live text: ended and pong, and anything else is null');
eq(parseControlText('{"t":"taken","session":4,"policy":"release_lease"}'), { t: 'taken', session: 4, policy: 'release_lease' }, 'takeover text: taken names the session and the policy');
eq([parseControlText('{"t":"taken","session":4,"policy":"whatever"}'), parseControlText('{"t":"given_back"}'), parseControlText('{"t":"error","error":"no"}')], [null, { t: 'given_back' }, { t: 'error', error: 'no' }], 'takeover text: an unknown policy is refused');
eq(screenSocketUrl('w1', { session: 'shared', fps: 5, scale: 0.5 }, { protocol: 'https:', host: 'hub.local:7443' }), 'wss://hub.local:7443/v1/stations/w1/screen?session=shared&fps=5&scale=0.5', 'live socket: wss on https, the view in the query');
eq(controlSocketUrl('a b', 12, { protocol: 'http:', host: 'localhost:7443' }), 'ws://localhost:7443/v1/stations/a%20b/control?session=12', 'takeover socket: ws on http, the id encoded');
eq([0, 1, 2, 3, 9].map(backoffMs), [1000, 2000, 5000, 10000, 10000], 'sockets: the reconnect waits, the last one repeats');
eq(framePoint(150, 75, { left: 50, top: 25, width: 200, height: 100 }, { w: 400, h: 200 }), { x: 200, y: 100 }, 'pointer: a client point is a point of the frame');
eq([framePoint(1000, -50, { left: 50, top: 25, width: 200, height: 100 }, { w: 400, h: 200 }), framePoint(1, 1, { left: 0, top: 0, width: 0, height: 0 }, { w: 4, h: 4 })], [{ x: 399, y: 0 }, null], 'pointer: clamped to the frame, and none before it has a size');
eq([inputButton(0), inputButton(1), inputButton(2), inputButton(3)], ['left', 'middle', 'right', null], 'pointer: the DOM buttons as the daemon names them');
eq([buttonAction('right', true), buttonAction('right', false), moveAction({ x: 3, y: 4 })], [{ action: 'down', button: 'right' }, { action: 'up', button: 'right' }, { action: 'move', x: 3, y: 4 }], 'pointer: press, release and move');
eq(
  [scrollNotches(120, 0), scrollNotches(-120, 0), scrollNotches(-1, 0), scrollNotches(0, 0), scrollNotches(5000, 0), scrollNotches(3, 1), scrollNotches(1, 2)],
  [2, -2, -1, 0, 20, 3, 10],
  'wheel: notches keep the sign (positive scrolls down), clamped to 20',
);
eq(wheelAction({ x: 5, y: 6 }, 0, 120, 0), { action: 'scroll', x: 5, y: 6, dx: 0, dy: 2 }, 'wheel: a turn down is dy above zero');
eq(wheelAction({ x: 5, y: 6 }, -240, -120, 0), { action: 'scroll', x: 5, y: 6, dx: -5, dy: -2 }, 'wheel: a turn up and left is negative');
const keyOf = (key: string, mods = ''): KeyEventLike => ({ key, ctrlKey: mods.includes('c'), altKey: mods.includes('a'), shiftKey: mods.includes('s'), metaKey: mods.includes('m') });
eq(
  [
    keyAction(keyOf('a')),
    keyAction(keyOf('S', 'cs')),
    keyAction(keyOf('Tab', 's')),
    keyAction(keyOf('Enter')),
    keyAction(keyOf(' ')),
    keyAction(keyOf(' ', 'c')),
    keyAction(keyOf('F5')),
    keyAction(keyOf('+', 'c')),
    keyAction(keyOf('k', 'm')),
    keyAction(keyOf('ArrowLeft', 'a')),
    keyAction(keyOf('Shift', 's')),
    keyAction(keyOf('Dead')),
    keyAction(keyOf('é', 'a')),
    keyAction(keyOf('é')),
    keyAction(keyOf('Escape')),
  ],
  [
    { action: 'type', text: 'a' },
    { action: 'key', combo: 'ctrl+shift+s' },
    { action: 'key', combo: 'shift+tab' },
    { action: 'key', combo: 'enter' },
    { action: 'type', text: ' ' },
    { action: 'key', combo: 'ctrl+space' },
    { action: 'key', combo: 'f5' },
    { action: 'key', combo: 'ctrl+plus' },
    { action: 'key', combo: 'meta+k' },
    { action: 'key', combo: 'alt+left' },
    null,
    null,
    null,
    { action: 'type', text: 'é' },
    { action: 'key', combo: 'escape' },
  ],
  'keys: printable text is typed, chords carry their modifiers in order, and an unnamed key is nothing',
);
eq(actionFrame({ action: 'key', combo: 'enter' }), '{"t":"input","action":{"action":"key","combo":"enter"}}', 'takeover: an input frame carries the action');
eq(takeoverBanner('agent_waits').includes('wait until you give it back'), true, 'takeover banner: the wait policy');
eq(takeoverBanner('release_lease').includes('back to the queue'), true, 'takeover banner: the release policy');

const leaseOf = (session: number | null, state: StationLease['state'], kind: StationLease['kind'] = 'pixel', task: string | null = null): StationLease => ({
  id: `l${session ?? 0}`, station: 'ws-1', agent: 7, task, kind, session, started_ms: 1, state, position: state === 'queued' ? 1 : null,
});
eq(
  sessionChoices([leaseOf(3, "active", "pixel", "render"), leaseOf(null, "queued"), leaseOf(null, "active", "shared"), leaseOf(5, "opening")], (cid) => `Ada${cid}`),
  [
    { value: 'shared', label: "Shared: the owner's screen" },
    { value: '3', label: 'Session 3: Ada7 · render' },
    { value: '5', label: 'Session 5: Ada7' },
  ],
  'sessions: the shared one, then each pixel lease that has a session and is not waiting',
);

eq(recordingSpan({ started_ms: 1000, ended_ms: 5000 }, null), 4000, 'recording span: start to end');
eq([recordingSpan({ started_ms: 1000, ended_ms: null }, 3000), recordingSpan({ started_ms: 1000, ended_ms: null }, null)], [2000, 0], 'recording span: an open one runs to its last action');
const evenlySpaced = [0, 1000, 2000, 3000, 4000];
eq([frameAt(0, evenlySpaced), frameAt(4000, evenlySpaced), frameAt(2000, evenlySpaced), frameAt(-5, evenlySpaced), frameAt(9000, evenlySpaced), frameAt(10, []), frameAt(3000, [0])], [1, 5, 3, 1, 5, 0, 1], 'timeline: the frame on screen at a time, clamped');
eq([frameAt(999, evenlySpaced), frameAt(1000, evenlySpaced)], [1, 2], 'timeline: a frame stays on screen until the next one is kept');
eq([frameTime(3, evenlySpaced), frameTime(1, evenlySpaced), frameTime(5, evenlySpaced), frameTime(9, evenlySpaced), frameTime(1, [0]), frameTime(1, [])], [2000, 0, 4000, 4000, 0, 0], 'timeline: a frame\'s time, the inverse of the frame at a time');
// The hub's times: a takeover holds the frames for a while, and the timeline shows the gap.
eq(frameTimes(5, 8200, 1000, [{ seq: 1, ts_ms: 1000 }, { seq: 2, ts_ms: 1200 }, { seq: 3, ts_ms: 1400 }, { seq: 4, ts_ms: 9000 }, { seq: 5, ts_ms: 9200 }]), [0, 200, 400, 8000, 8200], 'timeline: each frame at its captured time, from the start');
const gapped = frameTimes(5, 8200, 1000, [{ seq: 1, ts_ms: 1000 }, { seq: 2, ts_ms: 1200 }, { seq: 3, ts_ms: 1400 }, { seq: 4, ts_ms: 9000 }, { seq: 5, ts_ms: 9200 }]);
eq([frameAt(300, gapped), frameAt(5000, gapped), frameAt(8000, gapped), frameTime(4, gapped)], [2, 3, 4, 8000], 'timeline: the frame kept through a gap is the last before it');
eq(frameTimes(6, 5000, 0, [{ seq: 1, ts_ms: 0 }, { seq: 2, ts_ms: 1000 }, { seq: 3, ts_ms: 2000 }]), [0, 1000, 2000, 3000, 4000, 5000], 'timeline: frames past the known ones keep the known spacing, and end at the end');
eq(frameTimes(3, 3000, 0, [{ seq: 3, ts_ms: 2000 }]), [0, 1000, 2000], 'timeline: frames before the first known one are placed back from it');
eq(frameTimes(3, 1000, 0, [{ seq: 3, ts_ms: 500 }, { seq: 1, ts_ms: 0 }, { seq: 2, ts_ms: 400 }]), [0, 400, 500], 'timeline: the stamps in any order');
eq(frameTimes(2, 1000, 0, [{ seq: 1, ts_ms: 300 }, { seq: 2, ts_ms: 100 }]), [300, 300], 'timeline: a time never goes back');
eq([frameTimes(5, 4000, 1000, []), frameTimes(1, 0, 0, []), frameTimes(0, 4000, 0, [])], [[0, 1000, 2000, 3000, 4000], [0], []], 'timeline: without times the frames are spread evenly');
eq(frameTimes(3, 1000, 0, [{ seq: 7, ts_ms: 1 }, { seq: 0, ts_ms: 1 }]), [0, 500, 1000], 'timeline: a stamp naming no frame of the recording is ignored');
eq([activeAction([{ at_ms: 1100 }, { at_ms: 1500 }, { at_ms: 3000 }], 1000, 0), activeAction([{ at_ms: 1100 }, { at_ms: 1500 }, { at_ms: 3000 }], 1000, 100), activeAction([{ at_ms: 1100 }, { at_ms: 1500 }, { at_ms: 3000 }], 1000, 600), activeAction([{ at_ms: 1100 }, { at_ms: 1500 }, { at_ms: 3000 }], 1000, 5000)], [-1, 0, 1, 2], 'timeline: the newest action at or before a time');
eq([clockText(0), clockText(65_000), clockText(3_725_000), clockText(-5)], ['0:00', '1:05', '1:02:05', '0:00'], 'timeline: m:ss, h:mm:ss, never negative');
eq(actionText({ kind: 'owner_input', summary: { kind: 'type', text: 'hi' } }), 'owner input: text hi', 'actions: the kind, then the fields it was asked with');
eq(actionText({ kind: 'click', summary: { kind: 'click', x: 3, y: 4, button: 'left' } }), 'click: x 3, y 4, button left', 'actions: the fields in words');

const files = [{ path: 'a.md' }, { path: 'docs/x.md' }, { path: 'docs/sub/y.md' }, { path: 'zz/q.txt' }];
eq(treeOf(files, ''), { folders: [{ name: 'docs', path: 'docs' }, { name: 'zz', path: 'zz' }], files: [{ path: 'a.md' }] }, 'work folder: the root has its folders and its own files');
eq(treeOf(files, 'docs'), { folders: [{ name: 'sub', path: 'docs/sub' }], files: [{ path: 'docs/x.md' }] }, 'work folder: a folder has its folders and files, and no others');
eq([parentDir('docs/sub'), parentDir('a'), joinPath('', 'a'), joinPath('docs', 'x')], ['docs', '', 'a', 'docs/x'], 'work folder: parents and joined paths');
eq(crumbsOf('docs/sub'), [{ name: 'Work folder', path: '' }, { name: 'docs', path: 'docs' }, { name: 'sub', path: 'docs/sub' }], 'work folder: the trail to a folder');
eq([pendingPaths(['a', 'b']), pendingPaths([{ path: 'c' }, 5, { x: 1 }]), pendingPaths(null)], [['a', 'b'], ['c'], []], 'work folder: a paused sync\'s deletions, as strings or as objects');
eq([uploadProblem('a.txt', 10), uploadProblem('../x', 1), uploadProblem('a/b', 1), uploadProblem('', 1), uploadProblem('big', 512 * 1024 * 1024 + 1)].map((p) => p !== null), [false, true, true, true, true], 'work folder: upload names and sizes that cannot be kept');
eq([bytesText(0), bytesText(1536), bytesText(10 * 1024 * 1024), bytesText(512 * 1024 * 1024)], ['0 B', '1.5 KB', '10 MB', '512 MB'], 'work folder: sizes in words');

eq([readRate(' 120 '), readRate('1.5'), readRate('1000001'), readRate(''), readRate('0')], [120, null, null, null, 0], 'rate: whole credits from 0 to the bound');
eq([rateText(0), rateText(1500), dollarsPerHour(12)], ['Free', '$1.500 an hour', '0.012'], 'rate: free, or its dollars an hour');
const recDefaults: RecordingDefaults = { record: true, retention_days: 30, record_fps: 1, record_takeovers: false };
eq(readDefaults(defaultsForm(recDefaults)), { ok: true, defaults: recDefaults }, 'recording defaults: the form round-trips');
eq([readDefaults({ ...defaultsForm(recDefaults), record_fps: '6' }).ok, readDefaults({ ...defaultsForm(recDefaults), record_fps: '0' }).ok, readDefaults({ ...defaultsForm(recDefaults), retention_days: '0' }).ok], [false, false, false], 'recording defaults: out-of-bounds values are refused, frames a second from 1 to 5');
sectionEnd('workstation live, recordings and work folder tests: ok');


// ---- Workstations: the Machines list's row for a station (views/station-machine.ts) ----
const stationFacts = (over: Partial<StationFacts> = {}): StationFacts => ({ os: 'macos', os_version: '15.1', arch: 'arm64', hostname: 'mini', form: 'mini', cpus: 10, ram_mb: 16384, gpus: [], displays: [], account: { mode: 'agent_user', name: 'apz-agent' }, apps: 0, virtual_displays: false, ui_tree: false, ...over });
const station = (over: Partial<StationView> = {}): StationView => ({ id: 'st1', name: 'Studio', status: 'online', online: true, draining: false, revoked: false, version: '1', code: '', joined_ms: 0, last_seen_ms: 0, address: '', facts: stationFacts(), settings: defaults, leases: [], ...over });
const lease2 = (state: StationLease['state']): StationLease => ({ id: state, station: 'st1', agent: 1, task: null, kind: 'pixel', session: null, started_ms: 0, state, position: null });
const miniView = station({ facts: stationFacts({ gpus: [{ vendor: 'apple', name: 'M4', vram_mb: 0 }] }), leases: [lease2('active'), lease2('opening'), lease2('queued')] });
const mini = stationMachine(miniView);
eq([mini.kind, mini.vendor, mini.agents, mini.id, mini.gpus], ['workstation', 'apple', 2, 'st1', [{ vendor: 'apple', name: 'M4', memory_mib: null }]], 'station machine: kind, apple vendor on macOS, agents holding the station, gpu memory null at 0 VRAM');
const pc = stationMachine(station({ facts: stationFacts({ os: 'linux', form: 'desktop', gpus: [{ vendor: 'nvidia', name: 'RTX 4090', vram_mb: 24576 }] }) }));
eq([pc.vendor, pc.gpus[0]?.memory_mib, pc.agents], [null, 24576, 0], 'station machine: no vendor off macOS, known VRAM kept, no agents without leases');
eq(stationLine(miniView),'Workstation · Mac mini · macOS 15.1 · arm64 · 10 cores · 16 GiB · apple M4', 'station line: a macOS mini, with its GPU');
eq(stationLine(station({ facts: stationFacts({ os: 'windows', os_version: '11', arch: 'x64', form: 'laptop', cpus: 1, ram_mb: 8192, gpus: [{ vendor: 'NVIDIA', name: 'RTX 4090', vram_mb: 24576 }] }) })), 'Workstation · Laptop · Windows 11 · x64 · 1 core · 8 GiB · NVIDIA RTX 4090', 'station line: a laptop, singular core, the GPU vendor kept once');
eq([stationBadge(station()), stationBadge(station({ facts: stationFacts({ account: { mode: 'owner', name: 'studio-owner' } }) }))], [null, 'agents act as studio-owner'], 'station badge: only when the agents act as the owner');
eq(leaseCounts(station({ leases: [lease2('active'), lease2('opening'), lease2('queued'), lease2('queued')] })), { leases: 4, active: 2, queued: 2 }, 'lease counts: all, holding, and queued');
const plain: MachineNode = { id: 'p1', name: 'Rig', kind: 'laptop', arch: 'x86_64', backend: 'kvm', irqchip: '', class: '', cpus: 4, memory_mib: 8192, vsock_mode: '', started_at_ms: 0, gpus: [], stats: null };
eq([isStationNode(mini), isStationNode(plain)], [true, false], 'isStationNode: true for a station row, false for a plain machine');
eq(stationThumbnailPath('a/b c'), '/v1/stations/a%2Fb%20c/thumbnail', 'station thumbnail path: the id is encoded');
// The picture follows the form: the kind says "workstation" for all of them.
eq(
  [
    stationLook({ os: 'windows', form: 'laptop' }),
    stationLook({ os: 'macos', form: 'laptop' }),
    stationLook({ os: 'windows', form: 'mini' }),
    stationLook({ os: 'macos', form: 'mini' }),
    stationLook({ os: 'windows', form: 'desktop' }),
    stationLook({ os: 'linux', form: 'server' }),
    stationLook({ os: 'linux', form: 'vm' }),
  ],
  ['laptop', 'macbook', 'mac', 'mac', 'workstation', 'server', 'server'],
  'station picture: a laptop (a MacBook on macOS), a mini, a tower for a desktop, a rack for a server or a VM',
);
const winLaptop = stationMachine(station({ facts: stationFacts({ os: 'windows', form: 'laptop', gpus: [], ram_mb: 8192 }) }));
const winTower = stationMachine(station({ facts: stationFacts({ os: 'windows', form: 'desktop', gpus: [], ram_mb: 8192 }) }));
const laptopSpec: PictureSpec = { kind: 'workstation', vendor: null, memory_mib: 8192, gpus: [], look: stationLook(winLaptop.station.facts) };
const towerSpec: PictureSpec = { kind: 'workstation', vendor: null, memory_mib: 8192, gpus: [], look: stationLook(winTower.station.facts) };
eq(
  [lookOfSpec(laptopSpec), lookOfSpec(towerSpec), lookOfSpec({ kind: 'workstation', vendor: null, memory_mib: 0, gpus: [] }), lookOfSpec({ kind: 'laptop', vendor: 'apple', memory_mib: 0, gpus: [] })],
  ['laptop', 'workstation', 'workstation', 'macbook'],
  'picture look: a station\'s form wins over its kind; plain machines keep their kind\'s look',
);
eq(specKey(laptopSpec) !== specKey(towerSpec), true, 'picture key: a laptop and a tower of the same spec draw differently');
eq(
  [pictureNote({ online: true, revoked: false }), pictureNote({ online: false, revoked: false }), pictureNote({ online: false, revoked: true }), pictureNote({ online: true, revoked: true })],
  ['No screen open', 'Offline', 'Revoked', 'Revoked'],
  'row picture: why a row has no picture (revoked first)',
);
sectionEnd('station machine tests: ok');

// ---- The Machines row's flat views (views/machine-picture.ts): four labelled views, and the size caption ----
eq(
  VIEWS.map((v) => `${v.view}:${v.label}`),
  ['front:Front', 'side:Side', 'back:Back', 'top:Top'],
  'views strip: the four flat views in order, each labelled',
);
eq(sizeLine({ model: 'server', size_cm: [44, 60, 90] }), '44 × 60 × 90 cm · server', 'views caption: the size in cm and the model');
eq(sizeLine(null), null, 'views caption: omitted when the engine has no facts for the machine');
sectionEnd('machine views tests: ok');

// ---- IDE agent view model (src/ide/view.ts) ----
{
  const ag = (cid: number, name: string, over: Record<string, unknown> = {}): Agent => ({ cid, name, status: 'idle', work: null, ...over }) as unknown as Agent;
  const en = (seq: number, at_ms: number, kind: Entry['kind'], data: Record<string, unknown> = {}, cid = 1, work = 7): Entry => ({ seq, at_ms, cid, work, kind, data });

  eq(rowStatus({ status: 'working' }), 'working', 'rowStatus: working');
  eq(rowStatus({ status: 'offline' }, { state: 'done' }), 'sleeping', 'rowStatus: offline is sleeping whatever the last task');
  eq(rowStatus({ status: 'idle' }, { state: 'done' }), 'done', 'rowStatus: idle after a done task');
  eq(rowStatus({ status: 'idle' }, { state: 'failed' }), 'failed', 'rowStatus: idle after a failed task');
  eq(rowStatus({ status: 'idle' }, null), 'idle', 'rowStatus: idle with no task');

  eq(currentTask(ag(1, 'a', { work: { prompt: '\n  Fix the login bug  \nmore' } })), 'Fix the login bug', 'currentTask: first non-empty line, trimmed');
  eq(currentTask(ag(1, 'a')), null, 'currentTask: null with no work');
  eq(currentTask(ag(1, 'a', { work: { prompt: 'x'.repeat(200) } }))?.length, 120, 'currentTask: cut to 120 characters');

  const chat = buildChat([
    en(1, 0, 'work_start', { kind: 'chat', prompt: 'hello' }),
    en(2, 10, 'model_response', { content: [{ type: 'text', text: 'On it.' }, { type: 'tool_use', id: 'c1', name: 'read_file', input: { path: 'a.rs' } }] }),
    en(3, 20, 'tool_request', { id: 'c1', name: 'read_file', input: { path: 'src/main.rs' } }),
    en(4, 30, 'tool_result', { id: 'c1', is_error: false, content: 'fn main' }),
    en(5, 40, 'tool_request', { id: 'c2', name: 'bash', input: { command: 'cargo test' } }),
    en(6, 50, 'tool_result', { id: 'c2', is_error: true, content: 'boom' }),
    en(7, 60, 'model_response', { content: [{ type: 'text', text: 'Done.' }] }),
  ], true);
  eq(chat.map((c) => c.kind).join(','), 'owner,agent,steps,agent,working', 'buildChat: owner, prose, one steps item, prose, working');
  const stepsItem = chat[2];
  eq(stepsItem.kind === 'steps' ? stepsItem.steps.length : -1, 2, 'buildChat: both tool calls collapse into one steps item');
  eq(stepsItem.kind === 'steps' ? stepsItem.last : '', 'Ran command', 'buildChat: last is the label of the last call');
  eq(stepsItem.kind === 'steps' ? stepsItem.outcome : '', 'mixed', 'buildChat: one ok and one failed is mixed');
  eq(stepsItem.kind === 'steps' ? stepLabel(stepsItem.steps[0]) : '', 'Read src/main.rs', 'stepLabel: read names the path');

  // A code_engine call is one engine block: its updates fill it, and its nested checks go into it, never into a steps run.
  // `engine_update` is not in the Entry kinds yet, so the fixture entries are typed by hand.
  const ev = (seq: number, at_ms: number, kind: string, data: Record<string, unknown>): Entry => ({ seq, at_ms, cid: 1, work: 7, kind, data } as Entry);
  const engineRun = buildChat([
    ev(1, 10, 'tool_request', { call: 'k1', name: 'code_engine', input: { engine: 'claude-code', prompt: 'fix the parser', session: 'sess-12345678' } }),
    ev(2, 11, 'tool_decision', { call: 'k1', decision: 'allow' }),
    ev(3, 12, 'tool_request', { call: 'k1.e1', name: 'engine.read', input: { engine: 'claude-code', title: 'Read src/parser.rs', locations: [], raw: {} } }),
    ev(4, 13, 'tool_decision', { call: 'k1.e1', decision: 'allow' }),
    ev(5, 14, 'engine_update', { call: 'k1', item: 'tool_call', update: { sessionUpdate: 'tool_call', toolCallId: 't1', title: 'Edit parser.rs', kind: 'edit', status: 'pending', locations: [{ path: 'src/parser.rs', line: 40 }], content: [{ type: 'diff', path: 'src/parser.rs', oldText: 'a', newText: 'b' }] } }),
    ev(6, 15, 'engine_update', { call: 'k1', item: 'tool_status', update: { toolCallId: 't1', status: 'completed' } }),
    ev(7, 16, 'engine_update', { call: 'k1', item: 'plan', update: [{ content: 'Find the bug', status: 'completed', priority: 'high' }, { content: 'Fix it', status: 'in_progress', priority: 'medium' }] }),
    ev(8, 17, 'tool_request', { call: 'k1.e2', name: 'engine.exec', input: { engine: 'claude-code', title: 'Run cargo test', locations: [], raw: {} } }),
    ev(9, 18, 'tool_decision', { call: 'k1.e2', decision: 'pending', output: "waiting for the owner's approval (a42)" }),
    ev(10, 19, 'engine_update', { call: 'k1', item: 'message', text: 'Fixed the parser.' }),
    ev(11, 20, 'tool_request', { id: 'c9', name: 'bash', input: { command: 'ls' } }),
    ev(12, 21, 'tool_result', { id: 'c9', is_error: false }),
    ev(13, 22, 'tool_result', { call: 'k1', ok: true, output: 'Fixed in one edit.' }),
  ], false);
  eq(engineRun.map((c) => c.kind).join(','), 'engine,steps', 'buildChat: one engine item, then the plaza\'s own bash call as steps');
  const block = engineRun[0];
  eq(block?.kind === 'engine' ? block.engine : '', 'claude-code', 'buildChat: the engine block names the engine');
  eq(block?.kind === 'engine' ? block.session ?? '' : '', 'sess-12345678', 'buildChat: the engine block keeps the session');
  eq(block?.kind === 'engine' ? block.status : '', 'ok', 'buildChat: a successful result is ok');
  eq(block?.kind === 'engine' ? block.message : '', 'Fixed the parser.', 'buildChat: the engine message is its final text');
  eq(block?.kind === 'engine' ? block.summary ?? '' : '', 'Fixed in one edit.', 'buildChat: the result output is the summary');
  eq(block?.kind === 'engine' ? block.tools.map((t) => `${t.title}:${t.status}:${t.locations[0]?.line}`).join(',') : '', 'Edit parser.rs:completed:40', 'buildChat: a tool row takes its later status update');
  eq(block?.kind === 'engine' ? block.tools[0]?.content[0]?.type ?? '' : '', 'diff', 'buildChat: a tool row keeps its diff content');
  eq(block?.kind === 'engine' ? block.plan.map((p) => `${p.content}:${p.status}`).join(',') : '', 'Find the bug:completed,Fix it:in_progress', 'buildChat: the plan is the latest entries');
  eq(block?.kind === 'engine' ? block.checks.map((c) => `${c.name}:${c.decision}:${c.approval ?? ''}`).join(',') : '', 'engine.read:allow:,engine.exec:pending:a42', 'buildChat: nested checks, the held one with its approval id');
  eq(approvalPath('a7/../x y'), '/v1/approvals/a7%2F..%2Fx%20y', 'approvals: the id is one encoded path segment');
  eq(approvalIdIn("waiting for the owner's approval (a42)") ?? '', 'a42', 'approvals: a hub id is read from the held output');
  eq(approvalIdIn("waiting for the owner's approval (a42 b)") ?? '', '', 'approvals: an id of another shape is not read');
  eq(approvalIdIn("waiting for the owner's approval (appr-42)") ?? '', '', 'approvals: an id that is not the hub shape is not read');
  eq(engineRun[1]?.kind === 'steps' ? engineRun[1].steps.length : -1, 1, 'buildChat: nested checks make no steps, only the bash call does');

  const deniedRun = buildChat([
    ev(1, 10, 'tool_request', { call: 'k2', name: 'code_engine', input: { engine: 'codex', prompt: 'drop the table' } }),
    ev(2, 11, 'tool_decision', { call: 'k2', decision: 'deny', output: 'denied by the owner' }),
  ], false);
  eq(deniedRun.map((c) => (c.kind === 'engine' ? `${c.status}:${c.summary ?? ''}` : c.kind)).join(','), 'denied:denied by the owner', 'buildChat: a denied engine call is a denied engine item');

  const act = activitySplit([
    en(1, 0, 'model_request'),
    en(2, 100, 'model_response'),
    en(3, 110, 'tool_request', { id: 'c1', name: 'bash', input: { command: 'ls' } }),
    en(4, 150, 'tool_result', { id: 'c1', is_error: false }),
    en(5, 160, 'model_request'),
    en(6, 300, 'model_response'),
  ]);
  eq(act.thinkingMs, 240, 'activitySplit: thinking is request to response, summed');
  eq(act.toolMs, 40, 'activitySplit: tool time is request to result');
  eq(act.totalMs, 300, 'activitySplit: total is last minus first');
  eq(act.calls.length === 1 && act.calls[0].ms === 40 && act.calls[0].outcome === 'ok' && act.calls[0].name === 'bash', true, 'activitySplit: one call with name, ms and outcome');
  eq(activitySplit([en(1, 5, 'work_start')]).totalMs, 0, 'activitySplit: no span with one entry');

  const pal: PaletteItem[] = [
    { id: 'a', group: 'command', label: 'Open agent Zed' },
    { id: 'b', group: 'file', label: 'Read main.rs' },
    { id: 'c', group: 'command', label: 'Toggle terminal' },
  ];
  eq(filterPalette(pal, '').map((i) => i.id).join(','), 'a,b,c', 'filterPalette: empty query keeps input order');
  eq(filterPalette(pal, '', 2).length, 2, 'filterPalette: empty query is capped at the limit');
  eq(filterPalette(pal, 'term').map((i) => i.id).join(','), 'c', 'filterPalette: only matching items remain');
  const rank: PaletteItem[] = [
    { id: 'x', group: 'file', label: 'xa' },
    { id: 'y', group: 'file', label: 'ax' },
  ];
  eq(filterPalette(rank, 'a').map((i) => i.id).join(','), 'y,x', 'filterPalette: earlier match ranks first');
  eq(matchScore('Open agent', ''), 0, 'matchScore: empty query scores 0');
  eq(matchScore('abc', 'z'), null, 'matchScore: no match is null');
  const sub = matchScore('Open agent', 'oa');
  eq(sub !== null && sub > 0 && sub < 100, true, 'matchScore: a subsequence scores below any substring');
  eq(matchScore('Open agent', 'AGENT') !== null, true, 'matchScore: case-insensitive');

  eq(formatMs(340), '340 ms', 'formatMs: milliseconds');
  eq(formatMs(1200), '1.2 s', 'formatMs: seconds');
  eq(formatMs(125000), '2 min 5 s', 'formatMs: minutes');

  eq(mentionsOf('ping @all and @ana, then @ana again and @dev-2_x.').join(','), 'all,ana,dev-2_x', 'mentionsOf: names in order, no duplicates, trailing dot dropped');
  eq(mentionsOf('mail a@b.com now').length, 0, 'mentionsOf: an address is not a mention');

  const hues = ['zed', 'amy', '', 'Long Name Here'].map(authorHue);
  eq(hues.every((h) => Number.isInteger(h) && h >= 0 && h <= 5), true, 'authorHue: integers 0..5');
  eq(authorHue('amy') === authorHue('amy'), true, 'authorHue: stable for one name');
}
sectionEnd('ide agent view model tests: ok');

// ---- Rich content: the classifier against the shared vectors ----------------------------------------------------
// crates/apz-rich/tests/vectors.json is the file the Rust crate tests too (tests/vectors.rs), so the two classifiers
// cannot drift apart. The file's keys are sorted and the port's are not, so both sides are compared with sorted keys.
{
  const canon = (value: unknown): string =>
    JSON.stringify(value, (_key, v: unknown) =>
      v && typeof v === 'object' && !Array.isArray(v)
        ? Object.fromEntries(Object.entries(v as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
        : v,
    );
  const rawOf = (span: Span): string => (span.kind === 'ref' ? span.value.raw : span.value);
  const cases = richVectors as unknown as { name: string; text: string; ctx: RichCtx; spans: Span[] }[];
  eq(cases.length >= 60, true, `rich: the shared vectors have at least 60 cases (${cases.length})`);
  for (const c of cases) {
    const got = classify(c.text, c.ctx);
    if (canon(got) !== canon(c.spans)) {
      failures++;
      console.error(`FAIL rich vector ${c.name}\n  got      ${canon(got)}\n  expected ${canon(c.spans)}`);
    }
    eq(got.map(rawOf).join(''), c.text, `rich vector ${c.name}: the spans join back to the text`);
  }

  // The chip labels (the Rust crate's label tests).
  const repo = { owner: 'o', repo: 'r' };
  eq(label({ kind: 'github_pull', number: 12, ...repo }), 'o/r #12', 'rich label: pull');
  eq(label({ kind: 'github_commit', sha: 'abc1234def5678', ...repo }), 'o/r @abc1234', 'rich label: commit is seven characters');
  eq(label({ kind: 'github_file', ref: 'main', path: 'src/a.rs', line: 10, dir: false, ...repo }), 'r: src/a.rs:10', 'rich label: file with a line');
  eq(label({ kind: 'github_file', ref: 'main', path: 'crates', line: null, dir: true, ...repo }), 'r: crates/', 'rich label: folder gets its slash');
  eq(label({ kind: 'task', number: 528 }), '#528', 'rich label: task');
  eq(label({ kind: 'agent', name: 'Name', cid: null }), '@Name', 'rich label: agent by name');
  eq(label({ kind: 'agent', name: null, cid: 7 }), 'agent:7', 'rich label: agent by cid');
  eq(label({ kind: 'path', path: 'src/main.rs', line: 42, dir: false }), 'src/main.rs:42', 'rich label: path with a line');
  eq(label({ kind: 'image', url: 'https://x.io/img/a.png?v=2' }), 'a.png', 'rich label: image is its file name');
  eq(label({ kind: 'link', url: 'https://example.com/docs/x/?q=1#f' }), 'example.com/docs/x', 'rich label: link is domain and path');
  eq(label({ kind: 'library_tool', name: 'search', version: 3 }), 'search v3', 'rich label: library tool');
}
sectionEnd('rich content tests: ok');

// ---- Rich content chips and cards: the tone a state takes, an author's avatar and its monogram (src/ui/embed-model.ts) ----
{
  // A chip's border and icon take the tone of its state: open is success, a draft secondary, merged purple, closed an error.
  eq(toneOf('change', 'open'), 'ok', 'chip tone: an open pull request is success');
  eq(toneOf('change', 'draft'), 'neutral', 'chip tone: a draft is secondary');
  eq(toneOf('change', 'merged'), 'merged', 'chip tone: a merged pull request is purple');
  eq(toneOf('change', 'closed'), 'danger', 'chip tone: a closed pull request is an error');
  eq(toneOf('task', 'open'), 'neutral', 'chip tone: an open task is neutral');
  eq(toneOf('task', 'claimed'), 'info', 'chip tone: a claimed task is in progress');
  eq(toneOf('task', 'awaiting_review'), 'warn', 'chip tone: a task awaiting review is a warning');
  eq(toneOf('task', 'done'), 'ok', 'chip tone: a done task is success');
  eq(toneOf('task', 'failed'), 'danger', 'chip tone: a failed task is an error');
  eq(toneOf('agent', 'idle'), 'ok', 'chip tone: an idle agent is a green status dot');
  eq(toneOf('agent', 'working'), 'info', 'chip tone: a working agent is blue, not the owner orange');
  eq(toneOf('agent', 'offline'), 'neutral', 'chip tone: an offline agent is neutral');
  eq(toneOf('run', 'success'), 'ok', 'chip tone: a successful run');
  eq(toneOf('run', 'failure'), 'danger', 'chip tone: a failed run');
  eq(toneOf('run', 'in_progress'), 'info', 'chip tone: a run in progress');
  eq(toneOf('change', 'weird'), 'neutral', 'chip tone: an unknown state is neutral');
  eq(toneOf('change', 'constructor'), 'neutral', 'chip tone: a prototype name is not a state');

  // An author: a user object with an https avatar, a user object without one, or a plain name.
  eq(personOf({ login: 'ada', avatar_url: 'https://avatars.example/ada.png' }), { name: 'ada', avatar: 'https://avatars.example/ada.png' }, 'card author: a picture over https');
  eq(personOf({ login: 'ada', avatar_url: 'http://avatars.example/ada.png' }), { name: 'ada', avatar: null }, 'card author: an http picture is not shown');
  eq(personOf({ login: 'ada' }), { name: 'ada', avatar: null }, 'card author: no picture falls back to the monogram');
  eq(personOf({ name: 'Grace' }), { name: 'Grace', avatar: null }, 'card author: a name without a login');
  eq(personOf('Bob'), { name: 'Bob', avatar: null }, 'card author: a plain name');
  eq(personOf({ login: '' }), null, 'card author: an empty login is no author');
  eq(personOf(null), null, 'card author: null is no author');

  // The monogram: the first letter or digit of the name, upper case; a question mark when there is none.
  eq(monogramOf('zarathos'), 'Z', 'avatar monogram: the first letter, upper case');
  eq(monogramOf('@ada'), 'A', 'avatar monogram: a leading mark is skipped');
  eq(monogramOf('9lives'), '9', 'avatar monogram: a digit counts');
  eq(monogramOf('ünal'), 'Ü', 'avatar monogram: a letter outside ASCII');
  eq(monogramOf(''), '?', 'avatar monogram: an empty name');
  eq(monogramOf('  '), '?', 'avatar monogram: a blank name');
}
sectionEnd('rich card tests: ok');

// ---- IDE grouping by project, duplicate names and the start choice (src/ide/groups.ts) ----
{
  const ag = (cid: number, name: string, over: Record<string, unknown> = {}): Agent => ({ cid, name, status: 'idle', work: null, ...over }) as unknown as Agent;
  const pj = (id: string, title: string, lead_cid: number, tasks: string[] = [], updated_ms = 0): Project => ({ id, title, lead_cid, tasks, updated_ms, repo: null }) as unknown as Project;
  const tk = (id: string, agent_cid: number | null): Task => ({ id, agent_cid, state: 'open' }) as unknown as Task;
  const names = (g: { agents: Agent[] }): string => g.agents.map((a) => a.name).join(',');
  const work = (task: string) => ({ prompt: 'x', origin: { task, motion: null, from_cid: null } });

  // Alpha: led by zed (1), holds t1 which amy (2) claimed. Beta: led by amy (2), holds t2 which cy (4) is working on.
  // Gamma: no agents. t3 is held by dee (5), but no project holds t3.
  const projects = [pj('p1', 'Alpha', 1, ['t1'], 100), pj('p2', 'Beta', 2, ['t2'], 300), pj('p3', 'Gamma', 9, [], 900)];
  const tasks = [tk('t1', 2), tk('t2', null), tk('t3', 5)];
  const agents = [
    ag(1, 'zed'),
    ag(2, 'amy'),
    ag(3, 'bob'),
    ag(4, 'cy', { status: 'working', work: work('t2') }),
    ag(5, 'dee', { work: work('nope') }),
  ];
  const groups = groupByProject(agents, projects, tasks);
  eq(groups.map((g) => g.title).join(','), 'Beta,Alpha,No project', 'groupByProject: most recent project first, empty projects skipped, No project last');
  eq(names(groups[0]), 'amy,cy', 'groupByProject: the lead and a work-origin assignee are in the project');
  eq(names(groups[1]), 'amy,zed', 'groupByProject: an agent in two projects is listed in both (amy: lead of Beta, claimed t1 in Alpha)');
  eq(groups[2].id, null, 'groupByProject: No project has a null id');
  eq(names(groups[2]), 'bob,dee', 'groupByProject: agents in no project are sorted by name');
  eq(groupByProject([ag(1, 'zed')], [pj('p1', 'Alpha', 1)], []).map((g) => g.title).join(','), 'Alpha', 'groupByProject: no No project group when every agent has a project');
  eq(groupByProject([ag(2, 'amy')], [], []).map((g) => g.title).join(','), 'No project', 'groupByProject: with no projects every agent is in No project');
  eq(groupByProject([ag(2, 'amy')], [pj('p1', 'Alpha', 1, [], 5)], [tk('t9', 2)])[0]?.title, 'No project', 'groupByProject: a claimed task outside every project does not place the agent');
  eq(projectOf(agents[1], projects, tasks)?.id, 'p2', 'projectOf: the most recently active project of the agent');
  eq(projectOf(agents[2], projects, tasks), null, 'projectOf: null for an agent in no project');

  eq([...sharedNames([ag(1, 'Ada'), ag(2, 'ada'), ag(3, 'Bo')])].sort().join(','), '1,2', 'sharedNames: names that collide, case-insensitively');
  eq(sharedNames([ag(1, 'Ada'), ag(2, 'Bo')]).size, 0, 'sharedNames: no collision, no cid shown');

  // The sidebar order is Beta (amy, cy), Alpha (amy, zed), No project (bob, dee); the unique order is 2, 4, 1, 3, 5.
  eq(startAgent(groups, 1), 1, 'startAgent: the agent selected last time, while it is listed');
  eq(startAgent(groups, 99), 4, 'startAgent: a gone agent falls back to the first working one');
  eq(startAgent(groups, null), 4, 'startAgent: with nothing stored, the first working agent');
  const idle = groupByProject([ag(2, 'amy'), ag(1, 'zed')], [], []);
  eq(startAgent(idle, null), 2, 'startAgent: no working agent, the first agent in the sidebar');
  eq(startAgent([], null), null, 'startAgent: null when there are no agents');

  // The sidebar's groups with the workstations: Alpha (its repo, and the agent it leads), Repositories (a repo no project holds),
  // Workstations (two, one with no name), No project (bob). A workstation is never filed under a project.
  const repoAlpha = { ...pj('p1', 'Alpha', 1, ['t1'], 100), repo: 'alpha' } as Project;
  const wsOf = (id: string, kind: IdeWorkspaceInfo['kind'], name: string, extra: Partial<IdeWorkspaceInfo> = {}): IdeWorkspaceInfo =>
    ({ id, kind, owner: id.slice(id.indexOf(':') + 1).split('@')[0], name, writable: true, status: kind === 'station' ? 'online' : 'idle', ...extra }) as IdeWorkspaceInfo;
  const wsList: IdeWorkspaceInfo[] = [
    wsOf('repo:alpha@main', 'repo', 'alpha'),
    wsOf('repo:orphan@main', 'repo', 'orphan'),
    wsOf('station:w3', 'station', 'studio', { os: 'macos' }),
    wsOf('station:w4', 'station', '', { os: 'linux' }),
    wsOf('agent:1', 'agent', 'zed'),
  ];
  const sg = sidebarGroups([ag(1, 'zed'), ag(3, 'bob')], [repoAlpha], [tk('t1', null)], wsList);
  eq(sg.map((g) => g.title), ['Alpha', 'Repositories', 'Workstations', 'No project'], 'sidebar groups: projects, then repositories no project holds, then workstations, then No project last');
  const wsGroup = sg.find((g) => g.gid === 'workstations');
  eq([wsGroup?.count, wsGroup?.stations.map((w) => w.id).join(',')], [2, 'station:w3,station:w4'], 'sidebar groups: the workstations of the list, in the list order');
  eq(sg.find((g) => g.title === 'Alpha')?.repos.map((w) => w.id), ['repo:alpha@main'], 'sidebar groups: a repo sits under the project that names it');
  eq(sidebarGroups([ag(1, 'zed')], [], [], wsList.filter((w) => w.kind !== 'station')).map((g) => g.title), ['Repositories', 'No project'], 'sidebar groups: no workstations, no Workstations group');
  eq(sidebarGroups([ag(3, 'bob')], [], [], [wsList[2]!]).map((g) => g.title), ['Workstations', 'No project'], 'sidebar groups: with no project, workstations sit above No project');
  eq(matchingRows(wsGroup!, '')?.stations.length, 2, 'sidebar search: an empty query keeps every workstation');
  eq(matchingRows(wsGroup!, 'stud')?.stations.map((w) => w.id), ['station:w3'], 'sidebar search: a workstation matches by its name');
  eq(matchingRows(wsGroup!, 'workstations')?.stations.length, 2, 'sidebar search: a query that names the group keeps its workstations');
  eq(matchingRows(wsGroup!, 'zzz'), null, 'sidebar search: no match is no row');
}
sectionEnd('ide grouping tests: ok');

// The stored terminals of a workspace: one per slot with a session, counted from the terminal module's storage.
{
  const store = new Map<string, string>();
  const kv = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v), removeItem: (k: string) => void store.delete(k) };
  eq(storedTerminalCount(kv, 'agent:77'), 0, 'storedTerminalCount: none for a workspace with no sessions');
  writeSession(kv, sessionKey('agent:77', '0'), { term: 't-a', offset: 0 });
  writeSession(kv, sessionKey('agent:77', '2'), { term: 't-b', offset: 5 });
  writeSession(kv, sessionKey('agent:78', '1'), { term: 't-c', offset: 0 });
  eq(storedTerminalCount(kv, 'agent:77'), 2, 'storedTerminalCount: one per stored slot of that workspace');
  clearSession(kv, sessionKey('agent:77', '0'));
  eq(storedTerminalCount(kv, 'agent:77'), 1, 'storedTerminalCount: a cleared session is no longer counted');
  eq(storedTerminalCount(undefined, 'agent:77'), 0, 'storedTerminalCount: no storage, no terminals');
}
sectionEnd('ide stored terminals tests: ok');

{
  // The owner's board composer: the @-token before the caret, and what a mention can name.
  eq(mentionAt('ship it @bu', 11), { start: 8, query: 'bu' }, 'mentionAt: the partial name before the caret');
  eq(mentionAt('ping @all', 9), { start: 5, query: 'all' }, 'mentionAt: @all is a partial too');
  eq(mentionAt('mail a@b', 8), null, 'mentionAt: an address is not a mention');
  eq(mentionAt('done. @', 7), { start: 6, query: '' }, 'mentionAt: a bare @ is a mention to finish');
  eq(mentionAt('hi @ana later', 9), null, 'mentionAt: a word after a space ends the mention');
  const agent = (cid: number, name: string, role: string) => ({ cid, name, role }) as unknown as Agent;
  const names = mentionOptions([agent(3, 'Ana', 'builder'), agent(4, 'builder', 'builder'), agent(5, 'Dev', 'reviewer')]).map((o) => o.name);
  eq(names, ['all', 'Ana', 'builder', 'Dev', 'reviewer'], 'mentionOptions: all, then names and roles, no repeats (case-insensitive)');
}
sectionEnd('board owner composer tests: ok');

{
  // The Files tree repaints only when its rows would change: the same inputs in any set order are the same paint, and each
  // input that shows in a row (the workspace, a loaded or open folder, an entry's kind or binary flag, a changed mark, the
  // filter's kept paths, the active file) makes it a new one.
  const entry = (path: string, kind: 'file' | 'dir' = 'file', binary = false): TreeEntry => ({ path, kind, size: 0, hash: null, binary });
  const base: TreeKeyInput = {
    workspace: 'agent:1',
    all: [entry('src', 'dir'), entry('src/a.ts'), entry('logo.png', 'file', true)],
    loaded: new Set(['', 'src']),
    expanded: new Set(['src']),
    changed: new Set(['src/a.ts']),
    keep: null,
    active: 'src/a.ts',
    hasWorkspace: true,
  };
  const same = treeKey({ ...base, loaded: new Set(['src', '']), expanded: new Set(['src']), changed: new Set(['src/a.ts']) });
  eq(treeKey(base) === same, true, 'treeKey: the same inputs (sets in another order) give the same key');
  const differs = (over: Partial<TreeKeyInput>, name: string) => eq(treeKey({ ...base, ...over }) === treeKey(base), false, `treeKey: ${name}`);
  differs({ workspace: 'agent:2' }, 'a new workspace is a new paint');
  differs({ hasWorkspace: false }, 'no workspace is a new paint');
  differs({ loaded: new Set(['']) }, 'a folder that finishes loading is a new paint');
  differs({ expanded: new Set() }, 'a folder that is closed is a new paint');
  differs({ all: [entry('src', 'dir'), entry('src/a.ts'), entry('logo.png')] }, 'an entry that is no longer binary is a new paint');
  differs({ all: [...base.all, entry('src/b.ts')] }, 'a new entry is a new paint');
  differs({ changed: new Set() }, 'a changed mark that clears is a new paint');
  differs({ keep: new Set(['src/a.ts']) }, 'turning the changed-only filter on is a new paint');
  differs({ active: null }, 'the active file changing is a new paint');
  eq(treeKey({ ...base, keep: new Set(['src/a.ts']) }) === treeKey({ ...base, keep: new Set(['src/a.ts']) }), true, 'treeKey: the same filter keeps the same key');
  const cps = [{ n: 1, label: 'before refactor', files: 3 }];
  eq(checkpointsKey('agent:1', true, cps, 0) === checkpointsKey('agent:1', true, [{ ...cps[0] }], 0), true, 'checkpointsKey: the same checkpoints give the same key');
  eq(checkpointsKey('agent:1', true, cps, 0) === checkpointsKey('agent:1', true, [{ ...cps[0], files: 4 }], 0), false, 'checkpointsKey: a checkpoint whose file count changes is a new paint');
  eq(checkpointsKey('agent:1', true, cps, 0) === checkpointsKey('agent:1', true, [...cps, { n: 2, label: 'x', files: 0 }], 0), false, 'checkpointsKey: a new checkpoint is a new paint');
  eq(checkpointsKey('agent:1', false, cps, 0) === checkpointsKey(null, true, cps, 0), true, 'checkpointsKey: no panel (no diff, or no workspace) is the same paint');
  eq(checkpointsKey('agent:1', true, cps, 0) === checkpointsKey('agent:1', true, cps, 61_000), false, 'checkpointsKey: a new minute repaints, so relative times move on');
}
sectionEnd('ide files paint tests: ok');

{
  // Sleep, VM checkpoints and forks: the requests the console sends, against a recorded fetch (the paths are escaped
  // as the hub reads them, and a label or a fork count goes in the body only when it is given).
  const sent: { method: string; url: string; body: unknown }[] = [];
  const realFetch = globalThis.fetch;
  let reply: unknown = {};
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    sent.push({ method: init?.method ?? 'GET', url, body: typeof init?.body === 'string' ? JSON.parse(init.body) : null });
    return new Response(JSON.stringify(reply), { status: 200, headers: { 'content-type': 'application/json' } });
  }) as typeof fetch;
  try {
    await api.sleepAgent(7);
    await api.wakeAgent(7);
    reply = { checkpoints: [] };
    await api.vmCheckpoints(7);
    await api.takeVmCheckpoint(7, 'before deploy');
    await api.takeVmCheckpoint(7);
    await api.restoreVmCheckpoint(7, 'ck 1/x');
    await api.deleteVmCheckpoint(7, 'ck-1');
    await api.forkAgent(7, 3);
  } finally {
    globalThis.fetch = realFetch;
  }
  eq(
    sent,
    [
      { method: 'POST', url: '/v1/agents/7/sleep', body: {} },
      { method: 'POST', url: '/v1/agents/7/wake', body: {} },
      { method: 'GET', url: '/v1/agents/7/vm-checkpoints', body: null },
      { method: 'POST', url: '/v1/agents/7/vm-checkpoints', body: { label: 'before deploy' } },
      { method: 'POST', url: '/v1/agents/7/vm-checkpoints', body: {} },
      { method: 'POST', url: '/v1/agents/7/vm-checkpoints/ck%201%2Fx/restore', body: {} },
      { method: 'DELETE', url: '/v1/agents/7/vm-checkpoints/ck-1', body: null },
      { method: 'POST', url: '/v1/agents/7/fork', body: { n: 3 } },
    ],
    'vm api: each checkpoint, sleep and fork call goes to its endpoint with its body',
  );
  // The roster's status pill: asleep reads as its own word, in the muted tone the offline agents use.
  eq(statusPill('asleep'), { tone: 'muted', word: 'Asleep' }, 'status pill: asleep is muted and says so');
  eq(statusPill('offline'), { tone: 'muted', word: null }, 'status pill: offline needs no word');
  eq(statusPill('dozing'), { tone: 'muted', word: 'Asleep (light)' }, 'status pill: a dozing agent is asleep, light');
  eq([statusPill('working').tone, statusPill('idle').tone], ['info', 'ok'], 'status pill: working and idle keep their tones');
  // The sleep settings' validation, as the policy form reads its fields: whole numbers in range, blanks refused.
  eq([wholeInRange('0', 0, 1440), wholeInRange('1440', 0, 1440), wholeInRange(' 30 ', 0, 1440)], [0, 1440, 30], 'wholeInRange: the bounds and a padded whole number are accepted');
  eq([wholeInRange('1441', 0, 1440), wholeInRange('-1', 0, 1440), wholeInRange('2.5', 0, 1440), wholeInRange('', 0, 1440), wholeInRange('   ', 0, 1440), wholeInRange('x', 0, 1440)], [null, null, null, null, null, null], 'wholeInRange: out of range, negative, fractional, blank or text is refused');
  eq(sleepRoles('coder, reviewer,, coder ,'), ['coder', 'reviewer'], 'sleepRoles: trimmed, blanks and repeats dropped, order kept');
  eq(sleepRoles('   '), [], 'sleepRoles: a blank field names no role');
  eq([volumeNameOk('deps'), volumeNameOk('a'.repeat(20)), volumeNameOk('a'.repeat(21)), volumeNameOk('Deps'), volumeNameOk('-deps'), volumeNameOk('')], [true, true, false, false, false, false], 'volumes: a name is a-z, 0-9 and dashes, 1 to 20 characters, not starting with a dash');
  eq(parseVolumeList('deps, models  ,, ci-2').names, ['deps', 'models', 'ci-2'], 'volumes: a role list splits on commas and spaces, blanks dropped, order kept');
  eq(parseVolumeList('   ').names, [], 'volumes: a blank role list names no volume');
  eq([parseVolumeList('deps, Data').error === undefined, parseVolumeList('a,a').error === undefined, parseVolumeList('a,b,c,d,e').error === undefined], [false, false, false], 'volumes: a bad name, a repeat, or a fifth volume is refused');
}
sectionEnd('sleep and checkpoint tests: ok');

{
  // Egress credentials and the E2B API: the requests the owner's screens send (ids escaped, bodies as the hub reads them).
  const sent: { method: string; url: string; body: unknown }[] = [];
  const realFetch = globalThis.fetch;
  let reply: unknown = {};
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    sent.push({ method: init?.method ?? 'GET', url, body: typeof init?.body === 'string' ? JSON.parse(init.body) : null });
    return new Response(JSON.stringify(reply), { status: 200, headers: { 'content-type': 'application/json' } });
  }) as typeof fetch;
  try {
    reply = { rules: [] };
    await egressApi.rules();
    await egressApi.addRule({ roles: [], host: 'a.example', port: 443, methods: [], path_prefix: '/', header: 'Authorization', format: 'Bearer ${SECRET}', secret: 'tok', enabled: true });
    await egressApi.patchRule('r 1/x', { enabled: false });
    await egressApi.deleteRule('r1');
    reply = { keys: [] };
    await e2bApi.keys();
    await e2bApi.createKey('ci');
    await e2bApi.revokeKey('k1');
    reply = { sandboxes: [] };
    await e2bApi.sandboxes();
    await e2bApi.pauseSandbox('sb1');
    await e2bApi.resumeSandbox('sb1');
    await e2bApi.endSandbox('sb1');
    reply = { ok: true, from: 'n1', to: 'n2', bytes: 1, ms: 1 };
    await api.moveAgent(7, 'n2');
    reply = [];
    await api.nodes();
  } finally {
    globalThis.fetch = realFetch;
  }
  eq(
    sent,
    [
      { method: 'GET', url: '/v1/egress/inject', body: null },
      { method: 'POST', url: '/v1/egress/inject', body: { roles: [], host: 'a.example', port: 443, methods: [], path_prefix: '/', header: 'Authorization', format: 'Bearer ${SECRET}', secret: 'tok', enabled: true } },
      { method: 'PATCH', url: '/v1/egress/inject/r%201%2Fx', body: { enabled: false } },
      { method: 'DELETE', url: '/v1/egress/inject/r1', body: null },
      { method: 'GET', url: '/v1/e2b/keys', body: null },
      { method: 'POST', url: '/v1/e2b/keys', body: { label: 'ci' } },
      { method: 'DELETE', url: '/v1/e2b/keys/k1', body: null },
      { method: 'GET', url: '/v1/e2b/sandboxes', body: null },
      { method: 'POST', url: '/v1/e2b/sandboxes/sb1/pause', body: {} },
      { method: 'POST', url: '/v1/e2b/sandboxes/sb1/resume', body: {} },
      { method: 'DELETE', url: '/v1/e2b/sandboxes/sb1', body: null },
      { method: 'POST', url: '/v1/agents/7/move', body: { node: 'n2' } },
      { method: 'GET', url: '/v1/nodes', body: null },
    ],
    'egress and e2b api: each call goes to its endpoint with its body',
  );
  // The rule form: the defaults, the normalised body, and each refusal the owner sees before a request.
  const good: RuleForm = { ...defaultRuleForm(), host: ' API.Example.com ', path_prefix: '/v1', methods: 'get, post, get', roles: 'coder, reviewer,, coder', secret: 'github_token' };
  eq(defaultRuleForm(), { host: '', port: '443', methods: '', path_prefix: '/', roles: '', header: 'Authorization', format: 'Bearer ${SECRET}', secret: '', enabled: true }, 'egress form: the defaults are port 443, path /, Authorization and Bearer ${SECRET}');
  eq(ruleProblem(good), null, 'egress form: a complete rule is accepted');
  eq(
    ruleBody(good),
    { roles: ['coder', 'reviewer'], host: 'api.example.com', port: 443, methods: ['GET', 'POST'], path_prefix: '/v1', header: 'Authorization', format: 'Bearer ${SECRET}', secret: 'github_token', enabled: true },
    'egress form: the body has the host lower-cased, the lists split and de-duplicated, and the methods upper-cased',
  );
  const refused = (over: Partial<RuleForm>, start: string): boolean => ruleProblem({ ...good, ...over })?.startsWith(start) === true;
  eq(refused({ port: '80' }, 'Port 80 is refused'), true, 'egress form: port 80 is refused client-side too');
  eq([refused({ port: '0' }, 'The port'), refused({ port: '65536' }, 'The port'), refused({ port: '44.3' }, 'The port'), refused({ port: 'https' }, 'The port')], [true, true, true, true], 'egress form: a port that is not a whole number from 1 to 65535 is refused');
  eq(ruleProblem({ ...good, port: '8443' }), null, 'egress form: another TLS port is accepted');
  eq([refused({ header: 'Bad Header' }, 'The header is a name'), refused({ header: 'a:b' }, 'The header is a name'), refused({ header: '' }, 'The header is a name')], [true, true, true], 'egress form: a header with a space, a colon or nothing is refused');
  eq(ruleProblem({ ...good, header: 'X-Api-Key' }), null, 'egress form: a token header name is accepted');
  eq([refused({ format: 'Bearer token' }, 'The format must contain'), refused({ format: '' }, 'The format must contain')], [true, true], 'egress form: a format without ${SECRET} is refused');
  eq(ruleProblem({ ...good, format: 'Token ${SECRET}' }), null, 'egress form: a format with ${SECRET} is accepted');
  eq([refused({ host: '*' }, 'The host is a name'), refused({ host: '*.' }, 'The host is a name'), refused({ host: 'a..b' }, 'The host is a name'), refused({ host: 'exa mple.com' }, 'The host is a name'), refused({ host: '' }, 'Enter the host')], [true, true, true, true, true], 'egress form: a host that is not exact or *.suffix is refused');
  eq(ruleProblem({ ...good, host: '*.example.com' }), null, 'egress form: a *.suffix host is accepted');
  eq(refused({ path_prefix: 'v1' }, 'The path prefix'), true, 'egress form: a path prefix without a slash is refused');
  eq(refused({ methods: 'GET POST' }, 'Methods are names'), true, 'egress form: methods must be comma-separated names');
  eq(refused({ secret: '  ' }, 'Enter the name of the vault secret'), true, 'egress form: a blank secret name is refused');
  eq([methodsText([]), methodsText(['GET']), rolesText([]), rolesText(['coder'])], ['Any', 'GET', 'Every role', 'coder'], 'egress table: an empty method or role list reads as any or every');
  eq(listOf(' a, b ,a,, '), ['a', 'b'], 'listOf: trimmed, blanks and repeats dropped, order kept');
  // The E2B screens: the setup text, the buttons a sandbox row offers, the memory and time text.
  eq(e2bSetupText('https://hub.example', 'e2b_key'), 'E2B_API_URL=https://hub.example/e2b\nE2B_API_KEY=e2b_key\nE2B_SANDBOX_URL=http://127.0.0.1:49982', 'e2b setup: the API under the origin, the key, and the local gateway');
  eq(e2bSetupText('https://hub.example', null).split('\n')[1], 'E2B_API_KEY=<your key>', 'e2b setup: without a key shown, the placeholder asks for one');
  eq([sandboxActions('running'), sandboxActions('paused')], [['pause', 'end'], ['resume', 'end']], 'e2b sandboxes: a running one pauses, a paused one resumes, and either ends');
  eq([memoryText(512), memoryText(1536), memoryText(2048)], ['512 MB', '1.5 GB', '2.0 GB'], 'e2b sandboxes: memory reads in MB below a gibibyte, then GB');
  eq(sandboxTime('not a time'), '—', 'e2b sandboxes: an unreadable time shows a dash');
  // The templates: the newest build, the tags of the ready builds, and the failure of a newest build that failed.
  const buildSample = (over: Partial<E2bBuild>): E2bBuild => ({ buildID: 'b', status: 'ready', createdAt: '2026-10-01T00:00:00Z', finishedAt: null, cpuCount: 2, memoryMB: 512, tags: [], reason: null, ...over });
  const tplSample: E2bTemplate = {
    templateID: 't1',
    name: 'base',
    createdAt: '2026-10-01T00:00:00Z',
    updatedAt: '2026-10-02T00:00:00Z',
    builds: [
      buildSample({ buildID: 'b3', status: 'error', tags: ['v3'], reason: { message: 'step 4 failed', step: 'RUN npm ci' } }),
      buildSample({ buildID: 'b2', tags: ['v2', 'latest'] }),
      buildSample({ buildID: 'b1', tags: ['latest', 'v1'] }),
    ],
  };
  eq(latestBuild(tplSample)?.buildID, 'b3', 'e2b templates: the newest build is the first one listed');
  eq(templateTags(tplSample), ['v2', 'latest', 'v1'], 'e2b templates: the tags of the ready builds, each once, in listed order');
  eq(templateFailure(tplSample), 'step 4 failed', 'e2b templates: a newest build that failed shows its reason');
  eq(templateFailure({ ...tplSample, builds: [buildSample({ status: 'error' })] }), 'The build failed.', 'e2b templates: a failed build without a reason still says it failed');
  eq([templateFailure({ ...tplSample, builds: [buildSample({ status: 'ready' })] }), latestBuild({ ...tplSample, builds: [] }), templateTags({ ...tplSample, builds: [] })], [null, null, []], 'e2b templates: a ready newest build has no failure, and no builds means no latest build and no tags');
  // Move: the nodes an agent can move to, and the outcome line.
  const nodes = [
    { id: 'n1', name: 'alpha', draining: false, revoked: false },
    { id: 'n2', name: 'beta', draining: true, revoked: false },
    { id: 'n3', name: 'gamma', draining: false, revoked: true },
  ] as unknown as NodeView[];
  eq(movableNodes(nodes).map((n) => n.id), ['n1'], 'move: a draining or revoked node is not offered');
  eq(moveText({ from: 'n1', to: 'n2', bytes: 3 * 1048576, ms: 2500 }, (id) => id.toUpperCase()), 'Moved from N1 to N2: 3.0 MB in 2.5 s.', 'move: the outcome names both nodes, the MB moved and the seconds');
}
sectionEnd('egress credentials and e2b tests: ok');

// Metrics formatting: every case of the golden file the Rust formatter is checked against, and the dash for a value
// that is not there.
{
  for (const [unit, v, text] of formatGolden.format as [Unit, number, string][]) {
    eq(format(unit, v), text, `metrics format: ${unit} ${v} prints ${text}`);
  }
  for (const [max, expected] of formatGolden.nice as [number, number][]) {
    eq(nice(max), expected, `metrics nice: ${max} gives ${expected}`);
  }
  for (const [t, step, offset, text] of formatGolden.time as [number, number, number, string][]) {
    eq(timeLabel(t, step, offset), text, `metrics time label: ${t} at step ${step} and offset ${offset} reads ${text}`);
  }
  eq(format('usd', Infinity), '—', 'metrics format: an infinite value is a dash');
  eq(format('usd', NaN), '—', 'metrics format: NaN is a dash');
  eq(format('usd', null), '—', 'metrics format: no value is a dash');
  eq(format('usd', undefined), '—', 'metrics format: an undefined value is a dash');
}
sectionEnd('metrics format, nice and time labels: ok');
// The Metrics treemap's split (figures.ts): every item keeps a cell, the cells tile the box, and each cell is its share.
{
  const items: TreeItem[] = [
    { name: 'a', value: 50, tone: 'chart-1' },
    { name: 'b', value: 30, tone: 'chart-2' },
    { name: 'c', value: 15, tone: 'chart-3' },
    { name: 'd', value: 5, tone: 'chart-4' },
    { name: 'e', value: 0, tone: 'chart-5' },
  ];
  const cells = splitTreemap(items);
  eq(cells.map((c) => c.item.name).sort(), ['a', 'b', 'c', 'd', 'e'], 'treemap: every item keeps a cell, zero items included');
  const area = cells.reduce((s, c) => s + c.w * c.h, 0);
  eq(Math.round(area) / 100, 100, 'treemap: the cells tile the whole box (areas sum to 100 %)');
  const share = (name: string): number => {
    const cell = cells.find((c) => c.item.name === name);
    return Math.round(((cell?.w ?? 0) * (cell?.h ?? 0)) / 100);
  };
  eq([share('a'), share('b'), share('c'), share('d')], [50, 30, 15, 5], 'treemap: each cell is its value share of the box');
  sectionEnd('treemap split tests: ok');
}
// ---- Metrics charts: the thinning of long series and the x-axis label placement (src/metrics/charts.ts) ----
eq([thin([1, 5, 2, null, 3, 4], bucketStarts(6, 3)), thin([null, null, 1], bucketStarts(3, 2)), thin([3, 1, 2], [0], 'min')], [[5, 2, 4], [null, 1], [1]], 'metrics charts: a long series keeps the largest value of each pixel bucket, and a bucket with no value stays null');
eq([xLabelIndexes(10, 400), xLabelIndexes(10, 600), xLabelIndexes(1, 600)], [[0, 5, 9], [0, 2, 5, 7, 9], [0]], 'metrics charts: three x labels under 460 px and five above, the first and last always labelled, one point labelled once');
sectionEnd('metrics charts tests: ok');

// ---- Metrics tables: "Load older" appends a page under the rows read (src/metrics/bodies.ts) ----
{
  const pageRow = (scope: string) => ({ cells: [], scope, selected: false });
  const cols = [{ head: 'Call', num: false }];
  const first = { columns: cols, rows: [pageRow('a'), pageRow('b')], empty: '', more: '/v1/metrics/calls?before=1' };
  const older = appendPage(first, { columns: cols, rows: [pageRow('c')], empty: '', more: '/v1/metrics/calls?before=2' });
  eq(older.rows.map((r) => r.scope), ['a', 'b', 'c'], 'a page of older rows goes under the rows already read');
  eq(older.more, '/v1/metrics/calls?before=2', 'the page\'s own more replaces the table\'s');
  eq(first.rows.length, 2, 'appending a page leaves the table it was given as it was');
  eq(appendPage(older, { columns: cols, rows: [], empty: '' }).more, null, 'a last page with no more ends the paging');
  sectionEnd('metrics table paging tests: ok');
}

// ---- Metrics view state: the URL hash, the request, the saved pins and scopes, the live lease (src/metrics/state.ts) ----
{
  const full: MetricsState = {
    tab: 'latency',
    scope: 'node:n1',
    range: '6h',
    shift: -2,
    zoom: [0.25, 0.75],
    compare: 'scope:node:n2',
    group: 'model',
    trace: 'abc 123',
    filter: 'status=error',
    fixture: true,
    live: DEFAULT_STATE.live,
    annotations: DEFAULT_STATE.annotations,
    compact: DEFAULT_STATE.compact,
  };
  eq(toHash(full), '#/metrics?tab=latency&scope=node%3An1&range=6h&shift=-2&zoom=0.250%2C0.750&compare=scope%3Anode%3An2&group=model&trace=abc+123&filter=status%3Derror&fixture=1', 'metrics state: a full state writes every non-default field in the fixed order');
  eq(parseHash(toHash(full)), full, 'metrics state: a full state survives the round trip through the hash');
  eq(toHash(DEFAULT_STATE), '#/metrics', 'metrics state: the defaults write a bare route');
  eq(parseHash('#/metrics'), DEFAULT_STATE, 'metrics state: a bare route reads as the defaults');
  eq(parseHash('#/metrics?range=2y&zoom=0.8,0.2&shift=3&fixture=yes'), DEFAULT_STATE, 'metrics state: a bad range, zoom, shift or fixture falls back to the defaults');
  eq([parseHash('#/metrics?zoom=0.5,1.5').zoom, parseHash('#/metrics?zoom=0.1,0.9').zoom, parseHash('#/metrics?range=7d').range], [null, [0.1, 0.9], '7d'], 'metrics state: a zoom outside 0 to 1 is dropped, a valid one is read, a known range is read');
  eq(parseHash('#/metrics?compact=1').compact, false, 'metrics state: compact is a preference and is not read from the URL');
  eq(queryOf(full, 800, 120), { tab: 'latency', scope: 'node:n1', range: '6h', shift: -2, compare: 'scope:node:n2', group: 'model', width: 800, tz: 120, fixture: true, zoom: [0.25, 0.75], trace: 'abc 123', filter: 'status=error' }, 'metrics state: the hub query carries the zoom tuple, trace and filter when set');
  eq(queryOf(DEFAULT_STATE, 600, 0), { tab: 'overview', scope: 'global', range: '24h', shift: 0, compare: 'prev', group: 'provider', width: 600, tz: 0, fixture: false }, 'metrics state: the default hub query leaves out zoom, trace and filter');

  const none: Saved = { scopes: [], pins: [], recent: [], starter: true };
  const six: Saved = { ...none, recent: ['a', 'b', 'c', 'd', 'e', 'f'] };
  eq(visit(six, 'g').recent, ['g', 'a', 'b', 'c', 'd', 'e'], 'metrics state: a new scope goes first and the oldest drops off, so recent keeps six');
  eq(visit(six, 'c').recent, ['c', 'a', 'b', 'd', 'e', 'f'], 'metrics state: a visited scope moves to the front without a duplicate');
  eq(six.recent, ['a', 'b', 'c', 'd', 'e', 'f'], 'metrics state: visiting returns a new object and leaves the saved one as it was');
  eq(togglePinnedScope(togglePinnedScope(none, 'node:n1'), 'node:n1').scopes, [], 'metrics state: a pinned scope unpins on the second toggle');

  const pin = { tab: 'latency', scope: 'global', panel: 'p-p95' };
  const pinned = togglePin(none, pin);
  eq(pinned.pins, [pin], 'metrics state: a panel pin is added');
  eq(pinned.starter, false, 'metrics state: the first pin clears the starter flag');
  eq(togglePin(pinned, pin).pins, [], 'metrics state: the same pin again removes it');
  eq(togglePin(pinned, { ...pin, scope: 'node:n1' }).pins.length, 2, 'metrics state: the same panel on another scope is a different pin');

  const sent: LiveLease[] = [];
  const lease: LiveLease = { scope: 'global', tab: 'overview', group: 'provider', range: '15m' };
  let current: LiveLease | null = lease;
  const keeper = new LeaseKeeper(async (l) => { sent.push(l); }, () => current);
  keeper.poke();
  keeper.poke();
  eq(sent.length, 1, 'metrics state: a lease is sent once and not again on a poke within 10 s');
  current = { ...lease, range: '1h' };
  keeper.poke();
  eq(sent.length, 2, 'metrics state: a changed lease is sent at once');
  current = null;
  keeper.poke();
  current = lease;
  keeper.poke();
  eq(sent.length, 3, 'metrics state: a lease is sent again after the view stopped holding one');
  keeper.stop();
}
sectionEnd('metrics state tests: ok');


// The task graph layout's typed wrapper: only the ids and the links cross, and the layout comes back parsed. The harness
// cannot load the built wasm; apz-graph-web's own tests cover the JSON layer, and the wasm was checked in a browser.
{
  const sent: { graph: string; options: string }[] = [];
  const glue: GraphGlue = {
    default: async () => undefined,
    layout: (graph, options) => {
      sent.push({ graph, options });
      if (options.includes('"card_width":0')) throw new Error('card_width must be more than 0');
      return JSON.stringify({ width: 10, height: 20, columns: [['t1']], cards: [], routes: [] });
    },
  };
  const graph = wrapGraph(glue);
  const task: GraphTask = { id: 't1', goal: 'Ship', role: null, depth: 0, state: 'open', ready: true, blocked_by: [], agent_cid: null, reviewer_cid: null, step: null };
  eq(graph.layout({ tasks: [task], edges: [] }).height, 20, 'graph: the layout comes back parsed');
  eq(sent[0], { graph: '{"tasks":[{"id":"t1"}],"edges":[]}', options: '' }, 'graph: only ids and links cross, options default to the desk');
  graph.layout({ tasks: [task], edges: [] }, { card_width: 420 });
  eq(sent[1]?.options, '{"card_width":420}', 'graph: options cross as JSON');
  let refusal = '';
  try {
    graph.layout({ tasks: [task], edges: [] }, { card_width: 0 });
  } catch (e) {
    refusal = (e as Error).message;
  }
  eq(refusal, 'card_width must be more than 0', 'graph: a refusal reaches the caller with its message');
}
sectionEnd('graph wrapper tests: ok');

// The Work screen's pure parts (src/views/work-model.ts): what a task is to the screen, its line, the flow line, what
// "Only what needs me" keeps, and the map's geometry.
{
  const now = 1_000_000;
  const node = (id: string, over: Partial<GraphTask> = {}): GraphTask => ({
    id, goal: `Goal of ${id}\nmore`, role: null, depth: 0, state: 'open', ready: false, blocked_by: [], agent_cid: null, reviewer_cid: null, step: null, ...over,
  });
  eq(workState({ state: 'open', ready: true, blocked_by: [] }), 'ready', 'work: an open ready task is ready');
  eq(workState({ state: 'open', ready: false, blocked_by: [] }), 'waiting', 'work: an open task waiting on links');
  eq(workState({ state: 'open', ready: false, blocked_by: ['t1'] }), 'blocked', 'work: a failed link blocks');
  eq(workState({ state: 'claimed', ready: false, blocked_by: [] }, { stage: 'tool', tool: 'bash', since_ms: 0 }), 'working', 'work: a claim at work');
  eq(workState({ state: 'claimed', ready: false, blocked_by: [] }, { stage: 'approval', tool: 'plaza_pay', since_ms: 0 }), 'needs-you', 'work: a claim waiting on the owner needs you');
  eq(workState({ state: 'awaiting_review', ready: false, blocked_by: [] }), 'review', 'work: awaiting review');
  eq(['done', 'failed', 'withdrawn'].map((state) => workState({ state, ready: false, blocked_by: [] })), ['done', 'failed', 'withdrawn'], 'work: ended states');
  eq(listed(['a']), 'a', 'listed: one');
  eq(listed(['a', 'b', 'c']), 'a, b and c', 'listed: three');
  eq([shortSpan(40_000), shortSpan(12 * 60_000), shortSpan(3 * 3_600_000), shortSpan(2 * 86_400_000)], ['40s', '12m', '3h', '2d'], 'short spans');
  eq(stepText({ stage: 'tool', tool: 'bash', since_ms: now - 120_000 }, now), 'Running bash · 2m', 'a step names its tool and its time');
  eq(stepText({ stage: 'approval', tool: 'plaza_pay', since_ms: now }, now), 'Waits for you to approve plaza_pay · 0s', 'an approval step names the tool');
  eq(stepText(null, now), '', 'no step, no text');

  const edges: [string, string][] = [['t1', 't3'], ['t2', 't3'], ['t3', 't4']];
  const states = new Map([['t1', 'done'], ['t2', 'claimed'], ['t3', 'open'], ['t4', 'open']]);
  const ctx = {
    edges,
    stateOf: (id: string) => states.get(id),
    taskName: (id: string) => `T${id.slice(1)}`,
    nameOf: (cid: number) => (cid === 7 ? 'Ada' : `agent ${cid}`),
    stepOf: (cid: number) => (cid === 7 ? { stage: 'thinking' as const, tool: null, since_ms: now - 5_000 } : null),
    now,
  };
  eq(cardLine(node('t3'), 'waiting', ctx), 'After T2', 'work: a waiting card names the links not done yet');
  eq(cardLine(node('t5', { blocked_by: ['t1', 't2'] }), 'blocked', ctx), 'Blocked: T1 and T2 did not finish', 'work: a blocked card names the links that ended');
  eq(cardLine(node('t2', { state: 'claimed', agent_cid: 7 }), 'working', ctx), 'Ada · Thinking · 5s', 'work: a working card names its agent and step');
  const asks = { ...ctx, stepOf: () => ({ stage: 'approval' as const, tool: 'plaza_pay', since_ms: now - 120_000 }) };
  eq(cardLine(node('t2', { state: 'claimed', agent_cid: 7 }), 'needs-you', asks), 'Approve plaza_pay · Ada · 2m', 'work: a card that needs you leads with the decision');
  eq(cardLine(node('t6', { role: 'qa' }), 'ready', ctx), 'Any QA agent can claim it', 'work: a ready card names the role that can claim it');
  eq(cardLine(node('t6', { role: 'orchestrator' }), 'ready', ctx), 'Any Orchestrator agent can claim it', 'work: a role is a name, with no article to get wrong');
  eq(cardLine(node('t6'), 'ready', ctx), 'Any agent can claim it', 'work: a ready card with no role');
  eq(cardLine(node('t6', { role: 'qa' }), 'ready', { ...ctx, readyAt: () => now - 300_000 }), 'Unclaimed for 5m', 'work: a ready card says how long nobody has claimed it');
  eq(cardLine(node('t6', { state: 'awaiting_review', reviewer_cid: 9 }), 'review', ctx), 'Reviewed by agent 9', 'work: a card in review names its reviewer');

  const ws = new Map<string, WorkState>([['t1', 'done'], ['t2', 'needs-you'], ['t3', 'waiting'], ['t4', 'waiting'], ['t5', 'ready']]);
  eq([...needsMe(ws, edges)].sort(), ['t2', 't3', 't4'], 'work: only what needs me keeps the decision and the work after it');
  eq(needsMe(new Map([['t1', 'done']]), edges).size, 0, 'work: nothing waits on the owner, nothing is kept');

  const project = {
    id: 'pr1', title: 'Release', goal: 'g', kind: 'other', stage: 'implementation', lead_cid: 1, owner_approval: false, repo: null,
    docs: [{ kind: 'brief', title: 'The brief', file: 'f1', repo_path: null, commit: null, summary: 's', by_cid: 1, by: 'Ada', at_ms: 0 }],
    critiques: [{ by_cid: 3, by: 'Cy', verdict: 'approve', text: 'ok', revision: 1, at_ms: 0 }], revision: 1, approved_by: 'Mayor',
    tasks: ['t1', 't2'], history: [], created_ms: 0, updated_ms: 0,
  } as Project;
  const taskRow = (id: string, over: Partial<Task> = {}): Task => ({
    id, state: 'open', goal: id, acceptance: null, role: null, budget: 2000, assignee_id: null, agent_cid: null, note: null, outcome: null,
    created_at: 0, updated_at: 0, project: 'pr1', after: [], ready: true, waiting_on: [], blocked_by: [], ...over,
  });
  const segs = flowSegments(project, [taskRow('t1', { state: 'done' }), taskRow('t2', { ready: false, waiting_on: ['t1'] })]);
  eq(segs.map((x) => `${x.kind}:${x.state}`), ['plan:done', 'plan:done', 'plan:done', 'plan:done', 'plan:done', 'task:done', 'task:waiting', 'end:later', 'end:later'], 'work: a building project’s flow line');
  eq(flowSegments(null, [taskRow('t9')]).map((x) => x.kind), ['task'], 'work: tasks outside projects have task squares only');
  const cards = planCards(project);
  eq(cards.map((c) => c.state), ['done', 'done', 'done', 'done', 'done'], 'work: an approved plan is done');
  eq(cards[4]?.title, 'Approved by Mayor', 'work: the approval names who approved');
  eq(planCards({ ...project, stage: 'approval', approved_by: null, owner_approval: true })[4]?.state, 'you', 'work: a plan waiting on the owner is the owner’s');
  const asking = { ...project, stage: 'critique', approved_by: null, owner_approval: true, tasks: [] } as Project;
  eq(flowSegments(asking, []).map((x) => x.state), ['done', 'done', 'done', 'current', 'you', 'later', 'later'], 'work: the flow line shows the approval waiting on the owner');
  eq(flowSegments({ ...asking, owner_approval: false }, [])[4]?.state, 'later', 'work: the mayor’s approval is not the owner’s');
  const claimed = [taskRow('t1', { state: 'claimed', ready: false, agent_cid: 7 }), taskRow('t2', { state: 'claimed', ready: false, agent_cid: 8 })];
  const deciding = (cid: number) => (cid === 7 ? { stage: 'approval' as const, tool: 'plaza_pay', since_ms: 0 } : { stage: 'tool' as const, tool: 'bash', since_ms: 0 });
  eq(flowSegments(project, claimed, deciding).filter((x) => x.kind === 'task').map((x) => x.state), ['needs-you', 'working'], 'work: with the steps, a task waiting on the owner is the owner’s square');
  eq(flowSegments(project, claimed).filter((x) => x.kind === 'task').map((x) => x.state), ['working', 'working'], 'work: without the steps, a claim is at work');
  eq([waitsForOwner(asking, []), waitsForOwner(project, claimed, deciding), waitsForOwner(project, claimed), waitsForOwner(null, claimed, deciding)], [true, true, false, true], 'work: a plan or a task can wait on the owner, in a project or outside');

  const waits = waitsByRole(
    [
      taskRow('t1', { state: 'done', updated_at: 5_000 }),
      taskRow('t2', { role: 'qa', after: ['t1'], created_at: 1_000 }),
      taskRow('t3', { role: 'qa', created_at: 2_000 }),
      taskRow('t4', { created_at: 7_000 }),
      taskRow('t5', { role: 'qa', ready: false, created_at: 0 }),
      taskRow('t6', { role: 'qa', state: 'claimed', ready: false, created_at: 0 }),
    ],
    10_000,
  );
  eq(waits, [{ role: 'qa', count: 2, waited: 8_000 }, { role: null, count: 1, waited: 3_000 }], 'work: ready tasks nobody claimed, by role, waiting since posted or since their last link finished');

  const layout: Layout = {
    width: 300, height: 100, columns: [['t1'], ['t2']],
    cards: [{ id: 't1', column: 0, row: 0, x: 10, y: 10, width: 100, height: 80 }, { id: 't2', column: 1, row: 0, x: 160, y: 10, width: 100, height: 80 }],
    routes: [{ from: 't1', to: 't2', points: [[110, 50], [160, 50]] }],
  };
  const geo = compose(layout, { plan: { width: 50, height: 200, anchor: 190 }, delivery: { width: 40, height: 60 }, gap: 20 });
  eq([geo.width, geo.height], [50 + 20 + 300 + 20 + 40, 200], 'work: the map holds the plan, the graph and delivery');
  eq(geo.graph, { x: 70, y: 50 }, 'work: the graph is centred on the tallest part');
  eq(geo.posted, ['M50 190H60V100H80'], 'work: the plan posts the first column');
  eq(geo.delivered, ['M330 100H380V100H390'], 'work: the last tasks lead to delivery');
  eq(pathOf([[1, 2], [3, 2]], 10, 20), 'M11 22L13 22', 'work: a route moves onto the map');

  const loose = looseGraph([taskRow('t1', { project: null }), taskRow('t2', { project: null, after: ['t1', 't7'] }), taskRow('t3')]);
  eq(loose.tasks.map((t) => t.id), ['t1', 't2'], 'work: tasks outside projects only');
  eq(loose.edges, [['t1', 't2']], 'work: links among them only');
}
sectionEnd('work screen tests: ok');

if (failedSections.length) {
  console.error(`${failedSections.length} section(s) failed: ${failedSections.join(', ')}`);
  process.exit(1);
}

// ---- metrics tile rows ------------------------------------------------------
{
  eq([1, 3, 4, 5, 6, 7, 8, 9].map(narrowTileColumns), [1, 3, 4, 5, 3, 4, 4, 3], 'tiles: narrow rows are even, as the desktop wraps them');
  console.log('metrics tile rows tests: ok');
}

// ---- metrics band outline ---------------------------------------------------
{
  // A band from 10–12 at x 0 to 30–36 at x 1: the low edge comes back with its own values, not mirrored.
  const d = bandPath([12, 36], [10, 30], (j) => j * 100, (v) => v);
  eq(d, 'M0.0,12.0L100.0,36.0L100.0,30.0L0.0,10.0Z', 'band: the low edge returns right to left with its own values');
  // Two stacked layers: the second's lower edge is the first's top, walked back with its own values.
  const [a, b] = stackedPaths([[2, 4], [1, null]], (j) => j * 100, (v) => v);
  eq(a, 'M0.0,2.0L100.0,4.0L100.0,0.0L0.0,0.0Z', 'stacked: the bottom layer sits on zero');
  eq(b, 'M0.0,3.0L100.0,4.0L100.0,4.0L0.0,2.0Z', 'stacked: the next layer shares the edge below, a gap counts as zero');
  eq(wrapsAtSpaces('sonnet-5.5 on r-12'), 'sonnet\u20115.5 on r\u201112', 'table text: hyphens do not break, spaces do');
  console.log('metrics band tests: ok');
}

// ---- Machines: the screen's view and row from its hash (views/machines.ts), and the moved routes (router.ts) ----
eq(machinesRoute('#/machines').view, 'grid', 'machines: the bare route is the grid');
eq(machinesRoute('#/machines').id, null, 'machines: the grid has no open row');
eq(machinesRoute('#/machines?view=list').view, 'list', 'machines: ?view=list is the list');
eq(machinesRoute('#/machines?view=grid').view, 'grid', 'machines: ?view=grid is the grid');
eq(machinesRoute('#/machines/m-7').view, 'machine', 'machines: a machine id opens its own view');
eq(machinesRoute('#/machines/m-7').id, 'm-7', 'machines: the id is the machine');
eq(machinesRoute('#/machines/m-7').id, 'm-7', 'machines: the id is the row to open');
eq(viewHash('list'), '#/machines?view=list', 'machines: the list hash');
eq(viewHash('grid'), '#/machines', 'machines: the grid hash');
eq(movedTo('#/floor'), '#/machines', 'machines: #/floor moves to the grid');
eq(movedTo('#/machine'), '#/machines?view=list', 'machines: #/machine moves to the list');
eq(movedTo('#/machine/m-7'), '#/machines/m-7', 'machines: #/machine/<id> moves to that row');
eq(movedTo('#/machines'), null, 'machines: the screen itself does not move');
eq(movedTo('#/usage'), '#/metrics?tab=tokens', 'machines: #/usage still moves');
sectionEnd('machines route tests: ok');

// The machine view's pure helpers: VM rows, GPU card lines, the metrics series.
eq(
  vmRows({ runs: [{ id: 'r12', vms: [{ index: 3, state: 'running', guest_cid: 105 }] }] } as unknown as Parameters<typeof vmRows>[0]),
  [{ id: 'r12/3', run: 'r12', index: 3, state: 'running', cid: 105 }],
  'a VM row names its run and index, and its agent CID',
);
eq(vmRows({ runs: undefined } as unknown as Parameters<typeof vmRows>[0]), [], 'a machine with no runs lists no VMs');
const refCard = { id: 'rtx-4090-fe', name: 'RTX 4090 Founders', segment: 'consumer', form: 'desktop', slots: 3, size_mm: null, cooling: 'air', fans: 3, generic: false };
eq(
  gpuCardLine({ gpu: 'NVIDIA GeForce RTX 4090', vendor: 'nvidia', memory_mib: 24576, card: refCard }),
  'RTX 4090 Founders · desktop · 3-slot · air',
  'a GPU line: card name, form, slots, cooling',
);
eq(gpuCardLine({ gpu: 'x', vendor: 'nvidia', memory_mib: null, card: { ...refCard, generic: true } }), 'RTX 4090 Founders · desktop · 3-slot · air (generic)', 'a generic card says so');
eq(gpuCardLine({ gpu: 'Mystery GPU', vendor: 'other', memory_mib: null, card: null }), 'Mystery GPU · no reference card', 'a GPU with no card');
eq(parseGpuCards('null'), null, 'the engine says null for an unknown machine');
eq(parseGpuCards('{'), null, 'a broken reply is no GPUs');
eq(parseGpuCards('[]'), [], 'no GPUs is an empty list');
const cpuView = { panels: [{ metric: 'host.cpu_pct', body: { type: 'chart', series: [{ values: [1, null, 3] }] } }, { metric: 'host.gpu_util_pct', body: { type: 'empty', text: '' } }] };
eq(metricValues(cpuView as unknown as Parameters<typeof metricValues>[0], 'host.cpu_pct'), [1, null, 3], 'the first series of the metric');
eq(metricValues(cpuView as unknown as Parameters<typeof metricValues>[0], 'host.gpu_util_pct'), [], 'a panel that is not a chart has no values');
eq(metricValues({} as unknown as Parameters<typeof metricValues>[0], 'host.cpu_pct'), [], 'an empty view (a hub without metrics) has no values');
eq(metricValues(cpuView as unknown as Parameters<typeof metricValues>[0], 'host.nope'), [], 'a metric the view does not show');
eq(latestValue([1, null, 3, null]), 3, 'the newest number is the latest');
eq(latestValue([null, null]), null, 'no number, no latest');
eq(finiteValues([1, null, 3]), [1, 3], 'finite values only');
eq(parseStyle('blueprint'), 'blueprint', 'a stored style is read back');
eq(parseStyle('night'), 'night', 'Night is a style');
eq(parseStyle('neon'), 'ink', 'an unknown stored style is Ink');
eq(parseStyle(null), 'ink', 'no stored style is Ink');
eq(STYLES.map((s) => s.label), ['Ink', 'Blueprint', 'Night'], 'the three styles, labelled');
sectionEnd('machine view helper tests: ok');
