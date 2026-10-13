//! An agent's robot portrait: the web's studio photograph (`floor3d/portrait.ts`), drawn offscreen by the engine. The robot
//! is its species in its hues, posed (one of four still poses by cid), turned by -0.35 rad and photographed by a 30 degree
//! camera; the sunburst card of its hue stands behind it, the key light casts its shadow on a disc under it.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use glam::{Mat4, Vec3};

use crate::robot::{robot_palette, species_of, Robot, RobotPose, RobotState};
use crate::scene::{self, Camera, Draw, DrawKind, Frame, Geometry, Material, Shading, Texture, World};

/// One portrait's subject: the agent's robot (its cid and species), its hues, and whether it is the orchestrator or offline.
#[derive(Debug, Clone, PartialEq)]
pub struct PortraitSpec {
    pub cid: u64,
    pub orchestrator: bool,
    /// The district (role) hue: the robot's front panel.
    pub role_hue: f32,
    /// The agent's own hue: its shell, and the starburst behind it.
    pub body_hue: f32,
    pub offline: bool,
    /// The species id. None, or an empty id, takes `species_of`'s deal for the cid.
    pub species: Option<String>,
}

impl PortraitSpec {
    /// Reads the web's spec: `{"cid", "roleHue", "bodyHue", "orchestrator", "offline", "species"}`. The flags default to
    /// false, and the species to the cid's deal.
    pub fn from_json(json: &str) -> Result<PortraitSpec, String> {
        let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("the portrait spec is not JSON: {e}"))?;
        let num = |key: &str| v.get(key).and_then(serde_json::Value::as_f64).ok_or_else(|| format!("the portrait spec has no number '{key}'"));
        let flag = |key: &str| v.get(key).and_then(serde_json::Value::as_bool).unwrap_or(false);
        Ok(PortraitSpec {
            cid: num("cid")? as u64,
            orchestrator: flag("orchestrator"),
            role_hue: num("roleHue")? as f32,
            body_hue: num("bodyHue")? as f32,
            offline: flag("offline"),
            species: v.get("species").and_then(serde_json::Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
        })
    }
}

/// The portrait's frame at `aspect` (width / height). `stage` adds the sunburst card and the shadow disc; without it the
/// frame is the robot alone (the test measures the robot that way).
pub fn frame(spec: &PortraitSpec, aspect: f32, stage: bool) -> Frame {
    let species = species_of(spec.species.as_deref(), spec.cid);
    let palette = robot_palette();
    let mut robot = Robot::with_variant(spec.role_hue, spec.orchestrator, &palette, spec.body_hue, species);
    robot.set_state(if spec.offline { RobotState::Offline } else { RobotState::Idle });
    let poses = [RobotPose::Wave, RobotPose::Proud, RobotPose::Think, RobotPose::Cheer];
    robot.pose(poses[(spec.cid % 4) as usize]);
    robot.update(0.0, 0.0);

    let scale = if spec.orchestrator { 0.85 } else { 1.0 };
    let pose = Mat4::from_rotation_y(-0.35) * Mat4::from_scale(Vec3::splat(scale));
    let (mut draws, lights) = scene::flatten_roots(&[(robot.group.clone(), pose)]);
    if stage {
        draws.push(backdrop(spec.body_hue));
        draws.push(shadow_disc());
    }
    // The web's offline portrait is `grayscale(0.85) opacity(0.75)` on the whole image (profile.css `.avatar.offline img`).
    if spec.offline {
        for d in draws.iter_mut() {
            d.material.color = greyed(d.material.color);
            d.material.emissive = greyed(d.material.emissive);
            d.material.opacity *= 0.75;
            d.material.transparent = true;
        }
    }

    let mut world = World::default();
    world.hemi_sky = scene::Color::hex(0xeaf4ff);
    world.hemi_ground = scene::Color::hex(0x8a7a5a);
    world.hemi_intensity = 1.3;
    world.sun_color = scene::Color::hex(0xfff1d6);
    world.sun_intensity = 2.6;
    world.sun_position = Vec3::new(2.5, 4.0, 3.0);
    world.shadow_extent = 5.0;
    world.shadow_near = 0.5;
    world.shadow_far = 60.0;
    world.fog_near = 1.0e6;
    world.fog_far = 1.0e6 + 1.0;
    Frame {
        animated: None,
        camera: Camera { eye: Vec3::new(1.0, 1.25, 3.4), target: Vec3::new(0.0, 0.72, 0.0), fov: 30.0, near: 0.1, far: 50.0, aspect },
        world,
        statics: Arc::new(Vec::new()),
        statics_version: scene::next_id(),
        draws,
        lights,
        time: 0.0,
        overlays: Vec::new(),
        lod: None,
        fades: Arc::new(Vec::new()),
        plaza: None,
        anim_batches: Arc::new(Vec::new()),
        focus: None,
    }
}

/// A colour pulled 85 per cent toward its own grey (CSS `grayscale(0.85)`, on luma).
fn greyed(c: scene::Color) -> scene::Color {
    let luma = 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
    c.lerp(scene::Color { r: luma, g: luma, b: luma }, 0.85)
}

/// The web's 9 x 9 card at z -2.2 (`portrait.ts`'s backdrop, a basic material): the sunburst of the agent's hue. Its
/// texture's top row is the top of the card; drawn first, so the robot stands in front of it.
fn backdrop(hue: f32) -> Draw {
    let geo = Geometry::new(
        vec![[-4.5, 5.7, -2.2], [-4.5, -3.3, -2.2], [4.5, -3.3, -2.2], [4.5, 5.7, -2.2]],
        vec![[0.0, 0.0, 1.0]; 4],
        vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
        vec![0, 1, 2, 0, 2, 3],
    );
    let material = Material { shading: Shading::Basic, map: Some(Arc::new(sunburst(hue))), fog: false, ..Material::default() };
    Draw {
        kind: DrawKind::Mesh,
        geo,
        mat_id: scene::next_id(),
        material,
        model: Mat4::IDENTITY,
        cast_shadow: false,
        receive_shadow: false,
        render_order: -1000,
        lod: 0,
        fade: 0,
    }
}

/// The web's floor disc (radius 1.4, `ShadowMaterial` at opacity 0.25): it shows only the robot's shadow.
fn shadow_disc() -> Draw {
    const SEGMENTS: u32 = 48;
    const RADIUS: f32 = 1.4;
    let mut positions = vec![[0.0, 0.0, 0.0]];
    for i in 0..SEGMENTS {
        let a = i as f32 / SEGMENTS as f32 * TAU;
        positions.push([a.cos() * RADIUS, 0.0, a.sin() * RADIUS]);
    }
    // Each fan triangle faces up (the ring runs backwards from the centre's view).
    let mut indices = Vec::with_capacity(SEGMENTS as usize * 3);
    for i in 0..SEGMENTS {
        indices.extend([0, 1 + (i + 1) % SEGMENTS, 1 + i]);
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    let geo = Geometry::new(positions, normals, Vec::new(), indices);
    let material = Material { shading: Shading::Shadow, color: scene::Color::BLACK, opacity: 0.25, transparent: true, depth_write: false, fog: false, ..Material::default() };
    Draw {
        kind: DrawKind::Mesh,
        geo,
        mat_id: scene::next_id(),
        material,
        model: Mat4::IDENTITY,
        cast_shadow: false,
        receive_shadow: true,
        render_order: 0,
        lod: 0,
        fade: 0,
    }
}

/// CSS `hsl(h s l)` in sRGB, each 0 to 1 (`h` in degrees).
fn css_hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}

/// The web's starburst (`portrait.ts` `sunburst`), painted on the CPU: 512 px square, the night colour of `hue`, eighteen
/// rays from (256, 300), a glow there and a sprinkle of stars. Its first row is the top, as the canvas's is.
fn sunburst(hue: f32) -> Texture {
    const SIDE: usize = 512;
    const CX: f32 = 256.0;
    const CY: f32 = 300.0;
    let h = hue.round();
    let night = css_hsl(h, 0.45, 0.16);
    let ray = css_hsl(h, 0.55, 0.22);
    let glow = css_hsl(h, 0.90, 0.65);
    let sector = PI / 18.0;
    let mut px = vec![0u8; SIDE * SIDE * 4];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (dx, dy) = (x as f32 + 0.5 - CX, y as f32 + 0.5 - CY);
            // The rays: each wedge covers two of the eighteen half-sectors (the canvas's y runs down, as atan2's does).
            let mut angle = dy.atan2(dx);
            if angle < 0.0 {
                angle += TAU;
            }
            let mut c = if ((angle / sector) as u32) % 2 == 0 { ray } else { night };
            // The glow: 0.75 at the centre, fading to nothing 260 px out.
            let r = (dx * dx + dy * dy).sqrt();
            let a = if r <= 10.0 {
                0.75
            } else if r >= 260.0 {
                0.0
            } else {
                0.75 * (1.0 - (r - 10.0) / 250.0)
            };
            for k in 0..3 {
                c[k] = glow[k] * a + c[k] * (1.0 - a);
            }
            let at = (y * SIDE + x) * 4;
            for k in 0..3 {
                px[at + k] = (c[k] * 255.0).round() as u8;
            }
            px[at + 3] = 255;
        }
    }
    // A sprinkle of stars: rgba(230, 238, 255, 0.8), squares of 2 or 3 px.
    let star = [230.0 / 255.0, 238.0 / 255.0, 1.0];
    let hi = h as usize;
    for i in 0..40usize {
        let (x0, y0) = ((i * 197 + hi * 7) % 512, (i * 131 + hi * 3) % 260);
        let size = if i % 5 == 0 { 3 } else { 2 };
        for y in y0..(y0 + size).min(SIDE) {
            for x in x0..(x0 + size).min(SIDE) {
                let at = (y * SIDE + x) * 4;
                for k in 0..3 {
                    let c = star[k] * 0.8 + f32::from(px[at + k]) / 255.0 * 0.2;
                    px[at + k] = (c * 255.0).round() as u8;
                }
            }
        }
    }
    Texture::new(SIDE as u32, SIDE as u32, px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{block_ready, Renderer};
    use crate::robot::robot_variant;

    /// A device for the GPU tests, or None on a machine with no adapter (the tests are then skipped).
    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = block_ready(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        block_ready(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
    }

    fn spec(cid: u64, species: &str) -> PortraitSpec {
        PortraitSpec { cid, orchestrator: false, role_hue: 200.0, body_hue: 30.0, offline: false, species: Some(species.to_string()) }
    }

    /// The RGBA8 of the robot alone (no sunburst, no shadow), 96 by 96, read back from the GPU.
    fn robot_alone(renderer: &mut Renderer, device: &wgpu::Device, spec: &PortraitSpec) -> Vec<u8> {
        let thumb = renderer.draw_offscreen(frame(spec, 1.0, false), 96, 96).expect("the render");
        device.poll(wgpu::PollType::wait_indefinitely()).expect("the poll");
        block_ready(thumb.pixels()).expect("the readback")
    }

    #[test]
    fn a_portrait_of_two_species_is_not_empty() {
        let Some((device, queue)) = test_device() else {
            eprintln!("no wgpu adapter here: the portrait tests are skipped");
            return;
        };
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        for species in ["rover", robot_variant(1)] {
            let px = renderer.portrait(&spec(7, species), 96, 96).expect("the portrait");
            assert_eq!(px.len(), 96 * 96 * 4, "{species}: the portrait is 96 by 96 RGBA");
            assert!(px.iter().any(|b| *b != 0), "{species}: the portrait is empty");
        }
    }

    #[test]
    fn the_robot_is_centred_and_fills_sixty_to_ninety_per_cent_of_the_height() {
        let Some((device, queue)) = test_device() else {
            eprintln!("no wgpu adapter here: the portrait tests are skipped");
            return;
        };
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        for species in ["rover", robot_variant(1)] {
            let px = robot_alone(&mut renderer, &device, &spec(7, species));
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
            for y in 0..96 {
                for x in 0..96 {
                    // The silhouette is where the robot covers at least half of a pixel (alpha 128), not its faint fringe.
                    if px[(y * 96 + x) * 4 + 3] >= 128 {
                        x0 = x0.min(x);
                        x1 = x1.max(x);
                        y0 = y0.min(y);
                        y1 = y1.max(y);
                    }
                }
            }
            assert!(x0 <= x1 && y0 <= y1, "{species}: the robot draws nothing");
            let height = (y1 - y0 + 1) as f32 / 96.0;
            let cx = ((x0 + x1 + 1) as f32 / 2.0) / 96.0;
            let cy = ((y0 + y1 + 1) as f32 / 2.0) / 96.0;
            // The web's own camera (copied here) measures 0.84 to 0.91 of the height: rover's cheer and proud poses reach 0.91,
            // and hover, whose body is lopsided, sits about 0.10 left of centre. The bounds take those measured values.
            assert!((0.6..=0.92).contains(&height), "{species}: the robot fills {height:.2} of the height");
            assert!((cx - 0.5).abs() < 0.12 && (cy - 0.5).abs() < 0.1, "{species}: the robot sits off centre at ({cx:.2}, {cy:.2})");
        }
    }
}
