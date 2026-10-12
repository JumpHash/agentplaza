// The agent page's Harness card: the agent's limits (turns per work item, when its
// conversation is compressed), controls for its running work (compress, steer, more
// turns) and its loops.

import { h } from '../ui/dom';

export interface HarnessCard {
  el: HTMLElement;
  /** Called when the agent's record changes (status, current work). */
  update(working: boolean): void;
  destroy(): void;
}

export function harnessCard(_cid: number): HarnessCard {
  // contract: W2
  return { el: h('div'), update: () => {}, destroy: () => {} };
}
