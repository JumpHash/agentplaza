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
  ['TwoLiveSessions', 'CREATE UNIQUE INDEX sessions_one_live ON core.sessions (run_id) WHERE fenced_at IS NULL;', ''],
  ['StaleSessionWrites', 'AND run_id = NEW.run_id AND fenced_at IS NULL) THEN', 'AND run_id = NEW.run_id) THEN'],
  ['NoFenceOnMove', "OR (OLD.state = 'stalled' AND NEW.state = 'starting')", ''],
  ['TranscriptGaps', 'IF NEW.seq <> last + 1 THEN', 'IF NEW.seq <= last THEN'],
  ['ResultWithoutAllow', "AND kind = 'tool_decision' AND decision = 'allow') THEN", "AND kind = 'tool_decision') THEN"],
  ['TwoResults', "CREATE UNIQUE INDEX transcript_one_result   ON core.transcript (run_id, call_id) WHERE kind = 'tool_result';", ''],
  ['AnyAuthorityDecides', 'CHECK (decided_as IS NULL OR decided_as = chain[position])', 'CHECK (true)'],
  ['C004AnyChain', "CHECK (NOT c004 OR chain = ARRAY['owner'])", 'CHECK (true)'],
  ['NonceReuse', 'INSERT INTO core.used_nonces (nonce, approval_id) VALUES (OLD.nonce, OLD.id);', ''],
  ['DecisionsNotFinal', "IF OLD.status <> 'pending' THEN", 'IF false THEN'],
  ['ReadyBeforeDeps', "IF NEW.state = 'ready' AND OLD.state = 'draft' THEN", 'IF false THEN'],
  ['SelfVerify', 'CHECK (verifier_id IS NULL OR verifier_id IS DISTINCT FROM assignee_id)', 'CHECK (true)'],
  ['EndlessRework', 'NEW.attempts := OLD.attempts + 1;', 'NEW.attempts := OLD.attempts;'],
  ['DepCycles', 'SELECT 1 FROM reach WHERE id = NEW.task_id) THEN', 'SELECT 1 FROM reach WHERE false) THEN'],
  ['SupersedeWhileVerifying', "('verifying', 'accepted'),", "('verifying', 'accepted'), ('verifying', 'superseded'),"],
  ['AuditUnchained', 'coalesce((SELECT hash FROM core.audit ORDER BY id DESC LIMIT 1),', 'coalesce(NULL::bytea,'],
  ['TownsResurrect', "('active', 'archived'), ('paused', 'archived')) THEN", "('active', 'archived'), ('paused', 'archived'), ('archived', 'active')) THEN"],
  ['SpawnIntoPausedTown', "(SELECT state FROM core.towns WHERE id = NEW.town_id) <> 'active' THEN", "(SELECT state FROM core.towns WHERE id = NEW.town_id) IN ('proposed', 'rejected') THEN"],
  ['UnchainedTranscript', 'IF NEW.prev_hash <> coalesce(', 'IF false AND NEW.prev_hash <> coalesce('],
  ['IgnoreOvercommitCap', 'CHECK (overcommit_cap IS NULL OR overcommit <= overcommit_cap)', 'CHECK (true)'],
  ['RunsResurrect', "('stalled', 'running'),", "('stalled', 'running'), ('done', 'running'),"],
];
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'core-mutants-'));
let bad = 0;
for (const [name, from, to] of MUTANTS) {
  const n = schema.split(from).length - 1;
  if (n !== 1) { console.log(`FAIL  mutant ${name}: pattern found ${n} times`); bad++; continue; }
  const file = path.join(dir, `${name}.sql`);
  fs.writeFileSync(file, schema.replace(from, to));
  const r = spawnSync(process.execPath, [path.join(here, 'core.test.mjs')], { env: { ...process.env, SCHEMA: file }, encoding: 'utf8' });
  const killedBy = (r.stdout.match(/^FAIL  (.*?): /m) || [])[1];
  if (r.status !== 0 && killedBy) console.log(`ok    mutant ${name} caught by "${killedBy}"`);
  else { console.log(`FAIL  mutant ${name} survived`); bad++; }
}
fs.rmSync(dir, { recursive: true });
process.exit(bad ? 1 : 0);
