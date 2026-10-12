//! `floor.ts` lines ~123–700: district layout by role, the world's base (sky, ground, clouds,
//! plaza, grass, dust), district slabs, rooms and plaques, robots in and out, and the view's fit.

use std::collections::HashMap;

use super::{AgentInfo, Colors, District, Floor, Shared};
use crate::console::floor3d::geometry;
use crate::console::floor3d::scene::Mat;

/// `layoutDistricts(list)`: districts by role around the central dais, and each agent's home:
/// cid → (district index, x, y).
pub fn layout_districts(list: &[AgentInfo]) -> (Vec<District>, HashMap<u64, (usize, f32, f32)>) {
    let _ = list;
    (Vec::new(), HashMap::new()) // contract: W1
}

/// `agentHue(cid)` and `roleHue(role)`: degrees.
pub fn agent_hue(cid: u64) -> f32 { let _ = cid; 0.0 } // contract: W1
pub fn role_hue(role: &str) -> f32 { let _ = role; 0.0 } // contract: W1

/// The shared pad, packet, halo and token geometry and materials (floor.ts mount).
pub(super) fn shared(colors: &Colors) -> Shared {
    let _ = colors;
    Shared {
        pad_geo: geometry::circle(0.42, 28, 0.0, std::f32::consts::TAU),
        packet_geo: geometry::sphere(0.13, 14, 10),
        halo_geo: geometry::sphere(0.34, 14, 10),
        token_geo: geometry::box_geo(0.2, 0.2, 0.2),
        accent: Mat::basic(colors.signal),
        ok: Mat::basic(colors.ok),
        accent_halo: Mat::basic(colors.signal),
        ok_halo: Mat::basic(colors.ok),
        token: Mat::basic(colors.signal),
    } // contract: W1 (halo materials: transparent, additive, opacity 0.35, no depth write)
}

/// Sky, fog, lights (`floor.world`), ground, clouds, plaza, grass and dust, added to `floor.root`.
pub(super) fn base(floor: &mut Floor) {
    let _ = floor; // contract: W1
}

impl Floor {
    /// Keeps the floor in step with the agents: rebuilds the districts when the roles change
    /// (`districtSig`), adds and removes robots, updates names, states and labels' data.
    pub fn sync(&mut self, list: &[AgentInfo]) {
        let _ = list; // contract: W1
    }

    /// `resetView()`: the isometric-ish view that fits every district.
    pub fn reset_view(&mut self) {
        // contract: W1
    }
}
