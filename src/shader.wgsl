struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) in_vertex_index: u32,
) -> VertexOutput {
    var out: VertexOutput;
    let uv = vec2<f32>(
        f32((in_vertex_index << 1u) & 2u),
        f32(in_vertex_index & 2u)
    );
    out.clip_position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.clip_position.y = out.clip_position.y * -1.0;
    out.tex_coords = uv;
    return out;
}

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0) @binding(1)
var s_diffuse: sampler;

// 辅助函数：计算感知亮度 (人眼对绿色更敏感)
fn getLuma(color: vec3f) -> f32 {
    return dot(color, vec3f(0.2126, 0.7152, 0.0722));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(t_diffuse, s_diffuse, in.tex_coords);
    let originalRGB = color.rgb;
    
    let luma = getLuma(originalRGB);
    
    let liftedRGB = pow(originalRGB, vec3f(0.75)); 
    
    let protectFactor = smoothstep(0.05, 0.3, luma);
    
    let finalRGB = mix(liftedRGB, originalRGB, protectFactor);
    
    return vec4f(finalRGB, 1.0);
}