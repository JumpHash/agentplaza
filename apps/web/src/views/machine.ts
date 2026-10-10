/* The machine screen: what the node reports about itself, set the way the
   plaza schema would — identity, virtualisation, register. */

import { byId } from '../dom';
import { fmtDur, fmtNum, fmtTime } from '../format';
import { isOpen } from '../models/run';
import { store } from '../store';

export function renderMachine(): void {
  const m = store.machine;
  const notice = byId('mc-notice');
  if (store.machineError != null && !m) {
    notice.hidden = false;
    byId('mc-notice-text').textContent =
      `The node daemon is not answering at ${location.origin} (${store.machineError}). ` +
      'Retrying; the page catches up on its own.';
  } else {
    notice.hidden = true;
  }

  const live = byId('mc-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;

  byId('mc-cpus').textContent = m ? fmtNum(m.cpus) : '—';
  byId('mc-mem').textContent = m ? fmtNum(m.memory_mib) : '—';
  byId('mc-up').textContent = m ? fmtDur(Date.now() - m.started_at_ms) : '—';

  byId('mc-name').textContent = m?.name ?? '—';
  byId('mc-class').textContent = m?.class ?? '—';
  byId('mc-arch').textContent = m?.arch ?? '—';
  byId('mc-backend').textContent = m?.backend ?? '—';
  byId('mc-irqchip').textContent = m?.irqchip ?? '—';
  byId('mc-vsock').textContent = m?.vsock_mode ?? '—';
  byId('mc-started').textContent = m ? `${fmtTime(m.started_at_ms)} · ${new Date(m.started_at_ms).toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })}` : '—';
  byId('mc-runs').textContent = store.runsError != null ? '—' : fmtNum(store.runs.length);
  byId('mc-live-runs').textContent = store.runsError != null
    ? '—'
    : fmtNum(store.runs.filter((r) => isOpen(r)).length);
}
