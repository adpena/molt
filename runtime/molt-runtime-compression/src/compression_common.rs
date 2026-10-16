use crate::bridge::*;
use molt_runtime_core::obj_from_bits;

const COMPRESSION_STREAMS_BUFFER_SIZE: i64 = 8192;
#[unsafe(no_mangle)]
pub extern "C" fn molt_compression_streams_buffer_size() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        int_bits_from_i64(_py, COMPRESSION_STREAMS_BUFFER_SIZE)
    })
}

/// CPython 3.12 Clinic's explicit contiguous-buffer check. Later supported
/// versions rely on the SIMPLE provider contract instead. Geometry belongs to
/// the existing ABI descriptor; this helper owns only the argument diagnostic.
pub(crate) fn validate_clinic_buffer(
    py: &molt_runtime_core::PyToken,
    input: &molt_runtime_core::native_custody::ByteInput<'_>,
    bits: u64,
    function: &str,
    argument: &str,
) -> bool {
    if molt_runtime_core::rt_target_at_least(3, 13) || input.is_c_contiguous() {
        return true;
    }
    let name = type_name(py, obj_from_bits(bits));
    let message = format!("{function}() {argument} must be contiguous buffer, not {name}");
    raise_exception(py, "TypeError", &message);
    false
}
