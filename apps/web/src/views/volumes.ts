// Setup > Volumes: the shared read-only data volumes on the hub machine, and the roles that mount them. A volume is an
// ext4 image or a directory on the hub; an agent of a role mounts each volume the role names at /data/<name>. A role
// change applies to the agents spawned from now on. The pure helpers (the name rule, the role list parser) are exported
// and unit-tested (test/tests.ts).

import '../styles/mcp.css';
import { api, volumesApi, type DataVolume, type Settings } from '../api';
import { button, card, empty, field, input, modal, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText } from './vault-common';

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

  const tableError = h('p.mcp-danger');
  const volumesSlot = h('div', null, h('p.mcp-loading', null, 'Loading the volumes...'));
  const createError = h('p.mcp-danger');
  const rolesError = h('p.mcp-danger');
  const rolesSlot = h('div', null, h('p.mcp-loading', null, 'Loading the roles...'));

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

  const doRemove = async (v: DataVolume): Promise<void> => {
    replace(tableError);
    try {
      await volumesApi.remove(v.name);
      if (!alive) return;
      toast(`Removed ${v.name}`);
    } catch (err) {
      if (alive) replace(tableError, errorText(err));
    }
    if (alive) void loadVolumes();
  };

  const removeVolume = (v: DataVolume): void => {
    const close = modal(
      `Remove ${v.name}`,
      h(
        'div.mcp-confirm',
        null,
        h('p', null, 'The volume is removed from the hub.'),
        h(
          'div.mcp-actions',
          null,
          button('Cancel', () => close()),
          button(
            'Remove',
            () => {
              close();
              void doRemove(v);
            },
            { kind: 'danger' },
          ),
        ),
      ),
    );
  };

  const volumeRow = (v: DataVolume): HTMLElement => {
    const remove = button('Remove', () => removeVolume(v), { kind: 'danger', size: 'sm', icon: 'trash' });
    // A volume an agent mounts stays until that agent lets go of it.
    remove.disabled = v.in_use.length > 0;
    return h(
      'tr',
      null,
      h('td', { 'data-label': 'Name' }, v.name),
      h('td', { 'data-label': 'Size' }, `${(v.size_bytes / MIB).toFixed(1)} MiB`),
      h('td', { 'data-label': 'Mounted by' }, v.in_use.length ? v.in_use.join(', ') : '—'),
      h('td', { 'data-label': 'Source' }, v.source),
      h('td', { 'data-label': 'Actions', class: 'mcp-actions-cell' }, remove),
    );
  };

  const drawVolumes = (): void => {
    if (volumesError) {
      replace(volumesSlot, empty('alert', 'Could not load the volumes', errorText(volumesError)));
      return;
    }
    if (!volumes) return;
    replace(
      volumesSlot,
      volumes.length
        ? h(
            'div.mcp-table-wrap',
            null,
            h(
              'table',
              null,
              h(
                'thead',
                null,
                h('tr', null, h('th', null, 'Name'), h('th', null, 'Size'), h('th', null, 'Mounted by'), h('th', null, 'Source'), h('th', null, '')),
              ),
              h('tbody', null, ...volumes.map(volumeRow)),
            ),
          )
        : empty('wrench', 'No volumes yet', 'Create one below.'),
    );
  };

  const nameIn = input({ placeholder: 'deps', spellcheck: 'false' });
  const sourceIn = input({ placeholder: '/srv/volumes/deps.ext4', spellcheck: 'false' });

  const createVolume = async (): Promise<void> => {
    replace(createError);
    const name = nameIn.value.trim();
    const source = sourceIn.value.trim();
    if (!volumeNameOk(name)) {
      replace(createError, 'A name is a-z, 0-9 and dashes, up to 20 characters.');
      return;
    }
    if (!source.startsWith('/')) {
      replace(createError, 'The source is an absolute path on the hub machine.');
      return;
    }
    try {
      await volumesApi.create(name, source);
      if (!alive) return;
      toast(`Created ${name}`);
      nameIn.value = '';
      sourceIn.value = '';
    } catch (err) {
      if (alive) replace(createError, errorText(err));
      return;
    }
    if (alive) void loadVolumes();
  };

  const createBlock = h(
    'div.stack',
    null,
    createError,
    field('Name', nameIn, 'What agents see under /data/<name>. Up to 20 characters.'),
    field('Source', sourceIn, 'An ext4 image or a directory on the hub machine.'),
    h('div', null, button('Create volume', () => void createVolume())),
  );

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

  const roleIn = input({ placeholder: 'coder', spellcheck: 'false' });
  const addList = input({ placeholder: 'deps, models', spellcheck: 'false' });

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

  const roleRow = (role: string, names: string[]): HTMLElement => {
    const list = input({ spellcheck: 'false', 'aria-label': `Volumes for ${role}`, value: names.join(', ') });
    return h(
      'tr',
      null,
      h('td', { 'data-label': 'Role' }, role),
      h('td', { 'data-label': 'Volumes' }, list),
      h(
        'td',
        { 'data-label': 'Actions', class: 'mcp-actions-cell' },
        button('Save', () => saveRole(role, list), { kind: 'ghost', size: 'sm' }),
        button('Remove', () => removeRole(role), { kind: 'danger', size: 'sm', icon: 'trash' }),
      ),
    );
  };

  const drawRoles = (): void => {
    if (settingsError) {
      replace(rolesSlot, empty('alert', 'Could not load the roles', errorText(settingsError)));
      return;
    }
    if (!settings) return;
    const entries = Object.entries(settings.role_volumes ?? {});
    replace(
      rolesSlot,
      entries.length
        ? h(
            'div.mcp-table-wrap',
            null,
            h(
              'table',
              null,
              h('thead', null, h('tr', null, h('th', null, 'Role'), h('th', null, 'Volumes'), h('th', null, ''))),
              h('tbody', null, ...entries.map(([role, names]) => roleRow(role, names))),
            ),
          )
        : empty('wrench', 'No role mounts a volume', 'Add a role below.'),
    );
  };

  const addBlock = h(
    'div.stack',
    null,
    field('Role', roleIn),
    field('Volumes', addList, 'Up to four volume names, separated by commas.'),
    h('div', null, button('Add role', addRole)),
  );

  replace(
    root,
    h(
      'div.stack',
      null,
      h(
        'p',
        null,
        'A volume is a read-only data set on the hub machine: an ext4 image or a directory. An agent of a role mounts each volume the role names at /data/<name>. A role change applies to the agents spawned from now on.',
      ),
      card('Volumes', [tableError, volumesSlot, createBlock]),
      card('Roles', [rolesError, rolesSlot, addBlock]),
    ),
  );

  void loadVolumes();
  void loadSettings();
  return () => {
    alive = false;
  };
}
