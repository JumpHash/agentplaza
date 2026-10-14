// The Floor's shaders: three.js's MeshToonMaterial / MeshBasicMaterial / ShadowMaterial as the
// web floor uses them (physically based light units: Lambert = diffuse / PI), the ink hull
// outline, the painted sky dome, swaying grass tufts, dust points, and the shadow depth pass.

const PI: f32 = 3.141592653589793;

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
    // x: number of point lights; y: shadow map texel size.
    counts: vec4<f32>,
    // The fans' angles by fleet machine (hw_anim.wgsl reads them; here only for the layout).
    machine_angle: array<vec4<f32>, 32>,
    // Each fleet machine's state (render.rs `Globals`): z is its hover lift (units).
    machine_state: array<vec4<f32>, 128>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
@group(1) @binding(0) var map_tex: texture_2d<f32>;
@group(1) @binding(1) var map_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // The vertex's own colour: white, except in a batch of meshes that differ only in colour.
    @location(10) color: vec3<f32>,
    // The robot slot: 0 unchanged, 1 shell, 2 shell2, 3 panel, 4 glass (robot materials only).
    @location(11) slot: u32,
};

struct InstanceIn {
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
    // rgb: colour (material x instance); a: opacity.
    @location(7) color: vec4<f32>,
    // rgb: emissive x intensity; a: flags (1 receives shadow, 2 has a map, 4 two-sided).
    @location(8) emissive: vec4<f32>,
    // xy: uv repeat; zw: uv offset.
    @location(9) uv_xform: vec4<f32>,
    // rgb: the robot hues (shell, shell2, panel); used when the flags carry 8.
    @location(12) robot_shell: vec4<f32>,
    @location(13) robot_shell2: vec4<f32>,
    @location(14) robot_panel: vec4<f32>,
};

struct Varyings {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) emissive: vec4<f32>,
    @location(5) view_depth: f32,
    // x: grass tip shade (1 elsewhere).
    @location(6) extra: vec4<f32>,
    // Robot slots, flat (the vertex's own, not blended across a triangle): rgb: the slot's base
    // colour (hue x vertex colour) for slots 1 to 3; w: 1 when that applies. Zero elsewhere.
    @location(7) @interpolate(flat) robot: vec4<f32>,
    // 1 on a robot's glass (slot 4): no emissive.
    @location(8) @interpolate(flat) glass: f32,
};

fn model_of(i: InstanceIn) -> mat4x4<f32> {
    return mat4x4<f32>(i.m0, i.m1, i.m2, i.m3);
}

// The normal matrix of a translate-rotate-scale matrix: rotation x inverse scale squared.
fn world_normal(m: mat4x4<f32>, n: vec3<f32>) -> vec3<f32> {
    let c0 = m[0].xyz;
    let c1 = m[1].xyz;
    let c2 = m[2].xyz;
    let s = vec3<f32>(dot(c0, c0), dot(c1, c1), dot(c2, c2));
    return normalize(mat3x3<f32>(c0, c1, c2) * (n / max(s, vec3<f32>(1e-8))));
}

fn has(flags: f32, bit: f32) -> bool {
    return (u32(flags + 0.5) & u32(bit)) != 0u;
}

fn finish(v: Varyings, world: vec4<f32>, n: vec3<f32>, uv: vec2<f32>, i: InstanceIn) -> Varyings {
    var o = v;
    o.world = world.xyz;
    o.normal = n;
    o.uv = uv * i.uv_xform.xy + i.uv_xform.zw;
    o.color = i.color;
    o.emissive = i.emissive;
    o.view_depth = -(g.view * world).z;
    o.clip = g.view_proj * world;
    return o;
}

// The hover lift of a baked fleet machine's vertex: the slot above the robot byte is the machine slot + 1 (0: none).
fn lift(slot: u32) -> vec4<f32> {
    let k = slot >> 8u;
    if k == 0u {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(0.0, g.machine_state[min(k - 1u, 127u)].z, 0.0, 0.0);
}

@vertex
fn vs_mesh(v: VertexIn, i: InstanceIn) -> Varyings {
    let m = model_of(i);
    var o: Varyings;
    o.extra = vec4<f32>(1.0, 0.0, 0.0, 0.0);
    var r: Varyings;
    if has(i.emissive.a, 16.0) {
        // A billboard (three's Sprite): the quad on the camera's right and up axes, at the node's position and scale.
        let c = (m * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
        let right = vec3<f32>(g.view[0][0], g.view[1][0], g.view[2][0]);
        let up = vec3<f32>(g.view[0][1], g.view[1][1], g.view[2][1]);
        let toward = vec3<f32>(g.view[0][2], g.view[1][2], g.view[2][2]);
        let world = c + right * v.position.x * length(m[0].xyz) + up * v.position.y * length(m[1].xyz);
        r = finish(o, vec4<f32>(world, 1.0), toward, v.uv, i);
    } else {
        r = finish(o, m * vec4<f32>(v.position, 1.0) + lift(v.slot), world_normal(m, v.normal), v.uv, i);
    }
    let slot = v.slot & 0xffu;
    r.color = vec4<f32>(r.color.rgb * v.color, r.color.a);
    if has(i.emissive.a, 8.0) {
        // Slots 1 to 3 take the robot's hue (x the vertex colour); 4 is glass (no emissive).
        var hue = vec3<f32>(0.0);
        if slot == 1u {
            hue = i.robot_shell.rgb;
        } else if slot == 2u {
            hue = i.robot_shell2.rgb;
        } else if slot == 3u {
            hue = i.robot_panel.rgb;
        }
        let paint = slot >= 1u && slot <= 3u;
        r.robot = vec4<f32>(select(vec3<f32>(0.0), hue * v.color, paint), select(0.0, 1.0, paint));
        r.glass = select(0.0, 1.0, slot == 4u);
    }
    return r;
}

// Grass: each blade bends by its height, the wind a function of where the tuft stands.
@vertex
fn vs_grass(v: VertexIn, i: InstanceIn) -> Varyings {
    let m = model_of(i);
    let wp = m * vec4<f32>(0.0, 0.0, 0.0, 1.0);
    let t = g.eye.w;
    let sway = sin(t * 1.7 + wp.x * 0.35 + wp.z * 0.25) * 0.5 + sin(t * 3.1 + wp.x) * 0.15;
    var p = v.position;
    p.x += sway * v.position.y * 0.45;
    p.z += sway * v.position.y * 0.2;
    var o: Varyings;
    o.extra = vec4<f32>(0.75 + v.position.y * 1.1, 0.0, 0.0, 0.0);
    return finish(o, m * vec4<f32>(p, 1.0), world_normal(m, v.normal), v.uv, i);
}

// The ink hull: pushed out along its smooth normals by a width that grows with distance.
@vertex
fn vs_outline(v: VertexIn, i: InstanceIn) -> Varyings {
    let m = model_of(i);
    var mv = g.view * (m * vec4<f32>(v.position, 1.0) + lift(v.slot));
    let n = normalize((g.view * vec4<f32>(world_normal(m, v.normal), 0.0)).xyz);
    let grow = g.params.y * clamp(-mv.z * 0.06, 0.7, 4.0);
    mv = vec4<f32>(mv.xyz + n * grow, 1.0);
    var o: Varyings;
    o.clip = g.proj * mv;
    o.world = (m * vec4<f32>(v.position, 1.0) + lift(v.slot)).xyz;
    o.normal = n;
    o.uv = vec2<f32>(0.0);
    o.color = vec4<f32>(g.ink.rgb, 1.0);
    o.emissive = vec4<f32>(0.0);
    o.view_depth = -mv.z;
    o.extra = vec4<f32>(1.0, 0.0, 0.0, 0.0);
    return o;
}

// The sky dome: always at the far plane.
@vertex
fn vs_sky(v: VertexIn, i: InstanceIn) -> Varyings {
    let m = model_of(i);
    var o: Varyings;
    let world = m * vec4<f32>(v.position, 1.0);
    o.clip = g.view_proj * world;
    o.clip.z = o.clip.w;
    o.world = world.xyz;
    o.normal = normalize(v.position);
    o.uv = vec2<f32>(0.0);
    o.color = vec4<f32>(1.0);
    o.emissive = vec4<f32>(0.0);
    o.view_depth = 0.0;
    o.extra = vec4<f32>(1.0, 0.0, 0.0, 0.0);
    return o;
}

// Points: a square `size` world units wide at its depth (three's sizeAttenuation), from six
// vertices per instance.
@vertex
fn vs_points(@builtin(vertex_index) vi: u32, i: InstanceIn) -> Varyings {
    let m = model_of(i);
    let world = m * vec4<f32>(0.0, 0.0, 0.0, 1.0);
    let mv = g.view * world;
    let size_px = i.uv_xform.x * (g.params.z * 0.5) / max(-mv.z, 1e-3);
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    var clip = g.proj * mv;
    // Half the square in NDC; x is scaled by the aspect so the square stays square.
    let half_ndc = size_px / max(g.params.z, 1.0);
    clip.x += corners[vi].x * half_ndc * clip.w * (g.proj[0][0] / g.proj[1][1]);
    clip.y += corners[vi].y * half_ndc * clip.w;
    var o: Varyings;
    o.clip = clip;
    o.world = world.xyz;
    o.normal = vec3<f32>(0.0, 1.0, 0.0);
    o.uv = vec2<f32>(0.0);
    o.color = i.color;
    o.emissive = vec4<f32>(0.0);
    o.view_depth = -mv.z;
    o.extra = vec4<f32>(1.0, 0.0, 0.0, 0.0);
    return o;
}

@vertex
fn vs_shadow(v: VertexIn, i: InstanceIn) -> @builtin(position) vec4<f32> {
    return g.light_view_proj * (model_of(i) * vec4<f32>(v.position, 1.0) + lift(v.slot));
}

@vertex
fn vs_shadow_grass(v: VertexIn, i: InstanceIn) -> @builtin(position) vec4<f32> {
    return g.light_view_proj * model_of(i) * vec4<f32>(v.position, 1.0);
}

// ---- Lighting --------------------------------------------------------------------------------

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

fn base_color(v: Varyings) -> vec4<f32> {
    var c = v.color;
    if v.robot.w > 0.5 {
        c = vec4<f32>(v.robot.rgb, v.color.a);
    }
    // Sampled outside the branch: WebGPU requires textureSample in uniform control flow. The value is the same.
    let texel = textureSample(map_tex, map_sampler, v.uv);
    if has(v.emissive.a, 2.0) {
        c *= texel;
    }
    return c;
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

fn toon(v: Varyings, front: bool) -> vec4<f32> {
    var c = base_color(v);
    c = vec4<f32>(c.rgb * v.extra.x, c.a);
    var n = normalize(v.normal);
    if !front && has(v.emissive.a, 4.0) {
        n = -n;
    }
    var shadow = 1.0;
    if has(v.emissive.a, 1.0) {
        shadow = sun_shadow(v.world, n);
    }
    var irradiance = ramp(dot(n, g.sun_dir.xyz)) * g.sun_color.rgb * shadow;
    let n_points = i32(g.counts.x);
    for (var k = 0; k < n_points; k++) {
        let to_light = g.point_pos[k].xyz - v.world;
        let d = length(to_light);
        let l = to_light / max(d, 1e-5);
        irradiance += ramp(dot(n, l)) * g.point_color[k].rgb * attenuation(d, g.point_pos[k].w, g.point_color[k].w);
    }
    irradiance += mix(g.hemi_ground.rgb, g.hemi_sky.rgb, 0.5 * n.y + 0.5);
    // A robot's glass (slot 4) takes no emissive.
    let emissive = select(v.emissive.rgb, vec3<f32>(0.0), v.glass > 0.5);
    let lit = c.rgb / PI * irradiance + emissive;
    return vec4<f32>(encode(fogged(lit, v.view_depth)), c.a);
}

@fragment
fn fs_toon(v: Varyings, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return toon(v, front);
}

@fragment
fn fs_basic(v: Varyings) -> @location(0) vec4<f32> {
    let c = base_color(v);
    return vec4<f32>(encode(fogged(c.rgb, v.view_depth)), c.a);
}

// three's ShadowMaterial: only the shadow, as colour x opacity.
@fragment
fn fs_shadow_catcher(v: Varyings) -> @location(0) vec4<f32> {
    let lit = sun_shadow(v.world, normalize(v.normal));
    return vec4<f32>(encode(v.color.rgb), v.color.a * (1.0 - lit));
}

@fragment
fn fs_outline(v: Varyings) -> @location(0) vec4<f32> {
    // three's ink hull was a ShaderMaterial with no colour-space output: its linear ink reached the screen as it was.
    return vec4<f32>(raw(fogged(g.ink.rgb, v.view_depth)), 1.0);
}

// The painted sky: hard-edged bands near the horizon. three's sky was a ShaderMaterial with no colour-space output, so
// its linear colours reached the screen as they were (a darker night than the hexes): `raw` writes them the same way.
@fragment
fn fs_sky(v: Varyings) -> @location(0) vec4<f32> {
    let h = normalize(v.normal).y;
    var t = smoothstep(0.0, 0.55, h);
    t = floor(t * 6.0 + 0.5) / 6.0;
    var c = mix(g.sky_horizon.rgb, g.sky_top.rgb, t);
    c = mix(g.sky_ground.rgb, c, smoothstep(-0.05, 0.02, h));
    return vec4<f32>(raw(c), 1.0);
}

// A colour written to the screen as its stored values, with no colour-space encoding (three's ShaderMaterial without
// colorspace_fragment): on an sRGB target the hardware's encoding is undone first.
fn raw(c: vec3<f32>) -> vec3<f32> {
    if g.params.w >= 0.5 {
        return c;
    }
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}
