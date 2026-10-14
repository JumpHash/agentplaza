// An agent's portrait as the engine draws it (crates/apz-engine/src/portrait.rs): the spec, its cache key and its JSON,
// and the PNGs already drawn, by key. The engine resolves a missing species from the agent's id (species_of).

/** A robot portrait's subject. */
export interface PortraitSpec {
  cid: number;
  /** The district (role) hue, for the robot's front panel. */
  roleHue: number;
  /** The agent's own hue: its shell, and the backdrop. */
  bodyHue: number;
  orchestrator: boolean;
  state: 'idle' | 'working' | 'walking' | 'offline';
  /** The agent record's species (the engine deals one from the cid when missing). */
  species?: string | null;
}

const cache = new Map<string, string>();

/** The cache key of a portrait: its agent, hues, orchestrator, offline flag and species. */
export function portraitKey(spec: PortraitSpec): string {
  return `${spec.cid}|${spec.roleHue}|${Math.round(spec.bodyHue)}|${spec.orchestrator}|${spec.state === 'offline'}|${spec.species ?? ''}`;
}

/** The portrait's spec as the engine's JSON (`PortraitSpec::from_json`). */
export function portraitSpecJson(spec: PortraitSpec): string {
  return JSON.stringify({
    cid: spec.cid,
    roleHue: spec.roleHue,
    bodyHue: spec.bodyHue,
    orchestrator: spec.orchestrator,
    offline: spec.state === 'offline',
    species: spec.species ?? '',
  });
}

/** A portrait's PNG from the cache, by its key. */
export function cachedPortrait(key: string): string | undefined {
  return cache.get(key);
}

/** Keeps a portrait's PNG under its key. */
export function rememberPortrait(key: string, url: string): void {
  cache.set(key, url);
}
