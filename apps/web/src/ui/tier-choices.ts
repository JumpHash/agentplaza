// The model choices a tier may run, as its pickers list them: the chats' model chip and the models policy.

import { isContributor, type ModelInfo, type Settings, type Tier } from '../api';
import { choicesFrom, type ModelChoice } from './model-picker';

/**
 * The model choices for a tier. A worker may run any model: the policy's worker list comes
 * first as presets, then every model the providers list. An orchestrator may run only its
 * policy list. `current` (an override no longer offered) is kept as a choice.
 */
export function tierChoices(
  tier: Tier,
  settings: Settings,
  models: ModelInfo[],
  inherit?: { label: string },
  current?: string,
): ModelChoice[] {
  const names = new Map(models.map((m) => [`${m.provider}/${m.id}`, m.name ?? m.id] as [string, string]));
  const policy = settings.model_policy[tier];
  const out = choicesFrom(policy, names, inherit);
  if (tier === 'worker') {
    for (const c of out) if (c.value && policy.includes(c.value)) c.preset = true;
    const offered = new Set(out.map((c) => c.value));
    for (const m of models) {
      const value = `${m.provider}/${m.id}`;
      if (offered.has(value)) continue;
      offered.add(value);
      out.push({ value, label: m.name ?? m.id, provider: m.provider });
    }
  }
  if (current && !out.some((c) => c.value === current)) {
    out.push({ value: current, label: current, provider: current.split('/')[0] ?? '' });
  }
  const trainers = new Set(models.filter(isContributor).map((m) => `${m.provider}/${m.id}`));
  for (const c of out) c.trains = trainers.has(c.value.replace(/@.*$/, ''));
  return out;
}
