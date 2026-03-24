use crate::bridge::*;

const COMPRESSION_STREAMS_BUFFER_SIZE: i64 = 8192;
pub extern "C" fn molt_compression_streams_buffer_size() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        int_bits_from_i64(_py, COMPRESSION_STREAMS_BUFFER_SIZE)
    })
}
