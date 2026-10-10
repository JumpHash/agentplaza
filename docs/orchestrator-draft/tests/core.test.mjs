// Behaviour tests for orchestrator-draft/schema.sql on PostgreSQL 16 (PGlite, in-process). Run: ../check.sh
import { PGlite } from '@electric-sql/pglite';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const schema = fs.readFileSync(process.env.SCHEMA ?? path.join(here, '..', 'schema.sql'), 'utf8');
let failures = 0, passed = 0;

const A1 = '01JAAAAAAAAAAAAAAAAAAAAAA1', A2 = '01JAAAAAAAAAAAAAAAAAAAAAA2', A3 = '01JAAAAAAAAAAAAAAAAAAAAAA3';

async function fresh() {
  const db = new PGlite();
  await db.exec(schema);
  await db.exec(`
    INSERT INTO core.machines (id, name, arch, backend, class, page_size, cpus, memory_mib, disk_gib, cert_fingerprint)
      VALUES ('m1', 'hw-box-1', 'x86_64', 'kvm', 'server', 4096, 8, 32768, 500, '\\x01'),
             ('m2', 'pi-5', 'aarch64', 'kvm', 'pi5', 4096, 4, 8192, 120, '\\x02');
    INSERT INTO core.towns (id, name) VALUES ('shop', 'Shop');
    UPDATE core.towns SET state = 'active' WHERE id = 'shop';
    INSERT INTO core.agents (id, name, role, town_id, config_hash, codex_hash, public_key) VALUES
      ('${A1}', 'Vesna', 'builder', 'shop', '\\x01', '\\x01', '\\x01'),
      ('${A2}', 'Oskar', 'reviewer', 'shop', '\\x01', '\\x01', '\\x02'),
      ('${A3}', 'Mila', 'mayor', 'shop', '\\x01', '\\x01', '\\x03');
    INSERT INTO core.runs (id, agent_id, model_rung) VALUES ('r1', '${A1}', 'mid');
    INSERT INTO core.vms (id, run_id, machine_id, trust_class) VALUES ('vm1', 'r1', 'm1', 'standard'), ('vm2', 'r1', 'm2', 'standard');
    INSERT INTO core.sessions (id, run_id, vm_id, epoch, token_hash) VALUES ('s1', 'r1', 'vm1', 1, '\\x0a');
    UPDATE core.runs SET state = 'running', machine_id = 'm1' WHERE id = 'r1';`);
  return db;
}

async function test(name, fn) {
  try { await fn(); passed++; console.log(`ok    ${name}`); }
  catch (e) { failures++; console.log(`FAIL  ${name}: ${e.message}`); }
}
async function rejects(promise, pattern) {
  try { await promise; } catch (e) {
    if (!pattern.test(e.message)) throw new Error(`rejected for the wrong reason: ${e.message}`);
    return;
  }
  throw new Error('was accepted, expected rejection');
}
const eq = (a, b, what) => { if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`); };
const one = async (db, sql, args = []) => (await db.query(sql, args)).rows[0];
// Entry hashes in tests are just the seq as one byte; prev_hash links to the previous one (0135).
const h = (n) => Buffer.from([n]);
const tx = (db, seq, kind, extra = {}) => db.query(
  `INSERT INTO core.transcript (run_id, seq, kind, session_id, call_id, decision, law_id, body, entry_hash, prev_hash, seal)
   VALUES ('r1', $1, $2, $3, $4, $5, $6, '{}', $7, $8, '\\x5e')`,
  [seq, kind, extra.session === undefined ? 's1' : extra.session, extra.call ?? null, extra.decision ?? null, extra.law ?? null,
   h(seq), extra.prev ?? h(seq - 1 === 0 ? 0 : seq - 1)]);

// ---------------------------------------------------------------- sessions and fencing (0102)
await test('schema loads; fixtures valid', async () => { await fresh(); });

await test('only one live session per run', async () => {
  const db = await fresh();
  await rejects(db.query(`INSERT INTO core.sessions (id, run_id, vm_id, epoch, token_hash) VALUES ('s2', 'r1', 'vm2', 2, '\\x0b')`), /sessions_one_live/);
});

await test('a move fences the old session; the new epoch must be newer', async () => {
  const db = await fresh();
  await db.exec(`UPDATE core.runs SET state = 'stalled' WHERE id = 'r1'; UPDATE core.runs SET state = 'starting' WHERE id = 'r1';`);
  eq((await one(db, `SELECT fenced_at IS NOT NULL AS f FROM core.sessions WHERE id = 's1'`)).f, true, 's1 fenced');
  await rejects(db.query(`INSERT INTO core.sessions (id, run_id, vm_id, epoch, token_hash) VALUES ('s2', 'r1', 'vm2', 1, '\\x0b')`), /not newer/);
  await db.query(`INSERT INTO core.sessions (id, run_id, vm_id, epoch, token_hash) VALUES ('s2', 'r1', 'vm2', 2, '\\x0b')`);
  await rejects(db.query(`UPDATE core.sessions SET fenced_at = NULL WHERE id = 's1'`), /fenced for good/);
});

// ---------------------------------------------------------------- transcript (0084)
await test('transcript is gapless and in order', async () => {
  const db = await fresh();
  await tx(db, 1, 'user_turn', { session: null });
  await rejects(tx(db, 3, 'model_turn'), /expects seq 2/);
  await rejects(tx(db, 1, 'model_turn'), /expects seq 2/);
  await tx(db, 2, 'model_turn');
});

await test('transcript entries form a seal chain (0135)', async () => {
  const db = await fresh();
  await tx(db, 1, 'user_turn', { session: null });
  await rejects(tx(db, 2, 'model_turn', { prev: Buffer.from([9]) }), /breaks the seal chain/);
  await tx(db, 2, 'model_turn');
});

await test('a fenced session cannot write (OnlyCurrentSessionWrites)', async () => {
  const db = await fresh();
  await tx(db, 1, 'user_turn', { session: null });
  await db.exec(`UPDATE core.runs SET state = 'stalled' WHERE id = 'r1'; UPDATE core.runs SET state = 'starting' WHERE id = 'r1';
                 INSERT INTO core.sessions (id, run_id, vm_id, epoch, token_hash) VALUES ('s2', 'r1', 'vm2', 2, '\\x0b');`);
  await rejects(tx(db, 2, 'model_turn', { session: 's1' }), /not the live session/);
  await tx(db, 2, 'model_turn', { session: 's2' });
});

await test('harness entries must name their session', async () => {
  const db = await fresh();
  await rejects(tx(db, 1, 'tool_result', { session: null, call: 'c1' }), /must name the session/);
});

await test('one decision and one result per call; a result needs an allow', async () => {
  const db = await fresh();
  await tx(db, 1, 'tool_request', { call: 'c1' });
  await rejects(tx(db, 2, 'tool_result', { call: 'c1' }), /never allowed/);
  await tx(db, 2, 'tool_decision', { session: null, call: 'c1', decision: 'allow' });
  await rejects(tx(db, 3, 'tool_decision', { session: null, call: 'c1', decision: 'deny', law: 'S-001a' }), /transcript_one_decision/);
  await tx(db, 3, 'tool_result', { call: 'c1' });
  await rejects(tx(db, 4, 'tool_result', { call: 'c1' }), /transcript_one_result/);
});

await test('a denied call never gets a result; denials carry a law id', async () => {
  const db = await fresh();
  await tx(db, 1, 'tool_request', { call: 'c1' });
  await rejects(tx(db, 2, 'tool_decision', { session: null, call: 'c1', decision: 'deny' }), /denial_names_law/);
  await tx(db, 2, 'tool_decision', { session: null, call: 'c1', decision: 'deny', law: 'C-002b' });
  await rejects(tx(db, 3, 'tool_result', { call: 'c1' }), /never allowed/);
});

await test('transcript, events and task history are append-only', async () => {
  const db = await fresh();
  await tx(db, 1, 'user_turn', { session: null });
  await rejects(db.query(`UPDATE core.transcript SET body = '{"x":1}' WHERE seq = 1`), /append-only/);
  await rejects(db.query(`DELETE FROM core.transcript`), /append-only/);
  await db.query(`INSERT INTO core.events (kind, payload) VALUES ('run', '{}')`);
  await rejects(db.query(`DELETE FROM core.events`), /append-only/);
});

// ---------------------------------------------------------------- runs
await test('run state machine has no way back from done', async () => {
  const db = await fresh();
  await db.query(`UPDATE core.runs SET state = 'done', ended_at = now() WHERE id = 'r1'`);
  eq((await one(db, `SELECT fenced_at IS NOT NULL AS f FROM core.sessions WHERE id = 's1'`)).f, true, 'session fenced at end');
  await rejects(db.query(`UPDATE core.runs SET state = 'running', ended_at = NULL WHERE id = 'r1'`), /may not go from done to running/);
});

await test('StartRun idempotency key starts at most one run', async () => {
  const db = await fresh();
  await db.query(`INSERT INTO core.runs (id, agent_id, model_rung, idempotency_key) VALUES ('r2', '${A1}', 'cheap', 'k1')`);
  await rejects(db.query(`INSERT INTO core.runs (id, agent_id, model_rung, idempotency_key) VALUES ('r3', '${A1}', 'cheap', 'k1')`), /duplicate key/);
});

await test('idempotency key reused for a different request is refused', async () => {
  const db = await fresh();
  await db.query(`INSERT INTO core.idempotency (key, method, principal, request_hash) VALUES ('k', 'StartRun', 'dev1', '\\x01')`);
  await db.query(`UPDATE core.idempotency SET response = '\\x99' WHERE key = 'k'`);
  await rejects(db.query(`UPDATE core.idempotency SET request_hash = '\\x02' WHERE key = 'k'`), /different request/);
  await rejects(db.query(`UPDATE core.idempotency SET response = '\\x98' WHERE key = 'k'`), /already has a response/);
});

// ---------------------------------------------------------------- snapshots
await test('snapshots: seq within the transcript, parent continues the chain, only uploaded ones restore', async () => {
  const db = await fresh();
  await tx(db, 1, 'user_turn', { session: null });
  await tx(db, 2, 'model_turn');
  const snap = (id, parent, seq, arch = 'x86_64', up = true) => db.query(
    `INSERT INTO core.snapshots (id, run_id, parent_id, seq, arch, backend, kernel_build, manifest_hash, bytes_written, uploaded_at)
     VALUES ($1, 'r1', $2, $3, $4, 'kvm', '6.18.1', '\\x01', 100, CASE WHEN $5 THEN now() END)`, [id, parent, seq, arch, up]);
  await rejects(snap('x', null, 3), /beyond the transcript/);
  await snap('a', null, 1);
  await rejects(snap('b', 'a', 2, 'aarch64'), /does not continue/);
  await snap('b', 'a', 2, 'x86_64', false);
  eq((await one(db, `SELECT id FROM core.restorable_snapshots WHERE run_id = 'r1'`)).id, 'a', 'latest uploaded');
});

await test('chunk GC: chunks of ended runs become collectable', async () => {
  const db = await fresh();
  await tx(db, 1, 'user_turn', { session: null });
  await db.exec(`INSERT INTO core.chunks VALUES ('\\xaa', 262144), ('\\xbb', 1000);
    INSERT INTO core.snapshots (id, run_id, seq, arch, backend, kernel_build, manifest_hash, bytes_written) VALUES ('a', 'r1', 1, 'x86_64', 'kvm', 'k', '\\x01', 1);
    INSERT INTO core.snapshot_chunks VALUES ('a', '\\xaa');`);
  eq(Number((await one(db, `SELECT count(*) AS n FROM core.collectable_chunks`)).n), 1, 'orphan chunk only');
  await db.query(`UPDATE core.runs SET state = 'killed', ended_at = now() WHERE id = 'r1'`);
  eq(Number((await one(db, `SELECT count(*) AS n FROM core.collectable_chunks`)).n), 2, 'after the run ended');
});

// ---------------------------------------------------------------- approvals (0094, 0116)
const approval = (db, id, chain, c004 = false) => db.query(
  `INSERT INTO core.approvals (id, run_id, kind, summary, risk_class, c004, chain, step_deadline, nonce)
   VALUES ($1, 'r1', 'tool_call', 'push to main', 'high', $2, $3, now() + interval '1 hour', $4)`, [id, c004, chain, Buffer.from(id)]);

await test('C-004 approvals have the owner-only chain', async () => {
  const db = await fresh();
  await rejects(approval(db, 'p1', ['mayor:shop', 'owner'], true), /c004_owner_only/);
  await approval(db, 'p2', ['owner'], true);
});

await test('only the authority at the current step decides (DecidedByChain)', async () => {
  const db = await fresh();
  await approval(db, 'p1', ['mayor:shop', 'council', 'owner']);
  await rejects(db.query(`UPDATE core.approvals SET status = 'approved', decided_as = 'owner', decided_by = 'dev1' WHERE id = 'p1'`), /decided_by_chain/);
  await db.query(`UPDATE core.approvals SET position = 2, nonce = '\\x02' WHERE id = 'p1'`);
  await db.query(`UPDATE core.approvals SET status = 'approved', decided_as = 'council', decided_by = '${A3}' WHERE id = 'p1'`);
});

await test('escalation needs a fresh nonce; decisions are final; silence is not yes', async () => {
  const db = await fresh();
  await approval(db, 'p1', ['mayor:shop', 'owner']);
  await rejects(db.query(`UPDATE core.approvals SET position = 2 WHERE id = 'p1'`), /fresh nonce/);
  await rejects(db.query(`UPDATE core.approvals SET status = 'expired' WHERE id = 'p1'`), /only after its last step/);
  await db.query(`UPDATE core.approvals SET position = 2, nonce = '\\x0f' WHERE id = 'p1'`);
  await db.query(`UPDATE core.approvals SET status = 'expired' WHERE id = 'p1'`);
  await rejects(db.query(`UPDATE core.approvals SET status = 'approved', decided_as = 'owner', decided_by = 'dev1' WHERE id = 'p1'`), /is final/);
});

await test('a nonce is used once', async () => {
  const db = await fresh();
  await approval(db, 'p1', ['owner']);
  await db.query(`UPDATE core.approvals SET status = 'approved', decided_as = 'owner', decided_by = 'dev1' WHERE id = 'p1'`);
  await db.query(`INSERT INTO core.approvals (id, run_id, kind, summary, risk_class, chain, step_deadline, nonce)
                  VALUES ('p2', 'r1', 'tool_call', 'x', 'high', '{owner}', now(), 'p1')`);
  await rejects(db.query(`UPDATE core.approvals SET status = 'approved', decided_as = 'owner', decided_by = 'dev1' WHERE id = 'p2'`), /used_nonces_pkey/);
});

// ---------------------------------------------------------------- tasks (0123)
async function tasks(db) {
  await db.exec(`
    INSERT INTO core.tasks (id, root_id, title, town_id, budget_credits) VALUES ('T', 'T', 'Build the shop site', 'shop', 1000);
    INSERT INTO core.tasks (id, root_id, parent_id, title) VALUES ('T1', 'T', 'T', 'Design'), ('T2', 'T', 'T', 'Implement'), ('T3', 'T', 'T', 'Test');
    INSERT INTO core.task_deps VALUES ('T2', 'T1'), ('T3', 'T2');`);
}
const go = (db, id, state, set = '') => db.query(`UPDATE core.tasks SET state = '${state}'${set} WHERE id = '${id}'`);

await test('tasks start as draft; transitions follow the table', async () => {
  const db = await fresh();
  await rejects(db.query(`INSERT INTO core.tasks (id, root_id, title, state) VALUES ('X', 'X', 'x', 'running')`), /must start as draft/);
  await tasks(db);
  await rejects(go(db, 'T1', 'running'), /may not go from draft to running/);
});

await test('a task is ready only when its dependencies were accepted', async () => {
  const db = await fresh();
  await tasks(db);
  await rejects(go(db, 'T2', 'ready'), /depends on T1/);
  await go(db, 'T1', 'ready'); await go(db, 'T1', 'assigned', `, assignee_id = '${A1}'`);
  await go(db, 'T1', 'running'); await go(db, 'T1', 'verifying', `, verifier_id = '${A2}'`);
  await go(db, 'T1', 'accepted');
  await go(db, 'T2', 'ready');
  await go(db, 'T1', 'closed');
  eq((await one(db, `SELECT outcome FROM core.tasks WHERE id = 'T1'`)).outcome, 'accepted', 'outcome kept on close');
  await go(db, 'T3', 'cancelled');
  await rejects(go(db, 'T1', 'running'), /is closed/);
});

await test('the verifier is never the worker', async () => {
  const db = await fresh();
  await tasks(db);
  await go(db, 'T1', 'ready'); await go(db, 'T1', 'assigned', `, assignee_id = '${A1}'`); await go(db, 'T1', 'running');
  await rejects(go(db, 'T1', 'verifying', `, verifier_id = '${A1}'`), /verifier_not_worker/);
  await rejects(go(db, 'T1', 'verifying'), /no verifier/);
});

await test('rework is bounded by max_attempts', async () => {
  const db = await fresh();
  await tasks(db);
  await db.query(`UPDATE core.tasks SET max_attempts = 2 WHERE id = 'T1'`);
  await go(db, 'T1', 'ready'); await go(db, 'T1', 'assigned', `, assignee_id = '${A1}', verifier_id = '${A2}'`);
  await go(db, 'T1', 'running'); await go(db, 'T1', 'verifying'); await go(db, 'T1', 'rework');
  await go(db, 'T1', 'running'); await go(db, 'T1', 'verifying'); await go(db, 'T1', 'rework');
  await rejects(go(db, 'T1', 'running'), /attempts_bounded/);
  await go(db, 'T1', 'failed');
  await rejects(db.query(`UPDATE core.tasks SET attempts = 0 WHERE id = 'T1'`), /only with its state/);
});

await test('dependencies stay acyclic and inside one tree', async () => {
  const db = await fresh();
  await tasks(db);
  await rejects(db.query(`INSERT INTO core.task_deps VALUES ('T1', 'T3')`), /cycle/);
  await db.query(`INSERT INTO core.tasks (id, root_id, title) VALUES ('U', 'U', 'other')`);
  await rejects(db.query(`INSERT INTO core.task_deps VALUES ('T1', 'U')`), /crosses task trees/);
});

await test('superseding needs a change set, and verifying work is not superseded mid-check', async () => {
  const db = await fresh();
  await tasks(db);
  await db.query(`INSERT INTO core.change_sets (root_id, reason, proposed_by) VALUES ('T', 'spec changed', 'owner')`);
  await rejects(go(db, 'T2', 'superseded'), /superseded_needs_change_set/);
  await go(db, 'T2', 'superseded', ', superseded_by = 1');
  await go(db, 'T1', 'ready'); await go(db, 'T1', 'assigned', `, assignee_id = '${A1}', verifier_id = '${A2}'`);
  await go(db, 'T1', 'running'); await go(db, 'T1', 'verifying');
  await rejects(go(db, 'T1', 'superseded', ', superseded_by = 1'), /may not go from verifying to superseded/);
  const n = Number((await one(db, `SELECT count(*) AS n FROM core.task_events`)).n);
  eq(n, 5, 'task events recorded');
});

// ---------------------------------------------------------------- agents, machines
await test('agent names unique among live agents; retired names come free', async () => {
  const db = await fresh();
  const add = (id, name) => db.query(`INSERT INTO core.agents (id, name, role, config_hash, codex_hash, public_key) VALUES ($1, $2, 'builder', '\\x01', '\\x01', '\\x09')`, [id, name]);
  await rejects(add('01JAAAAAAAAAAAAAAAAAAAAAA9', 'vesna'), /agents_live_name/);
  await db.query(`UPDATE core.agents SET status = 'retired', retired_at = now() WHERE id = '${A1}'`);
  await add('01JAAAAAAAAAAAAAAAAAAAAAA9', 'Vesna');
  await rejects(add('not-a-ulid', 'X'), /check constraint/);
});

await test('HVF only on aarch64', async () => {
  const db = await fresh();
  await rejects(db.query(`INSERT INTO core.machines (id, name, arch, backend, class, page_size, cpus, memory_mib, disk_gib, cert_fingerprint)
                          VALUES ('m3', 'mac', 'x86_64', 'hvf', 'apple', 16384, 8, 16384, 500, '\\x03')`), /hvf_on_aarch64/);
});

await test('overcommit starts per machine class and stays under the owner cap (0133)', async () => {
  const db = await fresh();
  eq(Number((await one(db, `SELECT overcommit FROM core.machines WHERE id = 'm1'`)).overcommit), 4, 'server');
  eq(Number((await one(db, `SELECT overcommit FROM core.machines WHERE id = 'm2'`)).overcommit), 2, 'pi5');
  await db.query(`INSERT INTO core.machines (id, name, arch, backend, class, page_size, cpus, memory_mib, disk_gib, cert_fingerprint, overcommit_cap)
                  VALUES ('m4', 'pi-4', 'aarch64', 'kvm', 'pi4', 4096, 4, 4096, 60, '\\x04', 1.2)`);
  eq(Number((await one(db, `SELECT overcommit FROM core.machines WHERE id = 'm4'`)).overcommit), 1.2, 'capped');
  await rejects(db.query(`UPDATE core.machines SET overcommit = 3 WHERE id = 'm4'`), /overcommit_within_cap/);
});

await test('town lifecycle: proposed first, archived is final and retires its agents', async () => {
  const db = await fresh();
  await rejects(db.query(`INSERT INTO core.towns (id, name, state) VALUES ('lab', 'Lab', 'active')`), /must start as proposed/);
  await db.query(`INSERT INTO core.towns (id, name) VALUES ('lab', 'Lab')`);
  const add = (id) => db.query(`INSERT INTO core.agents (id, role, town_id, config_hash, codex_hash, public_key) VALUES ($1, 'researcher', 'lab', '\\x01', '\\x01', '\\x01')`, [id]);
  await rejects(add('01JAAAAAAAAAAAAAAAAAAAAAB1'), /not active/);
  await db.query(`UPDATE core.towns SET state = 'active' WHERE id = 'lab'`);
  await add('01JAAAAAAAAAAAAAAAAAAAAAB1');
  await db.query(`UPDATE core.towns SET state = 'paused' WHERE id = 'lab'`);
  await rejects(add('01JAAAAAAAAAAAAAAAAAAAAAB2'), /not active/);
  await db.query(`UPDATE core.towns SET state = 'archived' WHERE id = 'lab'`);
  eq((await one(db, `SELECT status FROM core.agents WHERE id = '01JAAAAAAAAAAAAAAAAAAAAAB1'`)).status, 'retired', 'agent retired with its town');
  await rejects(db.query(`UPDATE core.towns SET state = 'active' WHERE id = 'lab'`), /may not go from archived to active/);
});

// ---------------------------------------------------------------- audit hash chain (C-001)
await test('audit log is hash-chained and tamper-evident', async () => {
  const db = await fresh();
  for (let i = 0; i < 4; i++) {
    await db.query(`INSERT INTO core.audit (principal, action, resource, decision) VALUES ($1, 'tool.exec', 'vm1', 'allow')`, [A1]);
  }
  eq((await one(db, `SELECT core.verify_audit() AS bad`)).bad, null, 'intact chain');
  await rejects(db.query(`UPDATE core.audit SET decision = 'deny' WHERE id = 2`), /append-only/);
  // An attacker with superuser rights can switch triggers off; the chain still shows the edit.
  await db.exec(`ALTER TABLE core.audit DISABLE TRIGGER audit_append_only; UPDATE core.audit SET decision = 'deny' WHERE id = 2;`);
  eq(Number((await one(db, `SELECT core.verify_audit() AS bad`)).bad), 2, 'first broken row');
});

console.log(`\n${passed} passed, ${failures} failed`);
process.exit(failures ? 1 : 0);
