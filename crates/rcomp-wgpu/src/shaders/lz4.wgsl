const HASH_LOG: u32 = 12u;
const HASH_SIZE: u32 = 1u << HASH_LOG;
const INVALID_POSITION: u32 = 0xffffffffu;
const OUTPUT_ERROR: u32 = 0xffffffffu;

struct Parameters {
    total_size: u32,
    block_size: u32,
    output_slot_size: u32,
    output_slot_stride: u32,
    // Exclusive end of the blocks this submission compresses.
    block_end: u32,
    // First block of this submission. A batch is split into several
    // submissions so no single dispatch runs long enough to trip a driver
    // watchdog (Windows TDR).
    block_offset: u32,
    _padding_1: u32,
    _padding_2: u32,
};

@group(0) @binding(0)
var<storage, read> input_words: array<u32>;

@group(0) @binding(1)
var<storage, read_write> output_words: array<u32>;

@group(0) @binding(2)
var<storage, read_write> output_sizes: array<u32>;

@group(0) @binding(3)
var<storage, read_write> hash_tables: array<u32>;

@group(0) @binding(4)
var<uniform> parameters: Parameters;

fn read_input_byte(position: u32) -> u32 {
    let word = input_words[position >> 2u];
    let shift = (position & 3u) << 3u;
    return (word >> shift) & 0xffu;
}

fn read_input_u32(position: u32) -> u32 {
    return read_input_byte(position)
        | (read_input_byte(position + 1u) << 8u)
        | (read_input_byte(position + 2u) << 16u)
        | (read_input_byte(position + 3u) << 24u);
}

fn write_output_byte(position: u32, value: u32) {
    let word_index = position >> 2u;
    let shift = (position & 3u) << 3u;
    let mask = 0xffu << shift;
    output_words[word_index] =
        (output_words[word_index] & ~mask) | ((value & 0xffu) << shift);
}

fn emit_byte(
    destination_base: u32,
    position: ptr<function, u32>,
    capacity: u32,
    value: u32,
) -> bool {
    if (*position >= capacity) {
        return false;
    }
    write_output_byte(destination_base + *position, value);
    *position = *position + 1u;
    return true;
}

fn emit_length(
    destination_base: u32,
    position: ptr<function, u32>,
    capacity: u32,
    initial_length: u32,
) -> bool {
    var length = initial_length;
    while (length >= 255u) {
        if (!emit_byte(destination_base, position, capacity, 255u)) {
            return false;
        }
        length = length - 255u;
    }
    return emit_byte(destination_base, position, capacity, length);
}

@compute @workgroup_size(64)
fn rcomp_lz4_compress_blocks(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let block_index = parameters.block_offset + invocation.x;
    if (block_index >= parameters.block_end) {
        return;
    }

    let input_start = block_index * parameters.block_size;
    let input_length = min(parameters.block_size, parameters.total_size - input_start);
    let output_base = block_index * parameters.output_slot_stride;
    let hash_base = block_index * HASH_SIZE;

    for (var index = 0u; index < HASH_SIZE; index = index + 1u) {
        hash_tables[hash_base + index] = INVALID_POSITION;
    }

    var output_position = 0u;
    var anchor = 0u;
    var input_position = 0u;
    var match_find_limit = 0u;
    var match_copy_limit = 0u;
    if (input_length > 12u) {
        match_find_limit = input_length - 12u;
    }
    if (input_length > 5u) {
        match_copy_limit = input_length - 5u;
    }

    while (input_length >= 12u && input_position <= match_find_limit) {
        let sequence = read_input_u32(input_start + input_position);
        let hash = (sequence * 2654435761u) >> (32u - HASH_LOG);
        let reference = hash_tables[hash_base + hash];
        hash_tables[hash_base + hash] = input_position;

        if (reference == INVALID_POSITION
            || reference >= input_position
            || input_position - reference > 65535u
            || read_input_u32(input_start + reference) != sequence) {
            input_position = input_position + 1u;
            continue;
        }

        var match_length = 4u;
        while (input_position + match_length < match_copy_limit
            && read_input_byte(input_start + reference + match_length)
                == read_input_byte(input_start + input_position + match_length)) {
            match_length = match_length + 1u;
        }

        let literal_length = input_position - anchor;
        let match_code = match_length - 4u;
        let token_position = output_position;
        if (!emit_byte(
            output_base,
            &output_position,
            parameters.output_slot_size,
            0u,
        )) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }
        write_output_byte(
            output_base + token_position,
            (min(literal_length, 15u) << 4u) | min(match_code, 15u),
        );

        if (literal_length >= 15u
            && !emit_length(
                output_base,
                &output_position,
                parameters.output_slot_size,
                literal_length - 15u,
            )) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }
        for (var index = 0u; index < literal_length; index = index + 1u) {
            if (!emit_byte(
                output_base,
                &output_position,
                parameters.output_slot_size,
                read_input_byte(input_start + anchor + index),
            )) {
                output_sizes[block_index] = OUTPUT_ERROR;
                return;
            }
        }

        let match_offset = input_position - reference;
        if (!emit_byte(
            output_base,
            &output_position,
            parameters.output_slot_size,
            match_offset & 0xffu,
        ) || !emit_byte(
            output_base,
            &output_position,
            parameters.output_slot_size,
            match_offset >> 8u,
        )) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }
        if (match_code >= 15u
            && !emit_length(
                output_base,
                &output_position,
                parameters.output_slot_size,
                match_code - 15u,
            )) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }

        input_position = input_position + match_length;
        anchor = input_position;
    }

    let literal_length = input_length - anchor;
    if (!emit_byte(
        output_base,
        &output_position,
        parameters.output_slot_size,
        min(literal_length, 15u) << 4u,
    )) {
        output_sizes[block_index] = OUTPUT_ERROR;
        return;
    }
    if (literal_length >= 15u
        && !emit_length(
            output_base,
            &output_position,
            parameters.output_slot_size,
            literal_length - 15u,
        )) {
        output_sizes[block_index] = OUTPUT_ERROR;
        return;
    }
    for (var index = 0u; index < literal_length; index = index + 1u) {
        if (!emit_byte(
            output_base,
            &output_position,
            parameters.output_slot_size,
            read_input_byte(input_start + anchor + index),
        )) {
            output_sizes[block_index] = OUTPUT_ERROR;
            return;
        }
    }

    output_sizes[block_index] = output_position;
}
