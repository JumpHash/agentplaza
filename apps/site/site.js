// The site's behaviour: provider marks from the sprite, the hero window's tabs, the scroll
// choreography (margin progress, boot log, reveals, the window rising out of its tilt,
// parallax), and copy buttons. Without JS the page is plain, complete HTML.

document.documentElement.classList.add('js');
const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
const SVG_NS = 'http://www.w3.org/2000/svg';

/** Marks whose brand colour is black or near it are drawn in ink, so they sit on the paper. */
function inkFor(hex) {
  const n = parseInt(hex.slice(1), 16);
  const lum = 0.2126 * ((n >> 16) & 255) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255);
  return lum < 40 ? 'var(--ink)' : hex;
}

async function marks() {
  let doc;
  try {
    doc = new DOMParser().parseFromString(await (await fetch('assets/logos.svg')).text(), 'image/svg+xml');
  } catch {
    return;
  }
  const svgFor = (key) => {
    const symbol = doc.getElementById(key);
    if (!symbol) return null;
    const svg = document.createElementNS(SVG_NS, 'svg');
    svg.setAttribute('viewBox', symbol.getAttribute('viewBox') || '0 0 24 24');
    svg.setAttribute('aria-hidden', 'true');
    for (const p of symbol.querySelectorAll('path')) {
      const path = document.createElementNS(SVG_NS, 'path');
      path.setAttribute('d', p.getAttribute('d'));
      svg.appendChild(path);
    }
    svg.style.fill = inkFor(symbol.getAttribute('data-hex') || '#121613');
    return svg;
  };
  for (const chip of document.querySelectorAll('.chip[data-logo]')) {
    const svg = svgFor(chip.dataset.logo);
    const button = chip.querySelector('button');
    if (svg && button) {
      button.appendChild(svg);
      button.setAttribute('aria-label', chip.querySelector('.pop b')?.textContent ?? chip.dataset.logo);
    }
  }
  for (const el of document.querySelectorAll('.provider[data-logo]')) {
    const svg = svgFor(el.dataset.logo);
    if (!svg) continue;
    const box = document.createElement('span');
    box.className = 'mark';
    box.appendChild(svg);
    el.prepend(box);
  }
}

/** Splits a heading into word spans for the rise-in (the last word of the h1 in signal). */
function words(el, highlightLast) {
  const list = el.textContent.trim().split(/\s+/);
  el.setAttribute('aria-label', el.textContent.trim());
  el.textContent = '';
  list.forEach((word, i) => {
    const w = document.createElement('span');
    w.className = 'w' + (highlightLast && i >= list.length - 2 ? ' hl' : '');
    w.setAttribute('aria-hidden', 'true');
    w.style.setProperty('--i', String(i));
    const inner = document.createElement('span');
    inner.textContent = word;
    w.appendChild(inner);
    el.appendChild(w);
    if (i < list.length - 1) el.appendChild(document.createTextNode(' '));
  });
}

function tabs() {
  for (const win of document.querySelectorAll('[data-tabs]')) {
    const buttons = [...win.querySelectorAll('[data-tab]')];
    const title = win.querySelector('[data-tab-title]');
    const show = (key) => {
      for (const b of buttons) b.setAttribute('aria-selected', String(b.dataset.tab === key));
      for (const shot of win.querySelectorAll('[data-shot]')) {
        const on = shot.dataset.shot === key;
        shot.classList.toggle('active', on);
        if (shot.tagName === 'VIDEO') on ? shot.play().catch(() => {}) : shot.pause();
      }
      const b = buttons.find((x) => x.dataset.tab === key);
      if (title && b) title.textContent = b.textContent;
    };
    for (const b of buttons) b.addEventListener('click', () => show(b.dataset.tab));
    win.addEventListener('keydown', (ev) => {
      if (ev.key !== 'ArrowRight' && ev.key !== 'ArrowLeft') return;
      const at = buttons.findIndex((b) => b.getAttribute('aria-selected') === 'true');
      const next = buttons[(at + (ev.key === 'ArrowRight' ? 1 : buttons.length - 1)) % buttons.length];
      next.focus();
      show(next.dataset.tab);
    });
  }
  for (const img of document.querySelectorAll('img.shot, .plate img')) {
    img.addEventListener('error', () => (img.style.visibility = 'hidden'));
  }
}

/** Reveals, staggers, drawn diagrams and the boot log, as sections come into view. */
function reveals() {
  for (const group of document.querySelectorAll('[data-stagger]')) {
    [...group.children].forEach((child, i) => child.style.setProperty('--i', String(i)));
  }
  for (const h2 of document.querySelectorAll('.band h2')) {
    // Keep the grey second line as its own span: split each text run into words.
    for (const node of [...h2.childNodes]) {
      if (node.nodeType === Node.TEXT_NODE && node.textContent.trim()) {
        const frag = document.createElement('span');
        frag.textContent = node.textContent;
        h2.replaceChild(frag, node);
        words(frag, false);
        frag.replaceWith(...frag.childNodes);
      } else if (node.nodeType === Node.ELEMENT_NODE && node.classList.contains('dim')) {
        words(node, false);
      }
    }
    let i = 0;
    for (const w of h2.querySelectorAll('.w')) w.style.setProperty('--i', String(i++));
  }
  const line = document.querySelector('[data-bootline]');
  const logged = new Set();
  let typing = 0;
  const boot = (text) => {
    if (!line || logged.has(text)) return;
    logged.add(text);
    clearInterval(typing);
    let n = 0;
    line.textContent = '';
    typing = setInterval(() => {
      line.textContent = text.slice(0, ++n);
      if (n >= text.length) clearInterval(typing);
    }, reduced ? 0 : 18);
  };
  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        e.target.classList.add('in');
        if (e.target.dataset.boot) boot(e.target.dataset.boot);
        io.unobserve(e.target);
      }
    },
    { rootMargin: '0px 0px -18% 0px', threshold: 0.12 },
  );
  for (const el of document.querySelectorAll('[data-reveal], [data-stagger], [data-draw], .band h2, [data-boot]')) io.observe(el);

  // The nav marks the section in view.
  const links = new Map([...document.querySelectorAll('.nav-links a')].map((a) => [a.getAttribute('href').slice(1), a]));
  const spy = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        const a = links.get(e.target.id);
        if (!a) continue;
        if (e.isIntersecting) {
          for (const other of links.values()) other.removeAttribute('aria-current');
          a.setAttribute('aria-current', 'true');
        }
      }
    },
    { rootMargin: '-45% 0px -50% 0px' },
  );
  for (const id of links.keys()) {
    const el = document.getElementById(id);
    if (el) spy.observe(el);
  }
}

/** Scroll-linked motion, one rAF per frame: margin progress, the window's rise, parallax. */
function scrollMotion() {
  const root = document.documentElement;
  const bar = document.querySelector('.progress i');
  const stage = document.querySelector('[data-rise]');
  const hero = document.querySelector('.hero');
  const plates = [...document.querySelectorAll('[data-parallax]')];
  let queued = false;
  const frame = () => {
    queued = false;
    const y = scrollY;
    const max = root.scrollHeight - innerHeight;
    bar?.parentElement.style.setProperty('--p', String(max > 0 ? Math.min(1, y / max) : 0));
    if (reduced) return;
    hero?.style.setProperty('--scroll', String(Math.min(y, 1200)));
    if (stage) {
      // Flat once its top reaches 30% of the viewport.
      const top = stage.getBoundingClientRect().top;
      const t = Math.max(0, Math.min(1, (top - innerHeight * 0.3) / (innerHeight * 0.55)));
      stage.style.setProperty('--tilt', t.toFixed(3));
    }
    for (const plate of plates) {
      const r = plate.getBoundingClientRect();
      const mid = r.top + r.height / 2 - innerHeight / 2;
      const img = plate.querySelector('img');
      if (img) img.style.setProperty('--py', `${(-mid * Number(plate.dataset.parallax || 0.06)).toFixed(1)}px`);
    }
  };
  const request = () => {
    if (!queued) {
      queued = true;
      requestAnimationFrame(frame);
    }
  };
  addEventListener('scroll', request, { passive: true });
  addEventListener('resize', request);
  frame();
}

function copies() {
  for (const b of document.querySelectorAll('[data-copy]')) {
    b.addEventListener('click', async () => {
      const src = document.querySelector(b.dataset.copy);
      if (!src) return;
      const text = src.innerText.split('\n').filter((l) => l.startsWith('$ ')).map((l) => l.slice(2)).join('\n');
      try {
        await navigator.clipboard.writeText(text);
        b.textContent = 'Copied';
      } catch {
        b.textContent = 'Select & copy';
      }
      setTimeout(() => (b.textContent = 'Copy'), 1600);
    });
  }
}

const h1 = document.querySelector('h1[data-words]');
if (h1) words(h1, true);
marks();
tabs();
reveals();
scrollMotion();
copies();
// The hero's own boot line, once the page has loaded.
setTimeout(() => {
  const hero = document.querySelector('.hero[data-boot]');
  if (hero) hero.classList.add('in');
}, 900);
