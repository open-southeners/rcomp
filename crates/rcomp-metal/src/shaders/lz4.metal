#include <metal_stdlib>

using namespace metal;

constant uint HASH_LOG = 12u;
constant uint HASH_SIZE = 1u << HASH_LOG;
constant uint INVALID_POSITION = 0xffffffffu;
constant uint OUTPUT_ERROR = 0xffffffffu;

struct Lz4Parameters {
    uint total_size;
    uint block_size;
    uint output_slot_size;
    uint block_count;
};

inline uint read_u32(const device uchar *source, uint position)
{
    return static_cast<uint>(source[position])
        | (static_cast<uint>(source[position + 1u]) << 8u)
        | (static_cast<uint>(source[position + 2u]) << 16u)
        | (static_cast<uint>(source[position + 3u]) << 24u);
}

inline bool emit_byte(device uchar *destination, thread uint &position,
                      uint capacity, uchar value)
{
    if (position >= capacity) {
        return false;
    }
    destination[position++] = value;
    return true;
}

inline bool emit_length(device uchar *destination, thread uint &position,
                        uint capacity, uint length)
{
    while (length >= 255u) {
        if (!emit_byte(destination, position, capacity, 255u)) {
            return false;
        }
        length -= 255u;
    }
    return emit_byte(destination, position, capacity, static_cast<uchar>(length));
}

kernel void rcomp_lz4_compress_blocks(
    const device uchar *input [[buffer(0)]],
    device uchar *output_slots [[buffer(1)]],
    device uint *output_sizes [[buffer(2)]],
    device uint *hash_tables [[buffer(3)]],
    constant Lz4Parameters &parameters [[buffer(4)]],
    uint block_index [[thread_position_in_grid]])
{
    if (block_index >= parameters.block_count) {
        return;
    }

    const uint input_start = block_index * parameters.block_size;
    const uint input_length = min(parameters.block_size,
                                  parameters.total_size - input_start);
    const device uchar *source = input + input_start;
    const ulong output_offset = static_cast<ulong>(block_index)
        * static_cast<ulong>(parameters.output_slot_size);
    device uchar *destination = output_slots + output_offset;
    device uint *hash_table = hash_tables
        + static_cast<ulong>(block_index) * static_cast<ulong>(HASH_SIZE);

    for (uint index = 0u; index < HASH_SIZE; ++index) {
        hash_table[index] = INVALID_POSITION;
    }

    uint output_position = 0u;
    uint anchor = 0u;
    uint input_position = 0u;
    const uint match_find_limit = input_length > 12u ? input_length - 12u : 0u;
    const uint match_copy_limit = input_length > 5u ? input_length - 5u : 0u;

    while (input_length >= 12u && input_position <= match_find_limit) {
        const uint sequence = read_u32(source, input_position);
        const uint hash = (sequence * 2654435761u) >> (32u - HASH_LOG);
        const uint reference = hash_table[hash];
        hash_table[hash] = input_position;

        if (reference == INVALID_POSITION
            || reference >= input_position
            || input_position - reference > 65535u
            || read_u32(source, reference) != sequence) {
            ++input_position;
            continue;
        }

        uint match_length = 4u;
        while (input_position + match_length < match_copy_limit
               && source[reference + match_length]
                    == source[input_position + match_length]) {
            ++match_length;
        }

        const uint literal_length = input_position - anchor;
        const uint match_code = match_length - 4u;
        const uint token_position = output_position;
        if (!emit_byte(destination, output_position,
                       parameters.output_slot_size, 0u)) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }
        destination[token_position] = static_cast<uchar>(
            (min(literal_length, 15u) << 4u) | min(match_code, 15u));

        if (literal_length >= 15u
            && !emit_length(destination, output_position,
                            parameters.output_slot_size,
                            literal_length - 15u)) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }
        for (uint index = 0u; index < literal_length; ++index) {
            if (!emit_byte(destination, output_position,
                           parameters.output_slot_size,
                           source[anchor + index])) {
                output_sizes[block_index] = OUTPUT_ERROR;
                return;
            }
        }

        const uint match_offset = input_position - reference;
        if (!emit_byte(destination, output_position,
                       parameters.output_slot_size,
                       static_cast<uchar>(match_offset & 0xffu))
            || !emit_byte(destination, output_position,
                          parameters.output_slot_size,
                          static_cast<uchar>(match_offset >> 8u))) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }
        if (match_code >= 15u
            && !emit_length(destination, output_position,
                            parameters.output_slot_size, match_code - 15u)) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }

        input_position += match_length;
        anchor = input_position;
    }

    const uint literal_length = input_length - anchor;
    if (!emit_byte(destination, output_position,
                   parameters.output_slot_size,
                   static_cast<uchar>(min(literal_length, 15u) << 4u))) {
        output_sizes[block_index] = OUTPUT_ERROR;
        return;
    }
    if (literal_length >= 15u
        && !emit_length(destination, output_position,
                        parameters.output_slot_size, literal_length - 15u)) {
        output_sizes[block_index] = OUTPUT_ERROR;
        return;
    }
    for (uint index = 0u; index < literal_length; ++index) {
        if (!emit_byte(destination, output_position,
                       parameters.output_slot_size,
                       source[anchor + index])) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }
    }

    output_sizes[block_index] = output_position;
}
