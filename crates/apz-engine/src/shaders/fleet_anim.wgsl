// Fleet animation for the Machines view: cable light packets, status rings and LEDs, each drawn
// instanced. Every motion is a pure function of g.time (seconds), so the Ambient tier (12 fps) stays
// right without per-frame CPU work. The CPU writes instance data only when a cable's rate or a
// machine's status changes; the host wraps the clock to WRAP_PERIOD (3600 s, a multiple of every
// period here). WebGL2 has no storage buffers in vertex stages, so each instance's data arrives as
// vertex attributes.
//
// The cable is fleet.rs `cable_points`: a quadratic Bezier with its middle sagged CABLE_SAG below the
// chord, sampled at CABLE_SEGMENTS steps and clamped above the ground. A packet sits on the lerp
// between two samples (`point_on`). fleet.rs `packet_at_shader` mirrors these formulas in Rust.

struct Globals {
    view_proj: mat4x4<f32>,
    time: vec4<f32>, // x: seconds
}

@group(0) @binding(0) var<uniform> g: Globals;

const TAU: f32 = 6.2831853;
const CABLE_SEGMENTS: f32 = 24.0;
const CABLE_SAG: f32 = 1.5;
const PACKET_SPEED: f32 = 0.4; // a packet travels its cable in 2.5 s

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@fragment
fn fs_color(i: Out) -> @location(0) vec4<f32> {
    return i.color;
}

// Sample i (0..=24) of the cable: the Bezier at t = i / 24, clamped above the ground.
fn cable_sample(port_a: vec3<f32>, port_b: vec3<f32>, i: f32) -> vec3<f32> {
    let control = 0.5 * (port_a + port_b) - vec3<f32>(0.0, 2.0 * CABLE_SAG, 0.0);
    let t = i / CABLE_SEGMENTS;
    let u = 1.0 - t;
    var p = u * u * port_a + 2.0 * u * t * control + t * t * port_b;
    p.y = max(p.y, 0.0);
    return p;
}

// The point at fraction p (0..1) of the cable: lerp between the two samples around p * 24.
fn cable_at(port_a: vec3<f32>, port_b: vec3<f32>, p: f32) -> vec3<f32> {
    let f = clamp(p, 0.0, 1.0) * CABLE_SEGMENTS;
    let i = min(floor(f), CABLE_SEGMENTS - 1.0);
    return mix(cable_sample(port_a, port_b, i), cable_sample(port_a, port_b, i + 1.0), f - i);
}

// ---- Cable packets ---------------------------------------------------------------------------
// One instance per packet slot. from and to are the cable's ends (node port, hub port). params: x
// phase (0..1), y direction (+1 forward from `from`, -1 reversed), z alive (1 shows, 0 hides), w
// radius. Progress is fract(phase + time * PACKET_SPEED), so a slot loops its cable every 2.5 s.

struct PacketIn {
    @location(0) pos: vec3<f32>, // unit sphere vertex
    @location(1) port_a: vec3<f32>,
    @location(2) port_b: vec3<f32>,
    @location(3) params: vec4<f32>,
    @location(4) color: vec4<f32>,
}

@vertex
fn vs_packet(v: PacketIn) -> Out {
    let p = fract(v.params.x + g.time.x * PACKET_SPEED);
    let at = select(p, 1.0 - p, v.params.y < 0.0);
    let centre = cable_at(v.port_a, v.port_b, at);
    let world = centre + v.pos * (v.params.w * v.params.z);
    var o: Out;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.color = vec4<f32>(v.color.rgb * 2.0, v.color.a); // emissive, as the packet material was
    return o;
}

// ---- Status rings ----------------------------------------------------------------------------
// One instance per machine. The vertex is a unit ring in the ground plane (x, 0, z), inner radius
// 1.1 / 1.3 of the outer; the instance scales it. centre: xyz at the ring's place, w the outer
// radius. pulse: x rate (Hz), y phase (0..1), z online (1 pulses, 0 holds w), w held intensity
// (offline reads 0.2).

struct RingIn {
    @location(0) pos: vec3<f32>,
    @location(1) centre: vec4<f32>,
    @location(2) pulse: vec4<f32>,
    @location(3) color: vec4<f32>,
}

@vertex
fn vs_ring(v: RingIn) -> Out {
    // fleet.ts paintRing: online, the opacity pulses 0.6 to 0.9; otherwise it holds at pulse.w.
    let wave = 0.6 + 0.3 * (0.5 + 0.5 * sin(TAU * (g.time.x * v.pulse.x + v.pulse.y)));
    let alpha = select(v.pulse.w, wave, v.pulse.z > 0.5);
    // The band runs from the footprint (centre.w) plus 0.8 to plus 1.2: pos.y is 0 on its inner edge, 1 on its outer.
    let radius = v.centre.w + 0.8 + 0.4 * v.pos.y;
    let world = v.centre.xyz + vec3<f32>(v.pos.x, 0.0, v.pos.z) * radius;
    var o: Out;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.color = vec4<f32>(v.color.rgb, alpha);
    return o;
}

// ---- LEDs ------------------------------------------------------------------------------------
// One instance per LED. The vertex is a unit-sphere vertex scaled by centre.w (the radius).
// blink: x rate (Hz), y phase (0..1), z duty (fraction of each cycle lit). An LED is lit while
// fract(time * rate + phase) < duty; unlit, it collapses to a point.

struct LedIn {
    @location(0) pos: vec3<f32>,
    @location(1) centre: vec4<f32>,
    @location(2) blink: vec4<f32>,
    @location(3) color: vec4<f32>,
}

@vertex
fn vs_led(v: LedIn) -> Out {
    let lit = select(0.0, 1.0, fract(g.time.x * v.blink.x + v.blink.y) < v.blink.z);
    var o: Out;
    o.clip = g.view_proj * vec4<f32>(v.centre.xyz + v.pos * (v.centre.w * lit), 1.0);
    o.color = v.color;
    return o;
}
