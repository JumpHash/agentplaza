// The backdrop: one full-screen triangle that paints a theme's background spec under the consoles. The same
// picture is written on the CPU in backdrop.rs (render_cpu); keep the two in step. Coordinates are device pixels
// (fragment space) unless a name says css. Only f32, u32 and vec4 math, one uniform block: WebGL2 safe.

struct Uniforms {
    // w, h (target size in device px), scale (device px per css px), time (s).
    size: vec4<f32>,
    // x, y (pointer in css px), on (0 or 1), the sidebar's width (css px; the second field sits by it).
    pointer: vec4<f32>,
    // The ground colour (token "panel", the main column's ground).
    ground: vec4<f32>,
    // The pattern colour (token "pattern", or "amb-line" with its alpha for the grid).
    ink: vec4<f32>,
    // The pointer's light colour (token "signal").
    signal: vec4<f32>,
    // The three colour fields; a missing field is the ground.
    field0: vec4<f32>,
    field1: vec4<f32>,
    field2: vec4<f32>,
    // pattern id (0 none, 1 dots, 2 weave, 3 grid, 4 blueprint, 5 contours), cell (device px), strength, grain.
    params: vec4<f32>,
    // ornament id (not drawn here), motion (0 or 1), pointer light on (0 or 1), top fade (css px).
    params2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// How far the grain's noise strays from mid grey (the prototype's feTurbulence is soft, not white noise).
const GRAIN_SPREAD: f32 = 0.35;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    // One triangle that covers the viewport: (-1,-1), (3,-1), (-1,3).
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    var out: VsOut;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

// Coverage of a line centred on distance `d` from it, `w` device px wide.
fn line_cov(d: f32, w: f32) -> f32 {
    return clamp(w * 0.5 + 0.5 - d, 0.0, 1.0);
}

// The nearest multiple of `cell` to `v`.
fn nearest_line(v: f32, cell: f32) -> f32 {
    return floor(v / cell + 0.5) * cell;
}

// Distance from `v` to the nearest multiple of `cell`.
fn line_dist(v: f32, cell: f32) -> f32 {
    return abs(v - nearest_line(v, cell));
}

// Coverage of the pattern at `p` (device px), 0..1.
fn pattern_cover(p: vec2<f32>, kind: i32, cell: f32, lw: f32, s: f32) -> f32 {
    if (kind == 1) {
        // Dots (Loom's `.amb-pat`): a dot at each cell's centre, solid to 1 css px and gone at 1.2.
        let c = (floor(p / cell) + vec2<f32>(0.5, 0.5)) * cell;
        let d = distance(p, c) / s;
        return clamp((1.2 - d) / 0.2, 0.0, 1.0);
    }
    if (kind == 2) {
        // Weave: horizontal and vertical threads every cell, alternating over and under by cell parity.
        let h = line_cov(line_dist(p.y, cell), lw) * 0.5;
        let v = line_cov(line_dist(p.x, cell), lw) * 0.5;
        let cx = i32(floor(p.x / cell));
        let cy = i32(floor(p.y / cell));
        if (((cx + cy) & 1) == 0) {
            return max(h, v * (1.0 - h * 2.0));
        }
        return max(v, h * (1.0 - v * 2.0));
    }
    if (kind == 3) {
        // Grid (Graphite's `.amb-pat`): a 1 css px line at the start of every cell, both ways, under an elliptical
        // mask from the top (130% by 95% of the window at 62% across: solid to 18%, gone at 72%).
        let q = p / s;
        let cs = cell / s;
        let g = select(0.0, 1.0, (q.x % cs) < 1.0 || (q.y % cs) < 1.0);
        let wc = u.size.x / s;
        let hc = u.size.y / s;
        let e = length(vec2<f32>((q.x - 0.62 * wc) / (1.3 * wc), q.y / (0.95 * hc)));
        let mask = 1.0 - clamp((e - 0.18) / (0.72 - 0.18), 0.0, 1.0);
        return g * mask;
    }
    if (kind == 4) {
        // Blueprint: a fine grid at a quarter cell, and major lines at the cell.
        let fine_cell = cell * 0.25;
        let fine = max(line_cov(line_dist(p.x, fine_cell), lw), line_cov(line_dist(p.y, fine_cell), lw));
        let major = max(line_cov(line_dist(p.x, cell), lw), line_cov(line_dist(p.y, cell), lw));
        return max(fine * 0.35, major * 0.6);
    }
    if (kind == 5) {
        // Contours: iso-lines of a smooth sum of sines, one every 0.1 of the level.
        let q = p / cell;
        let level = (sin(q.x * 0.37) + sin(q.y * 0.29) + sin((q.x + q.y) * 0.19) + sin((q.x - q.y) * 0.23)) * 0.125 + 0.5;
        let t = fract(level * 10.0);
        return 0.6 * clamp(1.0 - min(t, 1.0 - t) / 0.05, 0.0, 1.0);
    }
    return 0.0;
}

// The prototype's colour fields (the Loom and Graphite consoles' `.amb-f1..3`): soft discs sized and placed in vmax
// (the window's longer side, css px), each a radial gradient from `pct` of its colour at the centre to nothing at its
// edge. With motion on, each drifts to its far place and back (ease in and out, 46 s, 61 s, 73 s).
fn ease(x: f32) -> f32 {
    return x * x * (3.0 - 2.0 * x);
}

fn drift(t: f32, period: f32) -> f32 {
    let u = (t / period) % 2.0;
    let tri = select(2.0 - u, u, u < 1.0);
    return ease(clamp(tri, 0.0, 1.0));
}

// The alpha of field `idx` at `p` (css px) in a `w` by `h` css px window with the sidebar `side` css px wide.
fn field_alpha(p: vec2<f32>, idx: i32, w: f32, h: f32, side: f32, t: f32, mot: f32) -> f32 {
    let v = max(w, h) * 0.01;
    var c = vec2<f32>(0.0, 0.0);
    var r = 0.0;
    var pct = 0.0;
    var to = vec3<f32>(0.0, 0.0, 1.0);
    var period = 1.0;
    if (idx == 0) {
        c = vec2<f32>(w - 12.0 * v, -2.0 * v);
        r = 32.0 * v;
        pct = 0.24;
        to = vec3<f32>(-16.0 * v, 9.0 * v, 1.12);
        period = 46.0;
    } else if (idx == 1) {
        c = vec2<f32>(side + 14.0 * v, -4.0 * v);
        r = 26.0 * v;
        pct = 0.18;
        to = vec3<f32>(20.0 * v, 7.0 * v, 0.88);
        period = 61.0;
    } else {
        c = vec2<f32>(w - 22.0 * v, h + 14.0 * v);
        r = 36.0 * v;
        pct = 0.15;
        to = vec3<f32>(-22.0 * v, -10.0 * v, 1.08);
        period = 73.0;
    }
    let k = mot * drift(t, period);
    c = c + to.xy * k;
    r = r * (1.0 + (to.z - 1.0) * k);
    let d = distance(p, c) / r;
    return pct * clamp(1.0 - d, 0.0, 1.0);
}

// A hash of an integer pixel coordinate and a frame seed, 0..1 (integer maths, so it matches on every GPU).
fn hash01(x: u32, y: u32, s: u32) -> f32 {
    var h: u32 = ((x * 0x27d4eb2du) ^ (y * 0x165667b1u)) ^ (s * 0x9e3779b9u);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = (h ^ (h >> 12u)) * 0x297a2d39u;
    h = h ^ (h >> 15u);
    return f32(h & 0xffffu) / 65535.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.pos.xy;
    let size = u.size.xy;
    let s = u.size.z;
    let t = u.size.w;
    let mot = u.params2.y;
    let lw = max(s, 1.0);
    let pc = p / s;
    let w = size.x / s;
    let h = size.y / s;

    // The panel, then the three fields over it, each at its strength (the skin's `--amb-k`).
    var col = u.ground.rgb;
    let strength = u.params.z;
    col = mix(col, u.field0.rgb, strength * field_alpha(pc, 0, w, h, u.pointer.w, t, mot));
    col = mix(col, u.field1.rgb, strength * field_alpha(pc, 1, w, h, u.pointer.w, t, mot));
    col = mix(col, u.field2.rgb, strength * field_alpha(pc, 2, w, h, u.pointer.w, t, mot));

    let kind = i32(u.params.x + 0.5);
    if (kind != 0) {
        var ink = u.ink.rgb;
        if (u.pointer.z > 0.5 && u.params2.z > 0.5) {
            let pp = u.pointer.xy * s;
            let light = 1.0 - smoothstep(0.0, 260.0 * s, distance(p, pp));
            ink = mix(ink, u.signal.rgb, 0.7 * light);
        }
        col = mix(col, ink, pattern_cover(p, kind, u.params.y, lw, s) * u.ink.a);
    }

    // Film grain: the prototype's noise at `grain` opacity, inverted on dark grounds; re-seeded 12 times a second
    // while the backdrop animates.
    let anim = max(mot, u.pointer.z * u.params2.z);
    var seed = 0.0;
    if (anim > 0.5) {
        seed = floor(t * 12.0);
    }
    var n = hash01(u32(floor(p.x / s)), u32(floor(p.y / s)), u32(max(seed, 0.0)));
    if (dot(u.ground.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)) < 0.5) {
        n = 1.0 - n;
    }
    // The prototype's fractal noise sits close to mid grey: a third of the hash's spread.
    n = 0.5 + (n - 0.5) * GRAIN_SPREAD;
    col = mix(col, vec3<f32>(n), u.params.w);

    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
