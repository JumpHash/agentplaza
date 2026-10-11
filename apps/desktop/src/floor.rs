//! The plaza floor's geometry — pure, iced-free.
//!
//! The floor is a 3D scene drawn by perspective projection: a ground plane
//! with a grid, a station pad per role, a robot standing on each agent's
//! station, and a status ring on the ground under it. Everything here is
//! computed from real daemon data — the roster's role order and a
//! conversation's activity — so the layout, the projection and the walk are
//! unit-tested without a display; the iced canvas in [`floorview`] only paints
//! what this module returns.
//!
//! # The camera
//!
//! A fixed RTS camera: a look-at at the plaza's origin, a **tilt of 55°** down
//! from the horizontal and a **yaw of 20°** about the vertical, at a distance
//! of [`CAM_DIST`]. The projection is an exact perspective divide: a world
//! point is taken into the camera's basis, and its screen position is
//! `centre + focal · (right, up) / depth`, with `focal = 0.85 · height` in
//! pixels and a near plane of [`NEAR`]. A point at or behind the near plane is
//! not drawn (`None`).
//!
//! # The layout rule (deterministic, no server geometry)
//!
//! Roles are taken in the order the register first shows them (the roster's
//! own order). The role at index `i` gets a station pad at grid cell
//! `(col, row) = (i % PADS_PER_ROW, i / PADS_PER_ROW)`, whose world position
//! is `x = (col − 2) · PAD_STEP`, `z = (row − ROW_CENTER) · PAD_STEP`. Agents
//! sharing a role stand on their pad, spread along `x` by their index within
//! the role. When a pair's conversation is active the two robots walk to a
//! spot straddling the midpoint between their stations, on an eased,
//! **bounded** clock ([`WALK_MS`]).

/// The camera's tilt below the horizontal, in degrees.
pub const TILT_DEG: f32 = 55.0;
/// The camera's yaw about the vertical, in degrees.
pub const YAW_DEG: f32 = 20.0;
/// How far the camera sits from the plaza's origin, in world units.
pub const CAM_DIST: f32 = 10.0;
/// The near plane: a point nearer than this is behind the camera and not drawn.
pub const NEAR: f32 = 0.2;
/// The focal length as a fraction of the viewport height, in pixels.
pub const FOCAL_OF_HEIGHT: f32 = 0.85;

/// Half the ground grid's extent, in world units.
pub const GRID_HALF: f32 = 4.0;
/// The spacing of the ground grid's lines.
pub const GRID_STEP: f32 = 1.0;

/// How many station pads sit on one row of the plaza.
pub const PADS_PER_ROW: usize = 5;
/// How far apart the pads sit, in world units.
pub const PAD_STEP: f32 = 1.5;
/// The row the plaza's pads straddle: row 0 sits [`ROW_CENTER`] rows above the
/// origin, so a small fleet's pads centre on the plaza.
pub const ROW_CENTER: f32 = 1.5;
/// A pad's half-extent on the ground.
pub const PAD_HALF: f32 = 0.6;

/// How far apart two agents sharing a role's pad stand, in world units.
pub const ROBOT_SPREAD: f32 = 0.5;
/// A robot body's half-width.
pub const ROBOT_HALF: f32 = 0.22;
/// A robot body's height.
pub const ROBOT_H: f32 = 0.5;
/// A status ring's radius on the ground.
pub const RING_R: f32 = 0.44;
/// How many segments a projection of the ring (a projected ellipse) is drawn
/// from.
pub const RING_SEGS: usize = 24;
/// Half the distance between a talking pair's two adjacent spots.
pub const WALK_GAP: f32 = 0.34;

/// How long a walk onto a conversation's spot (or back) takes, in
/// milliseconds. The walk is **bounded**: iced has no reduced-motion signal, so
/// the animation is short and always completes; a frame never waits on it.
pub const WALK_MS: u64 = 700;

/// The serial rate at which a robot's status ring says it is working, in
/// lines per second — the console's own telemetry ([`crate::map`]'s pulse), so
/// "working" means the serial is actually moving.
pub const WORKING_LPS: f32 = 0.05;

/// A point in the floor's world: `x` across, `y` up, `z` toward the near edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    /// Across the plaza.
    pub x: f32,
    /// Above the ground.
    pub y: f32,
    /// Toward the plaza's near edge (the camera's side).
    pub z: f32,
}

/// The fixed RTS camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// The tilt below the horizontal, in degrees.
    pub tilt_deg: f32,
    /// The yaw about the vertical, in degrees.
    pub yaw_deg: f32,
    /// The distance from the look-at (the plaza's origin).
    pub dist: f32,
}

impl Camera {
    /// The register's one camera.
    pub fn rts() -> Self {
        Self {
            tilt_deg: TILT_DEG,
            yaw_deg: YAW_DEG,
            dist: CAM_DIST,
        }
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::rts()
    }
}

/// A projected point: its screen position and its depth along the view
/// direction (`depth` grows with distance; the far plane is unreal — the floor
/// is bounded, so nothing here is clipped by distance).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projected {
    /// The screen `x`, in pixels.
    pub x: f32,
    /// The screen `y`, in pixels.
    pub y: f32,
    /// The distance in front of the camera, in world units. Smaller is nearer;
    /// the painter's order draws the largest first.
    pub depth: f32,
}

impl Projected {
    /// The point's screen position, as a plain pair.
    pub fn point(&self) -> (f32, f32) {
        (self.x, self.y)
    }
}

/// Projects `p` through `camera` into a `width` by `height` viewport, or
/// `None` when the point is at or behind the near plane.
pub fn project(p: Vec3, camera: &Camera, width: f32, height: f32) -> Option<Projected> {
    let tilt = camera.tilt_deg.to_radians();
    let yaw = camera.yaw_deg.to_radians();
    // The direction the camera sits in, from the look-at (the origin).
    let dir = Vec3 {
        x: tilt.cos() * yaw.sin(),
        y: tilt.sin(),
        z: tilt.cos() * yaw.cos(),
    };
    let eye = Vec3 {
        x: camera.dist * dir.x,
        y: camera.dist * dir.y,
        z: camera.dist * dir.z,
    };
    // The camera's basis: forward to the origin, right across the view, up in
    // the view's own vertical.
    let forward = Vec3 {
        x: -dir.x,
        y: -dir.y,
        z: -dir.z,
    };
    let right = normalize(cross(forward, Vec3 { x: 0.0, y: 1.0, z: 0.0 }));
    let up = cross(right, forward);
    let v = Vec3 {
        x: p.x - eye.x,
        y: p.y - eye.y,
        z: p.z - eye.z,
    };
    let depth = dot(v, forward);
    if depth < NEAR {
        return None;
    }
    let focal = height * FOCAL_OF_HEIGHT;
    Some(Projected {
        x: width * 0.5 + focal * dot(v, right) / depth,
        y: height * 0.5 - focal * dot(v, up) / depth,
        depth,
    })
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3 {
        x: a.y * b.z - a.z * b.y,
        y: a.z * b.x - a.x * b.z,
        z: a.x * b.y - a.y * b.x,
    }
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn normalize(a: Vec3) -> Vec3 {
    let length = dot(a, a).sqrt();
    if length <= f32::EPSILON {
        return a;
    }
    Vec3 {
        x: a.x / length,
        y: a.y / length,
        z: a.z / length,
    }
}

/// A role's station pad: where the agents of one role stand.
#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    /// The role the pad is filed under.
    pub role: String,
    /// The role's place in the register's order.
    pub index: usize,
    /// The pad's grid column.
    pub col: usize,
    /// The pad's grid row.
    pub row: usize,
    /// The pad's world `x`.
    pub x: f32,
    /// The pad's world `z`.
    pub z: f32,
}

/// Lays the roles out on station pads, in the order given (the register's own
/// order). The rule is stated on the module: `(i % PADS_PER_ROW, i /
/// PADS_PER_ROW)`, fixed positions, no server geometry.
pub fn pads(roles: &[String]) -> Vec<Pad> {
    roles
        .iter()
        .enumerate()
        .map(|(index, role)| {
            let col = index % PADS_PER_ROW;
            let row = index / PADS_PER_ROW;
            Pad {
                role: role.clone(),
                index,
                col,
                row,
                x: (col as f32 - (PADS_PER_ROW as f32 - 1.0) / 2.0) * PAD_STEP,
                z: (row as f32 - ROW_CENTER) * PAD_STEP,
            }
        })
        .collect()
}

/// The station a role's `k`-th of `n` agents stands on: the pad's centre,
/// spread along `x` so a role's agents do not overlap.
pub fn station(pad: &Pad, k: usize, n: usize) -> (f32, f32) {
    let offset = (k as f32 - (n as f32 - 1.0) / 2.0) * ROBOT_SPREAD;
    (pad.x + offset, pad.z)
}

/// The two adjacent spots a talking pair walks to: straddling the midpoint of
/// their stations, so the two robots stand next to each other. `from` is the
/// first CID's spot and `to` the second's.
pub fn pair_spots(a: (f32, f32), b: (f32, f32)) -> ((f32, f32), (f32, f32)) {
    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let (dx, dz) = (b.0 - a.0, b.1 - a.1);
    let length = (dx * dx + dz * dz).sqrt();
    // Two agents on the same spot have no direction to straddle: the pair sits
    // side by side across `x` rather than not walking at all.
    let (ux, uz) = if length <= f32::EPSILON {
        (1.0, 0.0)
    } else {
        (dx / length, dz / length)
    };
    (
        (mid.0 - ux * WALK_GAP, mid.1 - uz * WALK_GAP),
        (mid.0 + ux * WALK_GAP, mid.1 + uz * WALK_GAP),
    )
}

/// A straight line between two spots at `t` (0 is `from`, 1 is `to`).
pub fn lerp(from: (f32, f32), to: (f32, f32), t: f32) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0);
    (
        from.0 + (to.0 - from.0) * t,
        from.1 + (to.1 - from.1) * t,
    )
}

/// The easing the walk uses: smoothstep, `t²(3 − 2t)`, so the walk starts and
/// ends at rest. `eased(0) == 0`, `eased(1) == 1`, and it is monotonic over
/// `0..=1`.
pub fn eased(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far a walk that started at `since_ms` has progressed at `now_ms`, in
/// `0..=1`, on the bounded [`WALK_MS`] clock.
pub fn walk_progress(now_ms: u64, since_ms: u64) -> f32 {
    let elapsed = now_ms.saturating_sub(since_ms);
    eased((elapsed as f32 / WALK_MS as f32).min(1.0))
}

/// A robot's ground status: the ring the floor draws under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Recent serial activity — the console's own telemetry.
    Working,
    /// An active conversation, from the social API.
    Talking,
    /// Connected and quiet.
    Idle,
    /// The hub does not report the agent connected (only when a hub answered).
    Unreachable,
}

impl Status {
    /// The word the legend writes for this status.
    pub fn label(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Talking => "talking",
            Status::Idle => "idle",
            Status::Unreachable => "unreachable",
        }
    }

    /// The ink the status ring is drawn in, as the register's palette.
    pub fn ink(self) -> [u8; 3] {
        match self {
            Status::Working => [0x2e, 0x6a, 0x40],
            Status::Talking => [0xe8, 0x59, 0x0c],
            Status::Idle => [0x7d, 0x86, 0x7e],
            Status::Unreachable => [0xa3, 0x18, 0x2b],
        }
    }
}

/// One robot on the floor: an agent at its place, with its status.
#[derive(Debug, Clone, PartialEq)]
pub struct Robot {
    /// The agent's guest CID.
    pub cid: u64,
    /// The role it is filed under.
    pub role: String,
    /// Where it stands now, in world units (`x`, `z`) — its walked position.
    pub x: f32,
    /// Where it stands now, in world units (`z`).
    pub z: f32,
    /// Its ground status.
    pub status: Status,
    /// Whether it is standing on its own station (rather than at a
    /// conversation's spot).
    pub at_station: bool,
}

/// A line the floor draws between two adjacent robots: a talking pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    /// The first robot's world position.
    pub a: (f32, f32),
    /// The second robot's world position.
    pub b: (f32, f32),
}

/// The plaza as the floor pane draws it: the role pads, one robot per agent,
/// and the links between adjacent talking pairs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FloorLayout {
    /// The role pads, in the register's order.
    pub pads: Vec<Pad>,
    /// One robot per roster agent.
    pub robots: Vec<Robot>,
    /// A line between each adjacent talking pair.
    pub links: Vec<Link>,
}

impl FloorLayout {
    /// The robot with this CID, when the floor holds one.
    pub fn robot(&self, cid: u64) -> Option<&Robot> {
        self.robots.iter().find(|robot| robot.cid == cid)
    }

    /// The index of the robot with this CID, when the floor holds one.
    pub fn index_of(&self, cid: u64) -> Option<usize> {
        self.robots.iter().position(|robot| robot.cid == cid)
    }
}

/// One pair's walk state: when the pair's adjacency last changed, and which
/// way. The floor animates a robot toward its conversation's spot while
/// `active`, and back to its station after it goes quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Walk {
    /// The first CID of the pair.
    pub a: u64,
    /// The second CID of the pair.
    pub b: u64,
    /// Whether the pair is talking.
    pub active: bool,
    /// When the adjacency last changed, in milliseconds since the Unix epoch.
    pub since_ms: u64,
}

impl Walk {
    /// How far this walk has progressed at `now_ms`: toward the adjacent spot
    /// while talking, back to the stations after it went quiet.
    pub fn progress(&self, now_ms: u64) -> f32 {
        let t = walk_progress(now_ms, self.since_ms);
        if self.active {
            t
        } else {
            1.0 - t
        }
    }

    /// Whether the walk has finished: talking (fully adjacent) or quiet (fully
    /// home).
    pub fn settled(&self, now_ms: u64) -> bool {
        walk_progress(now_ms, self.since_ms) >= 1.0
    }

    /// Whether this walk is about `cid`.
    pub fn involves(&self, cid: u64) -> bool {
        self.a == cid || self.b == cid
    }

    /// The other CID of the pair.
    pub fn other(&self, cid: u64) -> u64 {
        if self.a == cid {
            self.b
        } else {
            self.a
        }
    }
}

/// The inks a role's robot body is tinted with. Deterministic: `role_ink`
/// hashes the role's bytes, so the same role is the same colour on every
/// frame and every host.
const ROLE_INKS: [[u8; 3]; 7] = [
    [0x2f, 0x6f, 0x6a], // teal
    [0xa8, 0x4b, 0x2a], // rust
    [0x3a, 0x4a, 0x8c], // indigo
    [0x5f, 0x6a, 0x2e], // olive
    [0x6a, 0x3f, 0x72], // plum
    [0x9a, 0x73, 0x1c], // ochre
    [0x8c, 0x4a, 0x6a], // rose
];

/// The ink a role's robot is tinted with: a stable hash of the role's bytes
/// into a fixed palette.
pub fn role_ink(role: &str) -> [u8; 3] {
    // FNV-1a, 64-bit: a stable hash with no dependency and no state.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in role.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let index = usize::try_from(hash % (ROLE_INKS.len() as u64)).unwrap_or(0);
    ROLE_INKS[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn the_projection_is_the_expected_screen_table() {
        // A table case: the RTS camera (55° tilt, 20° yaw, distance 10) into a
        // 1000×600 viewport (`focal = 0.85 · 600 = 510`). The numbers are the
        // perspective divide of each point's camera basis coordinates.
        let camera = Camera::rts();
        for (world, expected) in [
            ((0.0, 0.0, 0.0), (500.0, 300.0, 10.0)),
            ((1.0, 0.0, 0.0), (548.8833, 314.5744, 9.8038)),
            ((-1.0, 0.0, 0.0), (452.9977, 285.9864, 10.1962)),
            ((0.0, 1.0, 0.0), (500.0, 268.1376, 9.1808)),
            ((0.0, 0.0, 1.0), (481.5633, 341.4938, 9.4610)),
            ((3.0, 0.0, 3.75), (606.0334, 557.2032, 7.3903)),
            ((-3.0, 0.0, -3.75), (437.8562, 149.2588, 12.6097)),
        ] {
            let p = project(
                Vec3 {
                    x: world.0,
                    y: world.1,
                    z: world.2,
                },
                &camera,
                1000.0,
                600.0,
            )
            .unwrap_or_else(|| panic!("{world:?} is in front of the camera"));
            close(p.x, expected.0);
            close(p.y, expected.1);
            close(p.depth, expected.2);
        }
    }

    #[test]
    fn the_camera_looks_at_the_origin_from_above_and_in_front() {
        let camera = Camera::rts();
        // The look-at projects to the viewport centre, at exactly the
        // distance.
        let centre = project(Vec3 { x: 0.0, y: 0.0, z: 0.0 }, &camera, 800.0, 480.0)
            .expect("the origin is in front of the camera");
        close(centre.x, 400.0);
        close(centre.y, 240.0);
        close(centre.depth, CAM_DIST);
        // The eye is above the ground and on the near side of it.
        let tilt = TILT_DEG.to_radians();
        let yaw = YAW_DEG.to_radians();
        assert!((CAM_DIST * tilt.sin()).abs() > 5.0, "the camera is above the floor");
        assert!(CAM_DIST * tilt.cos() * yaw.cos() > 0.0, "and on the near side");
    }

    #[test]
    fn a_higher_point_lands_higher_and_a_farther_span_projects_smaller() {
        let camera = Camera::rts();
        let at = |x: f32, y: f32, z: f32| {
            project(Vec3 { x, y, z }, &camera, 1000.0, 600.0).expect("in front")
        };
        let ground = at(1.0, 0.0, 0.0);
        let lifted = at(1.0, 1.0, 0.0);
        assert!(lifted.y < ground.y, "up in the world is up on the screen");
        // Perspective foreshortening: the same one-unit world span across the
        // ground subtends less at a greater depth.
        let near = at(1.0, 0.0, 2.0).x - at(0.0, 0.0, 2.0).x;
        let far = at(1.0, 0.0, -2.0).x - at(0.0, 0.0, -2.0).x;
        assert!(
            near > far,
            "a farther span projects smaller ({near} vs {far})"
        );
        assert!(
            at(1.0, 0.0, 2.0).depth < at(1.0, 0.0, -2.0).depth,
            "the near point is nearer"
        );
        // To the camera's right is to the screen's right.
        assert!(at(2.0, 0.0, 0.0).x > 500.0, "east of the origin is right of the centre");
    }

    #[test]
    fn a_point_at_or_behind_the_near_plane_is_not_drawn() {
        let camera = Camera::rts();
        // Straight out in front of the eye, past it: behind the camera.
        let behind = Vec3 {
            x: CAM_DIST * TILT_DEG.to_radians().cos() * YAW_DEG.to_radians().sin() * 2.0,
            y: CAM_DIST * TILT_DEG.to_radians().sin() * 2.0,
            z: CAM_DIST * TILT_DEG.to_radians().cos() * YAW_DEG.to_radians().cos() * 2.0,
        };
        assert!(project(behind, &camera, 1000.0, 600.0).is_none());
    }

    #[test]
    fn the_layout_is_a_fixed_table_of_positions_for_a_fixed_role_order() {
        let roles: Vec<String> = ["architect", "builder", "reviewer", "auditor", "librarian"]
            .iter()
            .map(|role| role.to_string())
            .collect();
        let rows = pads(&roles);
        let table: [(&str, usize, usize, f32, f32); 5] = [
            ("architect", 0, 0, -3.0, -2.25),
            ("builder", 1, 0, -1.5, -2.25),
            ("reviewer", 2, 0, 0.0, -2.25),
            ("auditor", 3, 0, 1.5, -2.25),
            ("librarian", 4, 0, 3.0, -2.25),
        ];
        assert_eq!(rows.len(), table.len());
        for (pad, (role, col, row, x, z)) in rows.iter().zip(table) {
            assert_eq!(pad.role, role);
            assert_eq!((pad.col, pad.row), (col, row));
            close(pad.x, x);
            close(pad.z, z);
        }
        // A sixth role wraps to the second row.
        let mut roles = roles;
        roles.push("scout".to_string());
        let six = pads(&roles);
        assert_eq!((six[5].col, six[5].row), (0, 1));
        close(six[5].x, -3.0);
        close(six[5].z, -0.75);
    }

    #[test]
    fn agents_sharing_a_role_stand_apart_on_their_pad() {
        let pads = pads(&["builder".to_string()]);
        let pad = &pads[0];
        assert_eq!(station(pad, 0, 1), (pad.x, pad.z));
        let (left, _) = station(pad, 0, 2);
        let (right, _) = station(pad, 1, 2);
        close(left, pad.x - ROBOT_SPREAD / 2.0);
        close(right, pad.x + ROBOT_SPREAD / 2.0);
    }

    #[test]
    fn the_pair_straddles_its_midpoint_and_sits_side_by_side() {
        let ((ax, az), (bx, bz)) = pair_spots((0.0, 0.0), (2.0, 0.0));
        close(ax, 0.66);
        close(az, 0.0);
        close(bx, 1.34);
        close(bz, 0.0);
        // The two spots are WALK_GAP apart from the midpoint.
        close((ax - 1.0).abs(), WALK_GAP);
        close((bx - 1.0).abs(), WALK_GAP);
        // Two agents on the same station still get distinct spots.
        let ((ax, _), (bx, _)) = pair_spots((1.0, 1.0), (1.0, 1.0));
        assert_ne!(ax, bx);
    }

    #[test]
    fn the_walk_is_eased_and_bounded() {
        // The easing's endpoints and its bounds.
        close(eased(0.0), 0.0);
        close(eased(1.0), 1.0);
        close(eased(-1.0), 0.0);
        close(eased(2.0), 1.0);
        close(eased(0.5), 0.5);
        // A value off the midpoint distinguishes smoothstep from the identity:
        // `0.25² · (3 − 0.5) = 0.15625`, not `0.25`.
        close(eased(0.25), 0.15625);
        close(eased(0.75), 0.84375);
        let mut previous = 0.0;
        for step in 0..=20 {
            let value = eased(step as f32 / 20.0);
            assert!(value >= previous, "the easing is monotonic");
            assert!((0.0..=1.0).contains(&value));
            previous = value;
        }
        // The clock: 0 at the start, 1 at WALK_MS and after.
        close(walk_progress(1_000, 1_000), 0.0);
        close(walk_progress(1_000 + WALK_MS, 1_000), 1.0);
        close(walk_progress(1_000 + WALK_MS * 5, 1_000), 1.0);
        // A clock behind the start reads as not yet moved, not as negative.
        close(walk_progress(900, 1_000), 0.0);
    }

    #[test]
    fn a_walk_runs_toward_the_spot_then_back_home() {
        let walk = Walk {
            a: 1,
            b: 2,
            active: true,
            since_ms: 10_000,
        };
        close(walk.progress(10_000), 0.0);
        close(walk.progress(10_000 + WALK_MS), 1.0);
        assert!(walk.settled(10_000 + WALK_MS));
        let quiet = Walk {
            active: false,
            ..walk
        };
        close(quiet.progress(10_000), 1.0);
        close(quiet.progress(10_000 + WALK_MS), 0.0);
        assert!(quiet.settled(10_000 + WALK_MS));
        assert!(walk.involves(1) && walk.involves(2) && !walk.involves(3));
        assert_eq!(walk.other(1), 2);
        assert_eq!(walk.other(2), 1);
    }

    #[test]
    fn the_role_ink_is_stable_and_lands_in_the_palette() {
        // Deterministic: the same role is the same ink, on every frame and
        // every host. The hash is pinned for a few roles.
        assert_eq!(role_ink("builder"), ROLE_INKS[0]);
        assert_eq!(role_ink("architect"), ROLE_INKS[4]);
        assert_eq!(role_ink("reviewer"), ROLE_INKS[6]);
        assert_eq!(role_ink("builder"), role_ink("builder"));
        assert_eq!(role_ink(""), role_ink(""));
        // And every ink is one of the palette's.
        for role in ["architect", "builder", "reviewer", "auditor", "librarian", ""] {
            assert!(ROLE_INKS.contains(&role_ink(role)), "{role:?}");
        }
    }

    #[test]
    fn the_status_inks_are_the_registers_palette() {
        assert_eq!(Status::Working.ink(), [0x2e, 0x6a, 0x40]);
        assert_eq!(Status::Talking.ink(), [0xe8, 0x59, 0x0c]);
        assert_eq!(Status::Idle.ink(), [0x7d, 0x86, 0x7e]);
        assert_eq!(Status::Unreachable.ink(), [0xa3, 0x18, 0x2b]);
        assert_eq!(Status::Working.label(), "working");
        assert_eq!(Status::Talking.label(), "talking");
        assert_eq!(Status::Idle.label(), "idle");
        assert_eq!(Status::Unreachable.label(), "unreachable");
    }

    #[test]
    fn the_layout_finds_a_robot_by_cid() {
        let layout = FloorLayout {
            pads: Vec::new(),
            robots: vec![Robot {
                cid: 100,
                role: "builder".to_string(),
                x: 0.0,
                z: 0.0,
                status: Status::Idle,
                at_station: true,
            }],
            links: Vec::new(),
        };
        assert_eq!(layout.index_of(100), Some(0));
        assert!(layout.robot(100).is_some());
        assert!(layout.robot(999).is_none());
        assert_eq!(layout.index_of(999), None);
    }

    /// The projection distance to a point on the floor in front, used to keep
    /// the pipeline honest about `lerp` too.
    #[test]
    fn lerp_is_pinned_at_both_ends() {
        close(lerp((0.0, 1.0), (4.0, 5.0), 0.0).0, 0.0);
        close(lerp((0.0, 1.0), (4.0, 5.0), 1.0).1, 5.0);
        let mid = lerp((0.0, 1.0), (4.0, 5.0), 0.5);
        close(mid.0, 2.0);
        close(mid.1, 3.0);
        // Out-of-range t is clamped, never extrapolated.
        close(lerp((0.0, 0.0), (4.0, 4.0), 3.0).0, 4.0);
        close(lerp((0.0, 0.0), (4.0, 4.0), -2.0).0, 0.0);
    }
}
