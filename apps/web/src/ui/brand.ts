// The AgentPlaza brand: its mark (an inline SVG, drawn in the console's own palette) and the
// sidebar's brand block.

import { h } from './dom';

/** The mark at `size` px (square). */
export function brandMark(_size = 28): SVGSVGElement {
  // contract: U1
  return document.createElementNS('http://www.w3.org/2000/svg', 'svg');
}

/** The sidebar's brand block: the mark, the name and the tagline. */
export function brandBlock(): HTMLElement {
  // contract: U1
  return h('div.brand', null, brandMark(), 'AgentPlaza', h('small', null, 'the plaza register'));
}
