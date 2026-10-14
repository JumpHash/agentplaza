// The IDE's terminal host: the one element that holds the workspace's terminal strip, placed in the centre's Terminal view
// or in the drawer. It is made the first time it is wanted, in an agent's or a workstation's workspace (a repository has no
// terminal), and it keeps running when it moves.
import { hasTerminal } from '../ide/layout';
import { cidOf, type IdeCtx } from './ide-ctx';
import { terminalTabs } from './ide-terminal';

export function disposeTerminal(c: IdeCtx): void {
  c.term?.dispose();
  c.term = null;
  c.dom.termHost.remove();
}

/**
 * Puts the terminal where it belongs: in the centre's Terminal view while that view shows, else in the drawer while the
 * drawer shows the Terminal tab, else nowhere (detached, still alive). A move that is not needed is not made, so a redraw
 * does not move focus or the xterm DOM.
 */
export function renderTerminal(c: IdeCtx): void {
  const { termHost } = c.dom;
  const termWs = c.ws !== null && hasTerminal(c.ws.kind, cidOf(c.ws.id) !== null);
  const inCentre = termWs && c.centreView === 'terminal';
  const inDrawer = termWs && !inCentre && c.drawerOpen && c.drawerTab === 'terminal';
  const home = inCentre ? c.dom.frame.centre.views.terminal : inDrawer ? c.dom.frame.drawer.body : null;
  if (home) {
    if (termHost.parentElement !== home) home.append(termHost);
  } else {
    termHost.remove();
  }
  if (c.ws && home && !c.term && termWs) c.term = terminalTabs(termHost, c.ws.id);
}
