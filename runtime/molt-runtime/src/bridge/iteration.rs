use molt_runtime_core::prelude::*;

use super::ExceptionSentinel;

pub fn molt_iter_bridge(_py: &CoreGilToken, bits: u64) -> u64 {
    let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
    crate::object::ops_iter::molt_iter_attached_body(py, bits)
}

pub fn raise_not_iterable<T: ExceptionSentinel>(_py: &CoreGilToken, bits: u64) -> T {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        T::from_bits(crate::raise_not_iterable::<u64>(py, bits))
    }
}

pub fn tuple_from_iter_bits(_py: &CoreGilToken, iter_bits: u64) -> Option<u64> {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        unsafe { crate::tuple_from_iter_bits(py, iter_bits) }
    }
}
