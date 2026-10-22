//! [`Source`] over the hub: series from the store plus the rollup's open windows, the scope tree and entities from
//! the roster, the nodes and the node reports, calls and traces from the store, alerts from the engine.
//!
//! Scope resolution (which series of a metric fall under a scope) goes through the metric's catalog entity kind:
//! an agent-keyed metric under `node:n1` reads the agents placed on n1; a VM-keyed metric under `run:r12` reads
//! r12's VMs; under `provider:anthropic` an agent-keyed metric keeps the series labelled `provider=anthropic`.

use apz_metrics::query::AggFn;
use apz_metrics::trace::Trace;
use apz_metrics::{Event, HistSnapshot, Level};
use apz_metrics_view::source::*;
use apz_metrics_view::{Saved, ScopeNode};

use super::Metrics;
use crate::state::App;

/// The hub as a source, at a moment.
pub struct LiveSource<'a> {
    pub app: &'a App,
    pub m: &'a Metrics,
    pub now_ms: u64,
}

impl Source for LiveSource<'_> {
    fn now_ms(&self) -> u64 {
        self.now_ms
    }
    fn level(&self) -> (Level, bool) {
        misc::level(self)
    }
    fn series(&self, read: &Read) -> Vec<Group> {
        reads::series(self, read)
    }
    fn scalar(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        agg: AggFn,
    ) -> Option<f64> {
        reads::scalar(self, metric, scope, from_ms, to_ms, agg)
    }
    fn hist(&self, metric: &str, scope: &str, from_ms: u64, to_ms: u64) -> HistSnapshot {
        reads::hist(self, metric, scope, from_ms, to_ms)
    }
    fn hist_steps(
        &self,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        step_ms: u64,
    ) -> Vec<HistSnapshot> {
        reads::hist_steps(self, metric, scope, from_ms, to_ms, step_ms)
    }
    fn events(
        &self,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        kinds: &[&str],
        limit: usize,
    ) -> Vec<Event> {
        misc::events(self, scope, from_ms, to_ms, kinds, limit)
    }
    fn scopes(&self) -> Vec<ScopeNode> {
        super::scope::tree(self.app)
    }
    fn entities(&self, scope: &str, kind: &str) -> Vec<Entity> {
        super::scope::entities(self.app, scope, kind)
    }
    fn agent(&self, cid: u64) -> Option<AgentInfo> {
        super::scope::agent(self.app, cid)
    }
    fn calls(&self, scope: &str, before_ms: u64, limit: usize) -> Vec<CallRow> {
        misc::calls(self, scope, before_ms, limit)
    }
    fn prices(&self, models: &[String]) -> Prices {
        misc::prices(self, models)
    }
    fn traces(&self, scope: &str, from_ms: u64, to_ms: u64, limit: usize) -> Vec<Trace> {
        misc::traces(self, scope, from_ms, to_ms, limit)
    }
    fn trace(&self, id: &str) -> Option<Trace> {
        self.m.store.trace(id)
    }
    fn alerts(&self) -> AlertsState {
        self.m.alerts.state(self.m, self.app, self.now_ms)
    }
    fn self_cost(&self) -> SelfCost {
        misc::self_cost(self)
    }
    fn budgets(&self) -> Vec<Budget> {
        misc::budgets(self)
    }
    fn monthly_budget(&self) -> Option<f64> {
        self.m.settings.get().monthly_budget_usd
    }
    fn treasury(&self) -> Option<(f64, f64)> {
        misc::treasury(self)
    }
    fn report(&self, node: &str) -> serde_json::Value {
        let node = node.strip_prefix("node:").unwrap_or(node);
        super::lock(&self.m.links)
            .get(node)
            .map(|l| l.reports.clone())
            .unwrap_or(serde_json::Value::Null)
    }
    fn rate_limits(&self) -> Vec<(String, Option<f64>, Option<f64>)> {
        misc::rate_limits(self)
    }
    fn outside(&self) -> Vec<(String, bool, Option<f64>, String)> {
        misc::outside(self)
    }
    fn links(&self) -> Vec<(String, String, u64, f64, f64, u32, f64, f64)> {
        misc::links(self)
    }
    fn saved(&self) -> Saved {
        self.m.store.dashboards().unwrap_or_else(|| Saved {
            starter: true,
            ..Saved::default()
        })
    }
    fn basis_days(&self) -> u32 {
        self.m.settings.get().basis_days
    }
}

/// Series reads: the store's closed windows plus the rollup's open ones, filtered by scope, grouped, merged and
/// bucketed.
pub mod reads {
    use super::*;
    use crate::metrics::{lock, scope};
    use apz_metrics::agg::{Agg, Tier};
    use apz_metrics::catalog;
    use apz_metrics::query::{bucketize, merge_rows};
    use apz_metrics::{EntityKind, SeriesKey};
    use std::collections::BTreeMap;

    /// Histogram reads over a range longer than this use the hour aggregates instead of the minute ones.
    const HIST_HOUR_TIER_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

    /// See [`Source::series`].
    pub fn series(src: &LiveSource<'_>, read: &Read) -> Vec<Group> {
        let Some(def) = catalog::find(&read.metric) else {
            return Vec::new();
        };
        let filter = scope::filter(src.app, &read.scope, def.entity);
        let rows = rows_of(
            src,
            &read.metric,
            tier_for_step(read.step_ms),
            read.from_ms,
            read.to_ms,
            &*filter,
        );
        let whole = scope_name(src, &read.scope);
        let name = |key: &SeriesKey| match &read.group {
            GroupBy::All => (whole.clone(), Some(read.scope.clone())),
            GroupBy::Label(label) => (key.get(label).unwrap_or("other").to_string(), None),
            GroupBy::Entity => (
                scope::label(src.app, key.kind, &key.entity),
                Some(key.scope()),
            ),
            GroupBy::AgentProp(prop) => {
                scope::agent_prop(src.app, key, prop).unwrap_or_else(|| ("other".to_string(), None))
            }
        };
        group_series(read, rows, &name)
    }

    /// The coarsest tier whose step is at most `step_ms`: hours for an hour or more, minutes for a minute or more,
    /// else the 10 s chunks.
    fn tier_for_step(step_ms: u64) -> Tier {
        if step_ms >= Tier::Hour1.step_ms() {
            Tier::Hour1
        } else if step_ms >= Tier::Min1.step_ms() {
            Tier::Min1
        } else {
            Tier::Raw10s
        }
    }

    /// The name of a whole scope: the label of its entity ("n1 · rack-a"), or "Global" for the global scope.
    fn scope_name(src: &LiveSource<'_>, scope_id: &str) -> String {
        if scope_id == "global" {
            return "Global".to_string();
        }
        match scope_id.split_once(':') {
            Some((prefix, entity)) => match EntityKind::from_prefix(prefix) {
                Some(kind) => scope::label(src.app, kind, entity),
                None => scope_id.to_string(),
            },
            None => scope_id.to_string(),
        }
    }

    /// The windows of each series of `metric` at `tier` in `[from_ms, to_ms)` that `keep` accepts: the closed ones
    /// from the store and the open ones from the rollup, ascending by start.
    fn rows_of(
        src: &LiveSource<'_>,
        metric: &str,
        tier: Tier,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&SeriesKey) -> bool,
    ) -> Vec<(SeriesKey, Vec<(u64, Agg)>)> {
        let mut by_key: BTreeMap<SeriesKey, Vec<(u64, Agg)>> = BTreeMap::new();
        for (key, windows) in src.m.store.rows(tier, metric, from_ms, to_ms, keep) {
            by_key.entry(key).or_default().extend(windows);
        }
        let open: Vec<(SeriesKey, u64, Agg)> = lock(&src.m.rollup)
            .open_at(tier)
            .filter(|(key, start, _)| {
                key.metric == metric && *start >= from_ms && *start < to_ms && keep(key)
            })
            .map(|(key, start, agg)| (key.clone(), start, agg.clone()))
            .collect();
        for (key, start, agg) in open {
            by_key.entry(key).or_default().push((start, agg));
        }
        by_key
            .into_iter()
            .map(|(key, mut windows)| {
                windows.sort_by_key(|(start, _)| *start);
                (key, windows)
            })
            .collect()
    }

    /// Groups series by the name and scope `name` gives them, merges each group's windows, buckets them into the
    /// read's steps and orders the groups by total. With `top`, the groups past the first `top` fold into one
    /// "other" group, whose windows are merged before bucketing. Series with no windows are left out.
    fn group_series(
        read: &Read,
        series: Vec<(SeriesKey, Vec<(u64, Agg)>)>,
        name: &dyn Fn(&SeriesKey) -> (String, Option<String>),
    ) -> Vec<Group> {
        let bucket = |windows: &[Vec<(u64, Agg)>]| {
            bucketize(
                &merge_rows(windows),
                read.from_ms,
                read.to_ms,
                read.step_ms,
                read.agg,
            )
        };
        // Each group keeps the windows of its series, in the order the series first appear.
        let mut grouped: Vec<(String, Option<String>, Vec<Vec<(u64, Agg)>>)> = Vec::new();
        for (key, windows) in series {
            if windows.is_empty() {
                continue;
            }
            let (label, scope_id) = name(&key);
            match grouped.iter_mut().find(|group| group.0 == label) {
                Some(group) => group.2.push(windows),
                None => grouped.push((label, scope_id, vec![windows])),
            }
        }
        let mut groups: Vec<(Group, Vec<Vec<(u64, Agg)>>)> = grouped
            .into_iter()
            .map(|(name, scope, windows)| {
                let values = bucket(&windows);
                (
                    Group {
                        name,
                        scope,
                        values,
                    },
                    windows,
                )
            })
            .collect();
        groups.sort_by(|a, b| b.0.total().total_cmp(&a.0.total()));
        if let Some(top) = read.top
            && groups.len() > top
        {
            let folded: Vec<Vec<(u64, Agg)>> = groups
                .split_off(top)
                .into_iter()
                .flat_map(|(_, windows)| windows)
                .collect();
            let other = Group {
                name: "other".to_string(),
                scope: None,
                values: bucket(&folded),
            };
            groups.push((other, folded));
        }
        let mut out: Vec<Group> = groups.into_iter().map(|(group, _)| group).collect();
        out.sort_by(|a, b| b.total().total_cmp(&a.total()));
        out
    }

    /// See [`Source::scalar`].
    pub fn scalar(
        src: &LiveSource<'_>,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        agg: AggFn,
    ) -> Option<f64> {
        let read = Read {
            metric: metric.to_string(),
            scope: scope.to_string(),
            from_ms,
            to_ms,
            step_ms: to_ms.saturating_sub(from_ms).max(1),
            agg,
            group: GroupBy::All,
            top: None,
        };
        series(src, &read)
            .first()?
            .values
            .first()
            .copied()
            .flatten()
    }

    /// See [`Source::hist`].
    pub fn hist(
        src: &LiveSource<'_>,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
    ) -> HistSnapshot {
        let Some(def) = catalog::find(metric) else {
            return HistSnapshot::default();
        };
        let tier = if to_ms.saturating_sub(from_ms) > HIST_HOUR_TIER_MS {
            Tier::Hour1
        } else {
            Tier::Min1
        };
        let filter = scope::filter(src.app, scope, def.entity);
        let mut merged = HistSnapshot::default();
        for (_, windows) in rows_of(src, metric, tier, from_ms, to_ms, &*filter) {
            for (_, agg) in windows {
                if let Some(hist) = &agg.hist {
                    merged.merge(hist);
                }
            }
        }
        merged
    }

    /// See [`Source::hist_steps`].
    pub fn hist_steps(
        src: &LiveSource<'_>,
        metric: &str,
        scope: &str,
        from_ms: u64,
        to_ms: u64,
        step_ms: u64,
    ) -> Vec<HistSnapshot> {
        let Some(def) = catalog::find(metric) else {
            return Vec::new();
        };
        if step_ms == 0 || to_ms <= from_ms {
            return Vec::new();
        }
        let filter = scope::filter(src.app, scope, def.entity);
        let rows = rows_of(
            src,
            metric,
            tier_for_step(step_ms),
            from_ms,
            to_ms,
            &*filter,
        );
        let count = (to_ms - from_ms).div_ceil(step_ms);
        let mut steps = vec![HistSnapshot::default(); count as usize];
        for (_, windows) in rows {
            for (start_ms, agg) in windows {
                let Some(hist) = &agg.hist else {
                    continue;
                };
                if start_ms < from_ms || start_ms >= to_ms {
                    continue;
                }
                let index = ((start_ms - from_ms) / step_ms) as usize;
                steps[index].merge(hist);
            }
        }
        steps
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn window(start_ms: u64, value: f64) -> (u64, Agg) {
            (
                start_ms,
                Agg {
                    count: 1,
                    sum: value,
                    min: value,
                    max: value,
                    last: value,
                    hist: None,
                },
            )
        }

        fn key(provider: &str) -> SeriesKey {
            SeriesKey::new("m", EntityKind::Agent, "1").label("provider", provider)
        }

        fn read_of(top: Option<usize>) -> Read {
            Read {
                metric: "m".to_string(),
                scope: "global".to_string(),
                from_ms: 0,
                to_ms: 30,
                step_ms: 10,
                agg: AggFn::Sum,
                group: GroupBy::Label("provider".to_string()),
                top,
            }
        }

        fn by_provider(key: &SeriesKey) -> (String, Option<String>) {
            (key.get("provider").unwrap_or("other").to_string(), None)
        }

        #[test]
        fn a_top_limit_folds_the_smaller_groups_into_other_and_orders_by_total() {
            let series = vec![
                (
                    key("a"),
                    vec![window(0, 5.0), window(10, 5.0), window(20, 5.0)],
                ),
                (
                    key("b"),
                    vec![window(0, 1.0), window(10, 1.0), window(20, 1.0)],
                ),
                (key("c"), vec![window(0, 2.0)]),
                (key("d"), vec![window(10, 4.0)]),
            ];
            let groups = group_series(&read_of(Some(2)), series, &by_provider);
            let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
            assert_eq!(names, ["a", "other", "d"]);
            assert_eq!(groups[0].values, vec![Some(5.0), Some(5.0), Some(5.0)]);
            assert_eq!(groups[1].values, vec![Some(3.0), Some(1.0), Some(1.0)]);
            assert_eq!(groups[1].scope, None);
        }

        #[test]
        fn groups_with_one_name_merge_their_series_and_empty_series_leave_no_group() {
            let series = vec![
                (key("b"), vec![window(0, 5.0)]),
                (key("a"), vec![window(0, 1.0), window(10, 1.0)]),
                (
                    SeriesKey::new("m", EntityKind::Agent, "2").label("provider", "a"),
                    vec![window(0, 1.0)],
                ),
                (key("z"), Vec::new()),
            ];
            let groups = group_series(&read_of(None), series, &by_provider);
            let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
            assert_eq!(names, ["b", "a"]);
            assert_eq!(groups[1].values, vec![Some(2.0), Some(1.0), None]);
        }
    }
}

pub use super::source_misc as misc;
