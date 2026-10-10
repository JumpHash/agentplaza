/* The transcript row, shared by the run entry and the comparison panes:
   the sequence in the margin, the kind in its narrow column, the line as
   text, and marginalia for exits and completed sessions. Guest text is
   untrusted — it only ever lands as textContent. */

import { el } from '../dom';
import { pad } from '../format';
import { isExitLine, isSessionMark, kindOf } from '../serial';

export function txRow(i: number, line: string, cls?: string): HTMLElement {
  const row = el('div', 'row' + (cls ? ' ' + cls : ''));
  row.appendChild(el('span', 'seq', i >= 0 ? pad(i + 1, 4) : ''));
  row.appendChild(el('span', 'kind', cls === 'elide' ? '' : kindOf(line)));
  row.appendChild(el('pre', null, line));
  const marg = el('span', 'marg');
  if (isExitLine(line)) marg.appendChild(el('span', 'mk warn', 'exit'));
  else if (isSessionMark(line)) marg.appendChild(el('span', 'mk ok', 'session'));
  row.appendChild(marg);
  return row;
}
