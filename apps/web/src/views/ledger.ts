/* The ledger: the plaza's account-book. Every task the node holds as a ruled
   row — its folio number in the margin, its goal in the body, its role, its
   budget in credits and dollars, its state as a stamp or a mark, the agent
   that claimed it, and when it was entered and last changed — newest first,
   the way the register reads. A row opens its sheet below: the task's full
   facts, the note the agent left, and the owner's hand — claim it for the
   agent at a guest CID, finish it, or withdraw it before it ends.

   The node keeps no transition history, so this sheet shows the task's present
   state honestly and invents no journal. Every action is a POST or DELETE
   through the same api client the rest of the console uses, and a refusal is
   shown as the daemon's own message — verbatim, never swallowed. */

import { claimTask, createTask, finishTask, withdrawTask } from '../api';
import { announce, byId, el, toast } from '../dom';
import { fmtMoney, fmtNum, fmtStamp, pad } from '../format';
import type { Task } from '../models/task';
import { isOpenTask, seqOf } from '../models/task';
import { store } from '../store';

/* The table rebuilds only when the ledger's shape changes; the act error spans
   and the pinned row are updated in place, so focus and scroll survive. */
let ledgerSig: string | null = null;
const rowRefs = new Map<string, HTMLTableRowElement>();
let pinnedFor: string | null = null;

/** The state as the register marks a decision: a stamp for a task that has
    ended (the schema's outcome), a mark while it lives. */
function stateMark(task: Task): HTMLElement {
  switch (task.state) {
    case 'open':
      return el('span', 'mk warn', 'open');
    case 'claimed':
      return el('span', 'mk ok', 'claimed');
    case 'done':
      return el('span', 'stamp sm', 'accepted');
    case 'failed':
      return el('span', 'stamp sm deny', 'failed');
    case 'withdrawn':
      return el('span', 'stamp sm deny', 'cancelled');
  }
}

/** A `label ........ value` row for the detail sheet's ruled facts. */
function fact(dt: string, dd: string): HTMLElement {
  const row = el('div', 'lead');
  row.appendChild(el('dt', null, dt));
  row.appendChild(el('i'));
  row.appendChild(el('dd', 'm', dd));
  return row;
}

/* ------------------------------------------------------------ rendering */
export function renderLedger(): void {
  const notice = byId('ledger-notice');
  if (store.tasksError != null && store.tasks.length === 0) {
    notice.hidden = false;
    byId('ledger-notice-text').textContent =
      `The task ledger is not answering at ${location.origin} (${store.tasksError}). ` +
      'Retrying; the sheet catches up on its own.';
  } else {
    notice.hidden = true;
  }
  const live = byId('ledger-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;

  const tasks = store.tasks;
  byId('t-ledger').textContent = store.tasksError != null && tasks.length === 0 ? '—' : fmtNum(tasks.length);
  byId('t-ltasks-open').textContent = fmtNum(tasks.filter((t) => t.state === 'open').length);
  byId('t-ltasks-claimed').textContent = fmtNum(tasks.filter((t) => t.state === 'claimed').length);
  byId('t-ltasks-settled').textContent = fmtNum(tasks.filter((t) => !isOpenTask(t)).length);

  renderTable(tasks);
  renderDetail();
}

function renderTable(tasks: readonly Task[]): void {
  const has = tasks.length > 0;
  byId('ledger-empty').hidden = has || store.tasksError != null;
  byId('ledger-table').hidden = !has;
  const count = byId('ledger-count');
  count.textContent =
    store.tasksError != null
      ? 'last known'
      : fmtNum(tasks.length) + (tasks.length === 1 ? ' task' : ' tasks');
  if (!has) {
    byId('ledger-empty-text').textContent =
      'The ledger holds no tasks. Enter one above — a goal, and how its completion will be judged — ' +
      'and a live agent of this node may claim it by its guest CID.';
    ledgerSig = null;
    rowRefs.clear();
    byId('ledger-body').textContent = '';
    return;
  }

  const sig = tasks
    .map((t) => `${t.id}:${t.state}:${t.updated_at}:${t.assignee_id ?? ''}:${t.agent_cid ?? ''}`)
    .join('|');
  if (sig !== ledgerSig) {
    ledgerSig = sig;
    rowRefs.clear();
    const body = byId('ledger-body');
    body.textContent = '';
    for (const task of tasks) {
      const tr = el('tr', 'link');
      tr.tabIndex = 0;
      tr.setAttribute('data-go', `#ledger/${task.id}`);
      tr.setAttribute('aria-label', `${task.id}, ${task.state}: ${task.goal}`);
      tr.appendChild(el('td', null, pad(seqOf(task.id), 4)));
      tr.appendChild(el('td', 'goal', task.goal));
      const roleTd = el('td');
      roleTd.appendChild(task.role == null ? document.createTextNode('—') : el('span', 'chip role', task.role));
      tr.appendChild(roleTd);
      tr.appendChild(el('td', 'num', fmtMoney(task.budget)));
      const stateTd = el('td');
      stateTd.appendChild(stateMark(task));
      tr.appendChild(stateTd);
      const whoTd = el('td', 'm');
      whoTd.appendChild(document.createTextNode(task.assignee_id ?? '—'));
      if (task.agent_cid != null) whoTd.appendChild(el('span', 'chip', `cid ${task.agent_cid}`));
      tr.appendChild(whoTd);
      tr.appendChild(el('td', 'm', fmtStamp(task.created_at)));
      tr.appendChild(el('td', 'm', fmtStamp(task.updated_at)));
      body.appendChild(tr);
      rowRefs.set(task.id, tr);
    }
    announce(`${fmtNum(tasks.length)} tasks on the ledger.`);
  }
  for (const [id, tr] of rowRefs) tr.classList.toggle('sel', id === store.taskDetailId);
}

function renderDetail(): void {
  const id = store.taskDetailId;
  const task = store.taskOf(id);
  const pane = byId('ledger-detail');
  if (!task) {
    pane.hidden = true;
    return;
  }
  pane.hidden = false;
  if (pinnedFor !== task.id) {
    /* a different task's sheet starts clean — an old refusal is not this one */
    pinnedFor = task.id;
    for (const span of ['ld-claim-err', 'ld-finish-err', 'ld-withdraw-err']) byId(span).textContent = '';
  }
  byId('ld-id').textContent = task.id;
  const stateSlot = byId('ld-state');
  stateSlot.textContent = '';
  stateSlot.appendChild(stateMark(task));
  byId('ld-goal').textContent = task.goal;
  byId('ld-acceptance-line').textContent =
    task.acceptance == null ? 'no acceptance criteria were written' : `judged by: ${task.acceptance}`;

  const facts = byId('ld-facts');
  facts.textContent = '';
  facts.appendChild(fact('enter', fmtStamp(task.created_at)));
  facts.appendChild(fact('changed', fmtStamp(task.updated_at)));
  facts.appendChild(fact('state', task.state));
  facts.appendChild(fact('outcome', task.outcome ?? '—'));
  facts.appendChild(fact('role', task.role ?? '—'));
  facts.appendChild(fact('budget', fmtMoney(task.budget)));
  facts.appendChild(fact('assignee', task.assignee_id ?? '—'));
  facts.appendChild(fact('agent cid', task.agent_cid == null ? '—' : String(task.agent_cid)));
  facts.appendChild(fact('verifier', task.verifier_id ?? '—'));
  byId('ld-note-line').textContent = task.note == null ? '' : `the agent's note — ${task.note}`;
}

/* -------------------------------------------------------------- actions */
/** Run one acting call, naming the error span it fills: the daemon's own
    message on a refusal, the button held while the round trip is in flight. */
function act(button: HTMLButtonElement, errId: string, run: () => Promise<Task>): void {
  const err = byId(errId);
  err.textContent = '';
  const label = button.textContent;
  button.disabled = true;
  run()
    .then((task) => {
      store.mergeTask(task);
      store.taskDetailId = task.id;
      store.scheduleRender();
      toast(`Task ${task.id} — ${task.state}`);
      announce(`Task ${task.id} is now ${task.state}.`);
    })
    .catch((e: unknown) => {
      /* a refusal is the daemon's own words, shown verbatim — never swallowed */
      err.textContent = e instanceof Error ? e.message : String(e);
    })
    .finally(() => {
      button.disabled = false;
      button.textContent = label;
    });
}

function pinnedId(): string | null {
  return store.taskOf(store.taskDetailId) ? store.taskDetailId : null;
}

export function initLedger(): void {
  /* the create form: a goal is required; acceptance, budget and role are the
     task's own and go through the daemon's checks like anything else */
  byId('ledger-create').addEventListener('submit', (e) => {
    e.preventDefault();
    const err = byId('ledger-create-err');
    err.textContent = '';
    const goal = byId<HTMLInputElement>('lt-goal').value.trim();
    const acceptance = byId<HTMLInputElement>('lt-acceptance').value.trim();
    const role = byId<HTMLInputElement>('lt-role').value.trim();
    const budgetRaw = byId<HTMLInputElement>('lt-budget').value.trim();
    if (goal === '') {
      err.textContent = 'A task needs a goal — say what the work is for.';
      return;
    }
    const body: { goal: string; acceptance?: string; budget?: number; role?: string } = { goal };
    if (acceptance !== '') body.acceptance = acceptance;
    if (role !== '') body.role = role;
    if (budgetRaw !== '') {
      const budget = Number(budgetRaw);
      if (!Number.isInteger(budget) || budget < 0) {
        err.textContent = 'The budget is whole credits, 0 or more.';
        return;
      }
      body.budget = budget;
    }
    const btn = byId<HTMLButtonElement>('lt-enter');
    btn.disabled = true;
    btn.textContent = 'Entering…';
    createTask(body)
      .then((task) => {
        store.mergeTask(task);
        store.taskDetailId = task.id;
        byId<HTMLFormElement>('ledger-create').reset();
        toast(`Task ${task.id} entered`);
        announce(`Task ${task.id} entered.`);
        if (location.hash !== `#ledger/${task.id}`) location.hash = `#ledger/${task.id}`;
        else store.scheduleRender();
      })
      .catch((e: unknown) => {
        err.textContent = e instanceof Error ? e.message : String(e);
      })
      .finally(() => {
        btn.disabled = false;
        btn.textContent = 'Enter the task';
      });
  });

  /* claim: binds the pinned task to the live agent at the entered guest CID;
     the daemon refuses a CID no running VM holds, and says which */
  byId('ld-claim').addEventListener('submit', (e) => {
    e.preventDefault();
    const err = byId('ld-claim-err');
    err.textContent = '';
    const id = pinnedId();
    if (id == null) {
      err.textContent = 'Open a task first — pick a row above.';
      return;
    }
    const cidRaw = byId<HTMLInputElement>('ld-cid').value.trim();
    const cid = Number(cidRaw);
    if (cidRaw === '' || !Number.isInteger(cid) || cid < 0) {
      err.textContent = 'Enter the guest CID of a live agent (a whole number, 0 or more).';
      return;
    }
    act(byId<HTMLButtonElement>('ld-claim-btn'), 'ld-claim-err', () => claimTask(id, cid));
  });

  /* finish: ends the pinned task as done (accepted) or failed, with a note */
  byId('ld-finish').addEventListener('submit', (e) => {
    e.preventDefault();
    const err = byId('ld-finish-err');
    err.textContent = '';
    const id = pinnedId();
    if (id == null) {
      err.textContent = 'Open a task first — pick a row above.';
      return;
    }
    const picked = document.querySelector<HTMLInputElement>('input[name="ld-outcome"]:checked');
    const outcome = picked?.value === 'failed' ? 'failed' : 'done';
    const note = byId<HTMLInputElement>('ld-finish-note').value;
    act(byId<HTMLButtonElement>('ld-finish-btn'), 'ld-finish-err', () => finishTask(id, outcome, note));
  });

  byId('ld-withdraw').addEventListener('click', () => {
    const err = byId('ld-withdraw-err');
    err.textContent = '';
    const id = pinnedId();
    if (id == null) {
      err.textContent = 'Open a task first — pick a row above.';
      return;
    }
    act(byId<HTMLButtonElement>('ld-withdraw'), 'ld-withdraw-err', () => withdrawTask(id));
  });
}
