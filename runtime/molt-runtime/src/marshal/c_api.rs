//! Typed C-ABI hook transport; all wire semantics remain in read/write.
use super::*;
use molt_cpython_abi::hooks::{MarshalInputOwnership, OwnedHandleResult};

pub(crate) unsafe extern "C" fn write(
    bits: u64,
    version: i32,
    ownership: MarshalInputOwnership,
) -> OwnedHandleResult {
    crate::with_gil_entry_nopanic!(py, {
        let result = super::write::encode_with_ownership(py, bits, version, ownership)
            .and_then(|bytes| allocated(py, alloc_bytes(py, &bytes)));
        match result {
            Ok(value) => OwnedHandleResult::ok(value.into_bits()),
            Err(()) => OwnedHandleResult::error(),
        }
    })
}

pub(crate) unsafe extern "C" fn read(data: *const u8, length: usize) -> OwnedHandleResult {
    crate::with_gil_entry_nopanic!(py, {
        let bytes = if length == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(data, length) }
        };
        match super::read::decode(py, super::read::Source::memory(bytes)) {
            Ok(value) => OwnedHandleResult::ok(value.into_bits()),
            Err(()) => OwnedHandleResult::error(),
        }
    })
}
