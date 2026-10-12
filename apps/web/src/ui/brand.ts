// The AgentPlaza brand: its mark (an inline SVG) and the sidebar's brand block.

import { h } from './dom';

const SVG_NS = 'http://www.w3.org/2000/svg';

/** An SVG element with the given attributes (set one by one; no innerHTML). */
function svgEl(tag: string, attrs: Record<string, string>): SVGElement {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [name, value] of Object.entries(attrs)) el.setAttribute(name, value);
  return el;
}

/**
 * The mark: a plaza seen from above. An outer ring, eight radial spokes running in to the
 * centre, four agents in their rooms on the ring, and the xenon atom at the heart: a
 * signal-coloured core with its orbit. Ink is currentColor; the signal parts are coloured
 * by the .brand-core and .brand-orbit rules (presentation attributes cannot take var()).
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
  svg.appendChild(
    svgEl('circle', { cx: '16', cy: '16', r: '13', fill: 'none', stroke: 'currentColor', 'stroke-width': '2.25' }),
  );
  svg.appendChild(
    svgEl('path', {
      d: 'M29 16H23.5M25.25 25.25L21.25 21.25M16 29V23.5M6.75 25.25L10.75 21.25M3 16H8.5M6.75 6.75L10.75 10.75M16 3V8.5M25.25 6.75L21.25 10.75',
      fill: 'none',
      stroke: 'currentColor',
      'stroke-width': '1.75',
      'stroke-linecap': 'square',
    }),
  );
  for (const [x, y] of [
    [24, 24],
    [5.5, 24],
    [5.5, 5.5],
    [24, 5.5],
  ]) {
    svg.appendChild(svgEl('rect', { x: String(x), y: String(y), width: '2.6', height: '2.6', fill: 'currentColor' }));
  }
  svg.appendChild(
    svgEl('circle', {
      class: 'brand-orbit',
      cx: '16',
      cy: '16',
      r: '5.6',
      fill: 'none',
      'stroke-width': '1.25',
      'stroke-dasharray': '2 2.4',
    }),
  );
  svg.appendChild(svgEl('circle', { class: 'brand-core', cx: '16', cy: '16', r: '3.6' }));
  return svg;
}

/** The sidebar's brand block: a link home (the mark, then the wordmark and the tagline). */
export function brandBlock(): HTMLElement {
  return h(
    'a.brand',
    { href: '#/overview' },
    brandMark(30),
    h(
      'div.brand-text',
      null,
      h('div.brand-name', null, h('span.brand-agent', null, 'Agent'), h('span.brand-plaza', null, 'Plaza')),
      h('small.brand-tag', null, 'the plaza register'),
    ),
  );
}
