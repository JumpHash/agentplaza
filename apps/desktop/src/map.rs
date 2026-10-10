//! The fleet map's geometry and its activity feed — pure, iced-free.
//!
//! The map is a plate: the machine is the bounded substrate region, each run is
//! a lane across it, and each VM is a cell in its run's lane, placed by index
//! (`x` by VM index, `y` by run index). Every drawn thing is computed here from
//! real daemon data, so the layout, the state marks and the hit-testing are
//! unit-tested without a display; the iced canvas in
//! [`mapview`](crate::mapview) only paints what this module returns.

use std::collections::HashMap;

use crate::api::{Run, RunState, VmState};

/// The room a lane's label gets on the left of the substrate.
pub const PLOT_LEFT: f32 = 132.0;
/// The room the machine's model line gets above the substrate.
pub const SUBSTRATE_TOP: f32 = 56.0;
/// How tall one run's lane is.
pub const LANE_H: f32 = 72.0;
/// How far apart a lane's VM cells are.
pub const CELL_STEP: f32 = 54.0;
/// The first cell's offset inside the plot.
pub const CELL_INSET: f32 = 34.0;
/// A state mark's radius.
pub const MARK_R: f32 = 7.0;
/// How far from a cell's centre a click still lands on it.
pub const HIT_SLACK: f32 = 18.0;
/// The serial rate a full activity bar stands for, in lines per second.
pub const ACTIVITY_FULL: f32 = 6.0;

/// A rectangle in the canvas's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Rect {
    /// The rectangle's centre.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// The right edge.
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// The bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

/// One run, as a lane of the substrate.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    /// The run's id.
    pub run_id: String,
    /// The run's state.
    pub state: RunState,
    /// How many VMs the run has.
    pub vms: usize,
    /// vCPUs per VM.
    pub vcpus: u32,
    /// Guest memory per VM, in MiB.
    pub memory_mib: u32,
    /// The lane's vertical centre.
    pub y: f32,
    /// The lane's ruled baseline, below its label.
    pub rule_y: f32,
}

/// One VM, as a cell of its run's lane.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The id of the run the cell belongs to.
    pub run_id: String,
    /// The VM's index among the run's VMs.
    pub vm: u32,
    /// The VM's place among the run's VMs, as the lane draws it.
    pub index: usize,
    /// Which lane the cell sits in.
    pub lane: usize,
    /// The cell's centre.
    pub x: f32,
    /// The cell's centre.
    pub y: f32,
    /// The VM's state: the shape the mark is drawn as.
    pub state: VmState,
    /// The VM's vsock guest CID, when it has one.
    pub guest_cid: Option<u64>,
    /// The VM's serial rate, in lines per second, as last observed.
    pub lps: f32,
}

/// How many of a lane's cells fit across the plate, and how many are past its
/// right edge.
///
/// The plate is a fixed scale — cells sit [`CELL_STEP`] apart by index — so a
/// run with more VMs than its width holds has its tail clipped. Nothing here
/// hides a VM: the register and the run pane list every one, and the map says
/// how many of them it can draw.
pub fn lane_overflow(plate: &Layout, lane: usize) -> (usize, usize) {
    let right = plate.substrate.right();
    plate
        .cells
        .iter()
        .filter(|cell| cell.lane == lane)
        .fold((0, 0), |(drawn, past), cell| {
            if cell.x + MARK_R <= right {
                (drawn + 1, past)
            } else {
                (drawn, past + 1)
            }
        })
}

/// The plate: the substrate, its lanes and their cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The machine's bounded region.
    pub substrate: Rect,
    /// One lane per run, oldest first.
    pub lanes: Vec<Lane>,
    /// One cell per VM of every run.
    pub cells: Vec<Cell>,
}

/// The numeric part of a run id (`r12` -> 12); 0 when it is not `r<digits>`.
///
/// The map orders lanes by this, so `r9` sits before `r10`.
pub fn seq_of(id: &str) -> u32 {
    let digits = id.strip_prefix('r').unwrap_or(id);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return 0;
    }
    digits.parse().unwrap_or(0)
}

/// Lays the runs out inside a plate of `width` by `height`.
///
/// The substrate is bounded by the plate; lanes are stacked by run index and
/// cells by VM index, so two polls of the same fleet draw the same plate.
pub fn layout(runs: &[Run], activity: &Activity, width: f32, height: f32) -> Layout {
    let substrate = Rect {
        x: 8.0,
        y: SUBSTRATE_TOP,
        width: (width - 16.0).max(0.0),
        height: (height - SUBSTRATE_TOP - 8.0).max(0.0),
    };

    let mut ordered: Vec<&Run> = runs.iter().collect();
    ordered.sort_by_key(|run| seq_of(&run.id));

    let mut lanes = Vec::with_capacity(ordered.len());
    let mut cells = Vec::new();
    for (index, run) in ordered.into_iter().enumerate() {
        let y = substrate.y + LANE_H * (index as f32 + 0.5);
        lanes.push(Lane {
            run_id: run.id.clone(),
            state: run.state,
            vms: run.vms.len(),
            vcpus: run.vcpus,
            memory_mib: run.memory_mib,
            y,
            rule_y: substrate.y + LANE_H * (index as f32 + 1.0),
        });
        for (place, vm) in run.vms.iter().enumerate() {
            cells.push(Cell {
                run_id: run.id.clone(),
                vm: vm.id,
                index: place,
                lane: index,
                x: PLOT_LEFT + CELL_INSET + CELL_STEP * place as f32,
                y,
                state: vm.state,
                guest_cid: vm.guest_cid,
                lps: activity.lps(&run.id, vm.id),
            });
        }
    }
    Layout {
        substrate,
        lanes,
        cells,
    }
}

impl Layout {
    /// The cell under `(x, y)`, if any: the map's one interaction.
    pub fn cell_at(&self, x: f32, y: f32) -> Option<&Cell> {
        self.cells.iter().find(|cell| {
            (x - cell.x).abs() <= HIT_SLACK && (y - cell.y).abs() <= HIT_SLACK
        })
    }

    /// The lane of `run_id`, if the run is on the plate.
    pub fn lane_of(&self, run_id: &str) -> Option<&Lane> {
        self.lanes.iter().find(|lane| lane.run_id == run_id)
    }

    /// Whether the plate holds anything but the substrate.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// A per-VM sample of its serial capture: how much it grew, and when.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    /// The capture's line count when it was seen.
    lines: usize,
    /// When it was seen, in milliseconds since the Unix epoch.
    at_ms: u64,
    /// Smoothed lines per second; decays toward 0 as polls see no growth.
    lps: f32,
}

/// The weight each new sample carries in the smoothed rate.
pub const ACTIVITY_ALPHA: f32 = 0.4;

/// Every VM's serial rate, from the deltas between consecutive polls.
///
/// A VM's first sighting is only a baseline: it reports 0 lines/s, because one
/// poll cannot tell growth from a capture that was always that long.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Activity {
    runs: HashMap<String, Vec<Option<Sample>>>,
}

impl Activity {
    /// Records `lines` for one VM at `at_ms`, and answers its smoothed rate.
    pub fn observe(&mut self, run_id: &str, vm: u32, lines: usize, at_ms: u64) -> f32 {
        let samples = self.runs.entry(run_id.to_string()).or_default();
        let index = vm as usize;
        if samples.len() <= index {
            samples.resize(index + 1, None);
        }
        let previous = samples[index];
        let lps = match previous {
            None => 0.0,
            Some(previous) => {
                let grew = lines.saturating_sub(previous.lines) as f32;
                let seconds = (at_ms.saturating_sub(previous.at_ms) as f32 / 1_000.0).max(1.0);
                let instant = grew / seconds;
                previous.lps + ACTIVITY_ALPHA * (instant - previous.lps)
            }
        };
        samples[index] = Some(Sample { lines, at_ms, lps });
        lps
    }

    /// The smoothed rate last recorded for a VM; 0 when it was never seen.
    pub fn lps(&self, run_id: &str, vm: u32) -> f32 {
        self.runs
            .get(run_id)
            .and_then(|samples| samples.get(vm as usize))
            .and_then(|sample| sample.as_ref())
            .map_or(0.0, |sample| sample.lps)
    }

    /// Forgets every run that is no longer in the register.
    pub fn retain(&mut self, runs: &[Run]) {
        self.runs
            .retain(|id, _| runs.iter().any(|run| run.id.as_str() == id));
    }

    /// How many VMs are being watched: the map's feed size.
    pub fn len(&self) -> usize {
        self.runs.values().map(Vec::len).sum()
    }

    /// Whether nothing is being watched.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Vm;

    fn vm(id: u32, state: VmState, cid: Option<u64>) -> Vm {
        Vm {
            id,
            guest_cid: cid,
            state,
            serial: None,
        }
    }

    fn run(id: &str, state: RunState, vms: Vec<Vm>) -> Run {
        Run {
            id: id.to_string(),
            state,
            vcpus: 2,
            memory_mib: 512,
            beats: None,
            created_at: 1_000_000,
            ended_at: None,
            vms,
        }
    }

    fn fleet() -> Vec<Run> {
        vec![
            run(
                "r2",
                RunState::Killed,
                vec![vm(0, VmState::Killed, None), vm(1, VmState::Killed, None)],
            ),
            run(
                "r1",
                RunState::Running,
                vec![
                    vm(0, VmState::Running, Some(50)),
                    vm(1, VmState::Running, Some(51)),
                ],
            ),
        ]
    }

    #[test]
    fn lanes_follow_the_run_sequence_not_the_reply_order() {
        let plate = layout(&fleet(), &Activity::default(), 900.0, 500.0);
        assert_eq!(
            plate.lanes.iter().map(|lane| lane.run_id.as_str()).collect::<Vec<_>>(),
            vec!["r1", "r2"],
            "r1 is the older run even though the reply listed r2 first"
        );
        assert!(plate.lanes[0].y < plate.lanes[1].y);
        assert_eq!(plate.lanes[0].rule_y, SUBSTRATE_TOP + LANE_H);
    }

    #[test]
    fn cells_are_placed_by_index_within_their_lane() {
        let plate = layout(&fleet(), &Activity::default(), 900.0, 500.0);
        assert_eq!(plate.cells.len(), 4);
        let first_lane: Vec<&Cell> = plate.cells.iter().filter(|c| c.run_id == "r1").collect();
        assert_eq!(first_lane[0].index, 0);
        assert_eq!(first_lane[0].x, PLOT_LEFT + CELL_INSET);
        assert_eq!(first_lane[1].x, PLOT_LEFT + CELL_INSET + CELL_STEP);
        assert_eq!(first_lane[0].y, plate.lane_of("r1").expect("the lane").y);
        assert_ne!(
            first_lane[0].y,
            plate.cells.iter().find(|c| c.run_id == "r2").expect("r2").y
        );
    }

    #[test]
    fn a_cell_carries_the_vms_real_state_and_cid() {
        let plate = layout(&fleet(), &Activity::default(), 900.0, 500.0);
        let cell = plate.cells.iter().find(|c| c.run_id == "r1" && c.vm == 1).expect("r1/vm1");
        assert_eq!(cell.state, VmState::Running);
        assert_eq!(cell.guest_cid, Some(51));
        assert_eq!(cell.lps, 0.0, "nothing observed yet");

        let killed = plate.cells.iter().find(|c| c.run_id == "r2").expect("r2");
        assert_eq!(killed.state, VmState::Killed);
        assert_eq!(killed.guest_cid, None);
    }

    #[test]
    fn the_substrate_is_bounded_by_the_plate() {
        let plate = layout(&fleet(), &Activity::default(), 900.0, 500.0);
        assert_eq!(plate.substrate.x, 8.0);
        assert_eq!(plate.substrate.y, SUBSTRATE_TOP);
        assert!(plate.substrate.right() <= 900.0);
        assert!(plate.substrate.bottom() <= 500.0);

        // A plate too small to hold the substrate clamps instead of inverting.
        let tight = layout(&fleet(), &Activity::default(), 10.0, 10.0);
        assert_eq!(tight.substrate.width, 0.0);
        assert_eq!(tight.substrate.height, 0.0);
    }

    #[test]
    fn a_click_lands_on_the_cell_that_would_be_drawn_there() {
        let plate = layout(&fleet(), &Activity::default(), 900.0, 500.0);
        let target = plate.cells.iter().find(|c| c.run_id == "r1" && c.vm == 1).expect("r1/vm1");
        let hit = plate.cell_at(target.x + HIT_SLACK, target.y - HIT_SLACK).expect("a hit");
        assert_eq!((hit.run_id.as_str(), hit.vm), ("r1", 1));

        // The nearest cell's centre is its own; the gap between two cells is
        // not a hit at its middle when the cells are further apart than twice
        // the slack.
        assert!(plate.cell_at(target.x + HIT_SLACK + 1.0, target.y).is_none());
        assert!(plate.cell_at(0.0, 0.0).is_none());
    }

    #[test]
    fn an_empty_fleet_is_an_empty_plate() {
        let plate = layout(&[], &Activity::default(), 900.0, 500.0);
        assert!(plate.is_empty());
        assert!(plate.lanes.is_empty());
        assert!(plate.cell_at(0.0, 0.0).is_none());
        assert_eq!(plate.substrate.width, 900.0 - 16.0);
    }

    #[test]
    fn the_first_sighting_is_a_baseline_not_a_burst() {
        let mut activity = Activity::default();
        assert_eq!(activity.observe("r1", 0, 500, 1_000), 0.0);
        assert_eq!(activity.observe("r1", 0, 520, 3_000), 4.0, "20 lines in 2 s");
        // Smoothed, so the next equal delta moves it further.
        let next = activity.observe("r1", 0, 540, 5_000);
        assert!(next > 4.0 && next < 10.0, "lps {next}");
        assert_eq!(activity.lps("r1", 0), next);
    }

    #[test]
    fn a_quiet_vm_decays_to_zero() {
        let mut activity = Activity::default();
        activity.observe("r1", 0, 10, 1_000);
        activity.observe("r1", 0, 30, 3_000);
        let mut last = activity.lps("r1", 0);
        assert!(last > 0.0);
        // Each poll that sees no growth drags the smoothed rate down; the map
        // stops drawing the bar once it is under 0.02.
        for at in [5_000u64, 7_000, 9_000, 11_000, 13_000, 15_000, 17_000, 19_000, 21_000, 23_000, 25_000, 27_000] {
            let next = activity.observe("r1", 0, 30, at);
            assert!(next <= last, "the rate only falls while nothing is written");
            last = next;
        }
        assert!(last < 0.02, "a VM that stopped writing decays to 0, got {last}");
    }

    #[test]
    fn a_capture_that_shrank_is_not_negative_activity() {
        let mut activity = Activity::default();
        activity.observe("r1", 0, 100, 1_000);
        let lps = activity.observe("r1", 0, 40, 3_000);
        assert_eq!(lps, 0.0, "a shorter capture is a re-read, not negative growth");
    }

    #[test]
    fn observations_are_per_vm_and_survive_a_gap() {
        let mut activity = Activity::default();
        activity.observe("r1", 0, 0, 1_000);
        activity.observe("r1", 1, 0, 1_000);
        activity.observe("r1", 0, 20, 3_000);
        assert!(activity.lps("r1", 0) > 0.0);
        assert_eq!(activity.lps("r1", 1), 0.0, "vm 1 saw nothing");
        assert_eq!(activity.lps("r9", 0), 0.0, "an unseen run is quiet");
        assert_eq!(activity.len(), 2);
    }

    #[test]
    fn a_run_that_left_the_register_is_forgotten() {
        let mut activity = Activity::default();
        activity.observe("r1", 0, 0, 1_000);
        activity.observe("r2", 0, 0, 1_000);
        assert!(!activity.is_empty());

        // The register now holds r3: r1 and r2 are gone from it, so their
        // samples go too.
        activity.retain(&[run("r3", RunState::Running, vec![vm(0, VmState::Running, Some(60))])]);
        assert!(activity.is_empty(), "both runs left: nothing is watched");
        assert_eq!(activity.lps("r1", 0), 0.0);

        // A run still in the register keeps its rate.
        activity.observe("r3", 0, 0, 1_000);
        activity.observe("r3", 0, 20, 3_000);
        activity.retain(&[run("r3", RunState::Running, vec![vm(0, VmState::Running, Some(60))])]);
        assert!(activity.lps("r3", 0) > 0.0);
    }

    #[test]
    fn sequence_numbers_are_read_out_of_run_ids() {
        assert_eq!(seq_of("r1"), 1);
        assert_eq!(seq_of("r10"), 10);
        assert_eq!(seq_of("r9"), 9);
        assert_eq!(seq_of("x3"), 0);
        assert_eq!(seq_of("r"), 0);
        assert_eq!(seq_of("r1x"), 0);
        assert_eq!(seq_of(""), 0);
    }

    #[test]
    fn the_layout_carries_the_activity_it_was_given() {
        let mut activity = Activity::default();
        activity.observe("r1", 0, 0, 1_000);
        activity.observe("r1", 0, 12, 3_000);
        let plate = layout(&fleet(), &activity, 900.0, 500.0);
        let cell = plate.cells.iter().find(|c| c.run_id == "r1" && c.vm == 0).expect("r1/vm0");
        assert!(cell.lps > 0.0, "the pulse reaches the plate");
    }

    #[test]
    fn a_fifty_vm_run_lays_fifty_cells_and_the_plate_clips_the_tail() {
        // The scale preset's fleet, as the plate sees it: one lane with every
        // cell placed by index at the fixed step. The plate is a fixed scale,
        // so a lane this wide runs past its right edge and the canvas clips it;
        // the register and the run pane are where all fifty stay legible.
        let vms = (0..50)
            .map(|id| vm(id, VmState::Running, Some(10 + u64::from(id))))
            .collect();
        let plate = layout(&[run("r1", RunState::Running, vms)], &Activity::default(), 950.0, 700.0);

        assert_eq!(plate.lanes.len(), 1, "one run, one lane");
        assert_eq!(plate.cells.len(), 50);
        assert_eq!(plate.cells[0].x, PLOT_LEFT + CELL_INSET);
        assert_eq!(plate.cells[49].x, PLOT_LEFT + CELL_INSET + CELL_STEP * 49.0);
        assert_eq!(plate.cells[49].vm, 49, "the cells keep the VMs' own order");
        assert!(
            plate.cells[49].x > plate.substrate.right(),
            "the fiftieth cell is past the plate's edge: {} > {}",
            plate.cells[49].x,
            plate.substrate.right()
        );

        // What the plate can draw, and what it can only count: at this width
        // the fifteenth cell is the last that fits (166 + 54×14 + 7 = 929).
        assert_eq!(lane_overflow(&plate, 0), (15, 35));
        assert_eq!(plate.cells[14].x + MARK_R, 929.0);
        assert!(plate.cells[15].x + MARK_R > plate.substrate.right());

        // A lane that fits says nothing about clipping.
        let wide = layout(
            &[run("r1", RunState::Running, vec![vm(0, VmState::Running, None)])],
            &Activity::default(),
            900.0,
            500.0,
        );
        assert_eq!(lane_overflow(&wide, 0), (1, 0));
        assert_eq!(lane_overflow(&wide, 7), (0, 0), "no such lane, no cells");
    }
}
