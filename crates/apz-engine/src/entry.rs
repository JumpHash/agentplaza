//! The ENTER sequence as pure keyframes: the Machines view flies into a machine, its case opens, the camera dives
//! through the opening onto the machine's board, and the board's rooms build in. Back (or Esc) runs the same timeline
//! in reverse. This is the sequence on one time axis (seconds), with no scene or UI: the scene step samples
//! `stage_at` each frame and drives its nodes with the values.
//!
//! Ported from `apps/web/src/views/floor3d/entry.ts`. Parity: `testdata/entry_parity.json` (a node run of the web's
//! functions over a grid of inputs), checked by `entry_parity_with_the_web`.
//! - `clamp01`, `easeInOut`, `easeOutCubic`, `easeOutBack` -> `clamp01`, `ease_in_out`, `ease_out_cubic`, `ease_out_back`
//! - `span(t, a, b, ease)` -> `span(t, a, b, ease)`; the web's default ease (`easeInOut`) is passed explicitly here
//! - `ENTRY_SECONDS`, `KEY`, `WAVE`, `DIRECT_START`, `SINK`, `ROOM_DROP` -> the same names; the fields of `KEY` and
//!   `WAVE` in snake_case (`lightOut` -> `light_out`, `extFade` -> `ext_fade`, `boardFade` -> `board_fade`,
//!   `robotsBefore` -> `robots_before`, `robotsStagger` -> `robots_stagger`, `robotsBudget` -> `robots_budget`)
//! - `roomGap`, `robotGap`, `riseSpan`, `waveEnd`, `robotsAt`, `endAt` -> `room_gap`, `robot_gap`, `rise_span`,
//!   `wave_end`, `robots_at`, `end_at`; the room and robot counts are `usize`
//! - `stageAt(t, direct)` -> `stage_at(t, direct)`; the web's `Stage` interface -> `Stage`
//! - `robotsPopped(t, rooms, robots)` -> `robots_popped(t, rooms, robots)`, the count as `usize`
//!
//! Not ported (scene, DOM or handoff state, not pure): the `Pose`, `Exterior`, `BoardPart`, `EntryContext` and
//! `Handoff` types, `beginEntry`, `takeEntry`, `setReturn`, `takeReturn`, `fadeTree`, and the `EntrySequence` class
//! that drives the scene. The class's per-room lift (`span(t, a, b, easeOutCubic)` times `ROOM_DROP`) and per-robot
//! scale (`span(..., easeOutBack)`) are the scene step's and use the functions here.

/// Clamps to 0..=1.
pub fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

/// The cubic ease-in-out: slow at both ends.
pub fn ease_in_out(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// The cubic ease-out: fast start, slow end.
pub fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// The back ease-out: overshoots 1 a little, then settles.
pub fn ease_out_back(t: f64) -> f64 {
    let c1 = 1.4;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// The progress of `t` through `[a, b]`: 0 before `a`, 1 after `b`, `ease`d in between. A window that is empty or
/// reversed (`b <= a`) is a step at `b`.
pub fn span(t: f64, a: f64, b: f64, ease: fn(f64) -> f64) -> f64 {
    if b <= a {
        return if t >= b { 1.0 } else { 0.0 };
    }
    ease(clamp01((t - a) / (b - a)))
}

/// The sequence's length, seconds: the fly-in, the open, the dive and the first of the build wave.
pub const ENTRY_SECONDS: f64 = 2.4;

/// The keyframes: `[start, end]` in seconds on the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keys {
    /// The other machines fold down, their cables fade.
    pub dim: [f64; 2],
    /// The camera flies to the exterior's anchors.camera.
    pub fly: [f64; 2],
    /// The exterior's open part slides or swings (0 to 1).
    pub open: [f64; 2],
    /// The soft light spills out of the opening.
    pub light: [f64; 2],
    pub light_out: [f64; 2],
    /// The camera dives through the opening and settles at the board's default view.
    pub dive: [f64; 2],
    /// The exterior fades out.
    pub ext_fade: [f64; 2],
    /// The board fades in under it (a match cut).
    pub board_fade: [f64; 2],
    /// The atom glows up.
    pub atom: [f64; 2],
    /// A direct load: the board fades in over this range (it is shown from the wave on).
    pub direct: [f64; 2],
}

pub const KEY: Keys = Keys {
    dim: [0.0, 0.4],
    fly: [0.15, 1.1],
    open: [0.95, 1.5],
    light: [1.0, 1.4],
    light_out: [1.6, 2.0],
    dive: [1.45, 2.15],
    ext_fade: [1.45, 1.95],
    board_fade: [1.45, 1.85],
    atom: [1.7, 2.1],
    direct: [1.2, 1.6],
};

/// The build wave: each room rises from its slot, staggered, and the robots pop in after the last room.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wave {
    /// When the first room starts to rise, seconds.
    pub start: f64,
    /// How long a room's rise takes, seconds.
    pub rise: f64,
    /// The gap between the rooms' rises, seconds: 40 ms, compressed for a board with many rooms.
    pub stagger: f64,
    /// The rooms' starts all fall within this many seconds.
    pub budget: f64,
    /// The robots pop in this long before the last room's rise finishes.
    pub robots_before: f64,
    /// The gap between the robots' pops, seconds.
    pub robots_stagger: f64,
    /// The robots' pops all fall within this many seconds.
    pub robots_budget: f64,
    /// How long a robot's pop takes, seconds.
    pub pop: f64,
}

pub const WAVE: Wave = Wave {
    start: 1.7,
    rise: 0.25,
    stagger: 0.04,
    budget: 0.5,
    robots_before: 0.15,
    robots_stagger: 0.02,
    robots_budget: 0.25,
    pop: 0.3,
};

/// The direct load's board starts shown, at the wave's start.
pub const DIRECT_START: f64 = WAVE.start;
/// How far the other machines sink when the sequence starts, in units.
pub const SINK: f64 = 0.6;
/// How far a room sits below its slot before it rises, in units.
pub const ROOM_DROP: f64 = 0.8;

/// The gap between consecutive rooms' rises, seconds: `WAVE.stagger`, or less when the rooms would overrun the budget.
pub fn room_gap(rooms: usize) -> f64 {
    if rooms <= 1 {
        0.0
    } else {
        WAVE.stagger.min(WAVE.budget / (rooms - 1) as f64)
    }
}

/// The gap between consecutive robots' pops, seconds.
pub fn robot_gap(robots: usize) -> f64 {
    if robots <= 1 {
        0.0
    } else {
        WAVE.robots_stagger
            .min(WAVE.robots_budget / (robots - 1) as f64)
    }
}

/// When room `i`'s rise starts and ends, seconds.
pub fn rise_span(i: usize, rooms: usize) -> (f64, f64) {
    let a = WAVE.start + i as f64 * room_gap(rooms);
    (a, a + WAVE.rise)
}

/// When the last room's rise ends, seconds.
pub fn wave_end(rooms: usize) -> f64 {
    WAVE.start + rooms.saturating_sub(1) as f64 * room_gap(rooms) + WAVE.rise
}

/// When the first robot pops in, seconds: as the last room's rise finishes.
pub fn robots_at(rooms: usize) -> f64 {
    wave_end(rooms) - WAVE.robots_before
}

/// The timeline's end, seconds: at least `ENTRY_SECONDS`, after the last room rises and after the last robot has
/// popped in.
pub fn end_at(rooms: usize, robots: usize) -> f64 {
    let last_robot =
        robots_at(rooms) + robots.saturating_sub(1) as f64 * robot_gap(robots) + WAVE.pop + 0.02;
    ENTRY_SECONDS.max(wave_end(rooms) + 0.1).max(last_robot)
}

/// The sequence's state at one time. Every value is 0..1 (`dive` is 1 at its own view).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stage {
    /// The other machines fold down (0 at rest, 1 folded).
    pub dim: f64,
    /// The camera's fly-in to the exterior's anchors.camera.
    pub fly: f64,
    /// The exterior's open part (anchors.open).
    pub open: f64,
    /// The light spilling out of the opening.
    pub light: f64,
    /// The camera's dive, and the board's move from the exterior's place to its own (0 matched, 1 its own).
    pub dive: f64,
    /// The exterior's opacity.
    pub ext: f64,
    /// The other machines' opacity.
    pub others: f64,
    /// The other machines' cables' opacity.
    pub cables: f64,
    /// The board's opacity.
    pub board: f64,
    /// The board's atom's opacity.
    pub atom: f64,
}

/// The sequence at time `t` (seconds). A direct load (no exterior) plays the board's fade and the build wave only.
pub fn stage_at(t: f64, direct: bool) -> Stage {
    let at = |k: [f64; 2]| span(t, k[0], k[1], ease_in_out);
    let atom = span(t, KEY.atom[0], KEY.atom[1], ease_out_cubic);
    if direct {
        return Stage {
            dim: 0.0,
            fly: 0.0,
            open: 0.0,
            light: 0.0,
            dive: 1.0,
            ext: 0.0,
            others: 0.0,
            cables: 0.0,
            board: at(KEY.direct),
            atom,
        };
    }
    let dim = at(KEY.dim);
    let ext_fade = at(KEY.ext_fade);
    let light = at(KEY.light) * (1.0 - at(KEY.light_out));
    Stage {
        dim,
        fly: at(KEY.fly),
        open: at(KEY.open),
        light,
        dive: at(KEY.dive),
        ext: 1.0 - ext_fade,
        others: (1.0 - 0.65 * dim) * (1.0 - ext_fade),
        cables: (1.0 - 0.8 * dim) * (1.0 - ext_fade),
        board: at(KEY.board_fade),
        atom,
    }
}

/// How many robots have popped in (scale above zero) at time `t`.
pub fn robots_popped(t: f64, rooms: usize, robots: usize) -> usize {
    let rg = robot_gap(robots);
    let r0 = robots_at(rooms);
    (0..robots)
        .filter(|&j| {
            let a = r0 + j as f64 * rg;
            span(t, a, a + WAVE.pop, ease_out_back) > 0.002
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn rows<'a>(v: &'a Value, key: &str) -> &'a [Value] {
        v[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} is an array"))
    }

    fn ease_named(name: &str) -> fn(f64) -> f64 {
        match name {
            "easeInOut" => ease_in_out,
            "easeOutCubic" => ease_out_cubic,
            "easeOutBack" => ease_out_back,
            other => panic!("unknown ease {other} in the dump"),
        }
    }

    /// Every value the web's functions return, dumped by a node run of entry.ts (testdata/entry_parity.json), matches
    /// the Rust port within 1e-6 (relative above 1, absolute below). Every mismatch is listed, and the test fails at the
    /// end if there is any.
    #[test]
    fn entry_parity_with_the_web() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/testdata/entry_parity.json"
        );
        let text = std::fs::read_to_string(path).expect("testdata/entry_parity.json reads");
        let v: Value = serde_json::from_str(&text).expect("entry_parity.json parses");
        let num = |x: &Value| x.as_f64().expect("a number");
        let count = |x: &Value| x.as_u64().expect("a count") as usize;

        let mut bad: Vec<String> = Vec::new();
        let mut n = 0usize;
        let mut check = |what: String, got: f64, want: f64| {
            n += 1;
            if !((got - want).abs() <= 1e-6 * want.abs().max(1.0)) {
                bad.push(format!("{what}: rust {got}, web {want}"));
            }
        };

        // Constants: the web's values, field by field.
        let c = &v["constants"];
        check(
            "ENTRY_SECONDS".into(),
            ENTRY_SECONDS,
            num(&c["ENTRY_SECONDS"]),
        );
        check("DIRECT_START".into(), DIRECT_START, num(&c["DIRECT_START"]));
        check("SINK".into(), SINK, num(&c["SINK"]));
        check("ROOM_DROP".into(), ROOM_DROP, num(&c["ROOM_DROP"]));
        let keys = [
            ("dim", KEY.dim),
            ("fly", KEY.fly),
            ("open", KEY.open),
            ("light", KEY.light),
            ("lightOut", KEY.light_out),
            ("dive", KEY.dive),
            ("extFade", KEY.ext_fade),
            ("boardFade", KEY.board_fade),
            ("atom", KEY.atom),
            ("direct", KEY.direct),
        ];
        for (name, [a, b]) in keys {
            check(format!("KEY.{name}[0]"), a, num(&c["KEY"][name][0]));
            check(format!("KEY.{name}[1]"), b, num(&c["KEY"][name][1]));
        }
        let waves = [
            ("start", WAVE.start),
            ("rise", WAVE.rise),
            ("stagger", WAVE.stagger),
            ("budget", WAVE.budget),
            ("robotsBefore", WAVE.robots_before),
            ("robotsStagger", WAVE.robots_stagger),
            ("robotsBudget", WAVE.robots_budget),
            ("pop", WAVE.pop),
        ];
        for (name, got) in waves {
            check(format!("WAVE.{name}"), got, num(&c["WAVE"][name]));
        }

        for r in rows(&v, "clamp01") {
            let t = num(&r["t"]);
            check(format!("clamp01({t})"), clamp01(t), num(&r["out"]));
        }
        for r in rows(&v, "easeInOut") {
            let t = num(&r["t"]);
            check(format!("easeInOut({t})"), ease_in_out(t), num(&r["out"]));
        }
        for r in rows(&v, "easeOutCubic") {
            let t = num(&r["t"]);
            check(
                format!("easeOutCubic({t})"),
                ease_out_cubic(t),
                num(&r["out"]),
            );
        }
        for r in rows(&v, "easeOutBack") {
            let t = num(&r["t"]);
            check(
                format!("easeOutBack({t})"),
                ease_out_back(t),
                num(&r["out"]),
            );
        }
        for r in rows(&v, "span") {
            let (t, a, b) = (num(&r["t"]), num(&r["a"]), num(&r["b"]));
            let ease = r["ease"].as_str().expect("an ease name");
            check(
                format!("span({t}, {a}, {b}, {ease})"),
                span(t, a, b, ease_named(ease)),
                num(&r["out"]),
            );
        }
        for r in rows(&v, "roomGap") {
            let rooms = count(&r["rooms"]);
            check(format!("roomGap({rooms})"), room_gap(rooms), num(&r["out"]));
        }
        for r in rows(&v, "robotGap") {
            let robots = count(&r["robots"]);
            check(
                format!("robotGap({robots})"),
                robot_gap(robots),
                num(&r["out"]),
            );
        }
        for r in rows(&v, "riseSpan") {
            let (i, rooms) = (count(&r["i"]), count(&r["rooms"]));
            let (a, b) = rise_span(i, rooms);
            check(format!("riseSpan({i}, {rooms}).a"), a, num(&r["a"]));
            check(format!("riseSpan({i}, {rooms}).b"), b, num(&r["b"]));
        }
        for r in rows(&v, "waveEnd") {
            let rooms = count(&r["rooms"]);
            check(format!("waveEnd({rooms})"), wave_end(rooms), num(&r["out"]));
        }
        for r in rows(&v, "robotsAt") {
            let rooms = count(&r["rooms"]);
            check(
                format!("robotsAt({rooms})"),
                robots_at(rooms),
                num(&r["out"]),
            );
        }
        for r in rows(&v, "endAt") {
            let (rooms, robots) = (count(&r["rooms"]), count(&r["robots"]));
            check(
                format!("endAt({rooms}, {robots})"),
                end_at(rooms, robots),
                num(&r["out"]),
            );
        }
        for r in rows(&v, "robotsPopped") {
            let (t, rooms, robots) = (num(&r["t"]), count(&r["rooms"]), count(&r["robots"]));
            check(
                format!("robotsPopped({t}, {rooms}, {robots})"),
                robots_popped(t, rooms, robots) as f64,
                num(&r["out"]),
            );
        }
        for r in rows(&v, "stageAt") {
            let t = num(&r["t"]);
            let direct = r["direct"].as_bool().expect("a direct flag");
            let s = stage_at(t, direct);
            let got = [
                ("dim", s.dim),
                ("fly", s.fly),
                ("open", s.open),
                ("light", s.light),
                ("dive", s.dive),
                ("ext", s.ext),
                ("others", s.others),
                ("cables", s.cables),
                ("board", s.board),
                ("atom", s.atom),
            ];
            for (field, value) in got {
                check(
                    format!("stageAt({t}, {direct}).{field}"),
                    value,
                    num(&r[field]),
                );
            }
        }

        assert!(
            bad.is_empty(),
            "{} of {n} values differ from the web:\n{}",
            bad.len(),
            bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
        );
    }
}
