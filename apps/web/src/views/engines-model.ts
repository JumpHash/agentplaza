// The coding engines an agent may run, and the rule for switching one on or off. Pure, so the policy view and the
// tests share one rule (the same split as rules-model.ts).

export const ENGINES: { value: string; label: string }[] = [
  { value: 'claude', label: 'Claude Code' },
  { value: 'codex', label: 'Codex' },
  { value: 'opencode', label: 'OpenCode' },
  { value: 'gemini', label: 'Gemini CLI' },
  { value: 'qwen', label: 'Qwen Code' },
  { value: 'kilo', label: 'Kilo Code' },
  { value: 'cline', label: 'Cline' },
  { value: 'copilot', label: 'GitHub Copilot' },
  { value: 'goose', label: 'Goose' },
];

/** The engines enabled out of the box: Gemini CLI, Qwen Code and Kilo Code are listed but off until the owner turns them on (apid's settings). */
export const DEFAULT_ENGINES = ['claude', 'codex', 'opencode'];

/**
 * The enabled list after one engine is switched, in the order of ENGINES. Switching off the last engine is refused:
 * the list comes back as it was, because the hub needs at least one engine to run an agent.
 */
export function toggleEngine(enabled: string[], engine: string, on: boolean): string[] {
  const next = ENGINES.map((e) => e.value).filter((v) => (v === engine ? on : enabled.includes(v)));
  return next.length === 0 ? [...enabled] : next;
}
