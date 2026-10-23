// New project: three ways to begin. Talk it through with the orchestrator (its chat, opened in the IDE, where it drafts the
// project in the planner's chat beside it; files can go with the first message), open one directly with a title and a
// goal (files picked here are its first change), or bring a project that exists: a repository on a git platform, a git
// address, a folder on the hub, or files from this computer, with its mode, its environments and whether it is finished.

import '../styles/proto/newproject.css';
import { api } from '../api';
import { workApi, MAIN_CHAT } from '../api-work';
import { ENV_KINDS, projectsApi, type ProjectMode } from '../api-projects';
import { platforms } from '../api-platforms';
import { openTalk } from '../ide/open';
import { hrefIn } from '../router';
import { refreshProjects } from '../scope';
import { attempt, button, field, input, modal, select, textarea, toast } from './components';
import { h } from './dom';
import { attachFiles, dropZone, pickFiles, sizeText, uploadToProject, type PickedFile, type UploadProgress } from './file-drop';
import {
  MAX_ENVIRONMENTS,
  WHERE_KINDS,
  adoptInput,
  authorizationLabel,
  filesLine,
  grantedRepos,
  importLine,
  type BringForm,
  type WhereKind,
} from './new-project-model';

const MODE_HINTS: Record<ProjectMode, string> = {
  maintenance: 'Agents watch it and propose; you pick what is done',
  build: 'Agents keep building it',
};

const plural = (n: number, one: string, many = `${one}s`): string => `${n} ${n === 1 ? one : many}`;
const capitalize = (text: string): string => text.charAt(0).toUpperCase() + text.slice(1);
const messageOf = (err: unknown): string => (err instanceof Error ? err.message : String(err));

/** A line that shows while files are sent; hidden until there is something to say. */
function statusLine(): HTMLParagraphElement {
  const el = h('p.np-muted', null);
  el.hidden = true;
  return el;
}

function progressTo(el: HTMLElement): (p: UploadProgress) => void {
  return (p) => {
    el.hidden = false;
    el.textContent = `Sending ${p.done} of ${p.total} files…`;
  };
}

/** A picked file as a chip: its name, its size, and × to take it off the list. */
function chip(f: PickedFile, onRemove: () => void): HTMLElement {
  const remove = button('×', onRemove, { kind: 'ghost', size: 'sm', title: 'Remove' });
  return h('li.np-chip', null, h('span.np-chip-n', { title: f.path }, f.path), h('span.np-muted', null, sizeText(f.file.size)), remove);
}

/** A field whose control holds buttons: a div, so a click on the label is not a click on a button. */
function fieldBox(label: string, el: HTMLElement): HTMLElement {
  return h('div.field', null, h('span.field-l', null, label), el);
}

function checkbox(text: string, checked: boolean): { el: HTMLLabelElement; box: HTMLInputElement } {
  const box = h('input', { type: 'checkbox' });
  box.checked = checked;
  return { el: h('label.check', null, box, h('span', null, text)), box };
}

/** Files picked or dropped on a zone. `chips`: they show as chips with ×; otherwise as one line with a Clear. */
interface FileBox {
  el: HTMLElement;
  files: () => PickedFile[];
  add: (files: PickedFile[]) => void;
}

function fileBox(opts: { text: string; folders: boolean; chips?: boolean }): FileBox {
  let picked: PickedFile[] = [];
  const list = h('ul.np-chips');
  const summaryText = h('div.np-sum', null, '');
  const summary = h('div.np-row', null, summaryText, button('Clear', () => set([]), { kind: 'ghost', size: 'sm' }));
  summary.hidden = true;
  const draw = (): void => {
    if (opts.chips) {
      list.replaceChildren(...picked.map((f, i) => chip(f, () => set(picked.filter((_, j) => j !== i)))));
    } else {
      summaryText.textContent = filesLine(picked);
      summary.hidden = picked.length === 0;
    }
  };
  const set = (next: PickedFile[]): void => {
    picked = next;
    draw();
  };
  const add = (more: PickedFile[]): void => {
    if (more.length) set([...picked, ...more]);
  };
  const pickers = [button('Choose files', () => void pickFiles().then(add), { kind: 'ghost', size: 'sm' })];
  if (opts.folders) pickers.push(button('Choose a folder', () => void pickFiles({ folder: true }).then(add), { kind: 'ghost', size: 'sm' }));
  const zone = h('div.np-zone', null, h('div', null, opts.text), h('div.np-row', null, pickers));
  dropZone(zone, add);
  return { el: h('div.np-files', null, zone, opts.chips ? list : summary), files: () => picked, add };
}

export function openNewProject(): void {
  // ---- Talk it through: the orchestrator's chat, with files on the first message.
  const ask = textarea({ rows: 4, placeholder: 'What do you want done? “The scheduler tests are flaky and I want 0.9 out this week.”' });
  const talkFiles = fileBox({ text: 'Drop files here or choose them', folders: false, chips: true });
  dropZone(ask, talkFiles.add);
  const talk = button('Start a chat', () => void startTalk(), { kind: 'primary', icon: 'conversations' });
  const startTalk = async (): Promise<void> => {
    const text = ask.value.trim();
    if (!text) {
      toast('Say what you want done first', 'error');
      return;
    }
    talk.disabled = true;
    const picked = talkFiles.files();
    const attached = picked.length ? await attempt(() => attachFiles(picked)) : [];
    if (!attached) {
      talk.disabled = false;
      return;
    }
    // The orchestrator's chat is the owner's one line to it: the words go there, and the chat opens.
    const said = await attempt(() => workApi.say(MAIN_CHAT, text, attached.map((a) => a.id)));
    if (!said) {
      talk.disabled = false;
      return;
    }
    close();
    openTalk(MAIN_CHAT);
  };

  // ---- Or open it directly: an empty project of the owner's, with its first files if any.
  const title = input({ placeholder: 'Release 0.9' });
  const goal = textarea({ rows: 3, placeholder: 'What the project delivers' });
  const kind = select(
    [
      { value: 'code', label: 'Code (it gets a repository)' },
      { value: 'other', label: 'Other' },
    ],
    'code',
  );
  const directFiles = fileBox({ text: 'Drop files or a folder here', folders: true });
  const directStatus = statusLine();
  const direct = button('Open project', () => void openDirect(), { kind: 'primary' });
  const openDirect = async (): Promise<void> => {
    const t = title.value.trim();
    const g = goal.value.trim();
    if (!t || !g) {
      toast('A project needs a title and a goal', 'error');
      return;
    }
    direct.disabled = true;
    const project = await attempt(() => api.createProject({ title: t, goal: g, kind: kind.value as 'code' | 'other' }));
    if (!project) {
      direct.disabled = false;
      return;
    }
    const picked = directFiles.files();
    let sent = 0;
    let failure = '';
    if (picked.length) {
      try {
        sent = (await uploadToProject(project.id, picked, { title: 'First files', onProgress: progressTo(directStatus) })).added?.files ?? 0;
      } catch (err) {
        failure = messageOf(err);
      }
      directStatus.hidden = true;
    }
    if (failure) toast(`Project ${t} opened, but its files were not sent: ${failure}`, 'error');
    else toast(sent ? `Project ${t} opened with ${plural(sent, 'file')}` : `Project ${t} opened`);
    close();
    await refreshProjects();
    location.hash = hrefIn(project.id, 'projects', project.id);
  };

  // ---- Or bring a project that exists.
  const bringTitle = input({ placeholder: 'Shop' });
  const bringGoal = textarea({ rows: 2, placeholder: 'What it delivers' });
  const where = select(WHERE_KINDS, 'platform');

  // Where it is: a repository on the owner's git platform. Its accounts load when the dialog opens.
  const authSelect = select([{ value: '', label: 'Loading…' }]);
  const repoSelect = select([{ value: '', label: 'Loading…' }]);
  const signBox = checkbox("Sign exports with the hub's key on my account", false);
  const platformFields = h(
    'div.np-group',
    null,
    field('Account', authSelect),
    field('Repository', repoSelect),
    signBox.el,
  );
  const noAuthText = h('span', null, 'No git platform is set up yet. ');
  const noAuth = h('p.np-muted', null, noAuthText, h('a.np-link', { href: '#/platforms' }, 'Open Platforms'));
  noAuth.hidden = true;
  noAuth.querySelector('a')?.addEventListener('click', () => close());
  const platformBox = h('div.np-group', null, platformFields, noAuth);

  // Where it is: a git address, or a folder on the hub.
  const gitInput = input({ placeholder: 'https://…' });
  const gitBranch = input({ value: 'main' });
  const gitBox = h('div.np-group', null, field('Git address', gitInput), field('Branch', gitBranch));
  const folderInput = input({ placeholder: '/Users/me/code/shop' });
  const pathBranch = input({ value: 'main' });
  const pathBox = h('div.np-group', null, field("Folder on the hub's machine", folderInput, 'An absolute path, starting with /'), field('Branch', pathBranch));

  // Where it is: files from this computer, uploaded after the adoption.
  const bringFiles = fileBox({ text: 'Drop files or a folder here', folders: true });
  const filesBox = h('div.np-group', null, fieldBox('Files', bringFiles.el));

  const showWhere = (): void => {
    const k = where.value as WhereKind;
    platformBox.hidden = k !== 'platform';
    gitBox.hidden = k !== 'git';
    pathBox.hidden = k !== 'path';
    filesBox.hidden = k !== 'files';
  };
  where.addEventListener('change', showWhere);
  showWhere();

  let repoLoad = 0;
  const loadRepos = async (id: string): Promise<void> => {
    const token = ++repoLoad;
    repoSelect.disabled = true;
    repoSelect.replaceChildren(h('option', { value: '' }, 'Loading…'));
    // Read in the background while the dialog is open: a platform that does not answer says so in the list, not in a
    // toast over the dialog.
    let failed = false;
    const quiet = <T,>(load: () => Promise<T>): Promise<T | null> =>
      load().catch(() => {
        failed = true;
        return null;
      });
    const namespaces = await quiet(() => platforms.namespaces(id));
    const repos = await Promise.all((namespaces ?? []).filter((ns) => ns.grant).map((ns) => quiet(() => platforms.repos(id, ns.name))));
    if (token !== repoLoad) return;
    const names = grantedRepos(repos.flatMap((list) => list ?? []));
    if (names.length) repoSelect.replaceChildren(...names.map((n) => h('option', { value: n }, n)));
    else repoSelect.replaceChildren(h('option', { value: '' }, failed ? 'The platform did not answer' : 'No repositories granted yet'));
    repoSelect.disabled = names.length === 0;
  };
  authSelect.addEventListener('change', () => void loadRepos(authSelect.value));

  const loadAuthorizations = async (): Promise<void> => {
    const list = await attempt(() => platforms.list());
    if (!list) noAuthText.textContent = 'The git platforms could not be read. ';
    const integrations = list?.integrations ?? [];
    authSelect.replaceChildren(...integrations.map((i) => h('option', { value: i.id }, authorizationLabel(i))));
    const none = integrations.length === 0;
    platformFields.hidden = none;
    noAuth.hidden = !none;
    if (!none) await loadRepos(authSelect.value);
  };

  // Mode, finished, environments.
  const mode = select(
    [
      { value: 'maintenance', label: 'Maintenance' },
      { value: 'build', label: 'Build' },
    ],
    'maintenance',
  );
  const modeHint = h('small.hint', null, MODE_HINTS.maintenance);
  mode.addEventListener('change', () => {
    modeHint.textContent = MODE_HINTS[mode.value as ProjectMode];
  });
  const modeField = h('label.field', null, h('span.field-l', null, 'Mode'), mode, modeHint);
  const finished = checkbox('It is finished', true);
  const finishedHint = h('p.np-muted', null, 'On: it opens delivered. Off: it stays in implementation.');

  type EnvRow = { row: HTMLElement; name: HTMLInputElement; kind: HTMLSelectElement; url: HTMLInputElement };
  const envRows: EnvRow[] = [];
  const envKinds = ENV_KINDS.map((k) => ({ value: k, label: capitalize(k) }));
  const envList = h('div.np-envs');
  const syncAdd = (): void => {
    addEnv.disabled = envRows.length >= MAX_ENVIRONMENTS;
  };
  const addRow = (): void => {
    if (envRows.length >= MAX_ENVIRONMENTS) return;
    const rec: EnvRow = {
      row: h('div.np-env'),
      name: input({ placeholder: 'production' }),
      kind: select(envKinds, 'production'),
      url: input({ placeholder: 'https://…', class: 'np-url' }),
    };
    const drop = button(
      '',
      () => {
        envRows.splice(envRows.indexOf(rec), 1);
        rec.row.remove();
        syncAdd();
      },
      { kind: 'ghost', icon: 'x', title: 'Remove environment' },
    );
    drop.classList.add('np-rm');
    rec.row.append(rec.name, rec.kind, rec.url, drop);
    envRows.push(rec);
    envList.append(rec.row);
    syncAdd();
  };
  const addEnv = button('+ Add', () => addRow(), { kind: 'ghost', size: 'sm' });
  const envBox = h(
    'div.np-group',
    null,
    h('div.field-l', null, 'Environments (optional)'),
    h('p.np-muted', null, 'Each has a name, a kind and an address. Empty rows are ignored.'),
    envList,
    h('div.np-row', null, addEnv),
  );

  const bringStatus = statusLine();
  const readBringForm = (): BringForm => ({
    title: bringTitle.value,
    goal: bringGoal.value,
    where: where.value as WhereKind,
    platform: authSelect.value,
    repo: repoSelect.value,
    sign: signBox.box.checked,
    git: gitInput.value,
    folder: folderInput.value,
    branch: where.value === 'path' ? pathBranch.value : gitBranch.value,
    mode: mode.value as ProjectMode,
    finished: finished.box.checked,
    environments: envRows.map((r) => ({ name: r.name.value, kind: r.kind.value, url: r.url.value })),
  });
  const bring = button('Bring it in', () => void bringIn(), { kind: 'primary' });
  const bringIn = async (): Promise<void> => {
    const form = readBringForm();
    const built = adoptInput(form);
    const payload = built.input;
    if (!payload) {
      toast(built.error ?? 'Check the form', 'error');
      return;
    }
    if (form.where === 'files' && bringFiles.files().length === 0) {
      toast('Choose the files or the folder first', 'error');
      return;
    }
    bring.disabled = true;
    const adopted = await attempt(() => projectsApi.adopt(payload));
    if (!adopted) {
      bring.disabled = false;
      return;
    }
    const picked = form.where === 'files' ? bringFiles.files() : [];
    let sent = 0;
    let failure = '';
    if (picked.length) {
      try {
        sent = (await uploadToProject(adopted.project.id, picked, { title: 'First files', onProgress: progressTo(bringStatus) })).added?.files ?? 0;
      } catch (err) {
        failure = messageOf(err);
      }
      bringStatus.hidden = true;
    }
    const parts = [importLine(adopted) || `Project ${payload.title} opened`];
    if (sent) parts.push(`${plural(sent, 'file')} sent`);
    if (failure) parts.push(`Files were not sent: ${failure.replace(/\.+$/, '')}`);
    toast(parts.join('. '), adopted.problems.length > 0 || failure !== '' ? 'error' : 'info');
    close();
    await refreshProjects();
    location.hash = hrefIn(adopted.project.id, 'projects', adopted.project.id);
  };

  const close = modal(
    'New project',
    h(
      'div.np-root',
      null,
      h(
        'section.np-sec',
        { 'aria-label': 'Talk it through' },
        h('h3.np-h', null, 'Talk it through'),
        h('p.np-p', null, 'The orchestrator asks what it needs and drafts the project beside the chat: its tasks and its budget. You start it when it looks right.'),
        field('Your goal', ask),
        fieldBox('Files', talkFiles.el),
        h('div.np-foot', null, talk),
      ),
      h(
        'section.np-sec',
        { 'aria-label': 'Or open it directly' },
        h('h3.np-h', null, 'Or open it directly'),
        h('p.np-p', null, 'A project of yours: you put agents on it and give it tasks. Files you add become its first change.'),
        field('Title', title),
        field('Goal', goal),
        field('Kind', kind),
        fieldBox('Files (optional)', directFiles.el),
        directStatus,
        h('div.np-foot', null, direct),
      ),
      h(
        'section.np-sec',
        { 'aria-label': 'Or bring a project that exists' },
        h('h3.np-h', null, 'Or bring a project that exists'),
        h('p.np-p', null, 'A product that is finished, or nearly: the plaza imports its history and looks after it.'),
        field('Title', bringTitle),
        field('Goal (optional)', bringGoal),
        field('Where it is', where),
        platformBox,
        gitBox,
        pathBox,
        filesBox,
        modeField,
        h('div.np-group', null, finished.el, finishedHint),
        envBox,
        bringStatus,
        h('div.np-foot', null, bring),
      ),
    ),
  );
  void loadAuthorizations();
}
