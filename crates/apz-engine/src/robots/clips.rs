//! Clip evaluation, as `robots/clips.ts`. A channel's value at time t is `base + amp × wave(2π·freq·t +
//! phase)`, with "gait.<name>" resolved from the species' gait. Both apps read the clips the same way.
//! The phase is computed in f64, so a long uptime keeps the clips where the web has them.

use std::f64::consts::{PI, TAU};

use super::types::{Channel, Clip, Gait, Param};

/// The wave at a cycle position `c` (in cycles). `duty` is for square and pulse.
pub fn wave_at(wave: &str, c: f64, duty: f64) -> f64 {
    let f = c - c.floor();
    match wave {
        "abs" => (TAU * c).sin().abs(),
        "halfsin" => (TAU * c).sin().max(0.0),
        "tri" => 1.0 - 4.0 * (f - 0.5).abs(),
        "saw" => 2.0 * f - 1.0,
        "square" => {
            if f < duty {
                1.0
            } else {
                -1.0
            }
        }
        "pulse" => {
            if f < duty {
                1.0
            } else {
                0.0
            }
        }
        "hold" => 0.0,
        _ => (TAU * c).sin(),
    }
}

/// A number field, or the species' gait value for "gait.<name>"; the fallback when neither.
pub fn resolve(v: Option<&Param>, gait: &Gait, fallback: f64) -> f64 {
    match v {
        None => fallback,
        Some(Param::Num(n)) => f64::from(*n),
        Some(Param::Gait(name)) => name.strip_prefix("gait.").and_then(|k| gait.get(k)).map_or(fallback, f64::from),
    }
}

/// A channel's value at the global time `t` (seconds), and `elapsed` into a one-shot of length `dur`
/// (for the `in-out` envelope). Clips are periodic in the global time, so a blink lands where it always did.
pub fn channel_value(ch: &Channel, t: f64, gait: &Gait, elapsed: f64, dur: f64) -> f64 {
    let base = resolve(ch.base.as_ref(), gait, 0.0);
    let amp = resolve(ch.amp.as_ref(), gait, 0.0);
    let freq = resolve(ch.freq.as_ref(), gait, 0.0);
    let env = if ch.env.as_deref() == Some("in-out") && dur > 0.0 { (PI * (elapsed / dur).clamp(0.0, 1.0)).sin() } else { 1.0 };
    let phase = f64::from(ch.phase.unwrap_or(0.0)) / TAU;
    let w = wave_at(ch.wave.as_deref().unwrap_or("sin"), freq * t + phase, f64::from(ch.duty.unwrap_or(0.5)));
    base + amp * env * w
}

/// Calls `f` with each channel of the clip and its value at `t`. Rate channels are skipped: they
/// accumulate (see robot.rs), they are not blended to a target.
pub fn eval_clip(clip: &Clip, t: f64, gait: &Gait, elapsed: f64, mut f: impl FnMut(&Channel, f64)) {
    let dur = f64::from(clip.dur.unwrap_or(0.0));
    for ch in &clip.channels {
        if ch.rate.is_none() {
            f(ch, channel_value(ch, t, gait, elapsed, dur));
        }
    }
}

/// The rest value of a prop: rotations and offsets at 0, a scale and the eye lids at 1.
pub fn rest_of(prop: &str) -> f32 {
    if prop == "scale" || prop == "lid" {
        1.0
    } else {
        0.0
    }
}
