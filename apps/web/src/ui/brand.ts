// The AgentPlaza brand: its mark (an inline SVG, drawn in the console's own palette) and the
// sidebar's brand block.

import { h } from './dom';

const SVG_NS = 'http://www.w3.org/2000/svg';

/** An SVG element with the given attributes (set one by one; no innerHTML). */
function svgEl(tag: string, attrs: Record<string, string>): SVGElement {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [name, value] of Object.entries(attrs)) el.setAttribute(name, value);
  return el;
}

/**
 * The mark: a square plaza seen from above. An ink square, four spokes from its edges
 * to a ring at the centre, and a signal-coloured core. Ink is currentColor, so the mark
 * takes the theme's ink; the core is coloured by the .brand-mark-core rule.
 */
export function brandMark(size = 32): SVGSVGElement {
  const svg = svgEl('svg', {
    viewBox: '0 0 32 32',
    width: String(size),
    height: String(size),
    class: 'brand-mark',
    'aria-hidden': 'true',
    focusable: 'false',
  }) as SVGSVGElement;
  const ink = svgEl('g', { fill: 'none', stroke: 'currentColor', 'stroke-width': '2.5', 'stroke-linecap': 'square' });
  ink.appendChild(svgEl('rect', { x: '3', y: '3', width: '26', height: '26' }));
  ink.appendChild(svgEl('path', { d: 'M16 3V10M16 29V22M3 16H10M29 16H22' }));
  ink.appendChild(svgEl('circle', { cx: '16', cy: '16', r: '6' }));
  svg.appendChild(ink);
  svg.appendChild(svgEl('circle', { class: 'brand-mark-core', cx: '16', cy: '16', r: '2.5' }));
  return svg;
}

/** The sidebar's brand block: the mark, then the name and the tagline in a column. */
export function brandBlock(): HTMLElement {
  return h(
    'div.brand',
    null,
    brandMark(32),
    h('div.brand-text', null, h('span.brand-name', null, 'AgentPlaza'), h('small.brand-tag', null, 'the plaza register')),
  );
}
