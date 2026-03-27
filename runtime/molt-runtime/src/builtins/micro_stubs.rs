//! Stub exports for functions gated behind optional features.
//! These exist so the linker can resolve all imports even in micro builds.
//! Calling any of these at runtime raises a NotImplementedError.

#[cfg(not(feature = "stdlib_serialization"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cbor_parse_scalar(_ptr: *const u8, _len: u64, _out: *mut u64) -> i32 {
    -1
}

#[cfg(not(feature = "stdlib_serialization"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_cbor_parse_scalar_obj(_obj: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        crate::raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "cbor requires stdlib_serialization feature",
        )
    })
}

#[cfg(not(feature = "stdlib_serialization"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_msgpack_parse_scalar(
    _ptr: *const u8,
    _len: u64,
    _out: *mut u64,
) -> i32 {
    -1
}

#[cfg(not(feature = "stdlib_serialization"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_msgpack_parse_scalar_obj(_obj: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        crate::raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "msgpack requires stdlib_serialization feature",
        )
    })
}

#[cfg(not(feature = "stdlib_compression"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_deflate_raw(_input: u64, _level: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        crate::raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "deflate requires stdlib_compression feature",
        )
    })
}

#[cfg(not(feature = "stdlib_compression"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_inflate_raw(_input: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        crate::raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "inflate requires stdlib_compression feature",
        )
    })
}
