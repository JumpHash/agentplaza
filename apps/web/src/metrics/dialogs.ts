// The Metrics dialogs: a new (or edited) alert rule, and the metrics settings (collection level, retention, budgets,
// forecast basis, the opt-in outside endpoints).

import type { MetricDef, MetricsSettings, Rule, ScopeNode } from './model';

/** The alert rule dialog; `save` is called with the rule when the owner confirms. */
export function ruleDialog(catalog: MetricDef[], scopes: ScopeNode[], scope: string, save: (r: Rule) => Promise<void>, rule?: Rule): void {
  void catalog;
  void scopes;
  void scope;
  void save;
  void rule;
  throw new Error('ruleDialog: not written yet');
}

/** The settings dialog; `save` returns the saved settings (and the OpenMetrics token once, when just made). */
export function settingsDialog(current: MetricsSettings, scopes: ScopeNode[], save: (s: MetricsSettings) => Promise<{ openmetrics_token?: string }>): void {
  void current;
  void scopes;
  void save;
  throw new Error('settingsDialog: not written yet');
}
