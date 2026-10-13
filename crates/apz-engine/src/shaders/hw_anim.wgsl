// Hardware animation for the Machines view: fan spins and LED blinks as vertex functions of the clock,
// so their parts bake as static geometry (the batches merge them; no per-frame CPU pose). Each vertex
// carries its part's animation, in the batch frame at rest:
//   vs_spin:  the part turns about `pivot` on `axis` at `rps` revolutions per second, from `phase` radians
//             at time 0. The rest pose is the baked position, so the angle is phase + TAU * rps * t.
//   vs_blink: the emissive term is `glow` for the first half of each `period` (shifted by `phase` seconds)
//             and `glow * off` for the second half, as hardware.rs `Built::update` does.
// The clock is g.time.x in seconds. The host wraps it at WRAP_PERIOD (3600 s): a spin is seamless across
// the wrap only when its rps is a multiple of 1 / WRAP_PERIOD, so the baked rps must be quantised so.
// hardware.rs `spin_point_shader` and `blink_emissive_shader` mirror these formulas in Rust.
// WebGL2 has no storage buffers in vertex stages, so each vertex's data arrives as vertex attributes.

struct Globals {
    view_proj: mat4x4<f32>,
    time: vec4<f32>, // x: seconds
}

@group(0) @binding(0) var<uniform> g: Globals;

const TAU: f32 = 6.2831853;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) emissive: f32,
}

@fragment
fn fs_color(i: Out) -> @location(0) vec4<f32> {
    return i.color;
}

// The emissive term scales the base colour (the blink's on and off levels).
@fragment
fn fs_emissive(i: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(i.color.rgb * i.emissive, i.color.a);
}

// `v` turned by `angle` about the unit axis `k` (Rodrigues; right-handed, as Quat::from_axis_angle).
fn rotate_about(k: vec3<f32>, angle: f32, v: vec3<f32>) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return v * c + cross(k, v) * s + k * (dot(k, v) * (1.0 - c));
}

// ---- Spins (fans) ----------------------------------------------------------------------------

struct SpinIn {
    @location(0) pos: vec3<f32>,          // rest position, batch frame
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) pivot_phase: vec4<f32>,  // xyz pivot (the part's origin), w phase (radians at time 0)
    @location(4) axis_rps: vec4<f32>,     // xyz unit axis, batch frame, w revolutions per second
}

@vertex
fn vs_spin(v: SpinIn) -> Out {
    let angle = v.pivot_phase.w + TAU * v.axis_rps.w * g.time.x;
    let k = v.axis_rps.xyz;
    let world = v.pivot_phase.xyz + rotate_about(k, angle, v.pos - v.pivot_phase.xyz);
    var o: Out;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.color = v.color;
    o.normal = rotate_about(k, angle, v.normal);
    o.emissive = 1.0;
    return o;
}

// ---- Blinks (LEDs) ---------------------------------------------------------------------------

struct BlinkIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) blink: vec4<f32>, // x period (s), y phase (s), z glow, w off level
}

@vertex
fn vs_blink(v: BlinkIn) -> Out {
    let on = ((g.time.x + v.blink.y) % v.blink.x) < v.blink.x * 0.5;
    var o: Out;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    o.color = v.color;
    o.normal = v.normal;
    o.emissive = select(v.blink.z * v.blink.w, v.blink.z, on);
    return o;
}
