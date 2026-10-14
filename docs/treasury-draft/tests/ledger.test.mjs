// Behaviour tests for treasury-draft/schema.sql on a real PostgreSQL 16 engine (PGlite, in-process).
// Run: ../check.sh   (needs Node 22 and @electric-sql/pglite; see ../README.md)
import { PGlite } from '@electric-sql/pglite';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const schema = fs.readFileSync(process.env.SCHEMA ?? path.join(here, '..', 'schema.sql'), 'utf8');
let failures = 0, passed = 0;

async function fresh() {
  const db = new PGlite();
  await db.exec(schema);
  await db.exec(`
    INSERT INTO treasury.accounts (kind, owner_ref) VALUES
      ('mint', 'mint'), ('treasury', 'treasury'), ('external_spend', 'providers'),
      ('task_escrow', 'root'), ('task_escrow', 'sub1'), ('task_escrow', 'sub2');
    INSERT INTO treasury.accounts (kind, owner_ref) VALUES ('town_budget', 'shop'), ('town_budget', 'lab');
    INSERT INTO treasury.accounts (kind, owner_ref, credit_limit) VALUES
      ('agent_earnings', 'alice', 0), ('agent_earnings', 'bob', 50);`);
  return db;
}

// Post one entry: postings is [[kind, owner_ref, amount], ...]. Returns the entry id.
async function post(db, kind, root, task, postings) {
  await db.exec('BEGIN');
  try {
    const { rows } = await db.query(
      `INSERT INTO treasury.entries (kind, task_id, root_task, posted_by) VALUES ($1, $2, $3, 'treasury-service') RETURNING id`,
      [kind, task, root]);
    const id = rows[0].id;
    for (const [k, ref, amount] of postings) {
      await db.query(
        `INSERT INTO treasury.postings (entry_id, account_id, amount)
         SELECT $1, id, $4 FROM treasury.accounts WHERE kind = $2 AND owner_ref = $3`, [id, k, ref, amount]);
    }
    await db.exec('COMMIT');
    return id;
  } catch (e) {
    await db.exec('ROLLBACK').catch(() => {});
    throw e;
  }
}

const bal = async (db, kind, ref) =>
  Number((await db.query(`SELECT balance FROM treasury.balances WHERE kind = $1 AND owner_ref = $2`, [kind, ref])).rows[0].balance);

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

// The worked example from design/treasury-agora.md (not in the repo) ("Settlement"): root budget 1000, one subcontract of 400.
async function happyPath(db) {
  await post(db, 'mint', 'root', 'root', [['mint', 'mint', -1000], ['task_escrow', 'root', 1000]]);
  await post(db, 'spend', 'root', 'root', [['task_escrow', 'root', -200], ['external_spend', 'providers', 200]]);
  await post(db, 'subcontract', 'root', 'sub1', [['task_escrow', 'root', -400], ['task_escrow', 'sub1', 400]]);
  await post(db, 'spend', 'root', 'sub1', [['task_escrow', 'sub1', -300], ['external_spend', 'providers', 300]]);
  // margin: at most price - cost (100) and at most 20% of chain cost (0.2 × 500 = 100)
  await post(db, 'payment', 'root', 'sub1', [['task_escrow', 'sub1', -100], ['agent_earnings', 'alice', 100]]);
}

await test('schema loads on PostgreSQL 16', async () => {
  const db = await fresh();
  const v = (await db.query('SHOW server_version')).rows[0].server_version;
  if (!v.startsWith('16')) throw new Error(`server is ${v}`);
});

await test('balanced entries post; balances and chain_margin add up', async () => {
  const db = await fresh();
  await happyPath(db);
  await post(db, 'refund', 'root', 'root', [['task_escrow', 'root', -400], ['mint', 'mint', 400]]);
  eq(await bal(db, 'task_escrow', 'root'), 0, 'root escrow');
  eq(await bal(db, 'task_escrow', 'sub1'), 0, 'sub1 escrow');
  eq(await bal(db, 'agent_earnings', 'alice'), 100, 'alice');
  eq(await bal(db, 'external_spend', 'providers'), 500, 'external spend');
  eq(await bal(db, 'mint', 'mint'), -600, 'mint');
  const total = Number((await db.query('SELECT sum(balance) AS s FROM treasury.balances')).rows[0].s);
  eq(total, 0, 'sum of all balances');
  const m = (await db.query(`SELECT * FROM treasury.chain_margin WHERE root_task = 'root'`)).rows[0];
  eq(Number(m.real_cost), 500, 'chain real cost');
  eq(Number(m.paid_to_agents), 100, 'chain margin');
});

await test('an unbalanced entry is rejected at commit', async () => {
  const db = await fresh();
  await rejects(post(db, 'mint', 'root', 'root', [['mint', 'mint', -1000], ['task_escrow', 'root', 999]]), /does not balance/);
  eq(Number((await db.query('SELECT count(*) AS n FROM treasury.entries')).rows[0].n), 0, 'entries after rollback');
});

await test('escrow cannot overdraw', async () => {
  const db = await fresh();
  await post(db, 'mint', 'root', 'root', [['mint', 'mint', -100], ['task_escrow', 'root', 100]]);
  await rejects(post(db, 'spend', 'root', 'root', [['task_escrow', 'root', -101], ['external_spend', 'providers', 101]]), /below its limit/);
});

await test('earnings may go negative only down to the credit line', async () => {
  const db = await fresh();
  await post(db, 'fine', null, null, [['agent_earnings', 'bob', -50], ['treasury', 'treasury', 50]]);
  eq(await bal(db, 'agent_earnings', 'bob'), -50, 'bob after fine within credit line');
  await rejects(post(db, 'fine', null, null, [['agent_earnings', 'bob', -1], ['treasury', 'treasury', 1]]), /below its limit/);
  await rejects(post(db, 'fine', null, null, [['agent_earnings', 'alice', -1], ['treasury', 'treasury', 1]]), /below its limit/);
});

await test('mint and external spend may go negative/positive freely', async () => {
  const db = await fresh();
  await post(db, 'mint', 'root', 'root', [['mint', 'mint', -1000000], ['task_escrow', 'root', 1000000]]);
  eq(await bal(db, 'mint', 'mint'), -1000000, 'mint');
});

await test('journal is append-only (update and delete refused)', async () => {
  const db = await fresh();
  const id = await post(db, 'mint', 'root', 'root', [['mint', 'mint', -10], ['task_escrow', 'root', 10]]);
  await rejects(db.query(`UPDATE treasury.postings SET amount = 20 WHERE entry_id = $1 AND amount = 10`, [id]), /append-only/);
  await rejects(db.query(`DELETE FROM treasury.postings WHERE entry_id = $1`, [id]), /append-only/);
  await rejects(db.query(`UPDATE treasury.entries SET memo = 'x' WHERE id = $1`, [id]), /append-only/);
  await rejects(db.query(`DELETE FROM treasury.entries WHERE id = $1`, [id]), /append-only/);
});

await test('a zero posting is refused', async () => {
  const db = await fresh();
  await rejects(post(db, 'mint', 'root', 'root', [['mint', 'mint', 0]]), /check constraint/);
});

await test('fines come from earnings only, never escrow', async () => {
  const db = await fresh();
  await post(db, 'mint', 'root', 'root', [['mint', 'mint', -100], ['task_escrow', 'root', 100]]);
  await rejects(post(db, 'fine', null, 'root', [['task_escrow', 'root', -10], ['treasury', 'treasury', 10]]), /fine entry may not debit a task_escrow account/);
});

await test('payment is refused when it would break the 20% chain margin cap', async () => {
  const db = await fresh();
  await post(db, 'mint', 'root', 'root', [['mint', 'mint', -1000], ['task_escrow', 'root', 1000]]);
  await post(db, 'subcontract', 'root', 'sub1', [['task_escrow', 'root', -400], ['task_escrow', 'sub1', 400]]);
  await post(db, 'spend', 'root', 'sub1', [['task_escrow', 'sub1', -100], ['external_spend', 'providers', 100]]);
  await rejects(post(db, 'payment', 'root', 'sub1', [['task_escrow', 'sub1', -21], ['agent_earnings', 'alice', 21]]), /margin cap/);
  await post(db, 'payment', 'root', 'sub1', [['task_escrow', 'sub1', -20], ['agent_earnings', 'alice', 20]]);
});

await test('subcontract transfers do not count as real cost (no margin stacking)', async () => {
  const db = await fresh();
  await post(db, 'mint', 'root', 'root', [['mint', 'mint', -1000], ['task_escrow', 'root', 1000]]);
  await post(db, 'subcontract', 'root', 'sub1', [['task_escrow', 'root', -800], ['task_escrow', 'sub1', 800]]);
  await post(db, 'subcontract', 'root', 'sub2', [['task_escrow', 'sub1', -600], ['task_escrow', 'sub2', 600]]);
  await post(db, 'spend', 'root', 'sub2', [['task_escrow', 'sub2', -100], ['external_spend', 'providers', 100]]);
  await post(db, 'payment', 'root', 'sub2', [['task_escrow', 'sub2', -20], ['agent_earnings', 'bob', 20]]);
  // bob already took the chain's whole 20%; alice's hop gets nothing
  await rejects(post(db, 'payment', 'root', 'sub1', [['task_escrow', 'sub1', -1], ['agent_earnings', 'alice', 1]]), /margin cap/);
});

await test('only the treasury service posts', async () => {
  const db = await fresh();
  await rejects(db.query(`INSERT INTO treasury.entries (kind, posted_by) VALUES ('mint', 'agent:01JAGENT')`), /posted_by_service/);
  await rejects(db.query(`INSERT INTO treasury.entries (kind, posted_by) VALUES ('spend', 'treasury-service')`), /task_money_names_chain/);
});

await test('a clawback within the credit line restores escrow', async () => {
  const db = await fresh();
  await happyPath(db);
  await post(db, 'clawback', 'root', 'sub1', [['agent_earnings', 'alice', -100], ['task_escrow', 'root', 100]]);
  eq(await bal(db, 'agent_earnings', 'alice'), 0, 'alice after clawback');
  const m = (await db.query(`SELECT * FROM treasury.chain_margin WHERE root_task = 'root'`)).rows[0];
  eq(Number(m.paid_to_agents), 0, 'chain margin net of clawback');
});

await test('Agora calls invite 1 to 3 candidates; bids and awards are checked', async () => {
  const db = await fresh();
  const q = (inv) => db.query(
    `INSERT INTO agora.calls (task_id, requester, capability, spec_ref, max_price, deadline, invited)
     VALUES ('sub1', 'mayor', 'rust', 'art:1', 500, now() + interval '1 day', $1) RETURNING id`, [inv]);
  await rejects(q([]), /check constraint/);
  await rejects(q(['a', 'b', 'c', 'd']), /check constraint/);
  const id = (await q(['alice', 'bob'])).rows[0].id;
  await db.query(`INSERT INTO agora.bids (call_id, bidder, price, eta, confidence) VALUES ($1, 'alice', 400, '2 hours', 0.8)`, [id]);
  await rejects(db.query(`INSERT INTO agora.bids (call_id, bidder, price, eta, confidence) VALUES ($1, 'carol', 300, '2 hours', 0.9)`, [id]), /not invited/);
  await rejects(db.query(`INSERT INTO agora.bids (call_id, bidder, price, eta, confidence) VALUES ($1, 'bob', 600, '2 hours', 0.9)`, [id]), /max_price/);
  await db.query(`INSERT INTO agora.awards (call_id, contractor, price, warranty_until) VALUES ($1, 'alice', 400, now() + interval '7 days')`, [id]);
  await rejects(db.query(`INSERT INTO agora.awards (call_id, contractor, price, warranty_until) VALUES ($1, 'alice', 400, now())`, [id]), /duplicate key/);
  const id2 = (await q(['alice'])).rows[0].id;
  await rejects(db.query(`INSERT INTO agora.awards (call_id, contractor, price, warranty_until) VALUES ($1, 'alice', 400, now())`, [id2]), /matches no bid/);
});

await test('town budgets: funded by the mint, fund root tasks, take refunds, move between towns (0137)', async () => {
  const db = await fresh();
  await post(db, 'mint', 'shop-oct', null, [['mint', 'mint', -400000], ['town_budget', 'shop', 400000]]);
  await post(db, 'mint', 'lab-oct', null, [['mint', 'mint', -150000], ['town_budget', 'lab', 150000]]);
  await post(db, 'subcontract', 'root', 'root', [['town_budget', 'shop', -1000], ['task_escrow', 'root', 1000]]);
  await post(db, 'refund', 'root', 'root', [['task_escrow', 'root', -600], ['town_budget', 'shop', 600]]);
  await post(db, 'budget_move', 'move-1', null, [['town_budget', 'lab', -60000], ['town_budget', 'shop', 60000]]);
  eq(await bal(db, 'town_budget', 'shop'), 459600, 'shop budget');
  eq(await bal(db, 'town_budget', 'lab'), 90000, 'lab budget');
  await rejects(post(db, 'budget_move', 'move-2', null, [['town_budget', 'lab', -90001], ['town_budget', 'shop', 90001]]), /below its limit/);
  await rejects(post(db, 'budget_move', 'move-3', null, [['town_budget', 'lab', -10], ['agent_earnings', 'alice', 10]]), /budget_move entry may not credit a agent_earnings/);
});

console.log(`\n${passed} passed, ${failures} failed`);
process.exit(failures ? 1 : 0);
