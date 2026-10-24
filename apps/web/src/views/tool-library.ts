// The Tool library screen: every capability the plaza has as one tree of dotted paths. The owner walks the groups,
// searches the words, and reads an entry's schema, its policy and the seats that may use it. The hub serves the tree
// (crates/apid/src/tool_library/); the rules that shape the rows are in tool-library-model.ts.

import { toolLibraryApi, type ToolEntryDetail, type ToolHit, type ToolListing, type ToolNode } from '../api-tool-library';
import { badge, card, empty, table, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import {
  accessLabel,
  costLabel,
  costTone,
  crumbs,
  nodeLabel,
  parentOf,
  queryOf,
  runsLabel,
  schemaRows,
  seatsLine,
  unavailableText,
  visibleNodes,
} from './tool-library-model';

/** Renders the Tool library into `root`; returns the cleanup that stops it. */
export function mount(root: HTMLElement): () => void {
  let path = '';
  let listing: ToolListing | null = null;
  let detail: ToolEntryDetail | null = null;
  let hits: ToolHit[] | null = null;
  let failure = '';
  let filter = '';
  let query = '';
  let loadId = 0;
  let stopped = false;

  const draw = () => {
    if (!stopped) replace(root, render());
  };

  /** Shows a group's children; the answer is ignored when a later load has started. */
  const openGroup = async (target: string) => {
    const mine = ++loadId;
    path = target;
    detail = null;
    hits = null;
    query = '';
    try {
      const next = await toolLibraryApi.listing(target);
      if (mine !== loadId || stopped) return;
      listing = next;
      failure = '';
    } catch (err) {
      if (mine !== loadId || stopped) return;
      failure = err instanceof Error ? err.message : String(err);
    }
    draw();
  };

  /** Shows one entry's detail, with the seats that may use it. The listing stays the one of its group. */
  const openEntry = async (target: string) => {
    const mine = ++loadId;
    try {
      const next = await toolLibraryApi.entry(target);
      if (mine !== loadId || stopped) return;
      detail = next;
      failure = '';
    } catch (err) {
      if (mine !== loadId || stopped) return;
      failure = err instanceof Error ? err.message : String(err);
    }
    draw();
  };

  /** Runs a search over every entry, or within the group the owner is in. */
  const search = async (words: string) => {
    query = queryOf(words);
    if (!query) {
      hits = null;
      draw();
      return;
    }
    const mine = ++loadId;
    try {
      const answer = await toolLibraryApi.search(query, path);
      if (mine !== loadId || stopped) return;
      hits = answer.hits;
      failure = '';
    } catch (err) {
      if (mine !== loadId || stopped) return;
      failure = err instanceof Error ? err.message : String(err);
    }
    draw();
  };

  const nodeColumns: Column<ToolNode>[] = [
    { label: 'Name', cell: (node) => nodeLabel(node), width: '34%' },
    { label: 'What it is', cell: (node) => node.summary },
  ];

  const crumbBar = () =>
    h(
      'nav.tl-crumbs',
      { 'aria-label': 'Path' },
      h('button.link', { onclick: () => void openGroup('') }, 'Library'),
      ...crumbs(path).flatMap((crumb) => [
        ' / ',
        h('button.link', { onclick: () => void openGroup(crumb.path) }, crumb.name),
      ]),
    );

  const treeCard = () => {
    if (hits) {
      const rows: Column<ToolHit>[] = [
        { label: 'Path', cell: (hit) => hit.path },
        { label: 'What it is', cell: (hit) => hit.summary },
        { label: 'Runs', cell: (hit) => runsLabel({ runs: hit.runs }) },
      ];
      return card(
        `Matches for “${query}”`,
        hits.length
          ? table(rows, hits, (hit) => void openEntry(hit.path), (hit) => hit.path)
          : empty('zap', 'Nothing matches', 'Try other words, or open a group.'),
      );
    }
    const children = listing ? visibleNodes(listing.children, filter) : [];
    const groupSummary = listing?.summary ? h('p.sub', null, listing.summary) : null;
    return card(
      path ? path : 'Library',
      [
        groupSummary,
        children.length
          ? table(
              nodeColumns,
              children,
              (node) => (node.kind === 'group' ? void openGroup(node.path) : void openEntry(node.path)),
              (node) => node.path,
            )
          : empty('zap', 'Nothing here', 'This group has no tools you may use.'),
      ].filter((node): node is HTMLElement => node !== null),
    );
  };

  const detailCard = () => {
    if (!detail) {
      const internal = listing && listing.path === '' ? listing.internal : [];
      return card(
        'Not in the library',
        internal.length
          ? h(
              'ul.tl-internal',
              null,
              ...internal.map((item) => h('li', null, h('b', null, item.group), ` — ${item.reason}`)),
            )
          : empty('zap', 'Pick an entry', 'Its schema, policy and the seats that may use it show here.'),
      );
    }
    const entry = detail;
    const rows = schemaRows(entry.input_schema);
    const unavailable = unavailableText(entry);
    const parts: (Node | null)[] = [
        h('p.sub', null, entry.summary),
        unavailable ? h('p.tl-warn', null, `Not available to the seat: ${unavailable}`) : null,
        h('p', null, entry.description),
        h(
          'div.tl-chips',
          null,
          badge(runsLabel(entry), 'info'),
          badge(costLabel(entry.cost), costTone(entry.cost)),
          badge(accessLabel(entry.access), entry.access === 'operator' ? 'warn' : 'muted'),
          entry.approval ? badge('needs the owner’s approval', 'danger') : null,
          badge(`source: ${entry.source}`, 'muted'),
        ),
        h('p.sub', null, `Runs as: ${entry.exec}`),
        h('h3', null, 'Input'),
        rows.length
          ? table(
              [
                { label: 'Name', cell: (row) => (row.required ? `${row.name} *` : row.name) },
                { label: 'Type', cell: (row) => row.type },
                { label: 'What it is', cell: (row) => row.description },
              ],
              rows,
            )
          : h('p.sub', null, 'No named inputs.'),
        h('h3', null, 'Seats'),
        h('p.sub', null, seatsLine(entry.seats)),
        table(
          [
            { label: 'Seat', cell: (seat) => seat.seat },
            { label: 'May use it', cell: (seat) => (seat.allowed ? 'yes' : `no: ${seat.reason ?? ''}`) },
          ],
          entry.seats,
        ),
        h('h3', null, 'Schema'),
        h('pre.tl-schema', null, JSON.stringify(entry.input_schema, null, 2)),
    ];
    return card(
      entry.path,
      parts.filter((node): node is Node => node !== null),
    );
  };

  const render = (): HTMLElement => {
    const head = h(
      'div.page-h',
      null,
      h('h1', null, 'Tool library'),
      h('p.sub', null, 'Every capability the plaza has, as one tree. Agents explore the same tree with tools_list, tools_search and tools_describe.'),
    );
    const searchBox = h('input.tl-search', {
      type: 'search',
      placeholder: 'Search by words: rollback, repos, memory…',
      value: query,
      'aria-label': 'Search the tool library',
      oninput: (event: Event) => {
        const text = (event.target as HTMLInputElement).value;
        if (!queryOf(text)) {
          query = '';
          hits = null;
          filter = text;
          draw();
          return;
        }
        filter = text;
        void search(text);
      },
    });
    const filterBox = h('input.tl-filter', {
      type: 'text',
      placeholder: 'Filter this group',
      value: filter,
      'aria-label': 'Filter this group',
      oninput: (event: Event) => {
        filter = (event.target as HTMLInputElement).value;
        draw();
      },
    });
    if (failure) return h('div.tool-library', null, head, empty('zap', 'The tool library did not load', failure));
    if (!listing) return h('div.tool-library', null, head, empty('zap', 'Loading the tool library'));
    const up = path ? h('button.link', { onclick: () => void openGroup(parentOf(path)) }, '↑ up') : null;
    return h(
      'div.tool-library',
      null,
      head,
      h('div.tl-bar', null, searchBox, filterBox),
      h('div.tl-path', null, crumbBar(), up),
      h('div.tl-cols', null, h('div.tl-left', null, treeCard()), h('div.tl-right', null, detailCard())),
    );
  };

  root.setAttribute('data-screen', 'tool-library');
  void openGroup('');
  draw();
  return () => {
    stopped = true;
  };
}
