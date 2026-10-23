// The Releases screen. `#/releases` lists the project's releases, grouped by channel; `#/releases/<id>` is one release's
// page: its life (state, candidate, verdicts, promotions, sync), its changes (change, title, review run), its artifacts
// per target (target, artifact, digest, size, pool, run), and the owner's actions on it. The rules are in
// releases-model.ts. Every text from the hub is set as text (h() and the components), never as markup.

import '../styles/releases.css';
import { releasesApi, type Release, type ReleaseArtifact, type ReleaseChange, type ReleasePromotion, type ReleaseVerdict } from '../api-releases';
import { go, href } from '../router';
import { projectTitle, scope } from '../scope';
import { ago, attempt, badge, button, card, empty, field, input, modal, table, textarea, toast, type Column } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import {
  candidateText,
  groupByChannel,
  isWebUrl,
  promotionLine,
  requiredProblem,
  shortDigest,
  sizeText,
  stateText,
  stateTone,
  verdictLine,
} from './releases-model';

/** The screen's mount: a release when the route names one, the project's list otherwise. */
export function mount(root: HTMLElement, params: string[]): () => void {
  const view = h('div.rel');
  root.append(view);
  const id = params[0];
  const stop = id ? releasePage(view, id) : releaseList(view);
  return () => {
    stop();
    view.remove();
  };
}

// ---- The list ---------------------------------------------------------------------------------------------------------

function releaseList(view: HTMLElement): () => void {
  let alive = true;
  /** Undefined until the first answer; a failed reload keeps the list it had. */
  let list: Release[] | undefined;
  let failed = false;
  const project = scope.get();

  const columns: Column<Release>[] = [
    { label: 'Version', cell: (r) => h('a', { href: href('releases', r.id) }, r.version) },
    { label: 'State', cell: (r) => badge(stateText(r.state), stateTone(r.state)) },
    { label: 'Candidate', cell: (r) => candidateText(r.number) },
    { label: 'Changes', cell: (r) => r.changes.length, num: true },
    { label: 'Artifacts', cell: (r) => r.artifacts.length, num: true },
    { label: 'Updated', cell: (r) => ago(r.updated_ms) },
  ];

  const body = (): Node => {
    if (!project) return empty('package', 'Pick a project', 'Releases belong to a project. Choose one in the project switcher to see its releases.');
    if (list === undefined) {
      return failed
        ? h('div.rel-error', null, h('p', null, 'The releases could not be loaded.'), button('Try again', () => void load(), { kind: 'ghost' }))
        : h('p.muted', null, 'Loading the releases…');
    }
    if (!list.length) return empty('package', 'No releases yet', 'A release is cut from a landed change: its candidate, its checks and its promotions show here.');
    return h(
      'div.rel-groups',
      null,
      groupByChannel(list).map((g) => card(g.channel, table(columns, g.releases, (r) => go('releases', r.id), (r) => r.id), { flush: true })),
    );
  };

  const draw = (): void => {
    if (!alive) return;
    replace(
      view,
      h('header.rel-h', null, h('h1', null, 'Releases'), project ? h('p.muted', null, projectTitle(project)) : null),
      body(),
    );
  };

  const load = async (): Promise<void> => {
    if (!project) return;
    try {
      list = await releasesApi.list(project);
      failed = false;
    } catch {
      failed = true;
    }
    draw();
  };

  draw();
  void load();
  return () => {
    alive = false;
  };
}

// ---- One release ------------------------------------------------------------------------------------------------------

function releasePage(view: HTMLElement, id: string): () => void {
  let alive = true;
  /** Undefined until the first answer; an action's answer replaces it. */
  let rel: Release | undefined;
  let failed = false;
  let busy = false;

  /** Runs one action and shows its answer. The buttons are off while it is in flight. */
  const act = async (run: () => Promise<Release>, done: string): Promise<void> => {
    if (busy) return;
    busy = true;
    draw();
    const next = await attempt(run, done);
    busy = false;
    if (next) rel = next;
    draw();
  };

  const actionButton = (label: string, kind: 'primary' | 'danger' | 'ghost', onClick: () => void): HTMLButtonElement => {
    const b = button(label, onClick, { kind, size: 'sm' });
    b.disabled = busy;
    return b;
  };

  /** A dialog with one text field; the action runs only with a value in it. */
  const ask = (o: { title: string; label: string; multiline: boolean; verb: string; what: string; onOk: (value: string) => void }): void => {
    const box = o.multiline ? textarea({ rows: 3, autocomplete: 'off' }) : input({ autocomplete: 'off' });
    const close = modal(
      o.title,
      h('div', null, field(o.label, box)),
      undefined,
      [
        button('Cancel', () => close(), { kind: 'ghost' }),
        button(o.verb, () => {
          const problem = requiredProblem(box.value, o.what);
          if (problem) {
            toast(problem, 'error');
            return;
          }
          close();
          o.onOk(box.value.trim());
        }, { kind: 'primary' }),
      ],
    );
  };

  /** A yes or no before an action that changes the release's state. */
  const confirm = (title: string, text: string, verb: string, onYes: () => void): void => {
    const close = modal(
      title,
      h('p', null, text),
      undefined,
      [
        button('Keep', () => close(), { kind: 'ghost' }),
        button(verb, () => {
          close();
          onYes();
        }, { kind: 'primary' }),
      ],
    );
  };

  const actions = (r: Release): HTMLElement =>
    h(
      'div.rel-acts',
      null,
      actionButton('Cut a candidate', 'ghost', () => void act(() => releasesApi.candidate(id), 'Candidate cut')),
      actionButton('Record a pass…', 'ghost', () =>
        ask({
          title: 'Record a passing check',
          label: 'Why it passed',
          multiline: true,
          verb: 'Record',
          what: 'a reason',
          onOk: (reason) => void act(() => releasesApi.check(id, true, reason), 'Check recorded'),
        }),
      ),
      actionButton('Record a failure…', 'ghost', () =>
        ask({
          title: 'Record a failed check',
          label: 'Why it failed',
          multiline: true,
          verb: 'Record',
          what: 'a reason',
          onOk: (reason) => void act(() => releasesApi.check(id, false, reason), 'Check recorded'),
        }),
      ),
      actionButton('Qualify', 'ghost', () => void act(() => releasesApi.qualify(id), 'Qualified')),
      actionButton('Publish…', 'primary', () =>
        confirm(`Publish ${r.version}?`, 'The release moves to published under its current candidate.', 'Publish', () =>
          void act(() => releasesApi.publish(id), 'Published'),
        ),
      ),
      actionButton('Promote…', 'ghost', () =>
        ask({
          title: `Promote ${r.version}`,
          label: 'Environment',
          multiline: false,
          verb: 'Promote',
          what: 'an environment',
          onOk: (environment) => void act(() => releasesApi.promote(id, environment), 'Promoted'),
        }),
      ),
      actionButton('Yank…', 'danger', () =>
        ask({
          title: `Yank ${r.version}`,
          label: 'Why it is yanked',
          multiline: true,
          verb: 'Yank',
          what: 'a reason',
          onOk: (reason) => void act(() => releasesApi.yank(id, reason), 'Release yanked'),
        }),
      ),
    );

  const life = (r: Release): HTMLElement => {
    const verdicts = [...r.verdicts].sort((a, b) => a.seq - b.seq);
    const promotions = [...r.promotions].sort((a, b) => a.seq - b.seq);
    return card(
      'Life',
      h(
        'div',
        null,
        h(
          'dl.kv',
          null,
          h('dt', null, 'State'),
          h('dd', null, badge(stateText(r.state), stateTone(r.state))),
          h('dt', null, 'Candidate'),
          h('dd', null, candidateText(r.number)),
          h('dt', null, 'Channel'),
          h('dd', null, r.channel),
          h('dt', null, 'Lane'),
          h('dd', null, r.lane),
          h('dt', null, 'From'),
          h('dd', null, r.from),
          h('dt', null, 'Base'),
          h('dd', null, r.base ?? '—'),
          h('dt', null, 'Run'),
          h('dd', null, r.run ?? '—'),
          h('dt', null, 'SBOM'),
          h('dd', null, r.sbom || '—'),
          h('dt', null, 'Provenance'),
          h('dd', null, r.provenance || '—'),
        ),
        r.notes ? h('p.rel-notes', null, r.notes) : null,
        h('h3', null, 'Verdicts'),
        verdicts.length ? h('ul.rel-list', null, verdicts.map((v: ReleaseVerdict) => h('li', null, verdictLine(v)))) : h('p.muted', null, 'No verdicts yet.'),
        h('h3', null, 'Promotions'),
        promotions.length
          ? h('ul.rel-list', null, promotions.map((p: ReleasePromotion) => h('li', null, promotionLine(p))))
          : h('p.muted', null, 'Not promoted yet.'),
        r.sync.length
          ? [
              h('h3', null, 'Sync'),
              h(
                'ul.rel-list',
                null,
                r.sync.map((s) =>
                  h('li', null, `${s.platform}: ${s.state}`, s.url && isWebUrl(s.url) ? [' · ', h('a', { href: s.url, target: '_blank', rel: 'noopener' }, 'open')] : null),
                ),
              ),
            ]
          : null,
      ),
    );
  };

  const changes = (rows: ReleaseChange[]): HTMLElement =>
    card(
      'Changes',
      rows.length
        ? table<ReleaseChange>(
            [
              { label: 'Change', cell: (c) => h('code', null, c.change) },
              { label: 'Title', cell: (c) => c.title },
              { label: 'Review run', cell: (c) => (c.review_run ? h('code', null, c.review_run) : '—') },
            ],
            rows,
          )
        : h('p.muted', null, 'No changes in this release.'),
      { flush: true },
    );

  const artifacts = (rows: ReleaseArtifact[]): HTMLElement =>
    card(
      'Artifacts',
      rows.length
        ? table<ReleaseArtifact>(
            [
              { label: 'Target', cell: (a) => h('code', null, a.target) },
              { label: 'Artifact', cell: (a) => h('code', null, a.artifact) },
              { label: 'Digest', cell: (a) => h('code', { title: a.digest || null }, shortDigest(a.digest)) },
              { label: 'Size', cell: (a) => sizeText(a.size), num: true },
              { label: 'Pool', cell: (a) => a.pool || '—' },
              { label: 'Run', cell: (a) => (a.run ? h('code', null, a.run) : '—') },
            ],
            rows,
          )
        : h('p.muted', null, 'No artifacts yet.'),
      { flush: true },
    );

  const draw = (): void => {
    if (!alive) return;
    const back = h('a.rel-back', { href: href('releases') }, lu('arrow-left', 14), h('span', null, 'Releases'));
    if (rel === undefined) {
      replace(
        view,
        back,
        failed
          ? h('div.rel-error', null, h('p', null, 'The release could not be loaded.'), button('Try again', () => void load(), { kind: 'ghost' }))
          : h('p.muted', null, 'Loading the release…'),
      );
      return;
    }
    const r = rel;
    replace(
      view,
      back,
      h(
        'header.rel-h',
        null,
        h('h1', null, r.version),
        badge(stateText(r.state), stateTone(r.state)),
        h('p.muted', null, `${r.repo} · ${r.channel} channel · updated ${ago(r.updated_ms)}`),
      ),
      actions(r),
      life(r),
      changes(r.changes),
      artifacts(r.artifacts),
    );
  };

  const load = async (): Promise<void> => {
    try {
      rel = await releasesApi.get(id);
      failed = false;
    } catch {
      failed = true;
    }
    draw();
  };

  draw();
  void load();
  return () => {
    alive = false;
  };
}
