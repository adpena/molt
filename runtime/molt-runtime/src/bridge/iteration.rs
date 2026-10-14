use molt_runtime_core::prelude::*;

use super::ExceptionSentinel;

pub fn molt_iter_bridge(_py: &CoreGilToken, bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter(bits)
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
