// Clean-room pixel shader: just pass through the interpolated colour the VERTEX stage
// read from its constant buffer. Green readback => vertex-stage UBO bound correctly.
struct PSIn {
    float4 pos : SV_Position;
    float4 col : COLOR;
};
float4 main(PSIn i) : SV_Target {
    return i.col;
}
