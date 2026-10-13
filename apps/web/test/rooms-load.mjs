// Loads the room generator for the node scripts (rooms-prop.mjs, rooms-fixtures.mjs). gen.ts and verify.ts are
// bundled in memory with esbuild, together with the catalogue and roles JSON; nothing is written to disk.

import { build } from 'esbuild';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const web = dirname(dirname(fileURLToPath(import.meta.url)));
const rooms = join(web, 'src/views/floor3d/rooms');
const assets = join(web, '../../assets/rooms');

/** The desk count of a room (the default of gen.ts): clamp(floor(w * d / 6), 1, 4). */
export const cap = (w, d) => Math.min(4, Math.max(1, Math.floor((w * d) / 6)));

/** The lab URL of one input, for a failure message. */
export const labUrl = ({ role, w, d, door, seed }) =>
  `http://127.0.0.1:7700/#/room-lab?role=${role}&w=${w}&d=${d}&door=${door}&seed=${seed}`;

export async function loadRooms() {
  const entry = [
    "export * from './gen.ts';",
    "export * from './verify.ts';",
    `import catalog from ${JSON.stringify(join(assets, 'catalog.json'))};`,
    `import roles from ${JSON.stringify(join(assets, 'roles.json'))};`,
    'export { catalog, roles };',
  ].join('\n');
  const result = await build({
    stdin: { contents: entry, resolveDir: rooms, loader: 'ts', sourcefile: 'rooms-entry.ts' },
    bundle: true,
    platform: 'node',
    format: 'esm',
    write: false,
    logLevel: 'silent',
  });
  const code = result.outputFiles[0].text;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
}
