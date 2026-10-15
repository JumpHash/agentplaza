//! The hub side of loose DiLoCo training (port of trainer/apz_train/hub.py, with the averaging and
//! outer step from trainer/apz_train/diloco.py; protocol in trainer/PROTOCOL.md section 1).
//!
//! Round r closes when every island in the quorum has contributed, or `wait` has passed since the
//! round's first contribution. A delta more than `staleness` rounds behind the hub, or ahead of it,
//! is refused (`accepted` false) and carries the current weights so the island can rebase.
//! All arithmetic is float32, in the same order as the Python code.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::wire::{Tensor, Tensors};

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub outer_lr: f32,
    pub momentum: f32,
    pub staleness: u64,
    pub wait: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            outer_lr: 0.7,
            momentum: 0.9,
            staleness: 2,
            wait: Duration::from_secs(120),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Submitted {
    pub accepted: bool,
    pub round: u64,
    pub weights: Tensors,
}

struct Contribution {
    island: String,
    samples: f64,
    delta: Tensors,
}

#[derive(Default)]
struct State {
    weights: Option<Tensors>,
    velocity: Tensors,
    round: u64,
    /// In arrival order. The float32 sum in `average` follows this order, as the Python dict does.
    pending: Vec<Contribution>,
    first: Option<Instant>,
}

pub struct Hub {
    quorum: usize,
    cfg: Config,
    state: Mutex<State>,
    cond: Condvar,
}

impl Hub {
    pub fn new(islands: &[String], cfg: Config) -> Self {
        Self {
            quorum: islands.len(),
            cfg,
            state: Mutex::new(State::default()),
            cond: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The first call sets the global weights (and zero velocity); later calls get the current ones.
    pub fn init(&self, weights: Tensors) -> (u64, Tensors) {
        let mut st = self.lock();
        if st.weights.is_none() {
            st.velocity = weights
                .iter()
                .map(|(k, t)| {
                    (
                        k.clone(),
                        Tensor {
                            shape: t.shape.clone(),
                            data: vec![0.0; t.data.len()],
                        },
                    )
                })
                .collect();
            st.weights = Some(weights);
        }
        (st.round, st.weights.clone().unwrap_or_default())
    }

    pub fn state(&self) -> (u64, Tensors) {
        let st = self.lock();
        (st.round, st.weights.clone().unwrap_or_default())
    }

    /// Mirrors `Hub.submit` in hub.py. Blocks until the round closes (quorum, or `wait` after the
    /// first contribution). Errors on bad input; a stale or future round is refused with `accepted` false.
    pub fn submit(
        &self,
        island: &str,
        round: u64,
        samples: f64,
        delta: Tensors,
    ) -> Result<Submitted, String> {
        let mut st = self.lock();
        let Some(global) = st.weights.as_ref() else {
            return Err("hub has no global weights yet; call /init first".into());
        };
        if !(samples.is_finite() && samples > 0.0) {
            return Err(format!("samples must be a positive number, got {samples}"));
        }
        if round.saturating_add(self.cfg.staleness) < st.round || round > st.round {
            return Ok(Submitted {
                accepted: false,
                round: st.round,
                weights: global.clone(),
            });
        }
        check_same_layout(global, &delta)?;

        let target = st.round;
        if st.pending.is_empty() {
            st.first = Some(Instant::now());
        }
        match st.pending.iter().position(|c| c.island == island) {
            Some(i) => {
                st.pending[i].samples = samples;
                st.pending[i].delta = delta;
            }
            None => st.pending.push(Contribution {
                island: island.to_string(),
                samples,
                delta,
            }),
        }
        if st.pending.len() >= self.quorum {
            st.close(&self.cfg);
            self.cond.notify_all();
        }

        let deadline = st.first.and_then(|first| first.checked_add(self.cfg.wait));
        while st.round == target {
            match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        st.close(&self.cfg);
                        self.cond.notify_all();
                        break;
                    }
                    st = self
                        .cond
                        .wait_timeout(st, deadline - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                None => st = self.cond.wait(st).unwrap_or_else(PoisonError::into_inner),
            }
        }
        Ok(Submitted {
            accepted: true,
            round: st.round,
            weights: st.weights.clone().unwrap_or_default(),
        })
    }
}

impl State {
    /// Averages the pending deltas, applies the outer step and advances the round.
    fn close(&mut self, cfg: &Config) {
        let Some(weights) = self.weights.as_mut() else {
            return;
        };
        let avg = average(&self.pending);
        outer_update(
            weights,
            &mut self.velocity,
            &avg,
            cfg.outer_lr,
            cfg.momentum,
        );
        self.pending.clear();
        self.first = None;
        self.round += 1;
    }
}

/// Sample-weighted mean of the pending deltas: `sum(s_i * d_i) / sum(s_i)` per tensor, as float32.
/// Each weight is `s_i / total` rounded to float32 first, as in diloco.average_deltas.
fn average(pending: &[Contribution]) -> Tensors {
    let total = pending.iter().fold(0.0f64, |acc, c| acc + c.samples);
    let total = if total == 0.0 { 1.0 } else { total };
    let mut avg = Tensors::new();
    let Some(first) = pending.first() else {
        return avg;
    };
    for (name, t) in &first.delta {
        let mut acc = vec![0.0f32; t.data.len()];
        for c in pending {
            let Some(d) = c.delta.get(name) else { continue };
            let scale = (c.samples / total) as f32;
            for (a, x) in acc.iter_mut().zip(&d.data) {
                *a += scale * x;
            }
        }
        avg.insert(
            name.clone(),
            Tensor {
                shape: t.shape.clone(),
                data: acc,
            },
        );
    }
    avg
}

/// DiLoCo outer step, in place: `v = mu * v + avg`, then `theta = theta - lr * (avg + mu * v)`.
fn outer_update(
    weights: &mut Tensors,
    velocity: &mut Tensors,
    avg: &Tensors,
    outer_lr: f32,
    momentum: f32,
) {
    for (name, w) in weights.iter_mut() {
        let (Some(v), Some(g)) = (velocity.get_mut(name), avg.get(name)) else {
            continue;
        };
        for ((wi, vi), gi) in w.data.iter_mut().zip(v.data.iter_mut()).zip(&g.data) {
            *vi = momentum * *vi + *gi;
            *wi -= outer_lr * (*gi + momentum * *vi);
        }
    }
}

/// A delta must carry exactly the global tensors' names, shapes and lengths.
fn check_same_layout(global: &Tensors, delta: &Tensors) -> Result<(), String> {
    if global.len() != delta.len() {
        return Err(format!(
            "delta has {} tensors, the global weights have {}",
            delta.len(),
            global.len()
        ));
    }
    for (name, g) in global {
        match delta.get(name) {
            Some(d) if d.shape == g.shape && d.data.len() == g.data.len() => {}
            Some(d) => {
                return Err(format!(
                    "tensor {name}: shape {:?} does not match {:?}",
                    d.shape, g.shape
                ));
            }
            None => return Err(format!("delta has no tensor {name}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Instant as Clock;

    use super::*;

    // Every number below comes from testdata/diloco_fixture.py (run with apz_train's hub.py, islands
    // a, b, c, outer lr 0.7, momentum 0.9, staleness 2). Tensors: "w" [2,3] and "b" [4].

    fn tensors(w: Vec<f32>, b: Vec<f32>) -> Tensors {
        let mut m = Tensors::new();
        m.insert(
            "w".into(),
            Tensor {
                shape: vec![2, 3],
                data: w,
            },
        );
        m.insert(
            "b".into(),
            Tensor {
                shape: vec![4],
                data: b,
            },
        );
        m
    }

    fn w0() -> Tensors {
        tensors(
            vec![0.5, -1.0, 0.25, 2.0, -0.125, 0.75],
            vec![0.0, 1.5, -2.5, 4.0],
        )
    }
    fn d0a() -> Tensors {
        tensors(
            vec![0.25, 0.5, -0.75, 1.0, -0.5, 0.125],
            vec![0.5, -1.0, 0.25, 2.0],
        )
    }
    fn d0b() -> Tensors {
        tensors(
            vec![-0.5, 0.25, 0.5, 0.75, 0.0, -0.25],
            vec![1.0, 0.5, -0.5, -1.5],
        )
    }
    fn d0c() -> Tensors {
        tensors(
            vec![1.5, -0.25, 0.0, -1.0, 0.375, 0.5],
            vec![-0.25, 0.0, 1.0, 0.5],
        )
    }
    fn d1a() -> Tensors {
        tensors(vec![0.5; 6], vec![0.125, 0.25, 0.375, 0.5])
    }
    fn d2a() -> Tensors {
        tensors(
            vec![1.0, -1.0, 0.5, 0.25, 0.25, -0.5],
            vec![0.0, 0.5, 0.5, -0.5],
        )
    }
    fn d2b() -> Tensors {
        tensors(
            vec![0.125, 0.0, -0.125, 0.5, -0.5, 0.0],
            vec![2.0, -1.0, 0.0, 0.25],
        )
    }
    fn d2c() -> Tensors {
        tensors(
            vec![-0.5, 0.5, 0.25, 0.0, 1.0, -1.0],
            vec![0.75, 0.25, -0.25, 1.0],
        )
    }
    fn d3a() -> Tensors {
        tensors(
            vec![0.5, 0.25, 0.0, -0.25, 0.5, 1.0],
            vec![0.125, -0.125, 0.5, 0.0],
        )
    }
    fn d3b() -> Tensors {
        tensors(
            vec![-1.0, 0.0, 0.75, 0.25, -0.5, 0.5],
            vec![0.5, 0.5, 0.5, 0.5],
        )
    }
    fn d3c() -> Tensors {
        tensors(
            vec![0.0, 0.5, -0.5, 0.5, 0.0, 0.25],
            vec![-0.5, 1.0, 0.0, -1.0],
        )
    }

    // Global weights after each closed round (python: W1..W4).
    fn w1() -> Tensors {
        tensors(
            vec![
                0.112083346,
                -1.16625,
                0.08375001,
                1.7229166,
                -0.18041667,
                0.666875,
            ],
            vec![-0.66499996, 1.3891667, -2.66625, 4.3325],
        )
    }
    fn w2() -> Tensors {
        tensors(
            vec![
                -0.71829164,
                -1.9021249,
                -0.65212494,
                0.9397916,
                -0.8690417,
                -0.03356242,
            ],
            vec![-1.1147499, 1.0094167, -3.2358751, 3.8092499],
        )
    }
    fn w3() -> Tensors {
        tensors(
            vec![
                -1.1921916, -2.2494123, -1.29035, 0.30060416, -1.7556791, 0.48229384,
            ],
            vec![-2.6045249, 0.8251417, -3.5122876, 3.0714498],
        )
    }
    fn w4() -> Tensors {
        tensors(
            vec![
                -1.2665141,
                -2.9498878,
                -1.7269399,
                -0.54445624,
                -2.1671946,
                -0.056768775,
            ],
            vec![-3.3109472, -0.19966406, -4.093559, 3.126388],
        )
    }

    fn islands(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn new_hub(names: &[&str], wait: Duration) -> Arc<Hub> {
        Arc::new(Hub::new(
            &islands(names),
            Config {
                wait,
                ..Config::default()
            },
        ))
    }

    /// Blocks until `island` is pending in `round` (or the round has already moved on), so that
    /// contributions from threads land in a known order.
    fn wait_until_pending(hub: &Hub, island: &str, round: u64) {
        let start = Clock::now();
        loop {
            {
                let st = hub.lock();
                if st.round != round || st.pending.iter().any(|c| c.island == island) {
                    return;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "{island} never became pending"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Submits from one thread per island, in order, and returns the replies in that order.
    fn submit_in_order(hub: &Arc<Hub>, subs: Vec<(&str, u64, f64, Tensors)>) -> Vec<Submitted> {
        let mut handles = Vec::new();
        for (island, round, samples, delta) in subs {
            let h = Arc::clone(hub);
            let name = island.to_string();
            handles.push(thread::spawn(move || {
                h.submit(&name, round, samples, delta).unwrap()
            }));
            wait_until_pending(hub, island, round);
        }
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    }

    #[test]
    fn matches_python_hub_fixture() {
        let hub = new_hub(&["a", "b", "c"], Duration::from_secs(2));
        assert_eq!(hub.init(w0()), (0, w0()));

        // Round 0 closes on quorum.
        let out = submit_in_order(
            &hub,
            vec![
                ("a", 0, 10.0, d0a()),
                ("b", 0, 30.0, d0b()),
                ("c", 0, 20.0, d0c()),
            ],
        );
        for s in &out {
            assert_eq!((s.accepted, s.round, &s.weights), (true, 1, &w1()));
        }

        // Round 1 has only island a and closes on the wait timeout (2 s).
        let s = hub.submit("a", 1, 7.0, d1a()).unwrap();
        assert_eq!((s.accepted, s.round, s.weights), (true, 2, w2()));

        // Round 2 closes on quorum.
        let out = submit_in_order(
            &hub,
            vec![
                ("a", 2, 5.0, d2a()),
                ("b", 2, 5.0, d2b()),
                ("c", 2, 10.0, d2c()),
            ],
        );
        for s in &out {
            assert_eq!((s.accepted, s.round, &s.weights), (true, 3, &w3()));
        }

        // Stale (0 + 2 < 3) and ahead (9 > 3) deltas are refused with the current weights.
        let stale = hub.submit("b", 0, 30.0, d0b()).unwrap();
        assert_eq!(
            (stale.accepted, stale.round, stale.weights),
            (false, 3, w3())
        );
        let ahead = hub.submit("c", 9, 20.0, d0c()).unwrap();
        assert_eq!(
            (ahead.accepted, ahead.round, ahead.weights),
            (false, 3, w3())
        );

        // Round 3 closes on quorum.
        let out = submit_in_order(
            &hub,
            vec![
                ("a", 3, 4.0, d3a()),
                ("b", 3, 8.0, d3b()),
                ("c", 3, 12.0, d3c()),
            ],
        );
        for s in &out {
            assert_eq!((s.accepted, s.round, &s.weights), (true, 4, &w4()));
        }
        assert_eq!(hub.state(), (4, w4()));
    }

    #[test]
    fn init_keeps_the_first_weights() {
        let hub = new_hub(&["a"], Duration::from_secs(5));
        hub.init(w0());
        assert_eq!(hub.init(w1()), (0, w0()));
        assert_eq!(hub.state(), (0, w0()));
    }

    #[test]
    fn two_threads_close_a_round_once() {
        let hub = new_hub(&["x", "y"], Duration::from_secs(5));
        hub.init(w0());
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for (island, samples, delta) in [("x", 1.0, d0a()), ("y", 3.0, d0b())] {
            let (h, barrier) = (Arc::clone(&hub), Arc::clone(&barrier));
            handles.push(thread::spawn(move || {
                barrier.wait();
                h.submit(island, 0, samples, delta).unwrap()
            }));
        }
        let replies: Vec<Submitted> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(replies.iter().all(|s| s.accepted && s.round == 1));
        assert_eq!(replies[0].weights, replies[1].weights);
        assert_eq!(hub.state().0, 1, "the round must close exactly once");
    }

    #[test]
    fn resubmit_in_the_same_round_replaces_the_delta() {
        let hub = new_hub(&["a", "b"], Duration::from_secs(5));
        hub.init(w0());
        let first = {
            let h = Arc::clone(&hub);
            thread::spawn(move || h.submit("a", 0, 2.0, d0a()).unwrap())
        };
        wait_until_pending(&hub, "a", 0);
        let second = {
            let h = Arc::clone(&hub);
            thread::spawn(move || h.submit("a", 0, 6.0, d0c()).unwrap())
        };
        wait_for_delta(&hub, "a", &d0c());
        let b = hub.submit("b", 0, 2.0, d0b()).unwrap();
        assert_eq!((b.accepted, b.round), (true, 1));
        assert_eq!(first.join().unwrap().weights, b.weights);
        assert_eq!(second.join().unwrap().weights, b.weights);

        // Same as a fresh hub that only ever saw the replacement, in the same arrival order.
        let fresh = new_hub(&["a", "b"], Duration::from_secs(5));
        fresh.init(w0());
        let out = submit_in_order(&fresh, vec![("a", 0, 6.0, d0c()), ("b", 0, 2.0, d0b())]);
        assert_eq!(out[0].weights, b.weights);
    }

    /// Blocks until `island`'s pending delta equals `delta`.
    fn wait_for_delta(hub: &Hub, island: &str, delta: &Tensors) {
        let start = Clock::now();
        loop {
            {
                let st = hub.lock();
                if st
                    .pending
                    .iter()
                    .any(|c| c.island == island && c.delta == *delta)
                {
                    return;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "{island}'s delta never replaced"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn rejects_bad_submissions() {
        let hub = new_hub(&["a"], Duration::from_secs(5));
        assert!(
            hub.submit("a", 0, 1.0, d0a())
                .unwrap_err()
                .contains("no global weights")
        );
        hub.init(w0());
        assert!(hub.submit("a", 0, 0.0, d0a()).is_err());
        assert!(hub.submit("a", 0, f64::NAN, d0a()).is_err());
        let mut missing = d0a();
        missing.remove("b");
        assert!(
            hub.submit("a", 0, 1.0, missing)
                .unwrap_err()
                .contains("tensor")
        );
        let mut wrong_shape = d0a();
        wrong_shape.get_mut("w").unwrap().shape = vec![3, 2];
        assert!(
            hub.submit("a", 0, 1.0, wrong_shape)
                .unwrap_err()
                .contains("does not match")
        );
        assert_eq!(hub.state(), (0, w0()), "bad submissions change nothing");
    }
}
