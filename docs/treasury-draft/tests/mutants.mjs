// Schema mutants: deliberately broken copies of schema.sql. Each must make at least one behaviour test fail.
// Each mutation must apply exactly once, so a mutant can't silently be a no-op.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const schema = fs.readFileSync(path.join(here, '..', 'schema.sql'), 'utf8');
const MUTANTS = [
  ['NoMarginCap', "IF e.kind = 'payment' THEN", 'IF false THEN'],
  ['SubcontractCountsAsCost', "WHEN e.kind = 'spend' AND p.amount > 0", "WHEN e.kind IN ('spend', 'subcontract') AND p.amount > 0"],
  ['FineFromEscrow', "('fine',        'agent_earnings', 'debit'),", "('fine',        'agent_earnings', 'debit'), ('fine', 'task_escrow', 'debit'),"],
  ['EarningsUnlimited', "AND a.kind IN ('task_escrow', 'agent_earnings', 'treasury', 'town_budget')", "AND a.kind IN ('task_escrow', 'treasury', 'town_budget')"],
  ['PostingsEditable', 'CREATE TRIGGER postings_append_only BEFORE UPDATE OR DELETE ON treasury.postings\n  FOR EACH ROW EXECUTE FUNCTION treasury.forbid_change();', ''],
  ['ClawbackNotNetted', "WHEN e.kind IN ('payment', 'clawback') AND a.kind = 'agent_earnings'", "WHEN e.kind = 'payment' AND a.kind = 'agent_earnings'"],
  ['UninvitedBids', 'IF NOT NEW.bidder = ANY (c.invited) THEN', 'IF false THEN'],
  ['TownBudgetOverdraw', "AND a.kind IN ('task_escrow', 'agent_earnings', 'treasury', 'town_budget')", "AND a.kind IN ('task_escrow', 'agent_earnings', 'treasury')"],
  ['AgentsMayPost', "CHECK (posted_by IN ('treasury-service',", "CHECK (posted_by LIKE '%' OR posted_by IN ('treasury-service',"],
];
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ledger-mutants-'));
let bad = 0;
for (const [name, from, to] of MUTANTS) {
  const n = schema.split(from).length - 1;
  if (n !== 1) { console.log(`FAIL  mutant ${name}: pattern found ${n} times`); bad++; continue; }
  const file = path.join(dir, `${name}.sql`);
  fs.writeFileSync(file, schema.replace(from, to));
  const r = spawnSync(process.execPath, [path.join(here, 'ledger.test.mjs')], { env: { ...process.env, SCHEMA: file }, encoding: 'utf8' });
  const killedBy = (r.stdout.match(/^FAIL  (.*?): /m) || [])[1];
  if (r.status !== 0 && killedBy) console.log(`ok    mutant ${name} caught by "${killedBy}"`);
  else { console.log(`FAIL  mutant ${name} survived`); bad++; }
}
fs.rmSync(dir, { recursive: true });
process.exit(bad ? 1 : 0);
