//! Channels (PLAN 22.3): a release's channel sets how deep its qualification goes (22.10) and which store tracks it
//! follows. `nightly` takes the short pass, `beta` most stages and `stable` all of them. A channel the project defines
//! takes all of them and follows no store track until one is mapped.

use serde::Serialize;

/// How deep a channel's qualification goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Depth {
    Short,
    Most,
    Full,
}

/// The store tracks a channel publishes to: TestFlight's testing group and the Play Store's track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tracks {
    pub testflight: &'static str,
    pub play: &'static str,
}

/// One channel: its name, its qualification depth and the store tracks it follows, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Channel {
    pub name: String,
    pub depth: Depth,
    pub tracks: Option<Tracks>,
}

/// The channel called `name`. A name is lowercase and has no spaces; the built-in channels have their depth and tracks.
pub fn of(name: &str) -> Result<Channel, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_uppercase()) {
        return Err(format!("{name:?} is not a channel: lowercase, no spaces"));
    }
    let (depth, tracks) = match name {
        "nightly" => (
            Depth::Short,
            Some(Tracks {
                testflight: "internal",
                play: "internal",
            }),
        ),
        "beta" => (
            Depth::Most,
            Some(Tracks {
                testflight: "external",
                play: "closed",
            }),
        ),
        "stable" => (
            Depth::Full,
            Some(Tracks {
                testflight: "app-store",
                play: "production",
            }),
        ),
        _ => (Depth::Full, None),
    };
    Ok(Channel {
        name: name.to_string(),
        depth,
        tracks,
    })
}

/// The built-in channels: `nightly`, `beta` and `stable`.
pub fn builtin() -> Vec<Channel> {
    ["nightly", "beta", "stable"]
        .iter()
        .filter_map(|name| of(name).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_sets_its_depth_and_follows_its_store_tracks() {
        let nightly = of("nightly").unwrap();
        assert_eq!(nightly.depth, Depth::Short);
        assert_eq!(nightly.tracks.as_ref().unwrap().testflight, "internal");
        assert_eq!(of("beta").unwrap().depth, Depth::Most);
        let stable = of("stable").unwrap();
        assert_eq!(stable.depth, Depth::Full);
        assert_eq!(stable.tracks.unwrap().play, "production");
    }

    #[test]
    fn a_channel_the_project_defines_runs_every_stage_and_follows_no_track_until_mapped() {
        let own = of("lts").unwrap();
        assert_eq!(own.depth, Depth::Full);
        assert_eq!(own.tracks, None);
        assert_eq!(builtin().len(), 3);
    }

    #[test]
    fn a_channel_name_is_lowercase_without_spaces() {
        assert!(of("").unwrap_err().contains("not a channel"));
        assert!(of("Nightly").unwrap_err().contains("not a channel"));
        assert!(of("early access").unwrap_err().contains("not a channel"));
        assert_eq!(of("  beta ").unwrap().name, "beta");
    }
}
