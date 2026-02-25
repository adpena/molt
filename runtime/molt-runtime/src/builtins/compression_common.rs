use crate::builtins::numbers::int_bits_from_i64;

const COMPRESSION_STREAMS_BUFFER_SIZE: i64 = 8192;

#[unsafe(no_mangle)]
pub extern "C" fn molt_compression_streams_buffer_size() -> u64 {
    crate::with_gil_entry!(_py, {
        int_bits_from_i64(_py, COMPRESSION_STREAMS_BUFFER_SIZE)
    })
}
