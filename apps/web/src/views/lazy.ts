// A screen whose code loads on its first visit: the module is fetched once (its own chunk, see build.mjs), then the
// screen mounts like any other. Until it arrives the root says so; a failed load says why.

import { h, replace } from '../ui/dom';
import type { Cleanup, Mount } from './index';

export function lazyMount<M>(load: () => Promise<M>, pick: (m: M) => Mount): Mount {
  return (root, params) => {
    let gone = false;
    let cleanup: Cleanup | undefined;
    replace(root, h('p.lazy-note', null, 'Loading…'));
    load().then(
      (m) => {
        if (gone) return;
        root.replaceChildren();
        cleanup = pick(m)(root, params);
      },
      (err: unknown) => {
        if (!gone) replace(root, h('p.lazy-note', null, `This screen did not load: ${err instanceof Error ? err.message : String(err)}`));
      },
    );
    return () => {
      gone = true;
      cleanup?.();
    };
  };
}
