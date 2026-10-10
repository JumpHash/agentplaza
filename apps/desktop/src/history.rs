//! The session's rolling telemetry history: one observation per poll, the
//! figures derived from it, and the CSV the session exports as.
//!
//! Everything here is a pure derivation over what the daemon reported and what
//! [`Activity`](crate::map::Activity) measured, so the ring's lifecycle, the
//! RAM and throughput arithmetic, the gap segmentation and the CSV bytes are
//! all unit-tested without a display or a window.
//!
//! Three rules carry the honesty of the figures, and each is a test:
//!
//! * a poll that fails writes **nothing**, so unobserved time is un-inked;
//! * throughput counts only the VMs that are still up — a VM that has left the
//!   running state keeps a frozen EMA in the activity feed, and counting that
//!   would inflate the fleet line forever after a run ends;
//! * a run that leaves the register takes its per-VM rings with it.

use std::collections::HashMap;
use std::ops::Range;

use crate::api::{Run, VmState};
use crate::map::Activity;

/// How many observations the session keeps. At the default cadence this is
/// about two minutes of the register's own answers.
pub const HISTORY_CAP: usize = 120;

/// How many times the poll cadence a spacing may be before the charts treat it
/// as unobserved time rather than a continuous stretch.
pub const GAP_FACTOR: u64 = 2;

/// How many observations a chart needs before a figure is worth drawing.
pub const CHART_MIN_SAMPLES: usize = 3;

/// The first line of every session CSV, so a file found later says what it is.
pub const CSV_HEADER: &str = "observed_at,elapsed_s,scope,vms_live,ram_mib,lines_per_s";

/// How many of a run's VMs are still up.
pub fn live_vms(runs: &[Run]) -> usize {
    runs.iter().map(Run::live_vms).sum()
}

/// The guest memory the register's live VMs have committed, in MiB: what the
/// host has been asked to hold right now.
pub fn committed_mib(runs: &[Run]) -> u64 {
    runs.iter()
        .map(|run| run.live_vms() as u64 * u64::from(run.memory_mib))
        .sum()
}

/// One run's serial throughput: the smoothed rate of the VMs that are still
/// up, and nothing for a VM that has left the running state.
pub fn run_lps(run: &Run, activity: &Activity) -> f32 {
    total(
        run.vms
            .iter()
            .filter(|vm| vm.state == VmState::Running)
            .map(|vm| activity.lps(&run.id, vm.id)),
    )
}

/// The fleet's serial throughput: every running VM's smoothed rate, added up.
pub fn fleet_lps(runs: &[Run], activity: &Activity) -> f32 {
    total(runs.iter().map(|run| run_lps(run, activity)))
}

/// A sum of rates that never reads as negative zero.
///
/// `f32`'s own sum starts at `-0.0`, so an empty fleet would write `-0.00`
/// lines per second — a figure nobody measured, in a file that is evidence.
fn total(values: impl Iterator<Item = f32>) -> f32 {
    let sum: f32 = values.sum();
    if sum == 0.0 { 0.0 } else { sum }
}

/// One run's part of an observation.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSlice {
    /// The run's id.
    pub run: String,
    /// How many of its VMs were up.
    pub vms: usize,
    /// The guest memory they had committed, in MiB.
    pub ram_mib: u64,
    /// The run's own serial throughput.
    pub lps: f32,
}

/// One poll's observation of the fleet.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetSample {
    /// The entry number, counting from the session's first observation. It
    /// survives the cap, so the register can say how many it has aged out.
    pub no: u64,
    /// When the poll answered, in milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// How many VMs were up across the fleet.
    pub vms: usize,
    /// The guest memory the live VMs had committed, in MiB.
    pub ram_mib: u64,
    /// The fleet's serial throughput.
    pub lps: f32,
    /// Each run's own part of the observation, in the order the daemon listed
    /// them.
    pub runs: Vec<RunSlice>,
}

/// The session's observations: a capped rolling buffer of fleet samples, and a
/// capped ring of each VM's serial rate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionHistory {
    samples: Vec<FleetSample>,
    written: u64,
    per_vm: HashMap<String, Vec<Vec<f32>>>,
}

impl SessionHistory {
    /// Writes one observation of `runs`, answered by `activity`, and returns
    /// its entry number.
    ///
    /// Called once per poll that actually observed something: a poll that
    /// failed must not call it, so the charts never ink a figure nobody saw.
    pub fn sample(&mut self, at_ms: u64, runs: &[Run], activity: &Activity) -> u64 {
        self.written += 1;
        let slices: Vec<RunSlice> = runs
            .iter()
            .map(|run| RunSlice {
                run: run.id.clone(),
                vms: run.live_vms(),
                ram_mib: run.live_vms() as u64 * u64::from(run.memory_mib),
                lps: run_lps(run, activity),
            })
            .collect();
        let sample = FleetSample {
            no: self.written,
            at_ms,
            vms: slices.iter().map(|slice| slice.vms).sum(),
            ram_mib: slices.iter().map(|slice| slice.ram_mib).sum(),
            lps: total(slices.iter().map(|slice| slice.lps)),
            runs: slices,
        };
        self.samples.push(sample);
        self.trim();
        self.written
    }

    /// Records one VM's smoothed serial rate, whether it is still up or not:
    /// the ring is the VM's own history, and a run pane reading a finished VM
    /// still has the shape of its burst.
    pub fn observe_vm(&mut self, run: &str, vm: u32, lps: f32) {
        let rings = self.per_vm.entry(run.to_string()).or_default();
        let index = vm as usize;
        if rings.len() <= index {
            rings.resize_with(index + 1, Vec::new);
        }
        let ring = &mut rings[index];
        ring.push(lps);
        if ring.len() > HISTORY_CAP {
            let excess = ring.len() - HISTORY_CAP;
            ring.drain(..excess);
        }
    }

    /// Forgets every run that is no longer in the register: its per-VM rings
    /// go with it. The fleet samples already written stay — they are what the
    /// session observed, and they age out of the window on their own.
    pub fn retain(&mut self, runs: &[Run]) {
        self.per_vm
            .retain(|id, _| runs.iter().any(|run| run.id == *id));
    }

    /// Forgets one run's rings: a run id the daemon has begun again is a
    /// different run, and its history is not this one's.
    pub fn forget(&mut self, run: &str) {
        self.per_vm.remove(run);
    }

    /// One VM's rates, oldest first; empty when it was never observed.
    pub fn vm_series(&self, run: &str, vm: u32) -> &[f32] {
        self.per_vm
            .get(run)
            .and_then(|rings| rings.get(vm as usize))
            .map_or(&[], Vec::as_slice)
    }

    /// The observations still in the window, oldest first.
    pub fn samples(&self) -> &[FleetSample] {
        &self.samples
    }

    /// The newest observation.
    pub fn latest(&self) -> Option<&FleetSample> {
        self.samples.last()
    }

    /// How many observations are in the window.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether no poll has observed anything yet.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// How many observations the session has written, kept or aged out.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// How long the window spans, in milliseconds.
    pub fn span_ms(&self) -> u64 {
        match (self.samples.first(), self.samples.last()) {
            (Some(first), Some(last)) => last.at_ms.saturating_sub(first.at_ms),
            _ => 0,
        }
    }

    /// The stretches of consecutive observations, broken where two stand more
    /// than [`GAP_FACTOR`] cadences apart: unobserved time is un-inked.
    pub fn segments(&self, gap_ms: u64) -> Vec<Range<usize>> {
        segments(&self.samples, gap_ms)
    }

    /// Drops the oldest observations once the window is full.
    fn trim(&mut self) {
        if self.samples.len() > HISTORY_CAP {
            let excess = self.samples.len() - HISTORY_CAP;
            self.samples.drain(..excess);
        }
    }
}

/// The runs of consecutive observations whose spacing is at most `gap_ms`.
///
/// A chart draws one line per stretch and lifts its pen between them, so a
/// hidden window, a throttled poll or a daemon outage reads as a break rather
/// than as a straight line nobody observed.
pub fn segments(samples: &[FleetSample], gap_ms: u64) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for index in 0..samples.len() {
        let continuous = index > 0
            && samples[index].at_ms.saturating_sub(samples[index - 1].at_ms) <= gap_ms;
        if continuous {
            if let Some(last) = out.last_mut() {
                last.end = index + 1;
            }
        } else {
            out.push(index..index + 1);
        }
    }
    out
}

/// How many stretches the observations are drawn in, past the first.
pub fn gap_count(samples: &[FleetSample], gap_ms: u64) -> usize {
    segments(samples, gap_ms).len().saturating_sub(1)
}

/// The session's observations as a CSV, long form: one `fleet` row per
/// observation, then one row per run of that observation.
///
/// Long form rather than a wide sheet because runs arrive and leave
/// mid-session, and a wide sheet would lie with empty cells. Only run ids and
/// figures ever reach it — never guest serial text.
pub fn telemetry_csv(history: &SessionHistory) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');
    let Some(first) = history.samples().first() else {
        return out;
    };
    let t0 = first.at_ms;
    for sample in history.samples() {
        let elapsed = sample.at_ms.saturating_sub(t0) as f64 / 1_000.0;
        out.push_str(&format!(
            "{},{elapsed:.3},fleet,{},{},{:.2}\n",
            sample.at_ms, sample.vms, sample.ram_mib, sample.lps
        ));
        for slice in &sample.runs {
            out.push_str(&format!(
                "{},{elapsed:.3},{},{},{},{:.2}\n",
                sample.at_ms, slice.run, slice.vms, slice.ram_mib, slice.lps
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{RunState, Vm};

    fn vm(id: u32, state: VmState) -> Vm {
        Vm {
            id,
            guest_cid: None,
            state,
            serial: None,
        }
    }

    fn run(id: &str, memory_mib: u32, vms: Vec<Vm>) -> Run {
        Run {
            id: id.to_string(),
            state: RunState::Running,
            vcpus: 2,
            memory_mib,
            beats: None,
            created_at: 1_000_000,
            ended_at: None,
            vms,
        }
    }

    /// The scale preset's run: fifty 128 MiB VMs.
    fn scale(live: u32) -> Run {
        run(
            "r9",
            128,
            (0..live).map(|id| vm(id, VmState::Running)).collect(),
        )
    }

    #[test]
    fn the_ring_caps_and_each_observation_keeps_its_entry_number() {
        let mut history = SessionHistory::default();
        for step in 0..130u64 {
            history.sample(1_000 + step * 100, &[], &Activity::default());
        }
        assert_eq!(history.len(), HISTORY_CAP);
        assert_eq!(history.written(), 130);
        let samples = history.samples();
        assert_eq!(samples.first().expect("a window").no, 11, "ten aged out");
        assert_eq!(samples.last().expect("a window").no, 130);
        assert_eq!(
            samples.last().expect("a window").no - samples.first().expect("a window").no,
            HISTORY_CAP as u64 - 1,
            "the window is 120 consecutive observations"
        );
    }

    #[test]
    fn ram_is_the_live_vms_commitment_and_throughput_counts_the_fleet() {
        let mut activity = Activity::default();
        // r1: two VMs up, both writing; r2: one VM, already gone.
        activity.observe("r1", 0, 0, 1_000);
        activity.observe("r1", 0, 20, 3_000);
        activity.observe("r1", 1, 0, 3_000);

        let runs = vec![
            run(
                "r1",
                512,
                vec![vm(0, VmState::Running), vm(1, VmState::Running)],
            ),
            run("r2", 128, vec![vm(0, VmState::Exited)]),
        ];
        let mut history = SessionHistory::default();
        history.sample(5_000, &runs, &activity);

        let sample = history.latest().expect("an observation");
        assert_eq!(sample.ram_mib, 2 * 512, "r2's VM is not committed");
        assert_eq!(sample.vms, 2);
        assert_eq!(sample.runs.len(), 2);
        assert_eq!(sample.runs[0].run, "r1");
        assert_eq!(sample.runs[0].lps, sample.lps, "r2 adds nothing");
        assert!(sample.lps > 0.0);
    }

    #[test]
    fn a_frozen_ema_stops_counting_the_poll_its_vm_leaves() {
        // The VM was measured while up, so its EMA is a real rate in the feed.
        let mut activity = Activity::default();
        activity.observe("r1", 0, 0, 1_000);
        activity.observe("r1", 0, 40, 3_000);
        let running = activity.lps("r1", 0);
        assert!(running > 0.0, "the feed holds a rate of {running}");

        let mut history = SessionHistory::default();
        let up = run("r1", 512, vec![vm(0, VmState::Running)]);
        history.sample(4_000, std::slice::from_ref(&up), &activity);
        assert!(history.latest().expect("up").lps > 0.0);

        // The same feed, the same frozen EMA, a VM that has left the state.
        let gone = run("r1", 512, vec![vm(0, VmState::Exited)]);
        history.sample(6_000, std::slice::from_ref(&gone), &activity);
        let sample = history.latest().expect("gone");
        assert_eq!(
            sample.lps, 0.0,
            "a frozen EMA must not inflate the fleet line after the vm exits"
        );
        assert_eq!(sample.ram_mib, 0, "nor commit memory it no longer holds");
        assert_eq!(sample.runs[0].lps, 0.0);
    }

    #[test]
    fn a_per_vm_ring_caps_and_keeps_the_newest_rates() {
        let mut history = SessionHistory::default();
        for step in 0..HISTORY_CAP + 5 {
            history.observe_vm("r1", 0, step as f32);
        }
        let series = history.vm_series("r1", 0);
        assert_eq!(series.len(), HISTORY_CAP);
        assert_eq!(series.first(), Some(&5.0), "the oldest five aged out");
        assert_eq!(series.last(), Some(&(HISTORY_CAP as f32 + 4.0)));
        assert!(history.vm_series("r1", 7).is_empty(), "an unseen vm is quiet");
        assert!(history.vm_series("r2", 0).is_empty(), "an unseen run is quiet");
    }

    #[test]
    fn a_run_that_left_the_register_takes_its_vm_rings_with_it() {
        let mut history = SessionHistory::default();
        history.observe_vm("r1", 0, 3.0);
        history.observe_vm("r2", 0, 4.0);
        history.sample(1_000, &[run("r2", 128, vec![vm(0, VmState::Running)])], &Activity::default());

        history.retain(&[run("r9", 128, vec![vm(0, VmState::Running)])]);
        assert!(history.vm_series("r1", 0).is_empty());
        assert!(history.vm_series("r2", 0).is_empty());
        assert_eq!(
            history.len(),
            1,
            "the observations already written stay: they are the session's history"
        );
    }

    #[test]
    fn a_gap_in_the_observations_lifts_the_pen() {
        let mut history = SessionHistory::default();
        for at_ms in [1_000u64, 3_000, 5_000, 20_000, 22_000] {
            history.sample(at_ms, &[], &Activity::default());
        }
        // A two-second cadence: 15 s between polls is unobserved time.
        let spans = history.segments(4_000);
        assert_eq!(spans, vec![0..3, 3..5]);
        assert_eq!(gap_count(history.samples(), 4_000), 1);

        // The same observations at a slower cadence are continuous.
        assert_eq!(history.segments(30_000), vec![0..5]);
        assert_eq!(gap_count(history.samples(), 30_000), 0);
    }

    #[test]
    fn a_series_too_short_to_break_is_one_stretch() {
        assert!(segments(&[], 4_000).is_empty());
        let one = [FleetSample {
            no: 1,
            at_ms: 1_000,
            vms: 0,
            ram_mib: 0,
            lps: 0.0,
            runs: Vec::new(),
        }];
        assert_eq!(segments(&one, 4_000), vec![0..1]);
        assert_eq!(gap_count(&one, 4_000), 0);
    }

    #[test]
    fn the_span_is_the_window_the_charts_draw() {
        let mut history = SessionHistory::default();
        assert_eq!(history.span_ms(), 0);
        history.sample(10_000, &[], &Activity::default());
        assert_eq!(history.span_ms(), 0, "one observation spans nothing yet");
        history.sample(37_500, &[], &Activity::default());
        assert_eq!(history.span_ms(), 27_500);
    }

    #[test]
    fn the_csv_is_one_fleet_row_and_one_row_per_run_per_observation() {
        let mut activity = Activity::default();
        activity.observe("r9", 0, 0, 1_000);
        activity.observe("r9", 0, 40, 3_000);

        let mut history = SessionHistory::default();
        history.sample(1_760_000_000_000, &[], &Activity::default());
        history.sample(1_760_000_001_500, &[scale(2)], &activity);
        history.sample(1_760_000_003_000, &[scale(1)], &activity);

        let expected = "\
observed_at,elapsed_s,scope,vms_live,ram_mib,lines_per_s
1760000000000,0.000,fleet,0,0,0.00
1760000001500,1.500,fleet,2,256,8.00
1760000001500,1.500,r9,2,256,8.00
1760000003000,3.000,fleet,1,128,8.00
1760000003000,3.000,r9,1,128,8.00
";
        let csv = telemetry_csv(&history);
        assert_eq!(csv, expected, "the csv rows are byte-exact");
        // Every row carries the six fields of the header, and nothing of the
        // guest's own text can reach them: the daemon's run ids are the only
        // strings in the file.
        for row in csv.lines().skip(1) {
            assert_eq!(row.split(',').count(), 6, "{row}");
        }
    }

    #[test]
    fn a_session_that_observed_nothing_exports_its_header_alone() {
        assert_eq!(
            telemetry_csv(&SessionHistory::default()),
            "observed_at,elapsed_s,scope,vms_live,ram_mib,lines_per_s\n"
        );
    }

    #[test]
    fn the_elapsed_seconds_are_measured_from_the_first_observation() {
        let mut history = SessionHistory::default();
        history.sample(5_000, &[], &Activity::default());
        history.sample(65_250, &[], &Activity::default());
        let csv = telemetry_csv(&history);
        let rows: Vec<&str> = csv.lines().skip(1).collect();
        assert!(rows[0].starts_with("5000,0.000,"), "{:?}", rows[0]);
        assert!(rows[1].starts_with("65250,60.250,"), "{:?}", rows[1]);
    }

    #[test]
    fn the_fleet_totals_are_the_sums_of_the_runs_slices() {
        let mut activity = Activity::default();
        activity.observe("r1", 0, 0, 1_000);
        activity.observe("r1", 0, 10, 2_000);
        let runs = vec![
            run("r1", 256, vec![vm(0, VmState::Running)]),
            run("r2", 128, vec![vm(0, VmState::Running), vm(1, VmState::Running)]),
        ];
        let mut history = SessionHistory::default();
        history.sample(3_000, &runs, &activity);
        let sample = history.latest().expect("an observation");
        assert_eq!(sample.vms, live_vms(&runs));
        assert_eq!(sample.ram_mib, committed_mib(&runs));
        assert_eq!(sample.lps, fleet_lps(&runs, &activity));
        assert_eq!(sample.ram_mib, 256 + 2 * 128);
    }
}
