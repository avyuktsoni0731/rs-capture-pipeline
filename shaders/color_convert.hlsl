// BGRA (WGC) → NV12-style planes: full-size Y (R8), half-size interleaved UV (RG8).
// BT.709-style luma/chroma, 8-bit limited range (Y 16–235, Cb/Cr 16–240).

Texture2D<float4> InputBGRA : register(t0);
RWTexture2D<uint> OutputY : register(u0);
RWTexture2D<uint2> OutputUV : register(u1);

static const float3 KrKb = float3(0.2126f, 0.7152f, 0.0722f);

float3 LoadRgb(uint2 p, uint w, uint h)
{
    p.x = min(p.x, w - 1);
    p.y = min(p.y, h - 1);
    float4 px = InputBGRA.Load(int3(p, 0));
    // B8G8R8A8_UNORM memory order B,G,R,A → HLSL .rgba reads .x=B, .y=G, .z=R
    float b = px.r;
    float g = px.g;
    float r = px.b;
    return float3(r, g, b);
}

[numthreads(16, 16, 1)]
void CSMain(uint3 id : SV_DispatchThreadID)
{
    uint w, h;
    InputBGRA.GetDimensions(w, h);
    if (id.x >= w || id.y >= h)
        return;

    float3 rgb = LoadRgb(id.xy, w, h);
    float Y = dot(rgb, KrKb);
    float y_byte = 16.0f + 219.0f * Y;
    OutputY[id.xy] = (uint) clamp(y_byte, 16.0f, 235.0f);

    if ((id.x & 1u) != 0 || (id.y & 1u) != 0)
        return;

    float3 c00 = LoadRgb(id.xy + uint2(0, 0), w, h);
    float3 c10 = LoadRgb(id.xy + uint2(1, 0), w, h);
    float3 c01 = LoadRgb(id.xy + uint2(0, 1), w, h);
    float3 c11 = LoadRgb(id.xy + uint2(1, 1), w, h);
    float3 rgbAvg = (c00 + c10 + c01 + c11) * 0.25f;

    float Yavg = dot(rgbAvg, KrKb);
    float cb = 128.0f + 112.0f * (rgbAvg.b - Yavg) / (1.0f - 0.0722f);
    float cr = 128.0f + 112.0f * (rgbAvg.r - Yavg) / (1.0f - 0.2126f);
    cb = clamp(cb, 16.0f, 240.0f);
    cr = clamp(cr, 16.0f, 240.0f);

    uint2 coordUv = uint2(id.x / 2, id.y / 2);
    OutputUV[coordUv] = uint2((uint)cb, (uint)cr);
}
