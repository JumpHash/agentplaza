// The top bar's master switch: STOP ALL (cancel every agent's running work and hold the plaza)
// and RESUME, with the plaza's held state as a banner.

/** The top bar control; keeps itself current from the `plaza` SSE kind and /v1/plaza/state. */
export function plazaControl(): HTMLElement {
  // contract: U2
  return document.createElement('span');
}

/** The banner shown under the top bar while the plaza is held (empty otherwise). */
export function heldBanner(): HTMLElement {
  // contract: U2
  return document.createElement('div');
}
