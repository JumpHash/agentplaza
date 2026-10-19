// The artifact view model, mirroring crates/apz-view/src/model.rs and artifact.rs (an agent's shown work: a title and
// panels on the Metrics grid). Field names are the JSON ones (snake_case); the body enum is tagged with `type`.
// Metrics bodies (chart, table, tiles …) are the Metrics model's; the six artifact bodies are added here. Keep this
// file in step with the Rust model.

import type { Panel, Tone, Unit } from '../metrics/model';

/** A file in an artifact's store. */
export interface BlobRef {
  /** The blob's id within its artifact (`b1`, `b2` …). */
  id: string;
  content_type: string;
  bytes: number;
}

/** A picture an agent made or found. Alt text is required. */
export interface ImageBody {
  blob: BlobRef;
  alt: string;
  caption?: string;
  width?: number | null;
  height?: number | null;
}

/** `layered` draws columns of cards with routed links; `force` lays out an undirected graph. */
export type GraphLayout = 'layered' | 'force';

export interface GraphNode {
  id: string;
  label: string;
  sub?: string;
  tone?: Tone | null;
  /** Nodes with the same group are drawn together and share a colour when no tone is set. */
  group?: string;
}

export interface GraphEdge {
  from: string;
  to: string;
  label?: string;
}

export interface GraphBody {
  nodes: GraphNode[];
  edges: GraphEdge[];
  layout?: GraphLayout;
  directed?: boolean;
}

export type ColumnKind = 'text' | 'number' | 'time' | 'bool';

export interface DatasetColumn {
  key: string;
  label: string;
  kind: ColumnKind;
  /** How numbers are formatted (the Metrics units); plain when absent. */
  unit?: Unit | null;
}

export interface DatasetBody {
  columns: DatasetColumn[];
  /** Rows inline, each a list of values in column order. Empty when the rows are in `blob`. */
  rows: unknown[][];
  /** CSV (with a header row) or JSON Lines, for more rows than fit inline. */
  blob?: BlobRef | null;
  /** The row count, inline or in the blob. */
  total: number;
}

export interface CodeBody {
  /** A language name as Markdown fences give it: `rust`, `ts`, `python` …  */
  language: string;
  text: string;
  filename?: string;
}

/** One file of an HTML artifact. */
export interface HtmlFile {
  /** A relative path: `index.html`, `app.js`, `App.tsx`, `data/points.csv`. */
  path: string;
  text?: string | null;
  blob?: BlobRef | null;
}

export interface HtmlBody {
  /** The file that runs: an `.html` page, or one component whose default export is rendered. */
  entry: string;
  files: HtmlFile[];
  /** The frame's starting height in px, until the page reports its own. */
  height: number;
  /** The vendored libraries the artifact uses, in load order. Set by the hub. */
  libs?: string[];
  /** A still of the first render, shown until the frame runs. Set by the hub. */
  still?: BlobRef | null;
}

/** The artifact-only panel bodies. Each carries its `type` tag. */
export type ArtifactBody =
  | ({ type: 'image' } & ImageBody)
  | ({ type: 'graph' } & GraphBody)
  | ({ type: 'dataset' } & DatasetBody)
  | { type: 'document'; markdown: string }
  | ({ type: 'code' } & CodeBody)
  | ({ type: 'html' } & HtmlBody);

/** What an agent sends to `artifact_create` and `artifact_update`. */
export interface ArtifactSpec {
  title: string;
  sub?: string;
  panels: Panel[];
}

/** An artifact as the hub stores and serves it (`GET /v1/artifacts/{id}`). */
export interface Artifact {
  id: string;
  /** 1 for the first version; each update adds one. */
  version: number;
  /** The newest version. */
  latest: number;
  title: string;
  sub: string;
  panels: Panel[];
  /** The kind of the panels: `chart`, `table`, `dashboard`, `html` … */
  kind: string;
  /** The agent that made it (`a12 · Ada`), or `owner`. */
  author: string;
  project?: string | null;
  created_ms: number;
  updated_ms: number;
  /** Bytes across the spec and its blobs. */
  bytes: number;
}

/** One line of `GET /v1/artifacts`. */
export interface ArtifactSummary {
  id: string;
  title: string;
  kind: string;
  latest: number;
  author: string;
  project?: string | null;
  updated_ms: number;
  bytes: number;
  /** Whether a share link is live for it. */
  shared: boolean;
}

/** One page of a dataset's rows, from `ViewCtx.rows`. */
export interface RowsPage {
  columns: DatasetColumn[];
  rows: unknown[][];
  total: number;
  offset: number;
}

/** One saved version of an artifact, for the version switcher. */
export interface VersionInfo {
  version: number;
  created_ms: number;
  author?: string;
  title?: string;
}
