// The sessions screens' pure logic: the state labels and tones, the live and final test, the list's scope and order, the
// seat and cost text, the CID-or-id reference, the transcript's lines and the context's fill. The DOM is not touched
// here. Bundled with the other suites by test/tests.ts; it prints "sessions tests: ok".

import type { Session, SessionState, TranscriptEntry } from '../src/api-work';
import { barText } from '../src/ui/chat-tools';
import {
  clockText,
  contextFill,
  costText,
  count,
  hasCall,
  headingText,
  inScope,
  isCidRef,
  isFinal,
  isLive,
  modelText,
  newestWithCid,
  pausedText,
  resultText,
  seatText,
  sessionRows,
  stateLabel,
  stateTone,
  toolsByResult,
  transcriptLine,
  usageText,
} from '../src/views/session-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

function session(over: Partial<Session> = {}): Session {
  return {
    id: 's1',
    project: 'p1',
    task: null,
    seat: null,
    kind: 'long',
    parent: null,
    role: 'orchestrator',
    engine: 'harness',
    model: null,
    state: 'running',
    paused_by: null,
    node: null,
    cid: null,
    run: null,
    work: null,
    usage: { tokens_in: 0, tokens_out: 0, tokens_cached: 0, cost_micros: 0, model_calls: 0, tool_calls: 0 },
    context_bytes: 0,
    compactions: 0,
    handover: null,
    outcome: null,
    attempt: 1,
    verdict: null,
    created_ms: 0,
    started_ms: null,
    ended_ms: null,
    ...over,
  };
}

function entry(kind: string, data: Record<string, unknown>): TranscriptEntry {
  return { cid: 7, seq: 1, work: 2, at_ms: 0, kind, data };
}

// ---- States ---------------------------------------------------------------------------------------------------------

const STATES: SessionState[] = ['queued', 'placing', 'forking', 'running', 'paused', 'compacting', 'handing_over', 'ending', 'ended', 'failed'];
eq(STATES.map(stateLabel), ['Queued', 'Starting', 'Starting', 'Running', 'Paused', 'Compacting', 'Handing over', 'Ending', 'Ended', 'Failed'], 'every state has its label');
eq(stateTone('failed'), 'danger', 'a failed session is danger');
eq(stateTone('paused'), 'warn', 'a paused session is warn');
eq(stateTone('ended'), 'muted', 'an ended session is muted');
eq(STATES.filter(isFinal), ['ended', 'failed'], 'only ended and failed are final');
eq(STATES.filter((s) => isLive(session({ state: s }))).length, 8, 'the other eight are live');
eq(isLive(session({ state: 'paused' })), true, 'a paused session is still live');

// ---- Scope, live filter and order -----------------------------------------------------------------------------------

const old = session({ id: 's1', state: 'ended', created_ms: 100 });
const newFinal = session({ id: 's2', state: 'failed', created_ms: 300 });
const liveOld = session({ id: 's3', state: 'running', created_ms: 50 });
const liveNew = session({ id: 's4', state: 'paused', created_ms: 200 });
const elsewhere = session({ id: 's5', project: 'p2', state: 'running', created_ms: 400 });
const all = [old, newFinal, liveOld, liveNew, elsewhere];

eq(sessionRows(all, { project: null, liveOnly: false }).map((s) => s.id), ['s5', 's4', 's3', 's2', 's1'], 'every project: live first, then the newest first');
eq(sessionRows(all, { project: 'p1', liveOnly: false }).map((s) => s.id), ['s4', 's3', 's2', 's1'], 'the scope keeps its project only');
eq(sessionRows(all, { project: 'p1', liveOnly: true }).map((s) => s.id), ['s4', 's3'], 'live only drops the finished ones');
eq(sessionRows(all, { project: 'none', liveOnly: false }), [], 'a project with no sessions has no rows');
eq(inScope(elsewhere, null), true, 'no scope shows every project');
eq(inScope(elsewhere, 'p1'), false, 'a scope leaves other projects out');
const before = all.map((s) => s.id).join();
sessionRows(all, { project: null, liveOnly: false });
eq(all.map((s) => s.id).join(), before, 'sorting leaves the input as it was');

// ---- Seat, heading, pause reason ------------------------------------------------------------------------------------

eq(seatText({ kind: 'long', seat: null }), 'long-lived', 'a long session is long-lived');
eq(seatText({ kind: 'helper', seat: 'x' }), 'helper', 'a helper is a helper');
eq(seatText({ kind: 'step', seat: 'builder' }), 'builder', 'a step shows its seat');
eq(seatText({ kind: 'step', seat: null }), '—', 'a step with no seat shows a dash');
eq(headingText(session({ role: 'builder', kind: 'step', seat: 'builder' })), 'Builder · builder', 'the heading is role and seat');
eq(headingText(session({ role: 'orchestrator', kind: 'long' })), 'Orchestrator · long-lived', 'a long heading says long-lived');
eq(pausedText('budget'), 'Budget reached', 'a budget pause reads so');
eq(pausedText('card'), 'Waiting for you', 'a card pause waits for the owner');
eq(pausedText(null), null, 'no pause reason is null');

// ---- Usage and cost text --------------------------------------------------------------------------------------------

eq(costText(1_500_000), '$1.50', 'micro-dollars read as dollars');
eq(costText(0), '$0.00', 'nothing costs $0.00');
eq(costText(Number.NaN), '$0.00', 'a count that is not a number reads $0.00');
eq(usageText({ tokens_in: 41_200, tokens_out: 5_300, tokens_cached: 0, cost_micros: 0, model_calls: 0, tool_calls: 0 }), '41.2k in · 5.3k out', 'usage is compact in and out');
eq(usageText({ tokens_in: 999_500, tokens_out: 0, tokens_cached: 0, cost_micros: 0, model_calls: 0, tool_calls: 0 }), '1.0M in · 0 out', 'a count near a million reads 1.0M, never 1000k');
eq(count(1, 'turn', 'turns'), '1 turn', 'one is singular');
eq(count(3, 'turn', 'turns'), '3 turns', 'more is plural');

// ---- Route reference ------------------------------------------------------------------------------------------------

eq(isCidRef('105'), true, 'digits are a CID');
eq(isCidRef('s12'), false, 'a session id is not a CID');
eq(isCidRef(''), false, 'an empty reference is no CID');
eq(isCidRef('1a'), false, 'digits with letters are no CID');
const onVm = [session({ id: 's1', cid: 105, created_ms: 10 }), session({ id: 's2', cid: 105, created_ms: 30 }), session({ id: 's3', cid: 106, created_ms: 40 })];
eq(newestWithCid(onVm, 105)?.id, 's2', 'the newest session on the VM is the one shown');
eq(newestWithCid(onVm, 999), null, 'a VM with no session has none');

// ---- Transcript lines -----------------------------------------------------------------------------------------------

const response = transcriptLine(
  entry('model_response', {
    model: 'p/m',
    content: [
      { type: 'text', text: 'Hi' },
      { type: 'tool_use', name: 'read' },
      { type: 'text', text: 'there' },
    ],
    usage: { input_tokens: 3, output_tokens: 1, cache_read_tokens: 0, cache_write_tokens: 0 },
  }),
);
eq(response, { label: 'Model', text: 'Hi\nthere', mono: false, note: '3 in · 1 out', alert: false, known: true }, 'a model response: its text parts, its tokens');
eq(modelText('plain'), 'plain', 'a plain string is the text itself');
eq(modelText(undefined), '', 'no content is no text');

const okResult = transcriptLine(entry('tool_result', { call: 'c1', ok: true, output: 'done' }));
eq(okResult, { label: 'Tool result', text: 'done', mono: true, note: '', alert: false, known: true }, 'an ok tool result is mono output');
const failed = transcriptLine(entry('tool_result', { call: 'c2', ok: false, output: 'x'.repeat(500) }));
eq(failed.label, 'Tool failed', 'a failed tool result says so');
eq(failed.alert, true, 'a failed tool result is an alert');
eq(failed.text.length, 400, 'a tool output is clipped to 400 characters');
eq(failed.text.endsWith('…'), true, 'a clipped output ends in an ellipsis');
eq(resultText('short'), 'short', 'a short output is kept whole');
eq(
  transcriptLine(entry('tool_request', { call: 'c1', name: 'read', input: { path: 'a.md' } })),
  { label: 'Tool call', text: 'read {"path":"a.md"}', mono: true, note: '', alert: false, known: true },
  'a tool request is its name and its input as JSON',
);
eq(transcriptLine(entry('work_start', { kind: 'step', thread: 't', prompt: 'Build it', origin: {} })).text, 'Build it', 'the prompt is the start text');
eq(transcriptLine(entry('error', { message: 'boom' })), { label: 'Error', text: 'boom', mono: false, note: '', alert: true, known: true }, 'an error is an alert');
eq(transcriptLine(entry('work_end', { status: 'done', summary: 'Shipped' })).note, 'Done', 'the end of the work shows its outcome');
eq(
  transcriptLine(entry('tool_decision', { call: 'c1', decision: 'deny', output: 'no' })).text,
  'Denied: no',
  'a decision says what it was and what it produced',
);
eq(transcriptLine(entry('compressed', { before: 41_200, after: 9_000, folded: 3, reason: 'window' })).text, '41.2k → 9k tokens, 3 turns folded', 'a compression says its sizes and turns');
eq(transcriptLine(entry('sandbox_network', { host: 'x' })), { label: 'sandbox_network', text: '', mono: false, note: '', alert: false, known: false }, 'an unknown kind keeps its name');

// ---- Context --------------------------------------------------------------------------------------------------------

eq(contextFill({ used: 41_200, window: 200_000, fill: null }), 0.206, 'the fill is used over the window');
eq(contextFill({ used: 1, window: 2, fill: 0.5 }), 0.5, 'a fill the hub gives is taken as it is');
eq(contextFill({ used: 5, window: 2, fill: 3 }), 1, 'a fill is at most 1');
eq(contextFill({ used: 5, window: null }), null, 'no window means no fill');
eq(barText(41_200, 200_000, contextFill({ used: 41_200, window: 200_000 })), '41.2k / 200k · 21%', 'the fill text is the bar text');
eq(hasCall({}), false, 'a context with no calls has made none');
eq(hasCall({ cid: 105, calls: 0 }), false, 'a VM alone is no call: every context gets its CID once it has a VM');
eq(hasCall({ cid: 105, calls: 3 }), true, 'a context with calls has made a call');
eq(
  toolsByResult([
    { tool: 'bash', calls: 1, errors: 0, result_tokens: 10 },
    { tool: 'read', calls: 2, errors: 0, result_tokens: 90 },
    { tool: 'edit', calls: 1, errors: 0, result_tokens: 10 },
  ]).map((t) => t.tool),
  ['read', 'bash', 'edit'],
  'tools sort by their result tokens, then by name',
);

eq(clockText(new Date(2026, 9, 22, 14, 3, 7).getTime()), '14:03:07', 'a time reads on the clock, with seconds');

if (checks === 0) throw new Error('sessions tests ran no checks');
console.log(`sessions tests: ok (${checks} checks)`);
