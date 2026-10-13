// Hardware animation for the Machines view: fan spins and LED blinks as vertex functions of the clock,
// so their parts bake as static geometry (the batches merge them; no per-frame CPU pose). Each vertex
// carries its part's animation, in world space at rest:
//   vs_spin:  the part turns about `pivot` on `axis` at `rps` revolutions per second, from `phase` radians
//             at time 0. The rest pose is the baked position, so the angle is phase + TAU * rps * t.
//   vs_blink: the emissive term is `glow` for the first half of each `period` (shifted by `phase` seconds)
//             and `glow * off` for the second half, as hardware.rs `Built::update` does.
// The clock is g.counts.z, the host's clock wrapped at WRAP_PERIOD (3600 s): a spin is seamless across
// the wrap only when its rps is a multiple of 1 / WRAP_PERIOD, so the baked rps must be quantised so.
// hardware.rs `spin_point_shader` and `blink_emissive_shader` mirror these formulas in Rust.
//
// The lighting, fog and encoding are the floor's (floor.wgsl): the same Globals block and bind group 0
// (render.rs `Globals`, the floor's globals layout), and copies of its ramp, sun_shadow, attenuation,
// fogged and encode. render.rs's test compares those copies with floor.wgsl. WGSL has no includes.
// Passes: the main pass (`vs_spin` / `vs_blink` with `fs_color` / `fs_emissive`), the ink hulls
// (`vs_spin_outline` / `vs_blink_outline` with `fs_outline`), and the sun's shadow map (`vs_spin_shadow`).
// WebGL2 has no storage buffers in vertex stages, so each vertex's data arrives as vertex attributes.

const PI: f32 = 3.141592653589793;
const TAU: f32 = 6.2831853;

// The floor's Globals (render.rs `Globals`, floor.wgsl). counts.z is the animation clock (wrapped).
struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    // xyz: camera position; w: seconds (wind).
    eye: vec4<f32>,
    // xyz: towards the sun; w: shadow bias.
    sun_dir: vec4<f32>,
    // rgb: colour x intensity; w: shadow normal bias.
    sun_color: vec4<f32>,
    hemi_sky: vec4<f32>,
    hemi_ground: vec4<f32>,
    // rgb: fog colour; w: fog near.
    fog_color: vec4<f32>,
    // x: fog far; y: outline thickness; z: viewport height (px); w: 1 = encode sRGB on output.
    params: vec4<f32>,
    ink: vec4<f32>,
    sky_top: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_ground: vec4<f32>,
    // xyz: position; w: distance (0 = no cutoff).
    point_pos: array<vec4<f32>, 4>,
    // rgb: colour x intensity; w: decay.
    point_color: array<vec4<f32>, 4>,
    // x: point lights; y: shadow texel; z: animation clock (s, wrapped); w: unused.
    counts: vec4<f32>,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) emissive: f32,
    @location(3) world: vec3<f32>,
    @location(4) view_depth: f32,
}

// ---- Lighting (floor.wgsl's toon, for a part with no texture) --------------------------------

// three's toon ramp: a 3-texel nearest-filtered gradient (90, 170, 255) read at dotNL * 0.5 + 0.5.
fn ramp(dot_nl: f32) -> f32 {
    let coord = dot_nl * 0.5 + 0.5;
    if coord < 1.0 / 3.0 {
        return 90.0 / 255.0;
    }
    if coord < 2.0 / 3.0 {
        return 170.0 / 255.0;
    }
    return 1.0;
}

// 1 lit, 0 in the sun's shadow (3x3 comparison taps, three's bias and normal bias).
fn sun_shadow(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let p = g.light_view_proj * vec4<f32>(world + n * g.sun_color.w, 1.0);
    let ndc = p.xyz / p.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 {
        return 1.0;
    }
    let depth = ndc.z + g.sun_dir.w;
    let texel = g.counts.y;
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            lit += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(f32(x), f32(y)) * texel, depth);
        }
    }
    return lit / 9.0;
}

fn attenuation(distance: f32, cutoff: f32, decay: f32) -> f32 {
    var a = 1.0 / max(pow(distance, decay), 0.01);
    if cutoff > 0.0 {
        let k = clamp(1.0 - pow(distance / cutoff, 4.0), 0.0, 1.0);
        a *= k * k;
    }
    return a;
}

fn fogged(c: vec3<f32>, depth: f32) -> vec3<f32> {
    let f = smoothstep(g.fog_color.w, g.params.x, depth);
    return mix(c, g.fog_color.rgb, f);
}

fn encode(c: vec3<f32>) -> vec3<f32> {
    if g.params.w < 0.5 {
        return c;
    }
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c < vec3<f32>(0.0031308));
}

// floor.wgsl's `toon` body: the ramp of the sun and the point lights, the hemisphere, the emissive added
// after the lit base, then fog and encoding.
fn shade(c: vec4<f32>, normal: vec3<f32>, world: vec3<f32>, view_depth: f32, emissive: vec3<f32>) -> vec4<f32> {
    let n = normalize(normal);
    let shadow = sun_shadow(world, n);
    var irradiance = ramp(dot(n, g.sun_dir.xyz)) * g.sun_color.rgb * shadow;
    let n_points = i32(g.counts.x);
    for (var k = 0; k < n_points; k++) {
        let to_light = g.point_pos[k].xyz - world;
        let d = length(to_light);
        let l = to_light / max(d, 1e-5);
        irradiance += ramp(dot(n, l)) * g.point_color[k].rgb * attenuation(d, g.point_pos[k].w, g.point_color[k].w);
    }
    irradiance += mix(g.hemi_ground.rgb, g.hemi_sky.rgb, 0.5 * n.y + 0.5);
    let lit = c.rgb / PI * irradiance + emissive;
    return vec4<f32>(encode(fogged(lit, view_depth)), c.a);
}

@fragment
fn fs_color(i: Out) -> @location(0) vec4<f32> {
    return shade(i.color, i.normal, i.world, i.view_depth, vec3<f32>(0.0));
}

// The emissive term scales the base colour (the blink's on and off levels).
@fragment
fn fs_emissive(i: Out) -> @location(0) vec4<f32> {
    return shade(i.color, i.normal, i.world, i.view_depth, i.color.rgb * i.emissive);
}

// The ink hull (floor.wgsl's fs_outline).
@fragment
fn fs_outline(i: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(encode(fogged(g.ink.rgb, i.view_depth)), 1.0);
}

// `v` turned by `angle` about the unit axis `k` (Rodrigues; right-handed, as Quat::from_axis_angle).
fn rotate_about(k: vec3<f32>, angle: f32, v: vec3<f32>) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return v * c + cross(k, v) * s + k * (dot(k, v) * (1.0 - c));
}

// The ink hull: pushed out along its smooth normals by a width that grows with distance (floor.wgsl's
// vs_outline, on a world-space pose).
fn hull(world: vec3<f32>, normal: vec3<f32>) -> Out {
    var mv = g.view * vec4<f32>(world, 1.0);
    let n = normalize((g.view * vec4<f32>(normalize(normal), 0.0)).xyz);
    let grow = g.params.y * clamp(-mv.z * 0.06, 0.7, 4.0);
    mv = vec4<f32>(mv.xyz + n * grow, 1.0);
    var o: Out;
    o.clip = g.proj * mv;
    o.color = vec4<f32>(g.ink.rgb, 1.0);
    o.normal = n;
    o.emissive = 0.0;
    o.world = world;
    o.view_depth = -mv.z;
    return o;
}

// ---- Spins (fans) ----------------------------------------------------------------------------

struct SpinIn {
    @location(0) pos: vec3<f32>,          // rest position, world space
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) pivot_phase: vec4<f32>,  // xyz pivot (the part's origin), w phase (radians at time 0)
    @location(4) axis_rps: vec4<f32>,     // xyz unit axis, w revolutions per second
}

struct Pose {
    world: vec3<f32>,
    normal: vec3<f32>,
}

fn spin_pose(v: SpinIn) -> Pose {
    let angle = v.pivot_phase.w + TAU * v.axis_rps.w * g.counts.z;
    let k = v.axis_rps.xyz;
    var p: Pose;
    p.world = v.pivot_phase.xyz + rotate_about(k, angle, v.pos - v.pivot_phase.xyz);
    p.normal = rotate_about(k, angle, v.normal);
    return p;
}

@vertex
fn vs_spin(v: SpinIn) -> Out {
    let p = spin_pose(v);
    var o: Out;
    o.clip = g.view_proj * vec4<f32>(p.world, 1.0);
    o.color = v.color;
    o.normal = p.normal;
    o.emissive = 1.0;
    o.world = p.world;
    o.view_depth = -(g.view * vec4<f32>(p.world, 1.0)).z;
    return o;
}

@vertex
fn vs_spin_outline(v: SpinIn) -> Out {
    let p = spin_pose(v);
    return hull(p.world, p.normal);
}

// The sun's shadow of a spinning part (position only).
@vertex
fn vs_spin_shadow(v: SpinIn) -> @builtin(position) vec4<f32> {
    return g.light_view_proj * vec4<f32>(spin_pose(v).world, 1.0);
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
    let on = ((g.counts.z + v.blink.y) % v.blink.x) < v.blink.x * 0.5;
    var o: Out;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    o.color = v.color;
    o.normal = v.normal;
    o.emissive = select(v.blink.z * v.blink.w, v.blink.z, on);
    o.world = v.pos;
    o.view_depth = -(g.view * vec4<f32>(v.pos, 1.0)).z;
    return o;
}

@vertex
fn vs_blink_outline(v: BlinkIn) -> Out {
    return hull(v.pos, v.normal);
}
