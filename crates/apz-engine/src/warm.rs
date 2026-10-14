//! Budgeted builds (port of apps/web/src/views/floor3d/warm.ts): heavy scene builds run a few milliseconds per
//! frame as resumable steps, and built boards and layouts are cached by key so a board that comes back is not rebuilt.

use std::collections::VecDeque;

/// The time a frame may spend on builds, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    pub ms: f64,
}

/// One step of a build: more to do, or the built value.
pub enum Step<T> {
    More,
    Done(T),
}

/// A build run as steps. `run` spends a budget on it: steps run one after another and the budget is checked after
/// each, so at most the one step that crosses the budget runs past it. A call always runs one step, so a zero budget
/// still makes progress.
pub struct Warm<T> {
    job: Option<Box<dyn FnMut() -> Step<T>>>,
}

impl<T> Warm<T> {
    pub fn new(job: impl FnMut() -> Step<T> + 'static) -> Self {
        Warm {
            job: Some(Box::new(job)),
        }
    }

    /// Whether a build is queued and not yet done.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Runs steps until the build is done (Some(value); the job is then cleared) or `budget_ms` has passed on `now`
    /// (None: call again next frame). `now` is the host clock in milliseconds: performance.now() on the web,
    /// `native_clock` natively.
    pub fn run(&mut self, now: impl Fn() -> f64, budget_ms: f64) -> Option<T> {
        let mut job = self.job.take()?;
        let start = now();
        loop {
            match job() {
                Step::Done(value) => return Some(value),
                Step::More => {
                    if now() - start >= budget_ms {
                        self.job = Some(job);
                        return None;
                    }
                }
            }
        }
    }
}

/// The host clock on native builds, in milliseconds since the first call. The web host passes performance.now().
#[cfg(not(target_arch = "wasm32"))]
pub fn native_clock() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1000.0
}

/// A small LRU of built values keyed by string (a board's key: its model key, its params and its agent list). It holds
/// at most `max` values, the idle ones: `take` hands one out for use (the caller builds when it is None), `release`
/// (or `insert`) puts it back. The value that has waited longest is the one pushed out.
pub struct BuildCache<V> {
    max: usize,
    /// Oldest first: the back is the most recently used.
    entries: VecDeque<(String, V)>,
}

impl<V> BuildCache<V> {
    pub fn new(max: usize) -> Self {
        BuildCache {
            max,
            entries: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    /// The value under `key`, marked as the most recently used.
    pub fn get(&mut self, key: &str) -> Option<&V> {
        let i = self.position(key)?;
        let entry = self.entries.remove(i)?;
        self.entries.push_back(entry);
        self.entries.back().map(|(_, v)| v)
    }

    /// Takes the value under `key` out of the cache for use, like takeBuild. None: the caller builds one.
    pub fn take(&mut self, key: &str) -> Option<V> {
        let i = self.position(key)?;
        self.entries.remove(i).map(|(_, v)| v)
    }

    /// Stores a value under `key` as the most recently used, replacing any value under that key. Returns the values
    /// pushed out (the replaced one, then the oldest past `max`) for the caller to dispose.
    pub fn insert(&mut self, key: String, value: V) -> Vec<V> {
        let mut out = Vec::new();
        if let Some(i) = self.position(&key) {
            if let Some((_, old)) = self.entries.remove(i) {
                out.push(old);
            }
        }
        self.entries.push_back((key, value));
        while self.entries.len() > self.max {
            if let Some((_, old)) = self.entries.pop_front() {
                out.push(old);
            }
        }
        out
    }

    /// Returns a value taken for use, like releaseBuild. The same as `insert`.
    pub fn release(&mut self, key: String, value: V) -> Vec<V> {
        self.insert(key, value)
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|(k, _)| k == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn a_ten_step_build_finishes_over_several_calls_within_the_budget() {
        let clock = Rc::new(Cell::new(0.0_f64));
        let steps = Rc::new(Cell::new(0_u32));
        let (c, s) = (clock.clone(), steps.clone());
        let mut warm = Warm::new(move || {
            c.set(c.get() + 1.0); // each step costs 1 ms on the fake clock
            s.set(s.get() + 1);
            if s.get() == 10 {
                Step::Done(s.get())
            } else {
                Step::More
            }
        });
        let budget = 3.0;
        let mut calls = 0;
        let result = loop {
            calls += 1;
            let before = clock.get();
            let out = warm.run(|| clock.get(), budget);
            // A call stops once the budget is spent: at most the budget plus the one step that crosses it.
            assert!(
                clock.get() - before <= budget + 1.0,
                "call {calls} ran past its budget"
            );
            if let Some(v) = out {
                break v;
            }
            assert!(calls < 20, "the build never finished");
        };
        assert_eq!(result, 10);
        assert_eq!(calls, 4, "3 + 3 + 3 + 1 steps");
        assert!(!warm.busy());
        assert_eq!(warm.run(|| clock.get(), budget), None);
    }

    #[test]
    fn a_zero_budget_still_runs_one_step() {
        let clock = Rc::new(Cell::new(0.0_f64));
        let c = clock.clone();
        let mut warm = Warm::new(move || {
            c.set(c.get() + 1.0);
            if c.get() >= 2.0 {
                Step::Done("done")
            } else {
                Step::More
            }
        });
        assert_eq!(warm.run(|| clock.get(), 0.0), None);
        assert_eq!(warm.run(|| clock.get(), 0.0), Some("done"));
    }

    #[test]
    fn the_cache_pushes_out_the_least_recently_used_value_first() {
        let mut cache = BuildCache::new(2);
        assert!(cache.insert("a".into(), 1).is_empty());
        assert!(cache.insert("b".into(), 2).is_empty());
        assert_eq!(cache.get("a"), Some(&1)); // a is now the most recent, so b is the oldest
        assert_eq!(cache.insert("c".into(), 3), vec![2]);
        assert!(!cache.contains("b"));
        assert!(cache.contains("a") && cache.contains("c"));
        assert_eq!(cache.take("a"), Some(1));
        assert_eq!(cache.take("a"), None);
        assert!(cache.release("a".into(), 4).is_empty()); // c is oldest now, a is the newest
        assert_eq!(cache.insert("d".into(), 5), vec![3]);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn inserting_under_a_key_already_held_replaces_it() {
        let mut cache = BuildCache::new(2);
        cache.insert("a".into(), 1);
        assert_eq!(cache.insert("a".into(), 9), vec![1]);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.take("a"), Some(9));
    }
}
