// The owner's view context for one artifact: its blobs, its rows, its live frames and the renderers for code, datasets
// and graphs. The artifact screen and the inline embeds draw with it. The share viewer builds its own context.

import { artifactBlobUrl, artifactFrameUrl, artifactRows } from '../api-artifacts';
import type { ViewCtx } from './bodies';
import { renderCode } from './code';
import { renderDataset } from './dataset';
import { liveFrame } from './frame';
import { renderGraph } from './graph';
import type { Artifact } from './model';

/** The context an owner's artifact is drawn with. Frames and rows are fetched for the artifact's own version. */
export function ownerCtx(a: Artifact): ViewCtx {
  const rows = (panelId: string, q: { offset: number; limit: number; sort?: string; desc?: boolean; filter?: string }) =>
    artifactRows(a.id, panelId, q, a.version);
  return {
    blobUrl: (blob: string) => artifactBlobUrl(a.id, blob),
    rows,
    frame: (panelId, body) =>
      liveFrame(() => artifactFrameUrl(a.id, a.version, panelId).then((r) => r.url), body, {
        artifactId: a.id,
        panelId,
        stillUrl: body.still ? artifactBlobUrl(a.id, body.still.id) : undefined,
      }),
    code: (body) => renderCode(body),
    dataset: (panelId, body) => renderDataset(panelId, body, rows),
    graph: (body) => renderGraph(body),
  };
}
