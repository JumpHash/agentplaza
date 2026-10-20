#!/usr/bin/env node
// Generates the README artwork: animated SVGs that GitHub renders inside <img>.
//
//   node scripts/readme/gen-art.mjs [--fonts DIR] [--out DIR]
//
//   --fonts  folder with geist-latin.woff2, plex-mono-500-latin.woff2 and LICENSE.md
//            (default: $APZ_FONTS, else assets/fonts of the agentplaza.io site checked out beside this repository)
//   --out    output folder (default: docs/assets/readme of this repo)
//
// Plain Node 18+, no npm packages. Each file is self-contained: no scripts, no external
// fetches. Fonts are embedded as base64 WOFF2. Motion is CSS @keyframes on transform and
// opacity only, and stops under prefers-reduced-motion.

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const argv = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : fallback;
};
const FONTS = resolve(flag('--fonts', process.env.APZ_FONTS || join(REPO, '../agentplaza-landing/assets/fonts')));
const OUT = resolve(flag('--out', join(REPO, 'docs', 'assets', 'readme')));

// ---------------------------------------------------------------- fonts and licence

const b64 = (file) => readFileSync(join(FONTS, file)).toString('base64');
const FONT_CSS = [
  `@font-face{font-family:'Geist';font-style:normal;font-weight:100 900;font-display:block;src:url(data:font/woff2;base64,${b64('geist-latin.woff2')}) format('woff2')}`,
  `@font-face{font-family:'Plex Mono';font-style:normal;font-weight:500;font-display:block;src:url(data:font/woff2;base64,${b64('plex-mono-500-latin.woff2')}) format('woff2')}`,
].join('\n');
const LICENCE = readFileSync(join(FONTS, 'LICENSE.md'), 'utf8')
  .replace(/^#.*\n/, '')
  .replace(/-{2,}/g, '-')
  .trim();

// ---------------------------------------------------------------- palette and shared shapes

const SIG = '#e8590c'; // the signal orange, both themes
const PAL = {
  dark: {
    ground: '#121613', panel: '#1a1f1b', ink: '#e8ece6', ink2: '#a9b0a7',
    rule: '#2c332d', line: '#3b443c', grid: '#1c221d', shadow: '#2c332d',
    red: '#b3263a', redShadow: '#5e1220',
  },
  light: {
    ground: '#eef0ea', panel: '#f6f7f3', ink: '#151915', ink2: '#4b524a',
    rule: '#c8cdc4', line: '#c4c9bf', grid: '#e0e4db', shadow: '#151915',
    red: '#a3182c', redShadow: '#6e1020',
  },
};

const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const f2 = (n) => +n.toFixed(2);
const shade = (hex, k) => {
  const n = parseInt(hex.slice(1), 16);
  const c = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => Math.round(v * k));
  return `rgb(${c.join(' ')})`;
};

const HIVE = {
  outline: [
    'M21.20 7.85L18.60 12.35L13.40 12.35L10.80 7.85L13.40 3.35L18.60 3.35Z',
    'M11.76 24.20L9.16 28.70L3.96 28.70L1.36 24.20L3.96 19.70L9.16 19.70Z',
    'M30.64 24.20L28.04 28.70L22.84 28.70L20.24 24.20L22.84 19.70L28.04 19.70Z',
  ],
  core: 'M21.60 18.75L18.80 23.60L13.20 23.60L10.40 18.75L13.20 13.90L18.80 13.90Z',
};

// The AgentPlaza TV robot (the crowd drawing), flattened: its CSS classes become literal
// fills, and each robot takes its accent. Blink and the waiting glow are the only motion.
function robot({ id, x, y, s, accent, wait = false }) {
  const ear = shade(accent, 0.9);
  const halo = wait ? `<circle class="halo-w" cx="50" cy="7" r="9" fill="${SIG}"/>` : '';
  const waitTip = wait ? `<circle class="wait" cx="50" cy="7" r="4.4" fill="${SIG}"/>` : '';
  const edge = 'stroke="rgba(70,58,36,0.14)" stroke-width="0.6"';
  return `<g class="bot ${id}" transform="translate(${x} ${y}) scale(${s})">
<g class="ant"><line x1="50" y1="23" x2="50" y2="9" stroke="#2b2f38" stroke-width="2.2" stroke-linecap="round"/>${halo}<circle cx="50" cy="7" r="4.4" fill="${accent}"/>${waitTip}</g>
<rect x="4" y="39" width="9" height="17" rx="4.5" fill="${ear}"/><rect x="87" y="39" width="9" height="17" rx="4.5" fill="${ear}"/>
<rect x="10" y="20" width="80" height="57" rx="22" fill="url(#shell)" ${edge}/>
<rect x="19" y="30" width="62" height="37" rx="14" fill="url(#visor)" stroke="rgba(255,255,255,0.08)" stroke-width="0.6"/>
<rect x="23" y="34" width="54" height="29" rx="10" fill="#0b0d12"/>
<g class="eyes"><rect x="29" y="44" width="15" height="7" rx="3.5" fill="${accent}"/><rect x="32" y="45.5" width="9" height="4" rx="2" fill="#fff" opacity="0.55"/><rect x="56" y="44" width="15" height="7" rx="3.5" fill="${accent}"/><rect x="59" y="45.5" width="9" height="4" rx="2" fill="#fff" opacity="0.55"/></g>
<path d="M63 32.6h8.5q4.5 0 5.8 5.2" fill="none" stroke="#fff" stroke-width="1.4" stroke-linecap="round" opacity="0.55"/>
<path d="M25 32.6h16" fill="none" stroke="#fff" stroke-width="1.6" stroke-linecap="round" opacity="0.16"/>
<rect x="44" y="76" width="12" height="8" rx="3" fill="#2b2f38"/>
<rect x="26" y="82" width="48" height="26" rx="12" fill="url(#shell)" ${edge}/>
<rect x="36" y="90" width="28" height="9" rx="4.5" fill="#0b0d12"/>
<circle cx="43" cy="94.5" r="2.6" fill="${accent}"/>
<rect x="15" y="84" width="12" height="22" rx="6" fill="url(#shell)" ${edge}/>
<rect x="73" y="84" width="12" height="22" rx="6" fill="url(#shell)" ${edge}/>
</g>`;
}

const ROBOT_DEFS = `<linearGradient id="shell" x1="0" y1="0" x2="0.3" y2="1"><stop offset="0" stop-color="#fffdf8"/><stop offset="0.6" stop-color="#f4f1ea"/><stop offset="1" stop-color="#d8d1c3"/></linearGradient>
<linearGradient id="visor" x1="0" y1="0" x2="0.25" y2="1"><stop offset="0" stop-color="#525b6d"/><stop offset="0.45" stop-color="#2b313d"/><stop offset="1" stop-color="#161a22"/></linearGradient>`;

const gridPattern = (P) =>
  `<pattern id="grid" width="28" height="28" patternUnits="userSpaceOnUse"><path d="M28 0H0V28" fill="none" stroke="${P.grid}" stroke-width="1"/></pattern>`;

const svgOpen = (w, h, title, desc) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" role="img" aria-labelledby="t d">
<title id="t">${esc(title)}</title>
<desc id="d">${esc(desc)}</desc>
<!--
Fonts embedded in this file: Geist (Vercel) and IBM Plex Mono (IBM), SIL Open Font License 1.1.
${LICENCE.replace(/-{2,}/g, '-')}
-->`;

// Common stylesheet: the embedded fonts, the text classes, and the reduced-motion stop.
const baseCss = (extra) => `<![CDATA[
${FONT_CSS}
.sans{font-family:'Geist',system-ui,sans-serif}
.mono{font-family:'Plex Mono',ui-monospace,monospace;font-weight:500}
${extra}
@media (prefers-reduced-motion: reduce){*{animation:none !important}}
]]>`;

// ---------------------------------------------------------------- the hero (1280 x 434)

const PULSES = [
  { name: 'p1', cx: 868, cy: 276, s: 8, e: 18, d: 84 },
  { name: 'p2', cx: 1008, cy: 276, s: 13, e: 23, d: 84 },
  { name: 'p3', cx: 1147, cy: 276, s: 18, e: 28, d: 84 },
  { name: 'pa', cx: 1112, cy: 120, s: 34, e: 42, d: 22 }, // up the "keys stay on the hub" arrow
];
const LAW_ARRIVALS = PULSES.slice(0, 3).map((p) => p.e);

function pulseKF({ name, s, e, d }) {
  return `@keyframes ${name}{0%,${f2(s)}%{transform:translateY(0);opacity:0}${f2(s + 1)}%{opacity:1}${f2(e - 1)}%{opacity:1}${f2(e)}%,100%{transform:translateY(-${d}px);opacity:0}}`;
}

// A short white flash on the LAWS chip for each arrival, then dark for the rest of the 6 s.
function flashKF(name, arrivals, peak) {
  let k = '0%{opacity:0}';
  for (const a of arrivals) k += `${a - 1}%{opacity:0}${a}%{opacity:${peak}}${a + 3}%{opacity:0}`;
  return `@keyframes ${name}{${k}100%{opacity:0}}`;
}

const HERO_CARDS = [
  { x: 811, name: 'Della', role: 'architect', accent: '#5cc8ff', cls: 'r1' },
  { x: 951, name: 'Marlow', role: 'builder', accent: '#7ee0b5', cls: 'r2', wait: true },
  { x: 1090, name: 'Vera', role: 'auditor', accent: '#b69cff', cls: 'r3' },
];

function hero(theme) {
  const P = PAL[theme];
  const ink = P.ink;
  const css = baseCss(`
${PULSES.map(pulseKF).join('\n')}
${flashKF('fl', LAW_ARRIVALS, 0.45)}
@keyframes mpf{0%,41%{opacity:0}42%{opacity:1}47%{opacity:0}100%{opacity:0}}
@keyframes breathe{0%,100%{transform:scale(1);opacity:1}50%{transform:scale(1.07);opacity:0.8}}
@keyframes blink{0%,90%,100%{transform:scaleY(1)}94%{transform:scaleY(0.12)}}
@keyframes wait{0%,100%{opacity:0.4}50%{opacity:1}}
@keyframes halo{0%,100%{transform:scale(0.8);opacity:0.1}50%{transform:scale(1.8);opacity:0.5}}
.pl{opacity:0;animation-iteration-count:infinite;animation-timing-function:cubic-bezier(.45,0,.55,1)}
${PULSES.map((p) => `.${p.name}{animation-name:${p.name};animation-duration:6s}`).join('\n')}
.halo-glow{opacity:0.22}
.flash{opacity:0;animation:fl 6s linear infinite}
.mpf{opacity:0;animation:mpf 6s linear infinite}
.hive-core{transform-box:fill-box;transform-origin:50% 50%;animation:breathe 6s ease-in-out infinite}
.eyes{transform-box:fill-box;transform-origin:50% 50%;animation:blink 6s ease-in-out infinite}
.r1 .eyes{animation-delay:-0.6s}.r2 .eyes{animation-delay:-2.6s}.r3 .eyes{animation-delay:-4.4s}
.wait{animation:wait 6s ease-in-out infinite}
.halo-w{opacity:0.3;transform-box:fill-box;transform-origin:50% 50%;animation:halo 6s ease-in-out infinite}
`);

  const cards = HERO_CARDS.map((c) => {
    const x0 = c.x;
    const y0 = 276;
    return `<g>
<rect x="${x0 + 6}" y="${y0 + 6}" width="114" height="100" fill="${P.shadow}"/>
<rect x="${x0}" y="${y0}" width="114" height="100" fill="${P.panel}" stroke="${ink}" stroke-width="1.6"/>
${robot({ id: c.cls, x: f2(x0 + 8.56), y: f2(y0 + 7.48), s: 0.36, accent: c.accent, wait: c.wait })}
<rect x="${x0 + 59}" y="${y0 + 12}" width="45" height="13" fill="none" stroke="${ink}" stroke-width="1"/>
<text x="${x0 + 81.5}" y="${y0 + 21.5}" text-anchor="middle" class="mono" font-size="7.6" letter-spacing="0.4" fill="${ink}">MICROVM</text>
<rect x="${x0 + 11}" y="${y0 + 52}" width="62" height="13" fill="none" stroke="${SIG}" stroke-width="1"/>
<text x="${x0 + 42}" y="${y0 + 61.5}" text-anchor="middle" class="mono" font-size="7.6" letter-spacing="0.3" fill="${SIG}">NO NETWORK</text>
<text x="${x0 + 11}" y="${y0 + 81}" class="sans" font-size="14" font-weight="700" fill="${ink}">${c.name}</text>
<text x="${x0 + 11}" y="${y0 + 93}" class="mono" font-size="9.5" fill="${P.ink2}">${c.role}</text>
</g>`;
  }).join('\n');

  const chips = [
    { label: 'LAWS', x: 824, w: 88, on: true },
    { label: 'GATEWAY', x: 918, w: 87 },
    { label: 'VAULT', x: 1010, w: 88 },
    { label: 'TREASURY', x: 1104, w: 87 },
  ].map((c) => `<rect x="${c.x}" y="151" width="${c.w}" height="22" fill="${c.on ? SIG : P.panel}" ${c.on ? '' : `stroke="${ink}" stroke-width="1.4"`}/>
<text x="${c.x + c.w / 2}" y="166" text-anchor="middle" class="mono" font-size="10.5" letter-spacing="0.6" fill="${c.on ? '#fff' : ink}">${c.label}</text>`).join('\n');

  const pulses = PULSES.map((p) => `<g class="pl ${p.name}"><circle cx="${p.cx}" cy="${p.cy}" r="6" fill="${SIG}" class="halo-glow"/><circle cx="${p.cx}" cy="${p.cy}" r="3" fill="${SIG}"/></g>`).join('\n');

  return `${svgOpen(1280, 434, 'AgentPlaza: your AI agents, governed', 'AgentPlaza keeps each agent in its own microVM with no network, checks every tool call against your laws, and pays from a budget you control.')}
<defs><style>${css}</style>${gridPattern(P)}${ROBOT_DEFS}</defs>

<rect x="21" y="21" width="1248" height="402" fill="${P.shadow}"/>
<rect x="11" y="11" width="1248" height="402" fill="${P.ground}"/>
<rect x="11" y="11" width="1248" height="402" fill="url(#grid)"/>
<rect x="11" y="11" width="1248" height="402" fill="none" stroke="${ink}" stroke-width="2.5"/>

<g transform="translate(54 45) scale(1.2)" fill="none" stroke="${ink}" stroke-width="2.1" stroke-linejoin="miter">
${HIVE.outline.map((d) => `<path d="${d}"/>`).join('\n')}
<path class="hive-core" fill="${SIG}" stroke="none" d="${HIVE.core}"/>
</g>
<text x="104" y="78" class="sans" font-size="34" letter-spacing="-0.6"><tspan fill="${P.ink2}" font-weight="400">agent</tspan><tspan fill="${ink}" font-weight="700">plaza</tspan></text>
<rect x="55" y="101" width="480" height="28" fill="none" stroke="${P.line}" stroke-width="1.5"/>
<rect x="66" y="111" width="8" height="8" fill="${SIG}"/>
<text x="83" y="120" class="mono" font-size="14" fill="${ink}">Open source · Linux · macOS · Windows · Raspberry Pi</text>

<text x="52" y="225" class="sans" font-size="92" font-weight="800" letter-spacing="-3.4" fill="${ink}">Your AI agents,</text>
<text x="52" y="307" class="sans" font-size="92" font-weight="800" letter-spacing="-3.4" fill="${SIG}">governed.</text>
<text x="55" y="352" class="sans" font-size="20" fill="${P.ink2}">Each agent sealed in its own microVM, held to laws you write and paid from</text>
<text x="55" y="379" class="sans" font-size="20" fill="${P.ink2}">a budget you control.</text>

<rect x="817" y="47" width="108" height="34" fill="${P.redShadow}"/>
<rect x="811" y="41" width="108" height="34" fill="${P.red}"/>
<rect x="824" y="54" width="6" height="6" fill="#fff"/>
<text x="837" y="63" class="mono" font-size="13" letter-spacing="1" fill="#fff">STOP ALL</text>
<text x="811" y="92" class="mono" font-size="10" fill="${P.ink2}">one button holds</text>
<text x="811" y="105" class="mono" font-size="10" fill="${P.ink2}">the whole plaza</text>

<rect x="1003" y="47" width="210" height="41" fill="${P.shadow}"/>
<rect x="997" y="41" width="210" height="41" fill="${P.panel}" stroke="${ink}" stroke-width="2"/>
<rect class="mpf" x="997" y="41" width="210" height="41" fill="none" stroke="${SIG}" stroke-width="2"/>
<text x="1009" y="58" class="mono" font-size="12" letter-spacing="0.5" fill="${ink}">MODEL PROVIDERS</text>
<text x="1009" y="71" class="mono" font-size="10.5" fill="${P.ink2}">Anthropic · OpenAI · +5 more</text>
<line x1="1112" y1="120" x2="1112" y2="101" stroke="${ink}" stroke-width="1.6"/>
<polygon points="1112,92 1107,101 1117,101" fill="${ink}"/>
<text x="1102" y="106" text-anchor="end" class="mono" font-size="10" fill="${P.ink2}">keys stay on the hub</text>

<rect x="817" y="126" width="393" height="67" fill="${P.shadow}"/>
<rect x="811" y="120" width="393" height="67" fill="${P.panel}" stroke="${ink}" stroke-width="2"/>
<text x="824" y="141" class="mono" font-size="13" letter-spacing="0.3" fill="${ink}">HUB · YOUR MACHINE</text>
<text x="1190" y="141" text-anchor="end" class="mono" font-size="12" fill="${P.ink2}">apid</text>
${chips}
<rect class="flash" x="824" y="151" width="88" height="22" fill="#fff"/>

${[868, 1008, 1147].map((x) => `<line x1="${x}" y1="206" x2="${x}" y2="276" stroke="${P.ink2}" stroke-width="1.4" stroke-dasharray="3 4"/><rect x="${x - 2.5}" y="203.5" width="5" height="5" fill="${ink}"/>`).join('\n')}
<text x="1021" y="246" class="mono" font-size="10" fill="${P.ink2}">vsock only</text>

${cards}
${pulses}

<text x="811" y="392" class="mono" font-size="10" fill="${P.ink2}">every tool call is checked against the laws before it runs</text>
</svg>
`;
}

// ---------------------------------------------------------------- the feature cards (400 x 220)

const ICONS = {
  sealed: `<rect x="3" y="4" width="18" height="16" rx="3"/>
<rect x="9.5" y="11" width="5" height="4.5" rx="1"/>
<path class="lk" d="M10.5 11V9.6a1.5 1.5 0 0 1 3 0V11"/>`,
  laws: `<path d="M6.5 3.5h7.8l3.7 3.7v13.3H6.5z"/>
<path d="M14.3 3.5v3.7h3.7"/>
<path class="ck" pathLength="1" d="M9.3 14.2l2.1 2.1 3.4-4"/>`,
  treasury: `<ellipse class="sp" cx="12" cy="6" rx="7" ry="2.6"/>
<path d="M5 6v5.5c0 1.4 3.1 2.6 7 2.6s7-1.2 7-2.6V6"/>
<path d="M5 11.5v5.5c0 1.4 3.1 2.6 7 2.6s7-1.2 7-2.6v-5.5"/>`,
  weft: `<path d="M7 3v18"/>
<path d="M7 12h4.5c1.6 0 2.5-1 3.2-2.4"/>
<circle cx="7" cy="6" r="2.2"/>
<circle cx="7" cy="12" r="2.2"/>
<circle cx="7" cy="18" r="2.2"/>
<circle class="nd" cx="16.5" cy="9" r="2.2"/>
<circle class="nd nd2" cx="16.5" cy="15" r="2.2"/>`,
  models: `<rect x="7.5" y="7.5" width="9" height="9" rx="2"/>
<path d="M10 4.5v3M14 4.5v3M10 16.5v3M14 16.5v3M4.5 10h3M4.5 14h3M16.5 10h3M16.5 14h3"/>
<circle class="cd" cx="12" cy="12" r="1.6"/>`,
  fleet: `<circle cx="5.5" cy="6.5" r="2.5"/>
<circle cx="18.5" cy="6.5" r="2.5"/>
<circle cx="12" cy="17.5" r="2.5"/>
<path class="fl" d="M8 6.5h8M7.1 8.6l3.6 6.6M16.9 8.6l-3.6 6.6"/>`,
};

const ICON_CSS = {
  sealed: `.lk{stroke:${SIG};animation:lk 4s ease-in-out infinite}@keyframes lk{0%,100%{opacity:.35}50%{opacity:1}}`,
  laws: `.ck{stroke:${SIG};stroke-dasharray:1;animation:ck 4s ease-in-out infinite}@keyframes ck{0%{stroke-dashoffset:1}45%,80%{stroke-dashoffset:0}100%{stroke-dashoffset:-1}}`,
  treasury: `.sp{stroke:${SIG};transform-box:fill-box;transform-origin:50% 50%;animation:sp 3.2s ease-in-out infinite}@keyframes sp{0%,100%{transform:scaleX(1)}50%{transform:scaleX(.3)}}`,
  weft: `.nd{fill:${SIG};stroke:none;animation:nd 6s ease-in-out infinite}.nd2{animation-delay:-3s}@keyframes nd{0%,100%{opacity:.2}30%,60%{opacity:1}}`,
  models: `.cd{fill:${SIG};stroke:none;transform-box:fill-box;transform-origin:50% 50%;animation:cd 2.4s ease-in-out infinite}@keyframes cd{0%,100%{transform:scale(1)}50%{transform:scale(1.5)}}`,
  fleet: `.fl{stroke:${SIG};stroke-dasharray:2 2.5;animation:fl 1.8s linear infinite}@keyframes fl{to{stroke-dashoffset:-9}}`,
};

const CARDS = [
  { id: 'sealed', title: 'Sealed in microVMs', lines: ['Every agent runs in its own VM with no network:', 'the hub is its only way out.'] },
  { id: 'laws', title: 'Laws and approvals', lines: ['Every tool call is checked against the laws', 'you write; risky ones wait for you.'] },
  { id: 'treasury', title: 'A budget you control', lines: ['Agents earn, pay and are fined in credits;', 'model spend is metered per call.'] },
  { id: 'weft', title: 'History you can trust', lines: ['Weft records every change with its intent,', 'review and signature.'] },
  { id: 'models', title: 'Any model, any engine', lines: ['API providers and coding engines side by side,', 'health-checked and pinned.'] },
  { id: 'fleet', title: 'One machine or a fleet', lines: ['Laptops, servers and Raspberry Pis join as nodes;', 'watch them in 3D.'] },
];

function card(c, theme, index) {
  const P = PAL[theme];
  const css = baseCss(ICON_CSS[c.id]);
  return `${svgOpen(400, 220, c.title, c.lines.join(' '))}
<defs><style>${css}</style>${gridPattern(P)}</defs>
<rect width="400" height="220" fill="${P.ground}"/>
<rect width="400" height="220" fill="url(#grid)"/>
<rect x="10" y="10" width="384" height="204" fill="${P.shadow}"/>
<rect x="2" y="2" width="384" height="204" fill="${P.panel}" stroke="${P.ink}" stroke-width="2"/>

<rect x="28" y="26" width="64" height="64" rx="16" fill="${P.ground}" stroke="${P.line}" stroke-width="1.5"/>
<g transform="translate(39 37) scale(1.75)" fill="none" stroke="${P.ink}" stroke-width="1.45" stroke-linecap="round" stroke-linejoin="round">
${ICONS[c.id]}
</g>
<text x="372" y="52" text-anchor="end" class="mono" font-size="11" fill="${P.ink2}">0${index + 1}</text>
<text x="28" y="134" class="sans" font-size="21" font-weight="700" letter-spacing="-0.3" fill="${P.ink}">${esc(c.title)}</text>
<text x="28" y="160" class="sans" font-size="14" fill="${P.ink2}">${esc(c.lines[0])}</text>
<text x="28" y="180" class="sans" font-size="14" fill="${P.ink2}">${esc(c.lines[1])}</text>
</svg>
`;
}

// ---------------------------------------------------------------- write

mkdirSync(OUT, { recursive: true });
const files = {};
for (const theme of ['dark', 'light']) {
  files[`hero-${theme}.svg`] = hero(theme);
  CARDS.forEach((c, i) => {
    files[`card-${c.id}-${theme}.svg`] = card(c, theme, i);
  });
}
for (const [name, body] of Object.entries(files)) {
  const path = join(OUT, name);
  writeFileSync(path, body.replace(/\n{3,}/g, '\n\n'));
  console.log(`${name}\t${(Buffer.byteLength(body) / 1024).toFixed(0)} KB`);
}
