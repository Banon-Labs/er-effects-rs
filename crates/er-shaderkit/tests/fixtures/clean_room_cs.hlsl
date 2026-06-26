// Clean-room HLSL (no game assets). Provenance for clean_room_cs.dxil, compiled
// to DXIL Shader Model 6 with the project's dxc:
//   dxc -T cs_6_0 -E main -Fo clean_room_cs.dxil clean_room_cs.hlsl
// Used to test the DXIL -> SPIR-V translation path (dxil-spirv) deterministically
// without committing copyrighted Elden Ring shader bytecode.

RWStructuredBuffer<uint> Out : register(u0);

[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    // Read-modify-write so the UAV is read_write storage (naga rejects
    // write-only storage buffers).
    Out[id.x] = Out[id.x] * 2u + id.x;
}
