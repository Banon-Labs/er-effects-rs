// FIX verification: SIX constant buffers at the ER shader's SPARSE bindings
// {4,5,8,10,11,12}. The harness compacts these to {0..5} before drawing; green readback
// means the sparse->contiguous remap fixes the lavapipe descriptor-null failure.
cbuffer C4 : register(b4) { float4 c4; }
cbuffer C5 : register(b5) { float4 c5; }
cbuffer C8 : register(b8) { float4 c8; }
cbuffer C10 : register(b10) { float4 c10; }
cbuffer C11 : register(b11) { float4 c11; }
cbuffer C12 : register(b12) { float4 c12; }

struct VSOut {
    float4 pos : SV_Position;
    float4 col : COLOR;
};
VSOut main(uint vid : SV_VertexID) {
    VSOut o;
    float2 p = float2((vid << 1) & 2, vid & 2);
    o.pos = float4(p * 2.0 - 1.0, 0.0, 1.0);
    o.col = c4 + (c5 + c8 + c10 + c11 + c12) * 1e-9;
    return o;
}
