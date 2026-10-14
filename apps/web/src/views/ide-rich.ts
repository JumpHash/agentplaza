// The IDE's rich-content context (docs/design/workspace.md, section 3.4): what the IDE knows that the classifier needs
// (the workspace's paths from the loaded file tree, the project in view) and what a click on a reference does inside the
// IDE (open the file at its line, select the agent, open a task or a library item on its own page). The chat and the
// board panel take it through a getter, so it is read at each render.
import { ideApi, type Project } from '../api';
import { embedContextFromStore, HEAD_LINES, type EmbedContext } from '../ui/embeds';
import type { IdeCtx } from './ide-ctx';

/** A file's first lines are kept for this long, per workspace; a failed read for a shorter time is not kept at all. */
const HEAD_TTL_MS = 5 * 60_000;
const HEAD_MAX = 300;

const heads = new Map<string, { at: number; lines: string[] | null }>();

/** The workspace paths the IDE has loaded: its files, and its folders with a trailing `/`. The tree is loaded lazily, so
 * a path in a folder that is not open yet is plain text until the folder opens. */
function workspacePaths(c: IdeCtx): Set<string> {
  const paths = new Set<string>();
  for (const e of c.entries.values()) {
    if (!e.path) continue;
    paths.add(e.kind === 'dir' ? `${e.path}/` : e.path);
  }
  return paths;
}

/** The first lines of a workspace file, through the IDE's read API. Null when the file cannot be read (or is binary). */
async function readHead(c: IdeCtx, path: string): Promise<string[] | null> {
  const ws = c.ws;
  if (!ws) return null;
  const key = `${ws.id}\u0000${path}`;
  const have = heads.get(key);
  if (have && Date.now() - have.at < HEAD_TTL_MS) return have.lines;
  let lines: string[] | null = null;
  try {
    lines = (await ideApi.file(ws.id, path)).text.split('\n').slice(0, HEAD_LINES);
  } catch {
    lines = null;
  }
  if (lines !== null) {
    heads.set(key, { at: Date.now(), lines });
    if (heads.size > HEAD_MAX) heads.delete(heads.keys().next().value as string);
  }
  return lines;
}

/** The rich-content context for the IDE, with the project in view (the agent's, or the repository's). */
export function ideEmbeds(c: IdeCtx, project: Project | null): EmbedContext {
  return embedContextFromStore({
    project: project?.id ?? null,
    project_repo: project?.repo ?? null,
    paths: workspacePaths(c),
    // An agent in the IDE's sidebar is selected there; one the IDE does not list opens nothing.
    openAgent: (who) => {
      const cid = typeof who === 'number' ? who : c.agentList.find((a) => a.name === who)?.cid;
      if (cid !== undefined && c.agentList.some((a) => a.cid === cid)) c.act.selectAgent(cid);
    },
    // A folder reference has nothing to open in the editor; a file opens at its line.
    openPath: (path, line) => {
      if (!path.endsWith('/')) void c.act.openTab(path, line ?? undefined);
    },
    readHead: (path) => readHead(c, path),
  });
}
