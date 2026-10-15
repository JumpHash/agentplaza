// The coding engines an agent may run, and the rule for switching one on or off. Pure, so the policy view and the
// tests share one rule (the same split as rules-model.ts).

export const ENGINES: { value: string; label: string }[] = [
  { value: 'claude', label: 'Claude Code' },
  { value: 'codex', label: 'Codex' },
  { value: 'opencode', label: 'OpenCode' },
];

/**
 * The enabled list after one engine is switched, in the order of ENGINES. Switching off the last engine is refused:
 * the list comes back as it was, because the hub needs at least one engine to run an agent.
 */
export function toggleEngine(enabled: string[], engine: string, on: boolean): string[] {
  const next = ENGINES.map((e) => e.value).filter((v) => (v === engine ? on : enabled.includes(v)));
  return next.length === 0 ? [...enabled] : next;
}
