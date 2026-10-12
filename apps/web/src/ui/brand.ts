// The AgentPlaza brand: the hive mark (an inline SVG) and the sidebar's brand block.

import { h } from './dom';

const SVG_NS = 'http://www.w3.org/2000/svg';

/** An SVG element with the given attributes (set one by one; no innerHTML). */
function svgEl(tag: string, attrs: Record<string, string>): SVGElement {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [name, value] of Object.entries(attrs)) el.setAttribute(name, value);
  return el;
}

/** A flat-top hexagon's outline path, centred at (cx, cy) with circumradius r. */
function hex(cx: number, cy: number, r: number): string {
  const pts = [0, 60, 120, 180, 240, 300].map((deg) => {
    const a = (deg * Math.PI) / 180;
    return `${(cx + r * Math.cos(a)).toFixed(2)} ${(cy + r * Math.sin(a)).toFixed(2)}`;
  });
  return `M${pts.join('L')}Z`;
}

/** The hive's geometry: cell circumradius, the gap between cells, and the centre cell. */
const CELL_R = 5.6;
const CELL_GAP = 1.2;
const CENTRE = { x: 16, y: 18.75 };

/**
 * The mark: a hive. Agents are cells that work as one — three ink cells (one above, two
 * below) around the plaza's heart, the filled signal-coloured cell at the centre. Flat-top
 * hexagons on a true honeycomb (neighbours √3·r + gap apart), so the gaps are equal and the
 * mark stays crisp at 20–64px. Ink is currentColor; the centre is coloured by `.brand-core`
 * (a presentation attribute cannot take var()).
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
  const d = Math.sqrt(3) * CELL_R + CELL_GAP;
  // Neighbours of a flat-top cell lie straight up and at ±30° below the horizontal.
  const ring = [
    { x: CENTRE.x, y: CENTRE.y - d },
    { x: CENTRE.x - d * Math.cos(Math.PI / 6), y: CENTRE.y + d * Math.sin(Math.PI / 6) },
    { x: CENTRE.x + d * Math.cos(Math.PI / 6), y: CENTRE.y + d * Math.sin(Math.PI / 6) },
  ];
  // Thinner strokes at small sizes would vanish; thicker at large sizes look clumsy.
  const stroke = size <= 24 ? '2.4' : '2.1';
  for (const c of ring) {
    svg.appendChild(
      svgEl('path', {
        d: hex(c.x, c.y, CELL_R - 0.4),
        fill: 'none',
        stroke: 'currentColor',
        'stroke-width': stroke,
        'stroke-linejoin': 'miter',
      }),
    );
  }
  svg.appendChild(svgEl('path', { class: 'brand-core', d: hex(CENTRE.x, CENTRE.y, CELL_R) }));
  return svg;
}

/** The sidebar's brand block: a link home (the mark, then the wordmark and the tagline). */
export function brandBlock(): HTMLElement {
  return h(
    'a.brand',
    { href: '#/overview' },
    brandMark(34),
    h(
      'div.brand-text',
      null,
      h('div.brand-name', null, h('span.brand-agent', null, 'agent'), h('span.brand-plaza', null, 'plaza')),
      h('small.brand-tag', null, 'the plaza register'),
    ),
  );
}
