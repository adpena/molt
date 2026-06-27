use molt_runtime_core::prelude::*;

use super::ExceptionSentinel;

pub fn molt_iter(_py: &CoreGilToken, bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter(bits)
}

pub fn molt_iter_bridge(_py: &CoreGilToken, bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter(bits)
}

pub fn bridge_molt_iter_next(_py: &CoreGilToken, iter_bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter_next(iter_bits)
}

pub fn molt_iter_next(_py: &CoreGilToken, iter_bits: u64) -> Option<u64> {
    let result = crate::object::ops_iter::molt_iter_next(iter_bits);
    if result == MoltObject::none().bits() {
        crate::with_gil_entry_nopanic!(py, {
            if crate::exception_pending(py) {
                None
            } else {
                Some(result)
            }
        })
    } else {
        Some(result)
    }
}

pub fn raise_not_iterable<T: ExceptionSentinel>(_py: &CoreGilToken, bits: u64) -> T {
    crate::with_gil_entry_nopanic!(py, {
        T::from_bits(crate::raise_not_iterable::<u64>(py, bits))
    })
}

pub fn tuple_from_iter_bits(_py: &CoreGilToken, iter_bits: u64) -> Option<u64> {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::tuple_from_iter_bits(py, iter_bits) }
    })
}
