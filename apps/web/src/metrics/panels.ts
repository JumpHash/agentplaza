// A Metrics panel: its card (title, sub, right text, freshness stamp, pinned-from line, the ⋯ menu) and its body,
// one renderer per body type.

import { narrowTileColumns } from './state';
import { h, keepState, replace } from '../ui/dom';
import type { ChartCtx } from './charts';
import { lineChart, type ChartHandle } from './charts';
import { gauges, heatmap, spark, swimlanes, treemap, type FigureHandle } from './figures';
import { alerts, events, rules, traces } from './bodies-ops';
import { bars, flags, split, stats, table } from './bodies';
import { button } from '../ui/components';
import { menu, type MenuItem } from './overlays';
import { statusVar, type Body, type HealthItem, type Panel, type PanelAction, type Table, type Tile } from './model';

/** What a panel can ask the view to do. */
export interface PanelCtx extends ChartCtx {
  /** Narrow to a scope and/or open a tab (and a trace). */
  go(target: { scope?: string; tab?: string; trace?: string }): void;
  /** The Traces filter changed. */
  filter(f: string): void;
  /** Alert actions. */
  mute(rule: string): void;
  ack(rule: string, subject: string): void;
  toggleRule(id: string, on: boolean): void;
  newRule(): void;
  /** A head button: the hub's POST, then a toast and a reload of the view. */
  action(a: PanelAction): void;
  /** The next page of a table, from its `more` route. The table appends its rows and keeps the page's own `more`. */
  page(path: string): Promise<Table>;
  /** The panel menu's actions. */
  pin(p: Panel): void;
  isPinned(p: Panel): boolean;
  about(p: Panel): void;
  copyLink(p: Panel): void;
  /** Live panels show "live · N s ago"; others "as of HH:MM:SS". */
  liveNow: boolean;
  /** The label of the comparison period, as tile changes read it ("vs <label>"). */
  compareLabel?: string;
}

export interface PanelHandle {
  el: HTMLElement;
  /** Replaces the panel's body (a live frame). */
  update(p: Panel): void;
  /** Redraws charts at the current size. */
  redraw(): void;
  /** Updates the freshness stamp. */
  fresh(lastMs: number, live: boolean): void;
  destroy(): void;
}

/** A small button in a panel's head. It runs the hub's action; the view toasts and reloads. */
function headAction(a: PanelAction, ctx: PanelCtx): HTMLButtonElement {
  return button(a.label, () => ctx.action(a), { size: 'sm' });
}

/** The panel's card, spanning `p.span` twelfths of the grid. */
export function renderPanel(p: Panel, ctx: PanelCtx): PanelHandle {
  let current = p;
  const root = h('div.mx-card');
  root.classList.add(`mx-c${p.span}`);
  const bodyHost = h('div.mx-panel-body');
  const freshEl = h('span.mx-fresh');

  // The body's live handles: a line chart (with its table view when swapped in) and the figures.
  let chart: ChartHandle | null = null;
  let chartHost: HTMLElement | null = null;
  let tableEl: HTMLElement | null = null;
  let figures: FigureHandle[] = [];

  const dropBody = (): void => {
    for (const figure of figures) figure.destroy();
    figures = [];
    chart?.destroy();
    chart = null;
    chartHost = null;
    tableEl = null;
  };

  /** "View as table" swaps the chart for the chart's data table, and back. */
  const toggleTable = (): void => {
    if (!chart || !chartHost) return;
    if (tableEl) {
      tableEl.remove();
      tableEl = null;
      chartHost.hidden = false;
      chart.redraw();
    } else {
      tableEl = chart.table();
      chartHost.hidden = true;
      chartHost.after(tableEl);
    }
  };

  const toggleFull = (): void => {
    root.classList.toggle('mx-full');
    requestAnimationFrame(() => redraw());
  };

  const exportCsv = (): void => {
    if (!chart) return;
    const url = URL.createObjectURL(new Blob([chart.csv()], { type: 'text/csv' }));
    const link = h('a', { href: url, download: `${current.id}.csv` });
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };

  const menuItems = (): MenuItem[] => {
    const items: MenuItem[] = [];
    if (chart) items.push({ label: tableEl ? 'View as chart' : 'View as table', run: toggleTable });
    items.push({
      label: root.classList.contains('mx-full') ? 'Exit full screen' : 'Full screen',
      key: 'Esc',
      run: toggleFull,
    });
    items.push({ label: ctx.isPinned(current) ? 'Unpin' : 'Pin to My dashboard', run: () => ctx.pin(current) });
    if (chart) items.push({ label: 'Export CSV', run: exportCsv });
    items.push({ label: 'Copy link to this view', run: () => ctx.copyLink(current) });
    items.push({ label: 'About this metric', run: () => ctx.about(current) });
    const pinned = current.pinned;
    if (pinned) {
      items.push({ label: 'Open at its source', run: () => ctx.go({ scope: pinned.scope, tab: pinned.tab }) });
    }
    return items;
  };

  const pmBtn = h(
    'button.mx-pm',
    {
      type: 'button',
      'aria-label': 'Panel menu',
      'aria-haspopup': 'menu',
      title: 'Panel menu',
      onclick: (e: Event) => {
        e.stopPropagation();
        menu(pmBtn, menuItems());
      },
    },
    '⋯',
  );

  /** The card's header, pinned-from and sub lines. The body host moves with them, so its contents survive. */
  const paintCard = (): void => {
    // Tiles and health have no title: they sit bare on the grid, as in the design, unless pinned elsewhere.
    const bare = current.title === '' && !current.pinned;
    root.classList.toggle('mx-bare', bare);
    if (bare) {
      replace(root, bodyHost);
      return;
    }
    replace(
      root,
      h(
        'h3',
        null,
        h('span.mx-ttl', null, current.title),
        current.right ? h('small', null, current.right) : null,
        current.actions?.length ? h('span.mx-head-actions', null, current.actions.map((a) => headAction(a, ctx))) : null,
        current.live ? freshEl : null,
        pmBtn,
      ),
      current.pinned ? h('div.mx-pinned-from', null, `from ${current.pinned.from}`) : null,
      current.sub ? h('div.mx-sub', null, current.sub) : null,
      bodyHost,
    );
  };

  const healthBody = (items: HealthItem[]): HTMLElement =>
    h(
      'div.mx-health',
      null,
      items.map((item) => {
        const dot = h('i');
        dot.style.background = statusVar(item.status);
        return h(
          'button',
          { type: 'button', onclick: () => ctx.go({ tab: item.tab }) },
          h('div.mx-h', null, dot, item.area),
          h('div.mx-d', null, item.text),
        );
      }),
    );

  /** The change line: "▲ 12% vs <label>" coloured by whether the move is bad, or the note when there is no change. */
  const changeEl = (t: Tile): HTMLElement => {
    const pct = t.change_pct;
    if (pct == null) return h('div.mx-chg.mx-flat', null, t.note || ' ');
    const arrow = pct > 0 ? '▲ ' : pct < 0 ? '▼ ' : '';
    const vs = ctx.compareLabel ? ` vs ${ctx.compareLabel}` : '';
    const el = h('div.mx-chg', null, `${arrow}${Math.abs(pct).toFixed(0)}%${vs}`);
    if (pct === 0) el.classList.add('mx-flat');
    else el.classList.add((pct > 0) === t.up_bad ? 'mx-up-bad' : 'mx-up-good');
    return el;
  };

  const tileEl = (t: Tile): HTMLElement => {
    const line = spark(t.spark, t.tone, 200, 26);
    line.setAttribute('width', '100%');
    line.setAttribute('preserveAspectRatio', 'none');
    line.setAttribute('viewBox', '0 0 200 26');
    return h(
      'button.mx-tile',
      {
        type: 'button',
        title: `Open ${t.label}`,
        onclick: () => {
          if (t.tab) ctx.go({ tab: t.tab });
        },
      },
      h('div.mx-lbl', null, t.label),
      h('div.mx-val', null, t.value),
      changeEl(t),
      line,
    );
  };

  const tilesBody = (tiles: Tile[]): HTMLElement => {
    const el = h('div.mx-tiles', null, tiles.map((t) => tileEl(t)));
    // A narrow panel wraps the tiles into balanced rows (8 → 4 + 4); the desktop's tiles_row does the same.
    el.style.setProperty('--mx-tile-cols', String(narrowTileColumns(tiles.length)));
    return el;
  };

  /** Draws the body into the (empty) body host. Figures and charts get their own wrapper. */
  const drawBody = (body: Body): void => {
    const host = bodyHost;
    switch (body.type) {
      case 'health':
        host.append(healthBody(body.items));
        break;
      case 'tiles':
        host.append(tilesBody(body.tiles));
        break;
      case 'chart':
        chartHost = h('div.mx-chart');
        host.append(chartHost);
        chart = lineChart(chartHost, body, ctx);
        break;
      case 'heatmap': {
        const figure = h('div.mx-figure');
        host.append(figure);
        figures.push(heatmap(figure, body, ctx));
        break;
      }
      case 'treemap': {
        const figure = h('div.mx-figure');
        host.append(figure);
        figures.push(treemap(figure, body.unit, body.items));
        break;
      }
      case 'gauges': {
        const figure = h('div.mx-figure');
        host.append(figure);
        figures.push(gauges(figure, body.gauges));
        break;
      }
      case 'swimlanes': {
        const figure = h('div.mx-figure');
        host.append(figure);
        figures.push(swimlanes(figure, body, (scope) => ctx.go({ scope }), ctx.tz));
        break;
      }
      case 'table':
        table(host, body, ctx);
        break;
      case 'flags':
        flags(host, body.flags, ctx);
        break;
      case 'bars':
        bars(host, body.rows, body.note);
        break;
      case 'split':
        split(host, body.rows, body.legend);
        break;
      case 'stats':
        stats(host, body.stats);
        break;
      case 'alerts':
        alerts(host, body.rows, ctx);
        break;
      case 'rules':
        rules(host, body.rules, ctx);
        break;
      case 'traces':
        traces(host, body, ctx);
        break;
      case 'events':
        events(host, body.kinds, body.rows, ctx);
        break;
      case 'empty':
        host.append(h('div.mx-empty', null, body.text));
        break;
    }
  };

  /** Replaces the body. The reader's open parts, scroll and focus survive the redraw. */
  const paintBody = (body: Body): void => {
    dropBody();
    const restore = keepState(bodyHost);
    while (bodyHost.firstChild) bodyHost.removeChild(bodyHost.firstChild);
    drawBody(body);
    restore();
  };

  const redraw = (): void => {
    chart?.redraw();
    for (const figure of figures) figure.redraw();
  };

  const fresh = (lastMs: number, live: boolean): void => {
    freshEl.textContent = live
      ? `live · ${Math.max(0, Math.round((Date.now() - lastMs) / 1000))} s ago`
      : `as of ${new Date(lastMs + ctx.tz * 60_000).toISOString().slice(11, 19)}`;
    freshEl.classList.toggle('mx-stale', !live);
  };

  const update = (next: Panel): void => {
    root.classList.remove(`mx-c${current.span}`);
    root.classList.add(`mx-c${next.span}`);
    current = next;
    paintCard();
    const body = next.body;
    if (chart && body.type === 'chart') {
      chart.update(body);
      if (tableEl) {
        const refreshed = chart.table();
        tableEl.replaceWith(refreshed);
        tableEl = refreshed;
      }
    } else {
      paintBody(body);
    }
  };

  const destroy = (): void => dropBody();

  paintCard();
  paintBody(p.body);
  fresh(Date.now(), ctx.liveNow);
  return { el: root, update, redraw, fresh, destroy };
}

