use molt_runtime_core::prelude::*;

pub fn index_i64_from_obj(_py: &CoreGilToken, obj_bits: u64, err: &str) -> i64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::builtins::numbers::index_i64_from_obj(py, obj_bits, err)
    }
}

pub fn bridge_molt_add(_py: &molt_runtime_core::PyToken, a: u64, b: u64) -> u64 {
    let __root_py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };

    crate::molt_add_attached_body(__root_py, a, b)
}

pub fn bridge_molt_eq(_py: &molt_runtime_core::PyToken, a: u64, b: u64) -> u64 {
    let __root_py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };

    crate::molt_eq_attached_body(__root_py, a, b)
}

pub fn bridge_callargs_new(_py: &molt_runtime_core::PyToken, pos_cap: u64, kw_cap: u64) -> u64 {
    let __root_py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };

    crate::molt_callargs_new_attached_body(__root_py, pos_cap, kw_cap)
}

pub fn bridge_callargs_expand_star(
    _py: &molt_runtime_core::PyToken,
    builder_bits: u64,
    iterable_bits: u64,
) -> u64 {
    let __root_py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };

    unsafe {
        crate::molt_callargs_expand_star_attached_body(__root_py, builder_bits, iterable_bits)
    }
}

pub fn bridge_call_bind(
    _py: &molt_runtime_core::PyToken,
    call_bits: u64,
    builder_bits: u64,
) -> u64 {
    let __root_py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };

    crate::molt_call_bind_attached_body(__root_py, call_bits, builder_bits)
}

/// # Safety
///
/// `ptr` must identify a live sequence and the caller must hold GIL custody.
pub unsafe fn seq_read_len(ptr: *mut u8) -> usize {
    crate::gil_assert();
    unsafe { crate::object::seq_access::len(ptr) }
}

/// Borrow one sequence item for immediate use under the caller's GIL custody.
///
/// # Safety
///
/// `ptr` must identify a live sequence for the duration of the borrow.
pub unsafe fn seq_read_item_gil_borrowed(ptr: *mut u8, index: usize) -> Option<u64> {
    crate::gil_assert();
    let mut out = 0;
    if unsafe { crate::object::seq_access::read_item_gil_borrowed(ptr, index, &mut out) } != 0 {
        Some(out)
    } else {
        None
    }
}

/// Return one owned sequence item.
///
/// # Safety
///
/// `ptr` must identify a live sequence. The caller assumes the returned
/// reference and must publish or release it exactly once.
pub unsafe fn seq_read_item_owned(_py: &CoreGilToken, ptr: *mut u8, index: usize) -> Option<u64> {
    let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
    crate::gil_assert();
    let mut out = 0;
    if crate::object::seq_access::read_item_owned(py, ptr, index, &mut out) != 0 {
        Some(out)
    } else {
        None
    }
}

pub fn alloc_instance_for_class(_py: &CoreGilToken, class_bits: u64) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe { crate::alloc_instance_for_class(py, class_ptr) }
    }
}

pub fn validate_constructor_class(_py: &CoreGilToken, actual: u64, expected: u64) -> bool {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::itertools_class::validate_constructor_class(py, actual, expected)
    }
}

pub fn constructor_repr(_py: &CoreGilToken, instance: u64, first: u64, second: u64) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::itertools_class::constructor_repr(py, instance, first, second)
    }
}

pub fn alloc_itertools_class(
    _py: &CoreGilToken,
    name: &str,
    layout_size: i64,
    shape: ObjectShapeId,
    iter_fn: u64,
    next_fn: u64,
    constructor: Option<(u64, u64, &[u64])>,
) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::itertools_class::alloc_itertools_class(
            py,
            name,
            layout_size,
            shape,
            iter_fn,
            next_fn,
            constructor,
        )
    }
}

/// Read an iterator's declaring class while the caller owns it under the GIL.
///
/// # Safety
/// `ptr` must be a live runtime object.
pub unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    unsafe { crate::object::object_class_bits(ptr) }
}
