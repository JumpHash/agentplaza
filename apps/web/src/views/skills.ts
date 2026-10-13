// Skills: the skill library (docs/design/skill-library.md). `#/skills` lists skills, creates one from the template and
// imports them (a zip or .skill file, or GitHub); `#/skills/<id>` opens one in the editor (views/skill-editor.ts).

import '../styles/skills.css';
import { api, skillsApi, ApiError, type RoleView, type SkillPreview, type SkillRules, type SkillSource, type SkillSummary } from '../api';
import { go, href } from '../router';
import { ago, badge, button, field, input, modal, table, textarea, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { mountEditor } from './skill-editor';

type Status = 'loading' | 'ready' | 'error';
type SourceFilter = 'all' | SkillSource['kind'];

const INTRO = 'Versioned SKILL.md folders that agents read on demand, each checked against the plaza\'s rules as it is written. The live version is the one in use; the best version is the highest-scoring one kept.';
const EMPTY_TEXT = 'The library holds the skills your agents can read: a SKILL.md with the files it needs, checked and scored on every version. Start with New skill, or bring existing skills in with Import or From GitHub.';

const SOURCE_FILTERS: [SourceFilter, string][] = [['all', 'All sources'], ['built', 'Built'], ['upload', 'Upload'], ['github', 'GitHub']];
const SOURCE_LABEL: Record<SkillSource['kind'], string> = { built: 'Built', upload: 'Upload', github: 'GitHub' };

/** Rules for a tone by score: 90 and up good, 70 and up warn, the rest bad. */
function scoreTone(score: number): Tone {
  return score >= 90 ? 'ok' : score >= 70 ? 'warn' : 'danger';
}

/** A binding as a chip: `role:x` reads "Role: x", `plaza` reads "Whole plaza". */
function bindingChip(binding: string): HTMLElement {
  if (binding === 'plaza') return badge('Whole plaza', 'info');
  if (binding.startsWith('role:')) return badge(`Role: ${binding.slice('role:'.length)}`, 'info');
  return badge(binding, 'info');
}

/** A title as an id slug: lower case, runs of other characters become one dash. */
function slugOf(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
}

/** The source as a chip; a GitHub skill shows repo@commit and links to that commit's folder. */
function sourceCell(src: SkillSource): HTMLElement {
  if (src.kind !== 'github') return badge(SOURCE_LABEL[src.kind]);
  const path = src.path.split('/').filter(Boolean).map(encodeURIComponent).join('/');
  const url = `https://github.com/${src.repo}/tree/${src.commit}${path ? `/${path}` : ''}`;
  return h('a.skills-source', { href: url, target: '_blank', rel: 'noopener noreferrer' }, badge(`${src.repo}@${src.commit.slice(0, 7)}`, 'accent'));
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

function splitTags(text: string): string[] {
  return text.split(',').map((t) => t.trim()).filter(Boolean);
}

let rulesCache: Promise<SkillRules> | null = null;

/** The rules, with the SKILL.md template; fetched once and kept. */
function rules(): Promise<SkillRules> {
  if (!rulesCache) {
    rulesCache = skillsApi.rules().catch((err: unknown) => {
      rulesCache = null;
      throw err;
    });
  }
  return rulesCache;
}

let rolesCache: Promise<RoleView[]> | null = null;

/** The roles from GET /v1/roles, fetched once and kept. */
function roleList(): Promise<RoleView[]> {
  if (!rolesCache) {
    rolesCache = api.roles().catch((err: unknown) => {
      rolesCache = null;
      throw err;
    });
  }
  return rolesCache;
}

/** Bindings: an Every agent box (`plaza`), then a box per role (`role:<id>`), roles ordered by label. */
function bindingsField(): { node: HTMLElement; value: () => string[] } {
  const everyone = h('input', { type: 'checkbox' }) as HTMLInputElement;
  const roleBoxes: HTMLInputElement[] = [];
  const roleRows = h('div.skills-roles', null);
  const node = h('div.skills-binding', null, h('label.skills-check', null, everyone, ' Every agent'), roleRows);
  void roleList().then(
    (list) => {
      for (const r of [...list].sort((a, b) => a.title.localeCompare(b.title))) {
        const cb = h('input', { type: 'checkbox', value: r.id }) as HTMLInputElement;
        roleBoxes.push(cb);
        roleRows.append(h('label.skills-check', null, cb, ` Role: ${r.title}`));
      }
    },
    (err: unknown) => toast(errorText(err), 'error'),
  );
  return {
    node,
    value: () => {
      const out: string[] = everyone.checked ? ['plaza'] : [];
      for (const cb of roleBoxes) if (cb.checked) out.push(`role:${cb.value}`);
      return out;
    },
  };
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  if (params[0]) {
    return mountEditor(root, params[0], () => {
      location.hash = '#/skills';
    });
  }
  return mountList(root);
}

function mountList(root: HTMLElement): Cleanup {
  let alive = true;
  let seq = 0;
  let status: Status = 'loading';
  let failure = '';
  let skills: SkillSummary[] = [];
  let bindingFilter = 'all';
  let sourceFilter: SourceFilter = 'all';
  let showArchived = false;
  let query = '';
  const selected = new Set<string>();
  let closeDialog: (() => void) | null = null;

  const search = input({ type: 'search', placeholder: 'Search by name, title, description or tag', 'aria-label': 'Search skills', autocomplete: 'off' });
  const bindingSel = h('select.select', { 'aria-label': 'Binding' }) as HTMLSelectElement;
  const sourceSel = h(
    'select.select',
    { 'aria-label': 'Source' },
    SOURCE_FILTERS.map(([value, label]) => h('option', { value }, label)),
  ) as HTMLSelectElement;
  const archivedIn = h('input', { type: 'checkbox' }) as HTMLInputElement;
  const exportSlot = h('span.skills-export');
  const countLine = h('p.skills-count', { 'aria-live': 'polite' });
  const listBox = h('div.skills-list');

  search.addEventListener('input', () => {
    query = search.value.trim().toLowerCase();
    renderList();
  });
  bindingSel.addEventListener('change', () => {
    bindingFilter = bindingSel.value;
    renderList();
  });
  sourceSel.addEventListener('change', () => {
    sourceFilter = sourceSel.value as SourceFilter;
    renderList();
  });
  archivedIn.addEventListener('change', () => {
    showArchived = archivedIn.checked;
    void load();
  });

  /** The binding filter's options: all, whole plaza, each role in the library, unbound. */
  function renderBindingOptions(): void {
    const roles = [...new Set(skills.flatMap((s) => s.bindings.filter((b) => b.startsWith('role:'))))].sort();
    const options: [string, string][] = [
      ['all', 'All bindings'],
      ['plaza', 'Whole plaza'],
      ...roles.map((r): [string, string] => [r, `Role: ${r.slice('role:'.length)}`]),
      ['unbound', 'Unbound'],
    ];
    if (!options.some(([value]) => value === bindingFilter)) bindingFilter = 'all';
    bindingSel.replaceChildren(...options.map(([value, label]) => h('option', { value }, label)));
    bindingSel.value = bindingFilter;
  }

  /** The rows the filters leave: the archived toggle (the API returns archived rows too), binding, source and the search. */
  function visible(): SkillSummary[] {
    return skills.filter(
      (s) =>
        (showArchived || s.archived_ms == null) &&
        (bindingFilter === 'all' ||
          (bindingFilter === 'unbound' ? s.bindings.length === 0 : s.bindings.includes(bindingFilter))) &&
        (sourceFilter === 'all' || s.source.kind === sourceFilter) &&
        (!query || [s.name, s.title, s.description, ...s.tags].join(' ').toLowerCase().includes(query)),
    );
  }

  function clearFilters(): void {
    bindingFilter = 'all';
    sourceFilter = 'all';
    query = '';
    search.value = '';
    bindingSel.value = 'all';
    sourceSel.value = 'all';
    renderList();
  }

  /** Export selected: a link to the zip of the ticked skills, or a disabled button when none are ticked. */
  function renderExport(): void {
    const ids = [...selected];
    replace(
      exportSlot,
      ids.length
        ? h('a.btn', { href: skillsApi.exportManyUrl(ids) }, `Export selected (${ids.length})`)
        : button('Export selected', () => {}, { disabled: true }),
    );
  }

  const columns: Column<SkillSummary>[] = [
    {
      label: 'Select',
      cell: (s) => {
        const box = h('input', { type: 'checkbox', 'aria-label': `Select ${s.name}` }) as HTMLInputElement;
        box.checked = selected.has(s.id);
        box.addEventListener('change', () => {
          if (box.checked) selected.add(s.id);
          else selected.delete(s.id);
          renderExport();
        });
        return box;
      },
    },
    {
      label: 'Skill',
      cell: (s) =>
        h(
          'div.skills-name',
          null,
          h('a', { href: href('skills', s.id) }, s.name),
          s.description ? h('p.skills-desc', null, s.description) : null,
          s.tags.length ? h('div.skills-tags', null, s.tags.map((t) => h('span.skills-tag', null, t))) : null,
          s.archived_ms != null ? badge('Archived') : null,
        ),
    },
    { label: 'Bindings', cell: (s) => (s.bindings.length ? h('div.skills-chips', null, s.bindings.map((b) => bindingChip(b))) : h('span.skills-muted', null, 'Unbound')) },
    { label: 'Live', cell: (s) => (s.active_version != null ? `v${s.active_version}` : h('span.skills-muted', null, 'Unpublished')) },
    {
      label: 'Score',
      cell: (s) => {
        const parts: Node[] = [s.score != null ? badge(String(s.score), scoreTone(s.score)) : h('span.skills-muted', null, '—')];
        if (s.best_version != null && s.best_version !== s.active_version) parts.push(h('span.skills-best', null, `best: v${s.best_version}`));
        return h('div.skills-score', null, parts);
      },
    },
    { label: 'Source', cell: (s) => sourceCell(s.source) },
    { label: 'Updated', cell: (s) => ago(s.updated_ms) },
  ];

  function renderList(): void {
    if (status === 'loading') {
      countLine.textContent = '';
      replace(listBox, h('p.skills-muted', null, 'Loading the skills…'));
      return;
    }
    if (status === 'error') {
      countLine.textContent = '';
      replace(
        listBox,
        h('div.skills-error', { role: 'alert' }, h('p', null, `The skills could not be loaded. ${failure}`), button('Try again', () => void load(), { size: 'sm' })),
      );
      return;
    }
    const shown = visible();
    const total = skills.filter((s) => showArchived || s.archived_ms == null).length;
    countLine.textContent = total ? `Showing ${shown.length} of ${total}` : '';
    if (shown.length) {
      replace(listBox, h('div.skills-table-wrap', null, table(columns, shown, undefined, (s) => s.id)));
      return;
    }
    if (total === 0 && bindingFilter === 'all' && sourceFilter === 'all' && !query) {
      replace(
        listBox,
        h(
          'div.skills-empty',
          null,
          h('h3', null, 'No skills yet'),
          h('p', null, EMPTY_TEXT),
          h(
            'div.skills-actions',
            null,
            button('New skill', () => openNew(), { kind: 'primary' }),
            button('Import', () => openImport()),
            button('From GitHub', () => openGithub()),
          ),
        ),
      );
      return;
    }
    replace(
      listBox,
      h('div.skills-empty', null, h('p', null, 'No skills match these filters.'), button('Clear filters', () => clearFilters(), { size: 'sm' })),
    );
  }

  /** The new-skill dialog: title, optional id, tags, bindings and the SKILL.md that starts from the template. */
  function openNew(): void {
    const titleIn = input({ type: 'text', required: true, autocomplete: 'off', placeholder: 'What the skill is for' });
    const idIn = input({ type: 'text', autocomplete: 'off', placeholder: 'generated from the title' });
    const tagsIn = input({ type: 'text', autocomplete: 'off', placeholder: 'comma, separated' });
    const bindings = bindingsField();
    const bodyIn = textarea({ rows: 14, required: true, placeholder: 'Loading the template…' }) as HTMLTextAreaElement;
    const submit = h('button.btn.primary', { type: 'submit' }, 'Create skill') as HTMLButtonElement;

    titleIn.addEventListener('input', () => {
      idIn.placeholder = slugOf(titleIn.value) || 'generated from the title';
    });
    void rules().then(
      (r) => {
        if (bodyIn.value.trim() === '') bodyIn.value = r.template;
        bodyIn.placeholder = '';
      },
      (err: unknown) => {
        bodyIn.placeholder = '';
        toast(errorText(err), 'error');
      },
    );

    async function create(): Promise<void> {
      submit.disabled = true;
      try {
        const { skill } = await skillsApi.create({
          id: idIn.value.trim() || undefined,
          title: titleIn.value.trim(),
          tags: splitTags(tagsIn.value),
          bindings: bindings.value(),
          skill_md: bodyIn.value,
        });
        closeDialog?.();
        closeDialog = null;
        go('skills', skill.id);
      } catch (err) {
        submit.disabled = false;
        const text = errorText(err);
        toast(err instanceof ApiError && err.status === 409 ? `${text} (the id is already taken)` : text, 'error');
      }
    }

    const form = h(
      'form.skills-form',
      {
        onsubmit: (e: Event) => {
          e.preventDefault();
          void create();
        },
      },
      field('Title', titleIn),
      field('Id', idIn, 'Optional. Leave empty to use the slug of the title.'),
      field('Tags', tagsIn, 'Separate them with commas.'),
      bindings.node,
      field('SKILL.md', bodyIn, 'Starts from the template. Other files can be added in the editor.'),
      h('div.skills-actions', null, button('Cancel', () => closeDialog?.(), { kind: 'ghost' }), submit),
    );
    closeDialog = modal('New skill', form);
  }

  /**
   * The preview of an import: one row per skill found, ticked unless it has errors or already exists; then the bindings
   * for the picks and the Import button.
   */
  function showPreview(preview: SkillPreview, box: HTMLElement): void {
    if (preview.skills.length === 0) {
      replace(box, h('p.skills-error', { role: 'alert' }, 'No SKILL.md was found in it.'));
      return;
    }
    const picks: { path: string; box: HTMLInputElement }[] = [];
    const bindings = bindingsField();
    const commitBtn = h('button.btn.primary', { type: 'button' }) as HTMLButtonElement;

    function sync(): void {
      const n = picks.filter((p) => p.box.checked).length;
      commitBtn.textContent = `Import ${plural(n, 'skill')}`;
      commitBtn.disabled = n === 0;
    }

    async function commit(): Promise<void> {
      const chosen = picks.filter((p) => p.box.checked).map((p) => p.path);
      commitBtn.disabled = true;
      try {
        await skillsApi.importCommit(preview.preview_id, chosen, bindings.value());
        closeDialog?.();
        closeDialog = null;
        toast(`Imported ${plural(chosen.length, 'skill')}.`);
        void load();
      } catch (err) {
        commitBtn.disabled = false;
        toast(errorText(err), 'error');
      }
    }
    commitBtn.addEventListener('click', () => void commit());

    const rows = preview.skills.map((s) => {
      const r = s.report;
      const pick = h('input', { type: 'checkbox', 'aria-label': `Import ${s.path || 'the root'}` }) as HTMLInputElement;
      pick.checked = r.errors === 0 && s.exists == null;
      pick.addEventListener('change', sync);
      picks.push({ path: s.path, box: pick });
      const notes: string[] = [];
      if (r.errors > 0) notes.push('It has errors, so it is unticked.');
      if (s.exists) notes.push(`Already in the library as ${s.exists}, so it is unticked.`);
      return h(
        'div.skills-found-row',
        null,
        h('label.skills-pick', null, pick),
        h(
          'div.skills-found',
          null,
          h(
            'div.skills-found-head',
            null,
            h('strong', null, s.name ?? s.title ?? s.path),
            badge(String(r.score), scoreTone(r.score)),
          ),
          h('code.skills-path', null, s.path || '(the root of the file)'),
          h(
            'div.skills-meta',
            null,
            `${plural(r.errors, 'error')} · ${plural(r.warnings, 'warning')} · ${plural(s.files, 'file')}`,
          ),
          notes.length ? h('p.skills-note', null, notes.join(' ')) : null,
        ),
      );
    });

    replace(
      box,
      h(
        'div.skills-preview',
        null,
        h('p.skills-hint', null, `Found ${plural(preview.skills.length, 'skill')}. Tick the ones to import.`),
        h('div.skills-found-list', null, rows),
        bindings.node,
        h('p.skills-hint', null, 'The bindings apply to every skill picked.'),
        h('div.skills-actions', null, commitBtn),
      ),
    );
    sync();
  }

  /** The import dialog: a .zip or .skill file, read in the browser and previewed by the hub. */
  function openImport(): void {
    const fileIn = h('input', { type: 'file', accept: '.zip,.skill', 'aria-label': 'Zip or .skill file' }) as HTMLInputElement;
    const previewBox = h('div.skills-preview-box');

    fileIn.addEventListener('change', async () => {
      const file = fileIn.files?.[0];
      if (!file) return;
      replace(previewBox, h('p.skills-muted', null, 'Checking the file…'));
      try {
        const preview = await skillsApi.importPreview(await file.arrayBuffer());
        showPreview(preview, previewBox);
      } catch (err) {
        replace(previewBox, h('p.skills-error', { role: 'alert' }, errorText(err)));
      }
    });

    closeDialog = modal(
      'Import skills',
      h(
        'div.skills-form',
        null,
        field('Zip or .skill file', fileIn, 'The hub unpacks it in memory, within the size limits, and finds every folder with a SKILL.md.'),
        previewBox,
      ),
    );
  }

  /** The From GitHub dialog: a repository or folder URL, previewed the same way as a file. */
  function openGithub(): void {
    const urlIn = input({
      type: 'url',
      required: true,
      autocomplete: 'off',
      placeholder: 'https://github.com/owner/repo/tree/main/skills',
      'aria-label': 'GitHub URL',
    });
    const previewBox = h('div.skills-preview-box');

    async function previewGithub(): Promise<void> {
      replace(previewBox, h('p.skills-muted', null, 'Downloading the repo…'));
      try {
        showPreview(await skillsApi.githubPreview(urlIn.value.trim()), previewBox);
      } catch (err) {
        replace(previewBox, h('p.skills-error', { role: 'alert' }, errorText(err)));
      }
    }

    closeDialog = modal(
      'From GitHub',
      h(
        'form.skills-form',
        {
          onsubmit: (e: Event) => {
            e.preventDefault();
            void previewGithub();
          },
        },
        field(
          'Repository or folder',
          urlIn,
          'The hub downloads the repo at its current commit, and nothing in it runs. A token in the vault opens private repos.',
        ),
        h('div.skills-actions', null, h('button.btn.primary', { type: 'submit' }, 'Preview')),
        previewBox,
      ),
    );
  }

  async function load(): Promise<void> {
    const mine = ++seq;
    status = 'loading';
    renderList();
    try {
      const res = await skillsApi.list({ archived: showArchived });
      if (!alive || mine !== seq) return;
      if (!Array.isArray(res?.skills)) throw new Error('The hub did not answer with a skill list (is it an older version?)');
      skills = res.skills;
      for (const id of [...selected]) if (!skills.some((s) => s.id === id)) selected.delete(id);
      status = 'ready';
    } catch (err) {
      if (!alive || mine !== seq) return;
      failure = errorText(err);
      status = 'error';
    }
    renderBindingOptions();
    renderExport();
    renderList();
  }

  const head = h(
    'header.skills-head',
    null,
    h('div', null, h('h1', null, 'Skills'), h('p.skills-intro', null, INTRO)),
    h(
      'div.skills-actions',
      null,
      button('New skill', () => openNew(), { kind: 'primary' }),
      button('Import', () => openImport()),
      button('From GitHub', () => openGithub()),
      exportSlot,
    ),
  );
  const toolbar = h(
    'div.skills-toolbar',
    null,
    h(
      'div.skills-row',
      null,
      h('label.skills-filter', null, h('span', null, 'Binding'), bindingSel),
      h('label.skills-filter', null, h('span', null, 'Source'), sourceSel),
      h('label.skills-toggle', null, archivedIn, ' Show archived'),
    ),
    h('div.skills-row', null, search, countLine),
  );

  replace(root, h('section.skills-view', null, head, toolbar, listBox));
  renderBindingOptions();
  renderExport();
  renderList();
  void load();

  return () => {
    alive = false;
    closeDialog?.();
    closeDialog = null;
  };
}
