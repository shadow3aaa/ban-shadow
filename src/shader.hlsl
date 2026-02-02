Texture2D t_diffuse : register(t0);
SamplerState s_diffuse : register(s0);

struct VSOut {
    float4 pos : SV_POSITION;
    float2 uv : TEXCOORD0;
};

VSOut vs_main(uint id : SV_VertexID) {
    float2 uv = float2((id << 1) & 2, id & 2);
    VSOut o;
    o.uv = uv;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

float4 ps_main(VSOut input) : SV_Target {
    float4 color = t_diffuse.Sample(s_diffuse, input.uv);
    float luma = dot(color.rgb, float3(0.2126, 0.7152, 0.0722));
    float3 lifted = pow(color.rgb, 0.75);
    float protect = smoothstep(0.05, 0.3, luma);
    float3 finalRgb = lerp(lifted, color.rgb, protect);
    return float4(finalRgb, 1.0);
}
