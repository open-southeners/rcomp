#include <metal_stdlib>

using namespace metal;

kernel void rcomp_compute_smoke(
    const device uint *input [[buffer(0)]],
    device uint *output [[buffer(1)]],
    constant uint &element_count [[buffer(2)]],
    uint position [[thread_position_in_grid]])
{
    if (position < element_count) {
        output[position] = input[position] ^ 0xa5a5a5a5u;
    }
}

