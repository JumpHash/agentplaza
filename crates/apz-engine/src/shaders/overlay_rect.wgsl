@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
struct Out { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32, @location(0) rect: vec4<f32>) -> Out {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[i];
    var o: Out;
    o.clip = vec4<f32>(mix(rect.x, rect.z, c.x), mix(rect.y, rect.w, c.y), 0.0, 1.0);
    o.uv = c;
    return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4<f32> { return textureSample(tex, smp, o.uv); }
