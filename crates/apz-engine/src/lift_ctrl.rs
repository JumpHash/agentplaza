//! The lift controller: one cab between two stops (0 = the board's PCB top, 1 = the plate top), its doors and its
//! call queue. A port of `apps/web/src/views/floor3d/lift-ctrl.ts`. Pure logic: no wgpu, no nav grid. The design is
//! in `docs/design/navigation.md` ("Lifts").
//!
//! The robot side calls, for a robot id: `request(id, from_y, to_y)` once, when its path reaches the lift; `ticket(id)`;
//! `board(id)` while its ticket is `Boarding`; `alight(id)` while it is `Arrived`; `cancel(id)` when it has left;
//! `queue_index(id)` for its place in the line. A live request stays live until the robot alights or cancels. Nothing
//! here times a request out. Each frame reads `state()`: `cab_y` is the platform's surface height. The cab never moves
//! with its doors open, and the doors never open between stops.

/// The sub-step: the controller advances in steps of at most this (seconds), so a long frame cannot skip a phase.
const STEP: f32 = 1.0 / 60.0;
/// A frame's dt is clamped to this (seconds), as the nav world does.
const MAX_DT: f32 = 0.25;

/// The phase of the cab. Travel is the only phase in which the cab moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiftPhase {
    Idle,
    Opening,
    Unloading,
    Boarding,
    Closing,
    Travel,
}

/// A robot's ticket with the lift. `None` when the lift holds no request for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiftTicket {
    None,
    Queued,
    Boarding,
    Riding,
    Arrived,
}

/// What a frame reads from the controller.
#[derive(Clone, Debug, PartialEq)]
pub struct LiftState {
    pub cab_y: f32,
    pub doors_open: bool,
    /// The robots aboard (riding or arrived), in boarding order.
    pub boarding: Vec<u32>,
    /// Doors: 0 shut, 1 open.
    pub door_t: f32,
    pub phase: LiftPhase,
    /// The stop the cab is at, or the stop it travels to.
    pub stop: usize,
    /// Robots waiting at each stop (queued or admitted).
    pub calls: [usize; 2],
}

/// The tunables. `Default` gives the web's defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiftConfig {
    /// Riders the cab takes at once.
    pub capacity: usize,
    /// Average travel speed in units per second (the motion is smoothstep, so the peak is 1.5 times this).
    pub speed: f32,
    /// Seconds for the doors to open, and again to close.
    pub door_time: f32,
    /// The least time the doors stay open for boarding.
    pub dwell: f32,
    /// The most extra seconds the doors wait while an admitted robot is still walking in.
    pub hold_max: f32,
    /// Seconds riders need to leave the cab (the least), and the most before the rest are dropped.
    pub unload_time: f32,
    pub unload_max: f32,
    /// The cab opens at a stop this many times in a row while the head of the line waits at the other one.
    pub skip_max: usize,
}

impl Default for LiftConfig {
    fn default() -> Self {
        LiftConfig {
            capacity: 3,
            speed: 1.2,
            door_time: 0.5,
            dwell: 1.0,
            hold_max: 2.5,
            unload_time: 0.6,
            unload_max: 2.5,
            skip_max: 2,
        }
    }
}

/// The request-side state (the web's `queued` / `boarding` / `riding` / `arrived`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReqState {
    Queued,
    Boarding,
    Riding,
    Arrived,
}

#[derive(Clone, Copy, Debug)]
struct Req {
    id: u32,
    from: usize,
    to: usize,
    /// The arrival order: the queue is served by ascending seq. A robot that missed its door gets a new one (the back).
    seq: u64,
    state: ReqState,
}

/// The smoothstep of u in [0, 1]: the eased position of the travel (zero speed at both ends). Ports `smoothstep`.
pub fn smoothstep(u: f32) -> f32 {
    let k = u.clamp(0.0, 1.0);
    k * k * (3.0 - 2.0 * k)
}

#[derive(Clone, Debug)]
pub struct LiftController {
    pub low: f32,
    pub high: f32,
    cap: usize,
    speed: f32,
    door_time: f32,
    dwell: f32,
    hold_max: f32,
    unload_time: f32,
    unload_max: f32,
    skip_max: usize,
    /// Kept in insertion order, like the web's Map, so every walk over them is deterministic.
    reqs: Vec<Req>,
    seq: u64,
    stop: usize,
    phase: LiftPhase,
    /// Seconds spent in the current phase (travel: since its start).
    t: f32,
    /// Doors: 0 shut, 1 open.
    door: f32,
    cab_y: f32,
    travel_from: usize,
    travel_to: usize,
    travel_dur: f32,
    /// Openings here in a row while the head of the line waited at the other stop (see `decide`).
    skipped: usize,
}

impl LiftController {
    /// Ports the `LiftController` constructor. Errors when `high` is not above `low`.
    pub fn new(low: f32, high: f32, cfg: LiftConfig) -> Result<Self, &'static str> {
        if !(high > low) {
            return Err("lift: high must be above low");
        }
        Ok(LiftController {
            low,
            high,
            cap: cfg.capacity.max(1),
            speed: cfg.speed,
            door_time: cfg.door_time,
            dwell: cfg.dwell,
            hold_max: cfg.hold_max,
            unload_time: cfg.unload_time,
            unload_max: cfg.unload_max,
            skip_max: cfg.skip_max.max(1),
            reqs: Vec::new(),
            seq: 0,
            stop: 0,
            phase: LiftPhase::Idle,
            t: 0.0,
            door: 0.0,
            cab_y: low,
            travel_from: 0,
            travel_to: 0,
            travel_dur: 1.0,
            skipped: 0,
        })
    }

    /// The stop height of stop s. Ports `yOf`.
    fn y_of(&self, s: usize) -> f32 {
        if s == 0 {
            self.low
        } else {
            self.high
        }
    }

    /// The stop nearest to height y. Ports `stopOf`.
    pub fn stop_of(&self, y: f32) -> usize {
        if (y - self.low).abs() <= (y - self.high).abs() {
            0
        } else {
            1
        }
    }

    /// Robot `id` wants to ride from the stop at from_y to the stop at to_y. Returns false (and changes nothing) when
    /// the robot already has a request, or both heights are the same stop. Ports `request`.
    pub fn request(&mut self, id: u32, from_y: f32, to_y: f32) -> bool {
        if self.reqs.iter().any(|r| r.id == id) {
            return false;
        }
        let from = self.stop_of(from_y);
        let to = self.stop_of(to_y);
        if from == to {
            return false;
        }
        let seq = self.next_seq();
        self.reqs.push(Req { id, from, to, seq, state: ReqState::Queued });
        true
    }

    /// The robot's ticket. Ports `ticket`.
    pub fn ticket(&self, id: u32) -> LiftTicket {
        match self.find(id).map(|r| r.state) {
            None => LiftTicket::None,
            Some(ReqState::Queued) => LiftTicket::Queued,
            Some(ReqState::Boarding) => LiftTicket::Boarding,
            Some(ReqState::Riding) => LiftTicket::Riding,
            Some(ReqState::Arrived) => LiftTicket::Arrived,
        }
    }

    /// Robot `id` steps into the cab. True when it may (the ticket was `Boarding`): it is then `Riding`. Ports `board`.
    pub fn board(&mut self, id: u32) -> bool {
        if self.phase != LiftPhase::Boarding {
            return false;
        }
        match self.find_mut(id) {
            Some(r) if r.state == ReqState::Boarding => {
                r.state = ReqState::Riding;
                true
            }
            _ => false,
        }
    }

    /// Robot `id` steps out at its stop. True when its ticket was `Arrived`; the request ends. Ports `alight`.
    pub fn alight(&mut self, id: u32) -> bool {
        match self.find(id) {
            Some(r) if r.state == ReqState::Arrived => {
                self.reqs.retain(|r| r.id != id);
                true
            }
            _ => false,
        }
    }

    /// Robot `id` has left: its request (waiting, admitted or riding) is dropped. Unknown ids are a no-op. Ports `cancel`.
    pub fn cancel(&mut self, id: u32) {
        self.reqs.retain(|r| r.id != id);
    }

    /// The robot's place in the line at its stop: 0 is the front (the next to board). -1 when it is not waiting.
    /// Ports `queueIndex`.
    pub fn queue_index(&self, id: u32) -> i32 {
        let Some(r) = self.find(id) else { return -1 };
        if r.state != ReqState::Queued && r.state != ReqState::Boarding {
            return -1;
        }
        self.line(r.from).iter().position(|x| x.id == id).map_or(-1, |i| i as i32)
    }

    /// The robots aboard (riding or arrived), in boarding order. Ports `riders`.
    pub fn riders(&self) -> Vec<u32> {
        self.aboard().iter().map(|r| r.id).collect()
    }

    /// The frame's view of the cab. Ports `state`.
    pub fn state(&self) -> LiftState {
        LiftState {
            cab_y: self.cab_y,
            doors_open: self.door > 0.0,
            boarding: self.riders(),
            door_t: self.door,
            phase: self.phase,
            stop: if self.phase == LiftPhase::Travel { self.travel_to } else { self.stop },
            calls: [self.waiting(0), self.waiting(1)],
        }
    }

    /// True when no robot has a request and the cab is at rest with its doors shut. Ports `isIdle`.
    pub fn is_idle(&self) -> bool {
        self.reqs.is_empty() && self.phase == LiftPhase::Idle && self.door == 0.0
    }

    /// Advances the cab by dt seconds (clamped to [0, MAX_DT]; NaN, infinite and negative give 0). Ports `update`.
    pub fn update(&mut self, dt: f32) {
        let h = if dt.is_finite() { dt.clamp(0.0, MAX_DT) } else { 0.0 };
        let n = (h / STEP).ceil() as u32;
        for _ in 0..n {
            self.tick(h / n as f32);
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn find(&self, id: u32) -> Option<&Req> {
        self.reqs.iter().find(|r| r.id == id)
    }

    fn find_mut(&mut self, id: u32) -> Option<&mut Req> {
        self.reqs.iter_mut().find(|r| r.id == id)
    }

    fn waiting(&self, s: usize) -> usize {
        self.reqs
            .iter()
            .filter(|r| r.from == s && (r.state == ReqState::Queued || r.state == ReqState::Boarding))
            .count()
    }

    /// The line at a stop, front first (by arrival order): every live request there, waiting or admitted. Ports `line`.
    fn line(&self, s: usize) -> Vec<Req> {
        let mut v: Vec<Req> = self
            .reqs
            .iter()
            .filter(|r| r.from == s && (r.state == ReqState::Queued || r.state == ReqState::Boarding))
            .copied()
            .collect();
        v.sort_by_key(|r| r.seq);
        v
    }

    /// The waiting robots over all stops, in arrival order. Ports `queued`.
    fn queued(&self) -> Vec<Req> {
        let mut v: Vec<Req> = self.reqs.iter().filter(|r| r.state == ReqState::Queued).copied().collect();
        v.sort_by_key(|r| r.seq);
        v
    }

    /// The robots aboard, in boarding order. Ports `aboard`.
    fn aboard(&self) -> Vec<Req> {
        let mut v: Vec<Req> = self
            .reqs
            .iter()
            .filter(|r| r.state == ReqState::Riding || r.state == ReqState::Arrived)
            .copied()
            .collect();
        v.sort_by_key(|r| r.seq);
        v
    }

    /// One sub-step of the phase machine. Ports `tick`.
    fn tick(&mut self, h: f32) {
        match self.phase {
            LiftPhase::Idle => self.decide(),
            LiftPhase::Opening => {
                self.door = (self.door + h / self.door_time).min(1.0);
                if self.door >= 1.0 {
                    self.after_open();
                }
            }
            LiftPhase::Unloading => {
                self.t += h;
                let s = self.stop;
                let leaving: Vec<u32> = self.aboard().iter().filter(|r| r.to == s).map(|r| r.id).collect();
                if self.t >= self.unload_max {
                    // a robot that never stepped out is dropped (the view steps it off)
                    self.reqs.retain(|r| !leaving.contains(&r.id));
                    self.begin_boarding();
                } else if leaving.is_empty() && self.t >= self.unload_time {
                    self.begin_boarding();
                }
            }
            LiftPhase::Boarding => {
                self.t += h;
                let pending: Vec<u32> =
                    self.reqs.iter().filter(|r| r.state == ReqState::Boarding).map(|r| r.id).collect();
                if (self.t >= self.dwell && pending.is_empty()) || self.t >= self.dwell + self.hold_max {
                    for id in pending {
                        // missed the door: the back of the line
                        let seq = self.next_seq();
                        if let Some(r) = self.find_mut(id) {
                            r.state = ReqState::Queued;
                            r.seq = seq;
                        }
                    }
                    self.phase = LiftPhase::Closing;
                    self.t = 0.0;
                }
            }
            LiftPhase::Closing => {
                self.door = (self.door - h / self.door_time).max(0.0);
                if self.door <= 0.0 {
                    self.door = 0.0;
                    self.phase = LiftPhase::Idle;
                    self.t = 0.0;
                }
            }
            LiftPhase::Travel => {
                self.t += h;
                let u = (self.t / self.travel_dur).min(1.0);
                let y0 = self.y_of(self.travel_from);
                let y1 = self.y_of(self.travel_to);
                self.cab_y = if u >= 1.0 { y1 } else { y0 + (y1 - y0) * smoothstep(u) };
                if u >= 1.0 {
                    self.arrive();
                }
            }
        }
    }

    fn open(&mut self) {
        self.phase = LiftPhase::Opening;
        self.t = 0.0;
    }

    /// Idle with the doors shut: riders aboard go to their stop first (those bound here unload). Then the cab serves
    /// the robots waiting at its own stop, so it does not run empty to fetch the head of the line. The head at the
    /// other stop is fetched when nobody waits here, or after the cab has opened here skip_max times in a row (no
    /// starvation). Ports `decide`.
    fn decide(&mut self) {
        let s = self.stop;
        let aboard = self.aboard();
        if aboard.iter().any(|r| r.to == s) {
            self.open();
            return;
        }
        if let Some(first) = aboard.first() {
            self.start_travel(first.to);
            return;
        }
        let queued = self.queued();
        let Some(head) = queued.first().copied() else { return };
        if head.from == s {
            self.skipped = 0;
            self.open();
            return;
        }
        let here = queued.iter().any(|r| r.from == s);
        if here && self.skipped < self.skip_max {
            self.skipped += 1;
            self.open();
            return;
        }
        self.skipped = 0;
        self.start_travel(head.from);
    }

    /// Ports `afterOpen`: the doors are open. Riders bound here unload, the rest of the stop boards.
    fn after_open(&mut self) {
        self.door = 1.0;
        self.t = 0.0;
        let s = self.stop;
        let leaving: Vec<u32> = self.aboard().iter().filter(|r| r.to == s).map(|r| r.id).collect();
        if leaving.is_empty() {
            self.begin_boarding();
            return;
        }
        for id in leaving {
            // defensive: a rider at the stop is ready to leave
            if let Some(r) = self.find_mut(id) {
                if r.state == ReqState::Riding {
                    r.state = ReqState::Arrived;
                }
            }
        }
        self.phase = LiftPhase::Unloading;
    }

    /// Admits the front of this stop's line up to the free places (riders aboard are the ones still in the cab).
    /// Ports `beginBoarding`.
    fn begin_boarding(&mut self) {
        self.t = 0.0;
        let room = self.cap.saturating_sub(self.aboard().len());
        let admit: Vec<u32> = self
            .line(self.stop)
            .iter()
            .filter(|r| r.state == ReqState::Queued)
            .take(room)
            .map(|r| r.id)
            .collect();
        if admit.is_empty() {
            self.phase = LiftPhase::Closing;
            return;
        }
        for id in admit {
            if let Some(r) = self.find_mut(id) {
                r.state = ReqState::Boarding;
            }
        }
        self.phase = LiftPhase::Boarding;
    }

    /// Starts a trip to stop `to`. Refused when already there or while the doors are not shut. Ports `startTravel`.
    fn start_travel(&mut self, to: usize) {
        if to == self.stop || self.door > 0.0 {
            return;
        }
        self.travel_from = self.stop;
        self.travel_to = to;
        self.travel_dur = ((self.high - self.low).abs() / self.speed).max(1e-6);
        self.phase = LiftPhase::Travel;
        self.t = 0.0;
    }

    /// Ports `arrive`: the cab is at its stop; riders bound here are ready to leave.
    fn arrive(&mut self) {
        let s = self.travel_to;
        self.stop = s;
        self.cab_y = self.y_of(s);
        let bound: Vec<u32> = self.aboard().iter().filter(|r| r.to == s).map(|r| r.id).collect();
        for id in bound {
            if let Some(r) = self.find_mut(id) {
                r.state = ReqState::Arrived;
            }
        }
        self.phase = LiftPhase::Idle;
        self.t = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    /// A lift from 0 (low) to 4 (high), the web's defaults.
    fn lift() -> LiftController {
        LiftController::new(0.0, 4.0, LiftConfig::default()).expect("valid lift")
    }

    /// Steps the lift until `done` holds, for at most `secs` seconds. Returns whether it did.
    fn run_until(l: &mut LiftController, secs: f32, done: impl Fn(&LiftController) -> bool) -> bool {
        let steps = (secs / DT) as usize;
        for _ in 0..steps {
            if done(l) {
                return true;
            }
            l.update(DT);
        }
        done(l)
    }

    #[test]
    fn one_rider_goes_up() {
        let mut l = lift();
        assert!(l.request(1, 0.0, 4.0));
        assert_eq!(l.ticket(1), LiftTicket::Queued);
        assert!(run_until(&mut l, 5.0, |l| l.ticket(1) == LiftTicket::Boarding));
        assert!(l.board(1));
        assert_eq!(l.ticket(1), LiftTicket::Riding);
        assert_eq!(l.riders(), vec![1]);
        assert!(run_until(&mut l, 10.0, |l| l.ticket(1) == LiftTicket::Arrived));
        assert_eq!(l.state().stop, 1);
        assert_eq!(l.state().cab_y, 4.0);
        assert!(l.alight(1));
        assert!(run_until(&mut l, 5.0, |l| l.is_idle()));
    }

    #[test]
    fn capacity_three_leaves_the_fourth_for_the_next_trip() {
        let mut l = lift();
        for id in 1..=4 {
            assert!(l.request(id, 0.0, 4.0));
        }
        assert!(run_until(&mut l, 5.0, |l| l.ticket(1) == LiftTicket::Boarding));
        for id in 1..=3 {
            assert!(l.board(id));
        }
        assert_eq!(l.ticket(4), LiftTicket::Queued);
        assert_eq!(l.queue_index(4), 0);
        assert_eq!(l.state().calls[0], 1);
        assert!(!l.board(4));
        assert_eq!(l.riders(), vec![1, 2, 3]);

        assert!(run_until(&mut l, 10.0, |l| l.ticket(1) == LiftTicket::Arrived));
        assert_eq!(l.ticket(4), LiftTicket::Queued);
        for id in 1..=3 {
            assert!(l.alight(id));
        }
        assert!(run_until(&mut l, 10.0, |l| l.ticket(4) == LiftTicket::Boarding));
        assert!(l.board(4));
        assert_eq!(l.riders(), vec![4]);
    }

    #[test]
    fn boarder_who_never_boards_is_requeued_after_dwell_and_hold() {
        let mut l = lift();
        assert!(l.request(1, 0.0, 4.0));
        assert!(run_until(&mut l, 5.0, |l| l.ticket(1) == LiftTicket::Boarding));
        let mut secs = 0.0;
        while l.ticket(1) == LiftTicket::Boarding && secs < 10.0 {
            l.update(DT);
            secs += DT;
        }
        assert_eq!(l.ticket(1), LiftTicket::Queued);
        assert_eq!(l.state().phase, LiftPhase::Closing);
        // dwell (1.0) + hold_max (2.5), with a little slack for the frame steps
        assert!((secs - 3.5).abs() < 0.1, "requeued after {secs} s");
        assert_eq!(l.queue_index(1), 0);
    }

    #[test]
    fn travel_is_refused_while_the_doors_are_open() {
        let mut l = lift();
        assert!(l.request(1, 0.0, 4.0));
        assert!(run_until(&mut l, 5.0, |l| l.ticket(1) == LiftTicket::Boarding));
        assert!(l.state().doors_open);
        l.start_travel(1);
        assert_eq!(l.state().phase, LiftPhase::Boarding);
        assert_eq!(l.state().cab_y, 0.0);
    }

    #[test]
    fn nan_dt_is_ignored() {
        let mut l = lift();
        assert!(l.request(1, 0.0, 4.0));
        let before = l.state();
        l.update(f32::NAN);
        l.update(f32::INFINITY);
        l.update(-1.0);
        assert_eq!(l.state(), before);
        assert_eq!(l.ticket(1), LiftTicket::Queued);
    }
}
