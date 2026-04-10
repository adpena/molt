use crate::PyToken;
use crate::*;

pub extern "C" fn molt_header_size() -> u64 {
    crate::with_gil_entry!(_py, { std::mem::size_of::<MoltHeader>() as u64 })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_alloc(size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let size = usize_from_bits(size_bits);
        let total_size = size + std::mem::size_of::<MoltHeader>();
        let obj_ptr = alloc_object_zeroed_with_pool(_py, total_size, TYPE_ID_OBJECT);
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(obj_ptr).bits()
    })
}

unsafe fn alloc_dataclass_for_class_ptr(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    class_bits: u64,
) -> Option<u64> {
    unsafe {
        let field_names_name = attr_name_bits_from_bytes(_py, b"__molt_dataclass_field_names__")?;
        let field_names_bits = class_attr_lookup_raw_mro(_py, class_ptr, field_names_name);
        dec_ref_bits(_py, field_names_name);
        let field_names_bits = field_names_bits?;
        let Some(field_names_ptr) = obj_from_bits(field_names_bits).as_ptr() else {
            return Some(raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass field names must be a list/tuple of str",
            ));
        };
        let field_count = match object_type_id(field_names_ptr) {
            TYPE_ID_TUPLE => tuple_len(field_names_ptr),
            TYPE_ID_LIST => list_len(field_names_ptr),
            _ => {
                return Some(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "dataclass field names must be a list/tuple of str",
                ));
            }
        };
        let missing = missing_bits(_py);
        let mut values = Vec::with_capacity(field_count);
        values.resize(field_count, missing);
        let values_ptr = alloc_tuple(_py, &values);
        if values_ptr.is_null() {
            return Some(MoltObject::none().bits());
        }
        let values_bits = MoltObject::from_ptr(values_ptr).bits();
        let flags_bits =
            if let Some(flags_name) = attr_name_bits_from_bytes(_py, b"__molt_dataclass_flags__") {
                let bits = class_attr_lookup_raw_mro(_py, class_ptr, flags_name)
                    .unwrap_or_else(|| MoltObject::from_int(0).bits());
                dec_ref_bits(_py, flags_name);
                bits
            } else {
                MoltObject::from_int(0).bits()
            };
        let name_bits = class_name_bits(class_ptr);
        let inst_bits = molt_dataclass_new(name_bits, field_names_bits, values_bits, flags_bits);
        dec_ref_bits(_py, values_bits);
        if exception_pending(_py) {
            return Some(MoltObject::none().bits());
        }
        let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
            return Some(inst_bits);
        };
        let _ = dataclass_set_class_raw(_py, inst_ptr, class_bits);
        if exception_pending(_py) {
            return Some(MoltObject::none().bits());
        }
        Some(inst_bits)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_alloc_class(size_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(class_ptr) != TYPE_ID_TYPE {
                    return MoltObject::none().bits();
                }
                if let Some(inst_bits) = alloc_dataclass_for_class_ptr(_py, class_ptr, class_bits) {
                    return inst_bits;
                }
            }
        }
        let size = usize_from_bits(size_bits);
        let total_size = size + std::mem::size_of::<MoltHeader>();
        let obj_ptr = alloc_object_zeroed_with_pool(_py, total_size, TYPE_ID_OBJECT);
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            if class_bits != 0 {
                object_set_class_bits(_py, obj_ptr, class_bits);
                inc_ref_bits(_py, class_bits);
            }
        }
        MoltObject::from_ptr(obj_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_alloc_class_trusted(size_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(class_ptr) != TYPE_ID_TYPE {
                    return MoltObject::none().bits();
                }
                if let Some(inst_bits) = alloc_dataclass_for_class_ptr(_py, class_ptr, class_bits) {
                    return inst_bits;
                }
            }
        }
        let size = usize_from_bits(size_bits);
        let total_size = size + std::mem::size_of::<MoltHeader>();
        let obj_ptr = alloc_object_zeroed_with_pool(_py, total_size, TYPE_ID_OBJECT);
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            if class_bits != 0 {
                object_set_class_bits(_py, obj_ptr, class_bits);
                inc_ref_bits(_py, class_bits);
            }
        }
        MoltObject::from_ptr(obj_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_alloc_class_static(size_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(class_ptr) != TYPE_ID_TYPE {
                    return MoltObject::none().bits();
                }
                if let Some(inst_bits) = alloc_dataclass_for_class_ptr(_py, class_ptr, class_bits) {
                    return inst_bits;
                }
            }
        }
        let size = usize_from_bits(size_bits);
        let total_size = size + std::mem::size_of::<MoltHeader>();
        let obj_ptr = alloc_object_zeroed_with_pool(_py, total_size, TYPE_ID_OBJECT);
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            if class_bits != 0 {
                object_set_class_bits(_py, obj_ptr, class_bits);
            }
            let header = header_from_obj_ptr(obj_ptr);
            (*header).flags |= HEADER_FLAG_SKIP_CLASS_DECREF;
        }
        MoltObject::from_ptr(obj_ptr).bits()
    })
}

pub(crate) fn alloc_dict_with_pairs(_py: &PyToken<'_>, pairs: &[u64]) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(_py, total, TYPE_ID_DICT);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let order = Vec::with_capacity(pairs.len());
        let table = Vec::new();
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        for pair in pairs.chunks(2) {
            if pair.len() == 2 {
                dict_set_in_place(_py, ptr, pair[0], pair[1]);
            }
        }
    }
    ptr
}

pub(crate) fn alloc_set_like_with_entries(
    _py: &PyToken<'_>,
    entries: &[u64],
    type_id: u32,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(_py, total, type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let order = Vec::with_capacity(entries.len());
        let mut table = Vec::new();
        if !entries.is_empty() {
            table.resize(set_table_capacity(entries.len()), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        for &entry in entries {
            set_add_in_place(_py, ptr, entry);
        }
    }
    ptr
}

pub(crate) fn alloc_set_with_entries(_py: &PyToken<'_>, entries: &[u64]) -> *mut u8 {
    alloc_set_like_with_entries(_py, entries, TYPE_ID_SET)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_builder_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let debug = matches!(
            std::env::var("MOLT_DEBUG_LIST_BUILDER").ok().as_deref(),
            Some("1")
        );
        if debug {
            eprintln!(
                "molt debug list_builder_new capacity_bits=0x{:016x}",
                capacity_bits
            );
        }
        // Allocate wrapper object
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>(); // Store pointer to Vec
        let ptr = alloc_object(_py, total, TYPE_ID_LIST_BUILDER);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "list allocation failed");
        }
        unsafe {
            let capacity_obj = MoltObject::from_bits(capacity_bits);
            let capacity_hint = if capacity_obj.is_int() {
                let val = capacity_obj.as_int_unchecked();
                if val > 0 { val as usize } else { 0 }
            } else if capacity_obj.is_float() {
                usize_from_bits(capacity_bits)
            } else {
                0
            };
            if debug {
                eprintln!(
                    "molt debug list_builder_new capacity_hint={}",
                    capacity_hint
                );
            }
            let mut vec = Vec::<u64>::new();
            if capacity_hint > 0 && vec.try_reserve(capacity_hint).is_err() {
                return raise_exception::<_>(_py, "MemoryError", "list allocation failed");
            }
            let vec_ptr = Box::into_raw(Box::new(vec));
            *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        }
        bits_from_ptr(ptr)
    })
}

pub(crate) struct PtrDropGuard {
    ptr: *mut u8,
    active: bool,
}

impl PtrDropGuard {
    pub(crate) fn new(ptr: *mut u8) -> Self {
        Self {
            ptr,
            active: !ptr.is_null(),
        }
    }

    pub(crate) fn release(&mut self) {
        self.active = false;
    }
}

impl Drop for PtrDropGuard {
    fn drop(&mut self) {
        if self.active && !self.ptr.is_null() {
            unsafe {
                if std::env::var("MOLT_TRACE_CALLARGS").as_deref() == Ok("1")
                    && object_type_id(self.ptr) == TYPE_ID_CALLARGS
                {
                    let args_ptr = crate::call::bind::callargs_ptr(self.ptr);
                    eprintln!(
                        "[molt callargs] guard_drop builder_ptr=0x{:x} args_ptr=0x{:x}",
                        self.ptr as usize, args_ptr as usize,
                    );
                }
                molt_dec_ref(self.ptr);
            }
        }
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a list builder.
pub unsafe extern "C" fn molt_list_builder_append(builder_bits: u64, val: u64) {
    unsafe {
        crate::with_gil_entry!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return;
            }
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return;
            }
            let vec = &mut *vec_ptr;
            vec.push(val);
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a list builder.
pub unsafe extern "C" fn molt_list_builder_finish(builder_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let _guard = PtrDropGuard::new(builder_ptr);
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return MoltObject::none().bits();
            }
            *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

            // Reconstruct Box to drop it later, but we need the data
            let vec = Box::from_raw(vec_ptr);
            let slice = vec.as_slice();
            let capacity = vec.capacity().max(MAX_SMALL_LIST);
            let list_ptr = alloc_list_with_capacity(_py, slice, capacity);

            // Builder object will be cleaned up by GC/Ref counting eventually,
            // but the Vec heap allocation is owned by the Box we just reconstructed.
            // So dropping 'vec' here frees the temporary buffer. Correct.

            if list_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(list_ptr).bits()
            }
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a list builder with owned refs.
pub unsafe extern "C" fn molt_list_builder_finish_owned(builder_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let _guard = PtrDropGuard::new(builder_ptr);
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return MoltObject::none().bits();
            }
            *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

            let vec = Box::from_raw(vec_ptr);
            let slice = vec.as_slice();
            let capacity = vec.capacity().max(MAX_SMALL_LIST);
            let list_ptr = alloc_list_with_capacity_owned(_py, slice, capacity);

            if list_ptr.is_null() {
                for &elem in slice {
                    dec_ref_bits(_py, elem);
                }
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(list_ptr).bits()
            }
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a tuple builder.
pub unsafe extern "C" fn molt_tuple_builder_finish(builder_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let _guard = PtrDropGuard::new(builder_ptr);
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return MoltObject::none().bits();
            }
            *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

            let vec = Box::from_raw(vec_ptr);
            let slice = vec.as_slice();
            let capacity = vec.capacity().max(MAX_SMALL_LIST);
            let tuple_ptr = alloc_tuple_with_capacity(_py, slice, capacity);

            if tuple_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid. Elements in the builder's Vec
/// are assumed to already have their own reference (the compiler emitted
/// inc_ref before each append). No additional inc_ref is performed.
pub unsafe extern "C" fn molt_tuple_builder_finish_owned(builder_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let _guard = PtrDropGuard::new(builder_ptr);
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return MoltObject::none().bits();
            }
            *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

            let vec = Box::from_raw(vec_ptr);
            let slice = vec.as_slice();
            let capacity = vec.capacity().max(MAX_SMALL_LIST);
            let tuple_ptr = alloc_tuple_with_capacity_owned(_py, slice, capacity);

            if tuple_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_builder_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
        let ptr = alloc_object(_py, total, TYPE_ID_DICT_BUILDER);
        if ptr.is_null() {
            return 0;
        }
        unsafe {
            let capacity_hint = usize_from_bits(capacity_bits);
            let vec = Vec::with_capacity(capacity_hint * 2);
            let vec_ptr = Box::into_raw(Box::new(vec));
            *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        }
        bits_from_ptr(ptr)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_append(builder_bits: u64, key: u64, val: u64) {
    unsafe {
        crate::with_gil_entry!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return;
            }
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return;
            }
            let vec = &mut *vec_ptr;
            vec.push(key);
            vec.push(val);
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_finish(builder_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let _guard = PtrDropGuard::new(builder_ptr);
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return MoltObject::none().bits();
            }
            *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();
            let vec = Box::from_raw(vec_ptr);
            let ptr = alloc_dict_with_pairs(_py, vec.as_slice());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_builder_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
        let ptr = alloc_object(_py, total, TYPE_ID_SET_BUILDER);
        if ptr.is_null() {
            return 0;
        }
        unsafe {
            let capacity_hint = usize_from_bits(capacity_bits);
            let vec = Vec::with_capacity(capacity_hint);
            let vec_ptr = Box::into_raw(Box::new(vec));
            *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        }
        bits_from_ptr(ptr)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a set builder.
pub unsafe extern "C" fn molt_set_builder_append(builder_bits: u64, key: u64) {
    unsafe {
        crate::with_gil_entry!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return;
            }
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return;
            }
            let vec = &mut *vec_ptr;
            vec.push(key);
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a set builder.
pub unsafe extern "C" fn molt_set_builder_finish(builder_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let _guard = PtrDropGuard::new(builder_ptr);
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return MoltObject::none().bits();
            }
            *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();
            let vec = Box::from_raw(vec_ptr);
            let ptr = alloc_set_with_entries(_py, vec.as_slice());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        })
    }
}

// --- Allocation helpers ---

pub(crate) fn alloc_list_with_capacity(
    _py: &PyToken<'_>,
    elems: &[u64],
    capacity: usize,
) -> *mut u8 {
    let cap = capacity.max(elems.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut DataclassDesc>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_LIST);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(elems);
        for &elem in elems {
            inc_ref_bits(_py, elem);
        }
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        if crate::object::refcount_opt::slice_contains_heap_refs(elems) {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
    ptr
}

pub(crate) fn alloc_list_with_capacity_owned(
    _py: &PyToken<'_>,
    elems: &[u64],
    capacity: usize,
) -> *mut u8 {
    let cap = capacity.max(elems.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut DataclassDesc>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_LIST);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(elems);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        if crate::object::refcount_opt::slice_contains_heap_refs(elems) {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
    ptr
}

pub(crate) fn alloc_list(_py: &PyToken<'_>, elems: &[u64]) -> *mut u8 {
    let cap = if elems.len() <= MAX_SMALL_LIST {
        MAX_SMALL_LIST
    } else {
        elems.len()
    };
    alloc_list_with_capacity(_py, elems, cap)
}

pub(crate) fn alloc_tuple_with_capacity(
    _py: &PyToken<'_>,
    elems: &[u64],
    capacity: usize,
) -> *mut u8 {
    let cap = capacity.max(elems.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_TUPLE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(elems);
        for &elem in elems {
            inc_ref_bits(_py, elem);
        }
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        if crate::object::refcount_opt::slice_contains_heap_refs(elems) {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
    ptr
}

/// Like `alloc_tuple_with_capacity` but assumes the caller already owns
/// a reference to each element (no inc_ref). Used when the compiler
/// has already emitted inc_ref for each element before builder_append.
pub(crate) fn alloc_tuple_with_capacity_owned(
    _py: &PyToken<'_>,
    elems: &[u64],
    capacity: usize,
) -> *mut u8 {
    let cap = capacity.max(elems.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_TUPLE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(elems);
        // No inc_ref — ownership transferred from caller.
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        if crate::object::refcount_opt::slice_contains_heap_refs(elems) {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
    ptr
}

/// Cached empty tuple singleton. Allocated once, immortal (never freed).
static EMPTY_TUPLE_PTR: std::sync::atomic::AtomicPtr<u8> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

pub(crate) fn alloc_tuple(_py: &PyToken<'_>, elems: &[u64]) -> *mut u8 {
    // Fast path: return the immortal empty tuple singleton.
    if elems.is_empty() {
        let cached = EMPTY_TUPLE_PTR.load(std::sync::atomic::Ordering::Relaxed);
        let ptr = if !cached.is_null() {
            cached
        } else {
            let ptr = alloc_tuple_with_capacity(_py, &[], 0);
            if !ptr.is_null() {
                unsafe {
                    let header = header_from_obj_ptr(ptr);
                    (*header).flags |=
                        crate::object::HEADER_FLAG_IMMORTAL | crate::object::HEADER_FLAG_INTERNED;
                    (*header)
                        .ref_count
                        .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
                }
                // CAS: if another thread beat us, use theirs (single-threaded on WASM, but safe)
                let _ = EMPTY_TUPLE_PTR.compare_exchange(
                    std::ptr::null_mut(),
                    ptr,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            EMPTY_TUPLE_PTR.load(std::sync::atomic::Ordering::Relaxed)
        };
        return ptr;
    }
    let cap = if elems.len() <= MAX_SMALL_LIST {
        MAX_SMALL_LIST
    } else {
        elems.len()
    };
    alloc_tuple_with_capacity(_py, elems, cap)
}

pub(crate) fn alloc_range(
    _py: &PyToken<'_>,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_RANGE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = start_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = stop_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = step_bits;
        inc_ref_bits(_py, start_bits);
        inc_ref_bits(_py, stop_bits);
        inc_ref_bits(_py, step_bits);
    }
    ptr
}

pub(crate) fn alloc_slice_obj(
    _py: &PyToken<'_>,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_SLICE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = start_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = stop_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = step_bits;
        inc_ref_bits(_py, start_bits);
        inc_ref_bits(_py, stop_bits);
        inc_ref_bits(_py, step_bits);
    }
    ptr
}

pub(crate) fn alloc_generic_alias(_py: &PyToken<'_>, origin_bits: u64, args_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_GENERIC_ALIAS);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = origin_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = args_bits;
        inc_ref_bits(_py, origin_bits);
        inc_ref_bits(_py, args_bits);
    }
    ptr
}

pub(crate) fn alloc_union_type(_py: &PyToken<'_>, args_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_UNION);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = args_bits;
        inc_ref_bits(_py, args_bits);
    }
    ptr
}

// Context manager alloc moved to runtime/molt-runtime/src/builtins/context.rs.

pub(crate) fn alloc_function_obj(_py: &PyToken<'_>, fn_ptr: u64, arity: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 9 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_FUNCTION);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = fn_ptr;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = arity;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        let none_bits = MoltObject::none().bits();
        *(ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64) = none_bits;
        *(ptr.add(8 * std::mem::size_of::<u64>()) as *mut *const ()) = std::ptr::null();
        inc_ref_bits(_py, none_bits);
    }
    ptr
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn alloc_code_obj(
    _py: &PyToken<'_>,
    filename_bits: u64,
    name_bits: u64,
    firstlineno: i64,
    linetable_bits: u64,
    varnames_bits: u64,
    argcount: u64,
    posonlyargcount: u64,
    kwonlyargcount: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 8 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_CODE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = filename_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = name_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut i64) = firstlineno;
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = linetable_bits;
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = varnames_bits;
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = argcount;
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64) = posonlyargcount;
        *(ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64) = kwonlyargcount;
        if filename_bits != 0 {
            inc_ref_bits(_py, filename_bits);
        }
        if name_bits != 0 {
            inc_ref_bits(_py, name_bits);
        }
        if linetable_bits != 0 {
            inc_ref_bits(_py, linetable_bits);
        }
        if varnames_bits != 0 {
            inc_ref_bits(_py, varnames_bits);
        }
    }
    ptr
}

pub(crate) fn alloc_bound_method_obj(_py: &PyToken<'_>, func_bits: u64, self_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_BOUND_METHOD);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = func_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = self_bits;
        inc_ref_bits(_py, func_bits);
        inc_ref_bits(_py, self_bits);
    }
    ptr
}

pub(crate) fn alloc_module_obj(_py: &PyToken<'_>, name_bits: u64) -> *mut u8 {
    let _nursery_guard = crate::object::NurserySuspendGuard::new();
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_MODULE);
    if ptr.is_null() {
        dec_ref_bits(_py, dict_bits);
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = name_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = dict_bits;
        inc_ref_bits(_py, name_bits);
        // Mark module objects as immortal.  The native backend's Cranelift
        // code generation emits dec_ref_obj calls for every SSA value whose
        // last-use point is reached (Perceus-style reference counting).
        // Module objects are returned by MODULE_CACHE_GET which inc_refs,
        // and then dec_ref'd when the local goes dead.  However, modules
        // are also stored in the module cache (with their own inc_ref) and
        // in sys.modules, and multiple overlapping load sequences can cause
        // the compiled code's dec_refs to out-pace the inc_refs, freeing
        // the module while the cache still holds dangling bits.  Modules
        // are process-lifetime singletons in Molt, so marking them immortal
        // is both correct and avoids the refcount imbalance entirely.
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        (*header_ptr).flags |= crate::object::HEADER_FLAG_IMMORTAL;
    }
    ptr
}

pub(crate) fn alloc_class_obj(_py: &PyToken<'_>, name_bits: u64) -> *mut u8 {
    let _nursery_guard = crate::object::NurserySuspendGuard::new();
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let bases_bits = MoltObject::none().bits();
    let mro_bits = MoltObject::none().bits();
    let total = std::mem::size_of::<MoltHeader>() + 8 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_TYPE);
    if ptr.is_null() {
        dec_ref_bits(_py, dict_bits);
        return ptr;
    }
    let qualname_bits = name_bits;
    unsafe {
        *(ptr as *mut u64) = name_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = dict_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bases_bits;
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = mro_bits;
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        let none_bits = MoltObject::none().bits();
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64) = none_bits;
        *(ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64) = qualname_bits;
        inc_ref_bits(_py, name_bits);
        inc_ref_bits(_py, bases_bits);
        inc_ref_bits(_py, mro_bits);
        inc_ref_bits(_py, none_bits);
        inc_ref_bits(_py, qualname_bits);
    }
    ptr
}

pub(crate) fn alloc_classmethod_obj(_py: &PyToken<'_>, func_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_CLASSMETHOD);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = func_bits;
        inc_ref_bits(_py, func_bits);
    }
    ptr
}

pub(crate) fn alloc_staticmethod_obj(_py: &PyToken<'_>, func_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_STATICMETHOD);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = func_bits;
        inc_ref_bits(_py, func_bits);
    }
    ptr
}

pub(crate) fn alloc_property_obj(
    _py: &PyToken<'_>,
    get_bits: u64,
    set_bits: u64,
    del_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_PROPERTY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = get_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = set_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = del_bits;
        inc_ref_bits(_py, get_bits);
        inc_ref_bits(_py, set_bits);
        inc_ref_bits(_py, del_bits);
    }
    ptr
}

pub(crate) fn alloc_super_obj(_py: &PyToken<'_>, type_bits: u64, obj_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_SUPER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = type_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = obj_bits;
        inc_ref_bits(_py, type_bits);
        inc_ref_bits(_py, obj_bits);
    }
    ptr
}

// Context stack helpers moved to runtime/molt-runtime/src/builtins/context.rs.

// Frame stack helpers moved to runtime/molt-runtime/src/builtins/exceptions.rs.

pub(crate) fn alloc_bytes_like_with_len(_py: &PyToken<'_>, len: usize, type_id: u32) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<usize>() + len;
    let ptr = alloc_object(_py, total, type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let len_ptr = ptr as *mut usize;
        *len_ptr = len;
    }
    ptr
}

/// Cached empty string singleton.
static EMPTY_STRING_PTR: std::sync::atomic::AtomicPtr<u8> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// Interned single-character ASCII strings (0x00..0x7F).  Lazily populated on first access.
/// Each entry stores the raw object pointer (`null` = not yet initialised).
/// Using atomics avoids a mutex on the hot lookup path.
static ASCII_CHARS: [std::sync::atomic::AtomicPtr<u8>; 128] =
    { [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 128] };

/// Lazily initialise every slot in `ASCII_CHARS` that is still zero.  Called once (guarded
/// by `OnceLock`) on the first single-ASCII-char allocation.
fn init_ascii_chars(_py: &PyToken<'_>) {
    for byte in 0u8..128 {
        let slot = &ASCII_CHARS[byte as usize];
        if !slot.load(std::sync::atomic::Ordering::Relaxed).is_null() {
            continue;
        }
        let _nursery_guard = crate::object::NurserySuspendGuard::new();
        let ptr = alloc_bytes_like_with_len(_py, 1, TYPE_ID_STRING);
        if ptr.is_null() {
            continue;
        }
        unsafe {
            let data_ptr = ptr.add(std::mem::size_of::<usize>());
            *data_ptr = byte;
            let header = header_from_obj_ptr(ptr);
            (*header).flags |=
                crate::object::HEADER_FLAG_IMMORTAL | crate::object::HEADER_FLAG_INTERNED;
            (*header)
                .ref_count
                .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
        }
        // If another thread raced us, the first writer wins; second allocation leaks
        // harmlessly (both are immortal).
        let _ = slot.compare_exchange(
            std::ptr::null_mut(),
            ptr,
            std::sync::atomic::Ordering::Release,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

/// Try to return an interned single-ASCII-character string.
/// Returns the raw object pointer if the input is exactly one ASCII byte, else `None`.
#[inline]
fn try_intern_ascii_char(_py: &PyToken<'_>, bytes: &[u8]) -> Option<*mut u8> {
    if bytes.len() != 1 {
        return None;
    }
    let byte = bytes[0];
    if byte > 127 {
        return None;
    }
    if ASCII_CHARS[byte as usize]
        .load(std::sync::atomic::Ordering::Acquire)
        .is_null()
    {
        init_ascii_chars(_py);
    }
    let raw = ASCII_CHARS[byte as usize].load(std::sync::atomic::Ordering::Acquire);
    if raw.is_null() { None } else { Some(raw) }
}

#[derive(Clone, Copy)]
struct InternedPtr(*mut u8);

// SAFETY: these pointers only refer to immortal interned string objects, so
// sharing the pointer value across threads does not transfer mutable aliasing
// rights or ownership. The underlying objects are never freed.
unsafe impl Send for InternedPtr {}
unsafe impl Sync for InternedPtr {}

/// Molt-level string intern pool: maps identifier-like ASCII strings to their immortal
/// Molt object pointer (stored as `usize` to be `Send`).
///
/// Only strings that pass `string_intern::is_identifier_like` are candidates.  The
/// objects stored here are marked `HEADER_FLAG_IMMORTAL | HEADER_FLAG_INTERNED` so the
/// GC never frees them, and repeated allocations of the same identifier return the same
/// heap object (pointer equality).
fn molt_string_intern_pool()
-> &'static std::sync::Mutex<std::collections::HashMap<Box<[u8]>, InternedPtr>> {
    static POOL: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<Box<[u8]>, InternedPtr>>,
    > = std::sync::OnceLock::new();
    POOL.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub(crate) fn alloc_string(_py: &PyToken<'_>, bytes: &[u8]) -> *mut u8 {
    if bytes.is_empty() {
        let cached = EMPTY_STRING_PTR.load(std::sync::atomic::Ordering::Relaxed);
        let ptr = if !cached.is_null() {
            cached
        } else {
            let _nursery_guard = crate::object::NurserySuspendGuard::new();
            let ptr = alloc_bytes_like_with_len(_py, 0, TYPE_ID_STRING);
            if !ptr.is_null() {
                unsafe {
                    let header = header_from_obj_ptr(ptr);
                    (*header).flags |= crate::object::HEADER_FLAG_IMMORTAL;
                    (*header)
                        .ref_count
                        .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
                }
                let _ = EMPTY_STRING_PTR.compare_exchange(
                    std::ptr::null_mut(),
                    ptr,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            EMPTY_STRING_PTR.load(std::sync::atomic::Ordering::Relaxed)
        };
        return ptr;
    }

    // Fast path: single ASCII character strings (space, digits, punctuation, etc.)
    // are served from a dedicated 128-entry lookup table — no hashing, no locking.
    if let Some(ptr) = try_intern_ascii_char(_py, bytes) {
        return ptr;
    }

    // Auto-intern ASCII identifier-like strings (e.g. attribute names, keyword
    // identifiers).  These are the most frequently allocated strings in typical
    // Python programs, and making them immortal singletons allows pointer-equality
    // comparisons instead of byte-by-byte scans.
    //
    // Fast pre-check: all bytes must be ASCII and the string must look like an
    // identifier.  We use `is_identifier_like` from string_intern which is a
    // purely byte-level check with no allocation.
    let is_ident = bytes.is_ascii()
        && crate::object::string_intern::is_identifier_like(
            // SAFETY: we just verified all bytes are ASCII which is a subset of UTF-8.
            unsafe { std::str::from_utf8_unchecked(bytes) },
        );
    if is_ident {
        // Check the Molt-level pool first (no allocation on hit).
        if let Ok(pool) = molt_string_intern_pool().lock()
            && let Some(&raw) = pool.get(bytes)
        {
            // Cache hit: return the existing immortal object directly.
            return raw.0;
        }
        // Pool miss: allocate a new string object, mark it immortal+interned, and
        // insert it into the pool so future allocations reuse this object.
        let _nursery_guard = crate::object::NurserySuspendGuard::new();
        let ptr = alloc_bytes_like_with_len(_py, bytes.len(), TYPE_ID_STRING);
        if ptr.is_null() {
            return ptr;
        }
        unsafe {
            let data_ptr = ptr.add(std::mem::size_of::<usize>());
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
            let header = header_from_obj_ptr(ptr);
            (*header).flags |=
                crate::object::HEADER_FLAG_IMMORTAL | crate::object::HEADER_FLAG_INTERNED;
            (*header)
                .ref_count
                .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
        }
        // Insert into pool; on concurrent miss (two threads race) we accept the
        // redundant allocation — the first writer wins and the second allocation
        // leaks harmlessly (both are immortal, pool holds the canonical one).
        if let Ok(mut pool) = molt_string_intern_pool().lock() {
            pool.entry(bytes.to_vec().into_boxed_slice())
                .or_insert(InternedPtr(ptr));
            // Re-read: if another thread won the race, prefer their pointer.
            if let Some(&canonical) = pool.get(bytes) {
                return canonical.0;
            }
        }
        return ptr;
    }

    let ptr = alloc_bytes_like_with_len(_py, bytes.len(), TYPE_ID_STRING);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
    }
    ptr
}

pub(crate) fn alloc_bytes_like(_py: &PyToken<'_>, bytes: &[u8], type_id: u32) -> *mut u8 {
    let ptr = alloc_bytes_like_with_len(_py, bytes.len(), type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
    }
    ptr
}

/// Cached empty bytes singleton.
static EMPTY_BYTES_PTR: std::sync::atomic::AtomicPtr<u8> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

pub(crate) fn alloc_bytes(_py: &PyToken<'_>, bytes: &[u8]) -> *mut u8 {
    if bytes.is_empty() {
        let cached = EMPTY_BYTES_PTR.load(std::sync::atomic::Ordering::Relaxed);
        let ptr = if !cached.is_null() {
            cached
        } else {
            let ptr = alloc_bytes_like(_py, &[], TYPE_ID_BYTES);
            if !ptr.is_null() {
                unsafe {
                    let header = header_from_obj_ptr(ptr);
                    (*header).flags |= crate::object::HEADER_FLAG_IMMORTAL;
                    (*header)
                        .ref_count
                        .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
                }
                let _ = EMPTY_BYTES_PTR.compare_exchange(
                    std::ptr::null_mut(),
                    ptr,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            EMPTY_BYTES_PTR.load(std::sync::atomic::Ordering::Relaxed)
        };
        return ptr;
    }
    alloc_bytes_like(_py, bytes, TYPE_ID_BYTES)
}

pub(crate) fn clear_builder_singletons(_py: &PyToken<'_>) {
    crate::gil_assert();
    let mut released = std::collections::HashSet::new();

    let mut release_once = |ptr: *mut u8| {
        if !ptr.is_null() && released.insert(ptr as usize) {
            crate::object::release_shutdown_owned_bits(_py, MoltObject::from_ptr(ptr).bits());
        }
    };

    let empty_tuple =
        EMPTY_TUPLE_PTR.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel);
    release_once(empty_tuple);

    let empty_string =
        EMPTY_STRING_PTR.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel);
    release_once(empty_string);

    let empty_bytes =
        EMPTY_BYTES_PTR.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel);
    release_once(empty_bytes);

    for slot in &ASCII_CHARS {
        let ptr = slot.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel);
        release_once(ptr);
    }

    if let Ok(mut pool) = molt_string_intern_pool().lock() {
        let old = std::mem::take(&mut *pool);
        drop(pool);
        for (_key, ptr) in old {
            release_once(ptr.0);
        }
    }
}

pub(crate) fn alloc_bytearray(_py: &PyToken<'_>, bytes: &[u8]) -> *mut u8 {
    let cap = if bytes.len() <= MAX_SMALL_LIST {
        MAX_SMALL_LIST
    } else {
        bytes.len()
    };
    alloc_bytearray_with_capacity(_py, bytes, cap)
}

pub(crate) fn alloc_bytearray_with_capacity(
    _py: &PyToken<'_>,
    bytes: &[u8],
    capacity: usize,
) -> *mut u8 {
    let cap = capacity.max(bytes.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u8>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_BYTEARRAY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(bytes);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u8>) = vec_ptr;
    }
    ptr
}

pub(crate) fn alloc_bytearray_with_len(_py: &PyToken<'_>, len: usize) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u8>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_BYTEARRAY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let vec = vec![0u8; len];
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u8>) = vec_ptr;
    }
    ptr
}

pub(crate) fn alloc_intarray(_py: &PyToken<'_>, values: &[i64]) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<usize>()
        + std::mem::size_of_val(values);
    let ptr = alloc_object(_py, total, TYPE_ID_INTARRAY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let len_ptr = ptr as *mut usize;
        *len_ptr = values.len();
        let data_ptr = ptr.add(std::mem::size_of::<usize>()) as *mut i64;
        std::ptr::copy_nonoverlapping(values.as_ptr(), data_ptr, values.len());
    }
    ptr
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn alloc_memoryview(
    _py: &PyToken<'_>,
    owner_bits: u64,
    offset: isize,
    len: usize,
    itemsize: usize,
    stride: isize,
    readonly: bool,
    format_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<MemoryView>();
    let ptr = alloc_object(_py, total, TYPE_ID_MEMORYVIEW);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let shape = Box::new(vec![len as isize]);
        let strides = Box::new(vec![stride]);
        let mv_ptr = memoryview_ptr(ptr);
        (*mv_ptr).owner_bits = owner_bits;
        (*mv_ptr).offset = offset;
        (*mv_ptr).len = len;
        (*mv_ptr).itemsize = itemsize;
        (*mv_ptr).stride = stride;
        (*mv_ptr).readonly = if readonly { 1 } else { 0 };
        (*mv_ptr).ndim = 1;
        (*mv_ptr)._pad = [0; 6];
        (*mv_ptr).format_bits = format_bits;
        (*mv_ptr).shape_ptr = Box::into_raw(shape);
        (*mv_ptr).strides_ptr = Box::into_raw(strides);
    }
    inc_ref_bits(_py, owner_bits);
    inc_ref_bits(_py, format_bits);
    ptr
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn alloc_memoryview_shaped(
    _py: &PyToken<'_>,
    owner_bits: u64,
    offset: isize,
    itemsize: usize,
    readonly: bool,
    format_bits: u64,
    shape: Vec<isize>,
    strides: Vec<isize>,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<MemoryView>();
    let ptr = alloc_object(_py, total, TYPE_ID_MEMORYVIEW);
    if ptr.is_null() {
        return ptr;
    }
    let ndim = shape.len();
    let len = shape.first().copied().unwrap_or(0).max(0) as usize;
    let stride = strides.first().copied().unwrap_or(0);
    unsafe {
        let mv_ptr = memoryview_ptr(ptr);
        (*mv_ptr).owner_bits = owner_bits;
        (*mv_ptr).offset = offset;
        (*mv_ptr).len = len;
        (*mv_ptr).itemsize = itemsize;
        (*mv_ptr).stride = stride;
        (*mv_ptr).readonly = if readonly { 1 } else { 0 };
        (*mv_ptr).ndim = ndim.min(u8::MAX as usize) as u8;
        (*mv_ptr)._pad = [0; 6];
        (*mv_ptr).format_bits = format_bits;
        (*mv_ptr).shape_ptr = Box::into_raw(Box::new(shape));
        (*mv_ptr).strides_ptr = Box::into_raw(Box::new(strides));
    }
    inc_ref_bits(_py, owner_bits);
    inc_ref_bits(_py, format_bits);
    ptr
}
