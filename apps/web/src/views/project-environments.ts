// A project's Environments tab: where it runs. Each environment is a card with its address, what it runs, what is
// deployed there now, whether it has a deploy hook, its secrets (names only: the values live in the vault), its
// variables and its last five deploys. The owner adds, edits and removes environments, sets secrets and deploys. The
// rules are in environments-model.ts. Archived projects are read only; the others keep their environments after delivery.
// The markup is the prototype's Environments tab (styles/proto/projects.css).

import { ENV_KINDS, projectsApi, type Deploy, type EnvKind, type Environment, type EnvironmentInput, type EnvTrack } from '../api-projects';
import type { Project } from '../api';
import { on } from '../store';
import { ago, attempt, badge, button, field, input, modal, select, textarea, toast } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';
import {
  NAME_RULE,
  TRACK_CHOICES,
  VAR_RULE,
  defaultProtected,
  defaultTrack,
  deployOutcome,
  deployStatusText,
  deployTone,
  kindLabel,
  parseVariables,
  recentDeploys,
  sortEnvironments,
  trackText,
  validName,
  validUrl,
  validVarName,
  variableRows,
  variablesText,
  webUrl,
} from './environments-model';

const DEBOUNCE_MS = 300;

/** A dialog as the prototype draws it: the body, and the actions in the sheet's own footer. */
function dialog(title: string, body: (Node | null)[], foot: (close: () => void) => Node[]): () => void {
  const close = modal(title, h('div.pj-dlg', null, body));
  const scrims = document.querySelectorAll<HTMLElement>('.scrim');
  scrims[scrims.length - 1]?.querySelector('.sheet')?.append(h('footer.sheet-f', null, foot(close)));
  return close;
}

/** A button with a lucide icon (button() takes the console's own icon names only). */
function lbtn(label: string, onClick: (ev: MouseEvent) => void, o: { kind?: 'primary' | 'danger' | 'ghost'; size?: 'sm'; icon?: LucideName } = {}): HTMLButtonElement {
  const b = button(label, onClick, { kind: o.kind, size: o.size });
  if (o.icon) b.insertBefore(lu(o.icon, o.size === 'sm' ? 14 : 15), b.firstChild);
  return b;
}

/** components' empty(), with its icon given as a drawn icon (empty() takes the console's own icon names only). */
function emptyState(drawn: SVGSVGElement, title: string, text?: string): HTMLElement {
  return h('div.empty', null, drawn, h('b', null, title), text ? h('span', null, text) : null);
}

/** A yes or no in a dialog. `onYes` runs only on the yes. */
function confirmDialog(title: string, text: string, label: string, onYes: () => void): void {
  dialog(title, [h('p', null, text)], (close) => [
    lbtn('Keep', () => close(), { kind: 'ghost' }),
    lbtn(
      label,
      () => {
        close();
        onYes();
      },
      { kind: 'danger' },
    ),
  ]);
}

const KINDS: [EnvKind, string][] = ENV_KINDS.map((k) => [k, kindLabel(k)]);
const isWeb = (u: string): boolean => webUrl(u);

/** The Environments tab of project `p`. It loads the environments and redraws when the hub reports a change, debounced;
    it stops listening once it has left the page. */
export function environmentsTab(p: Project): HTMLElement {
  const root = h('div.pj-tab');
  const writable = !p.archived;
  /** Undefined until the first answer. A failed reload keeps the list it had. */
  let list: Environment[] | undefined;
  let failed = false;
  let alive = true;
  let attached = false;
  let debounce = 0;

  const reload = (): void => void load();

  const load = async (): Promise<void> => {
    const got = await attempt(() => projectsApi.environments(p.id));
    if (!alive) return;
    failed = got === undefined;
    if (got) list = sortEnvironments(got);
    draw();
  };

  const intro = (): HTMLElement =>
    h(
      'div.env-intro',
      null,
      h('p', null, 'Where this project runs. Each environment says what it runs, and what was deployed there.'),
      writable ? lbtn('Add an environment', () => openForm(null), { kind: 'primary', icon: 'plus' }) : null,
    );

  /** Adds an environment (`existing` null) or edits one. The name of an existing one is fixed. */
  const openForm = (existing: Environment | null): void => {
    const adding = existing === null;
    const first: EnvKind = existing?.kind ?? 'staging';
    const name = input({ value: existing?.name ?? '', placeholder: 'staging', maxlength: 40, readonly: !adding, autocomplete: 'off' });
    const kind = select(
      KINDS.map(([v, l]) => ({ value: v, label: l })),
      first,
    );
    const url = input({ type: 'url', value: existing?.url ?? '', placeholder: 'https://' });
    const track = select(TRACK_CHOICES, existing?.track ?? defaultTrack(first));
    const prot = h('input', { type: 'checkbox' });
    prot.checked = existing?.protected ?? defaultProtected(first);
    const hadHook = existing?.hook ?? false;
    const hook = input({ type: 'url', placeholder: hadHook ? 'Set — leave empty to keep it' : 'https://', autocomplete: 'off' });
    const dropHook = h('input', { type: 'checkbox' });
    const vars = textarea({ rows: 4, placeholder: 'NAME=value', spellcheck: 'false' });
    vars.value = variablesText(existing?.variables ?? {});
    const notes = textarea({ rows: 2 });
    notes.value = existing?.notes ?? '';

    // The track and the protection follow the kind while the owner has not changed them from the kind's default.
    let shown: EnvKind = first;
    kind.addEventListener('change', () => {
      const next = kind.value as EnvKind;
      if (track.value === defaultTrack(shown)) track.value = defaultTrack(next);
      if (prot.checked === defaultProtected(shown)) prot.checked = defaultProtected(next);
      shown = next;
    });

    dialog(
      adding ? 'Add an environment' : `Edit ${existing?.name ?? ''}`,
      [
        field('Name', name, adding ? NAME_RULE : 'A name is fixed once it is made.'),
        field('Kind', kind),
        field('URL', url, 'Where it is reached. Optional.'),
        field('Runs', track, 'What lands here: each landing on main, each signed release, or what you deploy by hand.'),
        h('label.check', null, prot, h('span', null, 'Protected. A deploy here waits for an approval, unless you start it yourself.')),
        field('Deploy hook URL', hook, 'Where a deploy is sent. It is kept in the vault: the console shows only that it is set.'),
        hadHook ? h('label.check', null, dropHook, h('span', null, 'Remove the hook')) : null,
        field('Variables', vars, `One NAME=value per line. ${VAR_RULE} Variables show on the card: keep private values in a secret.`),
        field('Notes', notes),
      ],
      (close) => [
        lbtn(
          'Save',
          () => {
            const envName = adding ? name.value.trim() : (existing?.name ?? '');
            if (adding && !validName(envName)) {
              toast(`The name is not valid. ${NAME_RULE}`, 'error');
              return;
            }
            if (!validUrl(url.value.trim())) {
              toast('The URL must start with https:// or http://', 'error');
              return;
            }
            if (hook.value.trim() && !webUrl(hook.value.trim())) {
              toast('The deploy hook must start with https:// or http://', 'error');
              return;
            }
            const parsed = parseVariables(vars.value);
            if (parsed.error) {
              toast(parsed.error, 'error');
              return;
            }
            const body: EnvironmentInput = {
              kind: kind.value as EnvKind,
              url: url.value.trim(),
              track: track.value as EnvTrack,
              protected: prot.checked,
              variables: parsed.vars,
              notes: notes.value.trim(),
            };
            if (dropHook.checked) body.hook = null;
            else if (hook.value.trim()) body.hook = hook.value.trim();
            void (async () => {
              const saved = await attempt(() => projectsApi.putEnvironment(p.id, envName, body), `${envName} saved`);
              if (saved) {
                close();
                reload();
              }
            })();
          },
          { kind: 'primary' },
        ),
      ],
    );
  };

  /** Asks for a deploy: an empty version is the newest signed release, or main's head. */
  const openDeploy = (e: Environment): void => {
    const version = input({ placeholder: 'Empty: the newest signed release or main’s head', autocomplete: 'off' });
    dialog(
      `Deploy ${e.name}`,
      [h('p', null, e.protected ? 'You are the owner: this deploys at once.' : 'It goes to its deploy hook when it has one, and is only recorded when it has none.'), field('Version', version)],
      (close) => [
        lbtn(
          'Deploy',
          () => {
            void (async () => {
              const d = await attempt(() => projectsApi.deploy(p.id, e.name, version.value.trim() || undefined));
              if (!d) return;
              const out = deployOutcome(e.name, d.status);
              toast(out.text, out.error ? 'error' : 'info');
              close();
              reload();
            })();
          },
          { kind: 'primary', icon: 'play' },
        ),
      ],
    );
  };

  /** The secrets of one environment: names are listed, values are typed in once and never read back. */
  const openSecret = (e: Environment): void => {
    const nameIn = input({ placeholder: 'STRIPE_KEY', maxlength: 64, autocomplete: 'off' });
    const value = input({ type: 'password', autocomplete: 'new-password' });
    const askRemove = (n: string): void => {
      close();
      confirmDialog(`Remove ${n} from the vault?`, `${n} is taken out of ${e.name}. Its value is removed too.`, 'Remove', () => {
        void (async () => {
          const ok = await attempt(() => projectsApi.deleteEnvSecret(p.id, e.name, n), `${n} removed`);
          if (ok) reload();
        })();
      });
    };
    const close = dialog(
      `A secret for ${e.name}`,
      [
        h('p', null, 'The value goes into the vault and is never shown again.'),
        e.secrets.length
          ? h(
              'div.env-sec-row',
              null,
              h('span.pj-faint', null, 'In the vault'),
              e.secrets.map((n) => h('span.env-chip', null, h('code', null, n), writable ? lbtn('Remove…', () => askRemove(n), { size: 'sm', kind: 'ghost' }) : null)),
            )
          : null,
        field('Name', nameIn, VAR_RULE),
        field('Value', value),
      ],
      (close) => [
        lbtn(
          'Save secret',
          () => {
            const n = nameIn.value.trim();
            if (!validVarName(n)) {
              toast(`The secret name is not valid. ${VAR_RULE}`, 'error');
              return;
            }
            if (!value.value) {
              toast('Give the secret a value', 'error');
              return;
            }
            void (async () => {
              const saved = await attempt(() => projectsApi.putEnvSecret(p.id, e.name, n, value.value), `${n} is in the vault`);
              if (saved) {
                close();
                reload();
              }
            })();
          },
          { kind: 'primary' },
        ),
      ],
    );
  };

  const askRemove = (e: Environment): void =>
    confirmDialog(`Remove ${e.name} and its secrets?`, 'This cannot be undone.', 'Remove', () => {
      void (async () => {
        const ok = await attempt(() => projectsApi.deleteEnvironment(p.id, e.name), `${e.name} removed`);
        if (ok) reload();
      })();
    });

  const deployRow = (d: Deploy): HTMLElement =>
    h(
      'div.env-dr',
      null,
      h('span.chip.t-muted.env-ver', null, d.version || '—'),
      badge(deployStatusText(d.status), deployTone(d.status)),
      h('span.muted', null, `${d.by} · ${ago(d.at_ms)}`),
      d.detail ? h('span.env-dd', null, d.detail) : null,
    );

  const envCard = (e: Environment): HTMLElement => {
    const url = e.url ?? '';
    const secrets = e.secrets ?? [];
    const vars = variableRows(e.variables ?? {});
    const deploys = recentDeploys(e.deploys ?? []);
    return h(
      'article.env-card',
      null,
      h('header.env-h', null, h('h2', null, e.name), h('span.env-chips', null, badge(kindLabel(e.kind), 'muted'), e.protected ? badge('Protected', 'info') : null)),
      url ? h('div.env-url', null, isWeb(url) ? h('a', { href: url, target: '_blank', rel: 'noopener' }, h('span', null, url), lu('external-link', 12)) : url) : null,
      h(
        'dl.env-kvg',
        null,
        h('dt', null, 'Runs'),
        h('dd', null, trackText(e.track)),
        h('dt', null, 'Now'),
        h('dd', null, e.version ? `${e.version} · ${ago(e.deployed_ms)}` : 'Nothing deployed yet'),
        h('dt', null, 'Deploy hook'),
        h('dd', null, e.hook ? 'set' : 'not set: deploys are recorded only'),
      ),
      h(
        'div.env-sec',
        null,
        h('span.pj-faint', null, 'Secrets'),
        secrets.length ? secrets.map((x) => h('code.env-secret', null, x)) : h('span.pj-faint', null, 'None'),
        writable ? lbtn('Secret…', () => openSecret(e), { size: 'sm', kind: 'ghost', icon: 'key-round' }) : null,
      ),
      vars.length ? h('dl.env-kvg', null, vars.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])) : null,
      e.notes ? h('p.env-notes', null, e.notes) : null,
      h('div.env-dep', null, h('span.pj-faint', null, 'Recent deploys'), deploys.length ? deploys.map(deployRow) : h('p.pj-faint', null, 'No deploys yet')),
      writable
        ? h(
            'footer.env-acts',
            null,
            lbtn('Remove…', () => askRemove(e), { size: 'sm', kind: 'ghost', icon: 'trash' }),
            lbtn('Edit…', () => openForm(e), { size: 'sm', icon: 'edit' }),
            lbtn('Deploy…', () => openDeploy(e), { size: 'sm', kind: 'primary', icon: 'play' }),
          )
        : null,
    );
  };

  function draw(): void {
    if (!alive) return;
    if (root.isConnected) attached = true;
    let rest: Node[];
    if (list === undefined) {
      rest = [h('p.pj-faint', null, failed ? 'The environments could not be loaded. Try again in a moment.' : 'Loading the environments…')];
    } else if (!list.length) {
      rest = [
        emptyState(lu('box', 22), 'No environments yet', writable ? 'Add production, staging or a preview: its address, what it runs, and how it is deployed.' : 'This project has none.'),
      ];
    } else {
      rest = [h('div.env-list', null, list.map(envCard))];
    }
    replace(root, intro(), rest);
  }

  // Live: a change the hub reports redraws the tab soon. Once the tab has left the page, it stops listening.
  const stop = (): void => {
    alive = false;
    off();
    watch.disconnect();
    window.clearTimeout(debounce);
  };
  const off = on('environment', (raw) => {
    const pid = raw && typeof raw === 'object' ? (raw as { project?: unknown }).project : undefined;
    if (typeof pid === 'string' && pid !== p.id) return;
    if (!root.isConnected && attached) {
      stop();
      return;
    }
    window.clearTimeout(debounce);
    debounce = window.setTimeout(reload, DEBOUNCE_MS);
  });
  // A tab that leaves the page without a change to hear about still stops here.
  const watch = new MutationObserver(() => {
    if (root.isConnected) attached = true;
    else if (attached) stop();
  });
  watch.observe(document.body, { childList: true, subtree: true });

  draw();
  void load();
  return root;
}
