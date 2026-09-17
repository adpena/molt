//! Canonical fixed-layout Python cell storage.
//!
//! A cell owns exactly one Molt value. The runtime missing sentinel represents
//! an empty cell; cells are not sequences and never share list storage.

use super::{MoltHeader, alloc_object, object_type_id};
use crate::{
    PyToken, TYPE_ID_CELL, TYPE_ID_TUPLE, dec_ref_bits, inc_ref_bits, obj_from_bits,
    raise_exception,
};
use molt_obj_model::MoltObject;

const CELL_VALUE_OFFSET: usize = 0;
const CELL_TOTAL_SIZE: usize = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();

pub(crate) struct CellTupleShape {
    pub(crate) len: usize,
    pub(crate) first_non_cell: Option<u64>,
}

/// Inspect immutable tuple storage without allocating or copying its edges.
/// The closure construction lanes invoke no Python callbacks while consuming
/// this result, so the tuple owner's existing reference keeps every borrowed
/// value live for the duration of validation.
pub(crate) unsafe fn inspect_cell_tuple(bits: u64) -> Option<CellTupleShape> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return None;
    }
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| CellTupleShape {
            len: items.len(),
            first_non_cell: items
                .iter()
                .copied()
                .find(|item| cell_ptr_from_bits(*item).is_none()),
        })
    }
}

pub(crate) struct CellValuePin<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
}

impl CellValuePin<'_, '_> {
    #[inline]
    pub(crate) fn bits(&self) -> u64 {
        self.bits
    }
}

impl Drop for CellValuePin<'_, '_> {
    fn drop(&mut self) {
        dec_ref_bits(self.py, self.bits);
    }
}

#[inline]
pub(crate) fn cell_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    obj_from_bits(bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_CELL })
}

#[inline]
pub(crate) unsafe fn cell_value_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(CELL_VALUE_OFFSET) as *const u64) }
}

#[inline]
pub(crate) unsafe fn pin_cell_value<'a, 'py>(
    py: &'a PyToken<'py>,
    ptr: *mut u8,
) -> CellValuePin<'a, 'py> {
    let bits = unsafe { cell_value_bits(ptr) };
    inc_ref_bits(py, bits);
    CellValuePin { py, bits }
}

/// Replace the single owned value. The incoming edge is retained and
/// published before the old value is released so finalizer reentry observes
/// the new cell state.
#[inline]
pub(crate) unsafe fn cell_replace_value(_py: &PyToken<'_>, ptr: *mut u8, value_bits: u64) {
    unsafe {
        crate::gil_assert();
        inc_ref_bits(_py, value_bits);
        let slot = ptr.add(CELL_VALUE_OFFSET) as *mut u64;
        let old_bits = slot.replace(value_bits);
        dec_ref_bits(_py, old_bits);
    }
}

/// Publish an empty cell and transfer the old owned edge to lifecycle custody.
#[inline]
pub(crate) unsafe fn cell_detach_value(ptr: *mut u8, missing_bits: u64) -> u64 {
    unsafe { (ptr.add(CELL_VALUE_OFFSET) as *mut u64).replace(missing_bits) }
}

pub(crate) fn alloc_cell(_py: &PyToken<'_>, value_bits: u64) -> *mut u8 {
    let ptr = alloc_object(_py, CELL_TOTAL_SIZE, TYPE_ID_CELL);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        inc_ref_bits(_py, value_bits);
        *(ptr.add(CELL_VALUE_OFFSET) as *mut u64) = value_bits;
    }
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cell_new(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_cell(_py, value_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cell_get(cell_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = cell_ptr_from_bits(cell_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected cell");
        };
        let value_bits = unsafe { cell_value_bits(ptr) };
        inc_ref_bits(_py, value_bits);
        value_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cell_set(cell_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = cell_ptr_from_bits(cell_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected cell");
        };
        unsafe { cell_replace_value(_py, ptr, value_bits) };
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_cycle_is_visited_and_cleared_by_canonical_lifecycle() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let ptr = alloc_cell(py, crate::missing_bits(py));
            assert!(!ptr.is_null());
            let bits = MoltObject::from_ptr(ptr).bits();
            let baseline = unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() };

            unsafe { cell_replace_value(py, ptr, bits) };
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() },
                baseline + 1,
            );
            let mut visited = Vec::new();
            unsafe {
                crate::object::heap_lifecycle::visit_owned_values(py, ptr, &mut |edge| {
                    visited.push(edge)
                });
            }
            assert_eq!(visited, [bits]);

            unsafe { crate::object::heap_lifecycle::clear_cycle_edges(py, ptr) };
            assert!(crate::is_missing_bits(py, unsafe { cell_value_bits(ptr) }));
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() },
                baseline,
            );
            dec_ref_bits(py, bits);
        });
    }
}
