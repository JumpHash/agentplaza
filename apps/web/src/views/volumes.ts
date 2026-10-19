// Setup > Volumes: the shared read-only data volumes on the hub machine, and the roles that mount them. A volume is an
// ext4 image or a directory on the hub; an agent of a role mounts each volume the role names at /data/<name>. A role
// change applies to the agents spawned from now on. The pure helpers (the name rule, the role list parser) are exported
// and unit-tested (test/tests.ts).

import '../styles/proto/settings-d.css';
import { api, volumesApi, type Agent, type DataVolume, type Settings } from '../api';
import { card, field, input, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText } from './vault-common';
import { btn, dialog, empty, head, ptable, type PCol } from './workstations-ui';

interface RoleRow { role: string; box: HTMLInputElement }

export const VOLUME_NAME_RE = /^[a-z0-9][a-z0-9-]{0,19}$/;
export const MAX_ROLE_VOLUMES = 4;
const MIB = 1024 * 1024;

/** A volume name: a-z, 0-9 and dashes, starting with a letter or digit, up to 20 characters. */
export function volumeNameOk(name: string): boolean {
  return VOLUME_NAME_RE.test(name);
}

/**
 * A role's volume list as typed: names separated by commas or spaces, blank parts dropped. Every name must be valid, no
 * name may repeat, and a role takes at most four. On a problem the error names it and the names are empty.
 */
export function parseVolumeList(text: string): { names: string[]; error?: string } {
  const names: string[] = [];
  for (const part of text.split(/[\s,]+/)) {
    if (!part) continue;
    if (!volumeNameOk(part)) return { names: [], error: `"${part}" is not a volume name: a-z, 0-9 and dashes, up to 20 characters` };
    if (names.includes(part)) return { names: [], error: `"${part}" is listed twice` };
    names.push(part);
  }
  if (names.length > MAX_ROLE_VOLUMES) return { names: [], error: `A role takes at most ${MAX_ROLE_VOLUMES} volumes` };
  return { names };
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let volumes: DataVolume[] | null = null;
  let volumesError: unknown = null;
  let settings: Settings | null = null;
  let settingsError: unknown = null;
  // The agents' names by CID, for the volumes each one mounts.
  let names = new Map<number, string>();

  // The error line of each card: a failed create or removal, a failed save of a role.
  const volError = h('div.e2b-err');
  const rolesError = h('div.e2b-err');
  const volumesSlot = h('div');
  const rolesSlot = h('div');
  const agentName = (cid: number): string => names.get(cid) ?? `agent ${cid}`;

  // ---- Volumes --------------------------------------------------------------------------------------------------------

  const loadVolumes = async (): Promise<void> => {
    try {
      volumes = await volumesApi.list();
      volumesError = null;
    } catch (err) {
      volumes = null;
      volumesError = err;
    }
    if (alive) drawVolumes();
  };

  const loadAgents = async (): Promise<void> => {
    const ag: Agent[] = await api.agents().catch((): Agent[] => []);
    if (!alive) return;
    names = new Map(ag.map((a): [number, string] => [a.cid, a.name]));
    drawVolumes();
  };

  const doRemove = async (v: DataVolume): Promise<void> => {
    replace(volError);
    try {
      await volumesApi.remove(v.name);
      if (!alive) return;
      toast(`Removed ${v.name}`);
    } catch (err) {
      if (alive) replace(volError, errorText(err));
    }
    if (alive) void loadVolumes();
  };

  const removeVolume = (v: DataVolume): void => {
    const close = dialog(`Remove ${v.name}`, h('p', null, 'The volume is removed from the hub.'), [
      btn('Cancel', () => close(), { kind: 'ghost' }),
      btn(
        'Remove',
        () => {
          close();
          void doRemove(v);
        },
        { kind: 'danger' },
      ),
    ]);
  };

  const volumeColumns = (): PCol<DataVolume>[] => [
    { label: 'Name', cell: (v) => h('b', null, v.name) },
    { label: 'Size', cls: 'num', cell: (v) => h('span', { style: 'white-space:nowrap' }, `${(v.size_bytes / MIB).toFixed(1)} MiB`) },
    { label: 'Mounted by', cell: (v) => (v.in_use.length ? v.in_use.map(agentName).join(', ') : '—') },
    { label: 'Source', cell: (v) => h('span.ws-mono', null, v.source) },
    // A volume an agent mounts stays until that agent lets go of it.
    {
      label: '',
      cls: 'num',
      cell: (v) => btn('Remove', () => removeVolume(v), { kind: 'danger', sm: true, icon: 'trash', disabled: v.in_use.length > 0 }),
    },
  ];

  const drawVolumes = (): void => {
    if (volumesError) {
      replace(volumesSlot, empty('alert', 'Could not load the volumes', errorText(volumesError)));
      return;
    }
    if (!volumes) return;
    replace(volumesSlot, volumes.length ? ptable(volumeColumns(), volumes) : empty('wrench', 'No volumes yet', 'Create one below.'));
  };

  const nameIn = input({ id: 'vol-name', placeholder: 'deps', spellcheck: 'false' });
  const sourceIn = input({ id: 'vol-source', placeholder: '/srv/volumes/deps.ext4', spellcheck: 'false' });

  const createVolume = async (): Promise<void> => {
    replace(volError);
    const name = nameIn.value.trim();
    const source = sourceIn.value.trim();
    if (!volumeNameOk(name)) {
      replace(volError, 'A name is a-z, 0-9 and dashes, up to 20 characters.');
      return;
    }
    if (!source.startsWith('/')) {
      replace(volError, 'The source is an absolute path on the hub machine.');
      return;
    }
    try {
      await volumesApi.create(name, source);
      if (!alive) return;
      toast(`Created ${name}`);
      nameIn.value = '';
      sourceIn.value = '';
    } catch (err) {
      if (alive) replace(volError, errorText(err));
      return;
    }
    if (alive) void loadVolumes();
  };

  // ---- Roles ----------------------------------------------------------------------------------------------------------

  const loadSettings = async (): Promise<void> => {
    try {
      settings = await api.settings();
      settingsError = null;
    } catch (err) {
      settings = null;
      settingsError = err;
    }
    if (alive) drawRoles();
  };

  /** A copy of the loaded settings with the role map changed. The save sends the whole settings, so the copy is the change. */
  const withRoles = (change: (map: Record<string, string[]>) => void): Settings | null => {
    if (!settings) return null;
    const map = { ...(settings.role_volumes ?? {}) };
    change(map);
    return { ...settings, role_volumes: map };
  };

  const saveRoles = async (copy: Settings, done: string): Promise<void> => {
    replace(rolesError);
    try {
      await api.saveSettings(copy);
      if (!alive) return;
      toast(done);
    } catch (err) {
      if (alive) replace(rolesError, errorText(err));
      return;
    }
    if (alive) void loadSettings();
  };

  const saveRole = (role: string, list: HTMLInputElement): void => {
    replace(rolesError);
    const parsed = parseVolumeList(list.value);
    if (parsed.error) {
      replace(rolesError, parsed.error);
      return;
    }
    const copy = withRoles((map) => {
      if (parsed.names.length) map[role] = parsed.names;
      else delete map[role];
    });
    if (copy) void saveRoles(copy, `Saved ${role}`);
  };

  const removeRole = (role: string): void => {
    const copy = withRoles((map) => {
      delete map[role];
    });
    if (copy) void saveRoles(copy, `Removed ${role}`);
  };

  const roleIn = input({ id: 'vol-new-role', placeholder: 'coder', spellcheck: 'false' });
  const addList = input({ id: 'vol-new-vols', placeholder: 'deps, models', spellcheck: 'false' });

  const addRole = (): void => {
    replace(rolesError);
    const role = roleIn.value.trim();
    if (!role) {
      replace(rolesError, 'Name the role.');
      return;
    }
    if (settings?.role_volumes?.[role]) {
      replace(rolesError, `${role} already has a row: change its volumes there.`);
      return;
    }
    const parsed = parseVolumeList(addList.value);
    if (parsed.error) {
      replace(rolesError, parsed.error);
      return;
    }
    if (!parsed.names.length) {
      replace(rolesError, 'Name at least one volume.');
      return;
    }
    const copy = withRoles((map) => {
      map[role] = parsed.names;
    });
    if (copy) void saveRoles(copy, `Added ${role}`);
  };

  const drawRoles = (): void => {
    if (settingsError) {
      replace(rolesSlot, empty('alert', 'Could not load the roles', errorText(settingsError)));
      return;
    }
    if (!settings) return;
    const entries = Object.entries(settings.role_volumes ?? {});
    if (!entries.length) {
      replace(rolesSlot, empty('wrench', 'No role mounts a volume', 'Add a role below.'));
      return;
    }
    const rows: RoleRow[] = entries.map(([role, list]) => ({
      role,
      box: input({ id: `vol-row-${role}`, 'aria-label': `Volumes for ${role}`, spellcheck: 'false', value: list.join(', ') }),
    }));
    replace(
      rolesSlot,
      ptable<RoleRow>(
        [
          { label: 'Role', cell: (r) => r.role },
          { label: 'Volumes', cell: (r) => r.box },
          {
            label: '',
            cls: 'num',
            cell: (r) => [
              btn('Save', () => saveRole(r.role, r.box), { kind: 'ghost', sm: true }),
              btn('Remove', () => removeRole(r.role), { kind: 'danger', sm: true, icon: 'trash' }),
            ],
          },
        ],
        rows,
      ),
    );
  };

  root.append(
    head(
      'Volumes',
      [],
      'A volume is a read-only data set on the hub machine: an ext4 image or a directory. An agent of a role mounts each volume the role names at /data/<name>. A role change applies to the agents spawned from now on.',
    ),
    h(
      'div.vol-grid',
      null,
      card(
        'Volumes',
        h(
          'div.ws-stack',
          null,
          volumesSlot,
          h(
            'div.ws-stack',
            null,
            volError,
            field('Name', nameIn, 'What agents see under /data/<name>. Up to 20 characters.'),
            field('Source', sourceIn, 'An ext4 image or a directory on the hub machine.'),
            h('div.vol-act', null, btn('Create volume', () => void createVolume())),
          ),
        ),
      ),
      card(
        'Roles',
        h(
          'div.ws-stack',
          null,
          rolesError,
          rolesSlot,
          h(
            'div.ws-stack',
            null,
            field('Role', roleIn),
            field('Volumes', addList, 'Up to four volume names, separated by commas.'),
            h('div', null, btn('Add role', addRole, { kind: 'primary' })),
          ),
        ),
      ),
    ),
  );

  void loadVolumes();
  void loadSettings();
  void loadAgents();
  return () => {
    alive = false;
  };
}
