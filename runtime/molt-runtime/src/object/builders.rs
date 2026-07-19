use crate::PyToken;
use crate::object::{ClassEdgeOwnership, MoltAuxWord, object_init_class_edge_unpublished};
use crate::*;

#[inline]
fn raw_payload_total_or_null(payload_size: usize) -> Option<usize> {
    crate::object::checked_object_total_size(payload_size)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_header_size() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { std::mem::size_of::<MoltHeader>() as u64 })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_alloc(size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(size) = usize_from_bits(size_bits) else {
            return MoltObject::none().bits();
        };
        let Some(total_size) = raw_payload_total_or_null(size) else {
            return MoltObject::none().bits();
        };
        let obj_ptr = crate::object::alloc_object_zeroed_unpublished_with_aux(
            _py,
            total_size,
            TYPE_ID_OBJECT,
            ObjectAuxPreselection::Default,
        );
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let header = crate::object::header_from_obj_ptr(obj_ptr);
            (*header).fetch_or_flags(crate::object::HEADER_FLAG_RAW_ALLOC);
        }
        MoltObject::from_ptr(obj_ptr).bits()
    })
}

pub(crate) unsafe fn alloc_dataclass_for_class_ptr(
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
        let _ =
            crate::object::ops_slice::dataclass_init_class_unpublished(_py, inst_ptr, class_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, inst_bits);
            return Some(MoltObject::none().bits());
        }
        Some(inst_bits)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_alloc_class(size_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { alloc_class_instance(_py, size_bits, class_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_publish_initialized(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return obj_bits;
        };
        unsafe {
            if (*header_from_obj_ptr(ptr)).gc_is_published() {
                return raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "object construction published more than once",
                );
            }
            crate::object::gc::gc_publish_initialized(_py, ptr);
        }
        obj_bits
    })
}

pub(crate) fn alloc_class_instance(_py: &PyToken<'_>, size_bits: u64, class_bits: u64) -> u64 {
    let mut type_id = TYPE_ID_OBJECT;
    if class_bits != 0 {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
            type_id = crate::object::class_instance_type_id(class_ptr);
        }
    }
    let Some(size) = usize_from_bits(size_bits) else {
        return MoltObject::none().bits();
    };
    let Some(total_size) = raw_payload_total_or_null(size) else {
        return MoltObject::none().bits();
    };
    let aux = if class_bits == 0 {
        ObjectAuxPreselection::Default
    } else {
        ObjectAuxPreselection::ClassInline
    };
    let obj_ptr =
        crate::object::alloc_object_zeroed_unpublished_with_aux(_py, total_size, type_id, aux);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        if class_bits != 0
            && !object_init_class_edge_unpublished(
                _py,
                obj_ptr,
                class_bits,
                ClassEdgeOwnership::Owned,
            )
        {
            dec_ref_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            return MoltObject::none().bits();
        }
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

/// Canonical exact-dict construction transaction.
///
/// Storage and initial edges remain unpublished until they are complete. The
/// single publication step applies the generated dynamic GC projection, so an
/// atomic/empty dict never leaks through the always-tracked allocation state.
pub(crate) fn alloc_dict_with_capacity_and_pairs(
    _py: &PyToken<'_>,
    capacity_hint: usize,
    pairs: &[u64],
) -> *mut u8 {
    let Some(order_capacity) = capacity_hint.checked_mul(2) else {
        return std::ptr::null_mut();
    };
    let table_capacity = if capacity_hint == 0 {
        0
    } else {
        let Some(capacity) = crate::object::ops::checked_dict_table_capacity(capacity_hint) else {
            return std::ptr::null_mut();
        };
        capacity
    };
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>()
        + std::mem::size_of::<*mut Vec<u64>>();
    let ptr = crate::object::alloc_object_zeroed_unpublished_with_aux(
        _py,
        total,
        TYPE_ID_DICT,
        ObjectAuxPreselection::Default,
    );
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let Some(order_ptr) =
            crate::object::backing::tracked_vec_box_with_capacity::<u64>(order_capacity)
        else {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        let Some(table_ptr) =
            crate::object::backing::tracked_vec_box_zeroed::<usize>(table_capacity)
        else {
            drop(crate::object::backing::tracked_vec_box_from_raw(order_ptr));
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        let Some(hashes_ptr) =
            crate::object::backing::tracked_vec_box_with_capacity::<u64>(capacity_hint)
        else {
            drop(crate::object::backing::tracked_vec_box_from_raw(table_ptr));
            drop(crate::object::backing::tracked_vec_box_from_raw(order_ptr));
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>() + std::mem::size_of::<*mut Vec<usize>>())
            as *mut *mut Vec<u64>) = hashes_ptr;
        for pair in pairs.chunks(2) {
            if pair.len() == 2 {
                dict_set_in_place(_py, ptr, pair[0], pair[1]);
            }
        }
        crate::object::gc::gc_publish_initialized(_py, ptr);
    }
    ptr
}

pub(crate) fn alloc_dict_with_pairs(_py: &PyToken<'_>, pairs: &[u64]) -> *mut u8 {
    alloc_dict_with_capacity_and_pairs(_py, pairs.len() / 2, pairs)
}

pub(crate) fn alloc_set_like_with_entries(
    _py: &PyToken<'_>,
    entries: &[u64],
    type_id: u32,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>()
        + std::mem::size_of::<*mut Vec<u64>>();
    let ptr = alloc_object(_py, total, type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let Some(order_ptr) =
            crate::object::backing::tracked_vec_box_with_capacity::<u64>(entries.len())
        else {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        let table_cap = if entries.is_empty() {
            0
        } else {
            set_table_capacity(entries.len())
        };
        let Some(table_ptr) = crate::object::backing::tracked_vec_box_zeroed::<usize>(table_cap)
        else {
            drop(crate::object::backing::tracked_vec_box_from_raw(order_ptr));
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        let Some(hashes_ptr) =
            crate::object::backing::tracked_vec_box_with_capacity::<u64>(entries.len())
        else {
            drop(crate::object::backing::tracked_vec_box_from_raw(table_ptr));
            drop(crate::object::backing::tracked_vec_box_from_raw(order_ptr));
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>() + std::mem::size_of::<*mut Vec<usize>>())
            as *mut *mut Vec<u64>) = hashes_ptr;
        for &entry in entries {
            set_add_in_place(_py, ptr, entry, HashContext::SetElement);
        }
    }
    ptr
}

pub(crate) fn alloc_set_with_entries(_py: &PyToken<'_>, entries: &[u64]) -> *mut u8 {
    alloc_set_like_with_entries(_py, entries, TYPE_ID_SET)
}

#[inline]
fn debug_list_builder_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MOLT_DEBUG_LIST_BUILDER").ok().as_deref(),
            Some("1")
        )
    })
}

/// Cached `MOLT_TRACE_CALLARGS` flag. `PtrDropGuard::drop` runs on every
/// CallArgs-builder drop — i.e. on every function/method/constructor call that
/// builds an argument tuple. Reading the env var there (`std::env::var`) took
/// the libc environ lock and heap-allocated per call; profiling a call-heavy
/// ETL loop showed `getenv` internals as a dominant frame. Cache it once.
#[inline]
fn trace_callargs_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_CALLARGS").as_deref() == Ok("1"))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_builder_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let debug = debug_list_builder_enabled();
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
                let Some(capacity) = usize_from_bits(capacity_bits) else {
                    dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                    return raise_exception::<_>(_py, "MemoryError", "list capacity is too large");
                };
                capacity
            } else {
                0
            };
            if debug {
                eprintln!(
                    "molt debug list_builder_new capacity_hint={}",
                    capacity_hint
                );
            }
            let Some(vec_ptr) =
                crate::object::backing::tracked_vec_box_with_capacity::<u64>(capacity_hint)
            else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return raise_exception::<_>(_py, "MemoryError", "list allocation failed");
            };
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
                if trace_callargs_enabled() && object_type_id(self.ptr) == TYPE_ID_CALLARGS {
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
        crate::with_gil_entry_nopanic!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return;
            }
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return;
            }
            let vec = &mut *vec_ptr;
            if !crate::object::backing::tracked_vec_reserve_or_raise(
                _py,
                vec_ptr,
                vec.len().saturating_add(1),
                "list allocation failed",
            ) {
                return;
            }
            vec.push(val);
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a list builder.
pub unsafe extern "C" fn molt_list_builder_finish(builder_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
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
            let vec = crate::object::backing::tracked_vec_box_from_raw(vec_ptr);
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
        crate::with_gil_entry_nopanic!(_py, {
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

            let vec = crate::object::backing::tracked_vec_box_from_raw(vec_ptr);
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
        crate::with_gil_entry_nopanic!(_py, {
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

            let vec = crate::object::backing::tracked_vec_box_from_raw(vec_ptr);
            let slice = vec.as_slice();
            let tuple_ptr = alloc_tuple(_py, slice);

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
/// `values_ptr` must point to `len` contiguous NaN-boxed values when `len > 0`.
pub unsafe extern "C" fn molt_tuple_from_values(values_ptr: *const u64, len: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Ok(len) = usize::try_from(len) else {
                return raise_exception::<_>(_py, "MemoryError", "tuple is too large");
            };
            if len > 0 && values_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "tuple values pointer is null");
            }
            let values = if len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(values_ptr, len)
            };
            let tuple_ptr = alloc_tuple(_py, values);
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
        crate::with_gil_entry_nopanic!(_py, {
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

            let vec = crate::object::backing::tracked_vec_box_from_raw(vec_ptr);
            let slice = vec.as_slice();
            let tuple_ptr = alloc_tuple_owned(_py, slice);

            if tuple_ptr.is_null() {
                for &elem in slice {
                    dec_ref_bits(_py, elem);
                }
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_builder_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
        let ptr = alloc_object(_py, total, TYPE_ID_DICT_BUILDER);
        if ptr.is_null() {
            return 0;
        }
        unsafe {
            let Some(capacity_hint) = usize_from_bits(capacity_bits) else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return 0;
            };
            let Some(vec_capacity) = capacity_hint.checked_mul(2) else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return 0;
            };
            let Some(vec_ptr) =
                crate::object::backing::tracked_vec_box_with_capacity::<u64>(vec_capacity)
            else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return 0;
            };
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
        crate::with_gil_entry_nopanic!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return;
            }
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return;
            }
            let vec = &mut *vec_ptr;
            if !crate::object::backing::tracked_vec_reserve_or_raise(
                _py,
                vec_ptr,
                vec.len().saturating_add(2),
                "dict allocation failed",
            ) {
                return;
            }
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
        crate::with_gil_entry_nopanic!(_py, {
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
            let vec = crate::object::backing::tracked_vec_box_from_raw(vec_ptr);
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
    crate::with_gil_entry_nopanic!(_py, {
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
        let ptr = alloc_object(_py, total, TYPE_ID_SET_BUILDER);
        if ptr.is_null() {
            return 0;
        }
        unsafe {
            let Some(capacity_hint) = usize_from_bits(capacity_bits) else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return 0;
            };
            let Some(vec_ptr) =
                crate::object::backing::tracked_vec_box_with_capacity::<u64>(capacity_hint)
            else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return 0;
            };
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
        crate::with_gil_entry_nopanic!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return;
            }
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                return;
            }
            let vec = &mut *vec_ptr;
            if !crate::object::backing::tracked_vec_reserve_or_raise(
                _py,
                vec_ptr,
                vec.len().saturating_add(1),
                "set allocation failed",
            ) {
                return;
            }
            vec.push(key);
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a set builder.
pub unsafe extern "C" fn molt_set_builder_finish(builder_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
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
            let vec = crate::object::backing::tracked_vec_box_from_raw(vec_ptr);
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
    let ptr = alloc_object_with_aux(_py, total, TYPE_ID_LIST, ObjectAuxPreselection::ClassInline);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let Some(vec_ptr) = crate::object::backing::tracked_vec_box_from_slice(elems, cap) else {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        for &elem in elems {
            inc_ref_bits(_py, elem);
        }
        crate::object::backing::tracked_vec_set_heap_edge_count(
            vec_ptr,
            crate::object::refcount_opt::slice_heap_ref_count(elems),
        );
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        if crate::object::refcount_opt::slice_contains_heap_refs(elems) {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
    }
    ptr
}

/// Allocate a list whose logical length is established in the same allocation
/// as its backing store. The CPython ABI uses this for `PyList_New(size)`:
/// callers must observe `size` immediately, while the bridge separately tracks
/// that the physical slots are not yet safe to read until populated.
pub(crate) fn alloc_list_filled(_py: &PyToken<'_>, len: usize, value: MoltObject) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut DataclassDesc>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object_with_aux(_py, total, TYPE_ID_LIST, ObjectAuxPreselection::ClassInline);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let Some(vec_ptr) = crate::object::backing::tracked_vec_box_with_capacity::<u64>(len)
        else {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        (*vec_ptr).resize(len, value.bits());
        for _ in 0..len {
            inc_ref_bits(_py, value.bits());
        }
        crate::object::backing::tracked_vec_set_heap_edge_count(
            vec_ptr,
            usize::from(value.is_ptr()).saturating_mul(len),
        );
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        if len != 0 && value.is_ptr() {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
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
    let ptr = alloc_object_with_aux(_py, total, TYPE_ID_LIST, ObjectAuxPreselection::ClassInline);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let Some(vec_ptr) = crate::object::backing::tracked_vec_box_from_slice(elems, cap) else {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        crate::object::backing::tracked_vec_set_heap_edge_count(
            vec_ptr,
            crate::object::refcount_opt::slice_heap_ref_count(elems),
        );
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        if crate::object::refcount_opt::slice_contains_heap_refs(elems) {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
    }
    ptr
}

#[inline]
fn specialized_list_object_size<Storage>() -> usize {
    std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Storage>()
        + std::mem::size_of::<u64>()
}

#[inline]
unsafe fn drop_list_int_storage(storage_ptr: *mut crate::object::layout::ListIntStorage) {
    unsafe {
        drop((*Box::from_raw(storage_ptr)).into_vec());
    }
}

#[inline]
unsafe fn drop_list_bool_storage(storage_ptr: *mut crate::object::layout::ListBoolStorage) {
    unsafe {
        drop((*Box::from_raw(storage_ptr)).into_vec());
    }
}

unsafe fn alloc_list_int_with_storage(
    _py: &PyToken<'_>,
    storage_ptr: *mut crate::object::layout::ListIntStorage,
) -> Result<*mut u8, u64> {
    let out_ptr = alloc_object(
        _py,
        specialized_list_object_size::<crate::object::layout::ListIntStorage>(),
        TYPE_ID_LIST_INT,
    );
    if out_ptr.is_null() {
        unsafe {
            drop_list_int_storage(storage_ptr);
        }
        return Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"));
    }
    unsafe {
        *(out_ptr as *mut *mut crate::object::layout::ListIntStorage) = storage_ptr;
    }
    Ok(out_ptr)
}

unsafe fn alloc_list_bool_with_storage(
    _py: &PyToken<'_>,
    storage_ptr: *mut crate::object::layout::ListBoolStorage,
) -> Result<*mut u8, u64> {
    let out_ptr = alloc_object(
        _py,
        specialized_list_object_size::<crate::object::layout::ListBoolStorage>(),
        TYPE_ID_LIST_BOOL,
    );
    if out_ptr.is_null() {
        unsafe {
            drop_list_bool_storage(storage_ptr);
        }
        return Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"));
    }
    unsafe {
        *(out_ptr as *mut *mut crate::object::layout::ListBoolStorage) = storage_ptr;
    }
    Ok(out_ptr)
}

pub(crate) fn alloc_list_int_from_raw_slice(
    _py: &PyToken<'_>,
    elems: &[i64],
) -> Result<*mut u8, u64> {
    let Some(storage_ptr) = crate::object::layout::ListIntStorage::from_slice(elems) else {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    unsafe { alloc_list_int_with_storage(_py, storage_ptr) }
}

pub(crate) fn alloc_list_bool_from_raw_slice(
    _py: &PyToken<'_>,
    elems: &[u8],
) -> Result<*mut u8, u64> {
    let Some(storage_ptr) = crate::object::layout::ListBoolStorage::from_slice(elems) else {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    unsafe { alloc_list_bool_with_storage(_py, storage_ptr) }
}

pub(crate) fn alloc_list_int_filled(
    _py: &PyToken<'_>,
    len: usize,
    value: i64,
) -> Result<*mut u8, u64> {
    let Some(storage_ptr) = crate::object::layout::ListIntStorage::filled(len, value) else {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    unsafe { alloc_list_int_with_storage(_py, storage_ptr) }
}

pub(crate) fn alloc_list_bool_filled(
    _py: &PyToken<'_>,
    len: usize,
    value: u8,
) -> Result<*mut u8, u64> {
    let Some(storage_ptr) = crate::object::layout::ListBoolStorage::filled(len, value) else {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    unsafe { alloc_list_bool_with_storage(_py, storage_ptr) }
}

pub(crate) fn alloc_list_int_from_repeated_raw_slice(
    _py: &PyToken<'_>,
    elems: &[i64],
    times: usize,
) -> Result<*mut u8, u64> {
    let Some(storage_ptr) = crate::object::layout::ListIntStorage::repeated_slice(elems, times)
    else {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    unsafe { alloc_list_int_with_storage(_py, storage_ptr) }
}

pub(crate) fn alloc_list_bool_from_repeated_raw_slice(
    _py: &PyToken<'_>,
    elems: &[u8],
    times: usize,
) -> Result<*mut u8, u64> {
    let Some(storage_ptr) = crate::object::layout::ListBoolStorage::repeated_slice(elems, times)
    else {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    unsafe { alloc_list_bool_with_storage(_py, storage_ptr) }
}

pub(crate) fn alloc_list_int_from_raw_iter<F>(
    _py: &PyToken<'_>,
    len: usize,
    mut raw_at: F,
) -> Result<*mut u8, u64>
where
    F: FnMut(usize) -> i64,
{
    let Some(storage_ptr) = crate::object::layout::ListIntStorage::with_capacity(len) else {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    unsafe {
        let storage = &mut *storage_ptr;
        for idx in 0..len {
            if !storage.push(raw_at(idx)) {
                drop_list_int_storage(storage_ptr);
                return Err(raise_exception::<u64>(
                    _py,
                    "MemoryError",
                    "list allocation failed",
                ));
            }
        }
        alloc_list_int_with_storage(_py, storage_ptr)
    }
}

pub(crate) fn alloc_list_bool_from_raw_iter<F>(
    _py: &PyToken<'_>,
    len: usize,
    mut raw_at: F,
) -> Result<*mut u8, u64>
where
    F: FnMut(usize) -> u8,
{
    let Some(storage_ptr) = crate::object::layout::ListBoolStorage::with_capacity(len) else {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    unsafe {
        let storage = &mut *storage_ptr;
        for idx in 0..len {
            if !storage.push(raw_at(idx)) {
                drop_list_bool_storage(storage_ptr);
                return Err(raise_exception::<u64>(
                    _py,
                    "MemoryError",
                    "list allocation failed",
                ));
            }
        }
        alloc_list_bool_with_storage(_py, storage_ptr)
    }
}

pub(crate) fn alloc_list(_py: &PyToken<'_>, elems: &[u64]) -> *mut u8 {
    let cap = if elems.len() <= MAX_SMALL_LIST {
        MAX_SMALL_LIST
    } else {
        elems.len()
    };
    alloc_list_with_capacity(_py, elems, cap)
}

fn alloc_tuple_exact(_py: &PyToken<'_>, elems: &[u64], owned: bool) -> *mut u8 {
    let Some(total) = crate::object::layout::TupleStorage::object_size(elems.len()) else {
        return std::ptr::null_mut();
    };
    let ptr = alloc_object_with_aux(
        _py,
        total,
        TYPE_ID_TUPLE,
        ObjectAuxPreselection::ClassInline,
    );
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        crate::object::layout::tuple_storage_set_len_unpublished(ptr, elems.len());
        let items = crate::object::layout::tuple_storage_items_mut(ptr);
        std::ptr::copy_nonoverlapping(elems.as_ptr(), items, elems.len());
        if !owned {
            for &elem in elems {
                inc_ref_bits(_py, elem);
            }
        }
        if crate::object::refcount_opt::slice_contains_heap_refs(elems) {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
    }
    ptr
}

/// Allocate a fixed-length tuple whose slots begin as the invalid zero
/// construction sentinel. This is the
/// construction-only authority for `PyTuple_New`; later writes cannot grow it.
pub(crate) fn alloc_tuple_uninitialized(_py: &PyToken<'_>, len: usize) -> *mut u8 {
    if len == 0 {
        return alloc_tuple(_py, &[]);
    }
    let Some(total) = crate::object::layout::TupleStorage::object_size(len) else {
        return std::ptr::null_mut();
    };
    let ptr = alloc_object_with_aux(
        _py,
        total,
        TYPE_ID_TUPLE,
        ObjectAuxPreselection::ClassInline,
    );
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        crate::object::layout::tuple_storage_set_len_unpublished(ptr, len);
        let items = crate::object::layout::tuple_storage_items_mut(ptr);
        for index in 0..len {
            items.add(index).write(0);
        }
    }
    ptr
}

/// Allocate an exact tuple by transferring the caller's existing element
/// references. Ownership transfers only on success.
pub(crate) fn alloc_tuple_owned(_py: &PyToken<'_>, elems: &[u64]) -> *mut u8 {
    if elems.is_empty() {
        return alloc_tuple(_py, elems);
    }
    alloc_tuple_exact(_py, elems, true)
}

/// Runtime-owned authority for stable-address canonical heap objects.
///
/// Hits are lock-free. A miss is serialized through `singleton_init`, so a
/// fully initialized and immortal object is release-published exactly once and
/// no losing allocation can leak. The intern pool keeps its lock through
/// allocation and insertion for the same reason. Runtime teardown drains every
/// pointer from this owner before the state itself is dropped.
pub(crate) struct CanonicalObjectCache {
    singleton_init: std::sync::Mutex<()>,
    empty_tuple: std::sync::atomic::AtomicPtr<u8>,
    empty_string: std::sync::atomic::AtomicPtr<u8>,
    empty_bytes: std::sync::atomic::AtomicPtr<u8>,
    ascii_chars: [std::sync::atomic::AtomicPtr<u8>; 128],
    interned_strings: std::sync::Mutex<std::collections::HashMap<Box<[u8]>, usize>>,
}

impl CanonicalObjectCache {
    pub(crate) fn new() -> Self {
        Self {
            singleton_init: std::sync::Mutex::new(()),
            empty_tuple: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            empty_string: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            empty_bytes: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            ascii_chars: [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 128],
            interned_strings: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[inline]
unsafe fn prepare_canonical_object(ptr: *mut u8, interned: bool) {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        let flags = crate::object::HEADER_FLAG_IMMORTAL
            | if interned {
                crate::object::HEADER_FLAG_INTERNED
            } else {
                0
            };
        (*header).fetch_or_flags(flags);
        (*header).make_immortal();
    }
}

pub(crate) fn alloc_tuple(_py: &PyToken<'_>, elems: &[u64]) -> *mut u8 {
    // Fast path: return the immortal empty tuple singleton.
    if elems.is_empty() {
        let cache = &runtime_state(_py).canonical_objects;
        let cached = cache.empty_tuple.load(std::sync::atomic::Ordering::Acquire);
        if !cached.is_null() {
            return cached;
        }
        let _init = cache
            .singleton_init
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cached = cache.empty_tuple.load(std::sync::atomic::Ordering::Acquire);
        if !cached.is_null() {
            return cached;
        }
        let candidate = alloc_tuple_exact(_py, &[], false);
        if candidate.is_null() {
            return std::ptr::null_mut();
        }
        unsafe {
            crate::object::gc::gc_untrack(
                _py,
                candidate,
                TYPE_ID_TUPLE,
                crate::object::gc::GcUntrackReason::DynamicProjection,
            );
            prepare_canonical_object(candidate, true);
        }
        cache
            .empty_tuple
            .store(candidate, std::sync::atomic::Ordering::Release);
        return candidate;
    }
    alloc_tuple_exact(_py, elems, false)
}

pub(crate) fn alloc_range(
    _py: &PyToken<'_>,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> *mut u8 {
    use crate::object::heap_kinds_generated::HeapAcyclicSlot;
    if !acyclic_slot_edge(HeapAcyclicSlot::RangeStart, start_bits)
        || !acyclic_slot_edge(HeapAcyclicSlot::RangeStop, stop_bits)
        || !acyclic_slot_edge(HeapAcyclicSlot::RangeStep, step_bits)
    {
        raise_exception::<u64>(
            _py,
            "SystemError",
            "range constructor violated generated int_triplet acyclic capability",
        );
        return std::ptr::null_mut();
    }
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

#[inline]
fn acyclic_int_edge(bits: u64) -> bool {
    let obj = obj_from_bits(bits);
    obj.as_int().is_some()
        || obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BIGINT })
}

fn code_string_tuple_edge(bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return false;
    }
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| {
            items.iter().copied().all(|item| {
                obj_from_bits(item)
                    .as_ptr()
                    .is_some_and(|item_ptr| object_type_id(item_ptr) == TYPE_ID_STRING)
            })
        })
    }
    .unwrap_or(false)
}

#[inline]
pub(crate) fn acyclic_slot_edge(
    slot: crate::object::heap_kinds_generated::HeapAcyclicSlot,
    bits: u64,
) -> bool {
    use crate::object::heap_kinds_generated::{HeapAcyclicEdgeDomain, heap_acyclic_slot_domain};
    let obj = obj_from_bits(bits);
    match heap_acyclic_slot_domain(slot) {
        HeapAcyclicEdgeDomain::Int => acyclic_int_edge(bits),
        HeapAcyclicEdgeDomain::Str => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING }),
        HeapAcyclicEdgeDomain::BytesOrNone => {
            obj.is_none()
                || obj
                    .as_ptr()
                    .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTES })
        }
        HeapAcyclicEdgeDomain::StrTuple => code_string_tuple_edge(bits),
        HeapAcyclicEdgeDomain::StrOrNone => {
            obj.is_none()
                || obj
                    .as_ptr()
                    .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
        }
    }
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
    let ptr = alloc_object_with_aux(
        _py,
        total,
        TYPE_ID_GENERIC_ALIAS,
        ObjectAuxPreselection::ClassInline,
    );
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
    // Slots 0..9 are the function object fields (fn_ptr, arity, dict, closure,
    // code, trampoline, annotations, annotate, call_target, globals); slot 10
    // is the `__defaults__`/`__kwdefaults__` mutation version stamp, and slot 11
    // is the explicit globals-override flag used by `types.FunctionType`.
    // Slots 10/11 are plain u64 values, NOT refcounted objects — dealloc leaves
    // them alone. The defaults version is 0 at creation and bumped only on a
    // user-reachable mutation of defaults attrs, so compile-time defaults stay
    // valid IFF the version is still 0 ("never mutated since creation").
    let total = std::mem::size_of::<MoltHeader>() + 12 * std::mem::size_of::<u64>();
    let ptr = alloc_object_with_aux(
        _py,
        total,
        TYPE_ID_FUNCTION,
        ObjectAuxPreselection::ClassInline,
    );
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = fn_ptr;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = arity;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        std::ptr::write(
            ptr.add(4 * std::mem::size_of::<u64>()) as *mut std::sync::atomic::AtomicU64,
            std::sync::atomic::AtomicU64::new(0),
        );
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        let none_bits = MoltObject::none().bits();
        *(ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64) = none_bits;
        *(ptr.add(8 * std::mem::size_of::<u64>()) as *mut *const ()) = std::ptr::null();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(call_target) = crate::builtins::functions::runtime_callable_target_ptr(fn_ptr) {
            crate::object::layout::function_set_call_target_ptr(ptr, call_target);
        }
        *(ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(10 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(11 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        inc_ref_bits(_py, none_bits);
    }
    ptr
}

#[allow(clippy::too_many_arguments)]
/// Allocate a code object and retain all object-valued fields.
///
/// `filename_bits`, `name_bits`, `linetable_bits`, `varnames_bits`, and
/// `names_bits` are borrowed inputs.  The returned code object owns one
/// reference to each non-zero field; callers that created temporary field
/// objects must drop their creator reference after this constructor returns.
pub(crate) fn alloc_code_obj(
    _py: &PyToken<'_>,
    filename_bits: u64,
    name_bits: u64,
    firstlineno: i64,
    linetable_bits: u64,
    varnames_bits: u64,
    names_bits: u64,
    argcount: u64,
    posonlyargcount: u64,
    kwonlyargcount: u64,
) -> *mut u8 {
    use crate::object::heap_kinds_generated::HeapAcyclicSlot;
    if !acyclic_slot_edge(HeapAcyclicSlot::CodeFilename, filename_bits)
        || !acyclic_slot_edge(HeapAcyclicSlot::CodeName, name_bits)
        || !acyclic_slot_edge(HeapAcyclicSlot::CodeLinetable, linetable_bits)
        || !acyclic_slot_edge(HeapAcyclicSlot::CodeVarnames, varnames_bits)
        || !acyclic_slot_edge(HeapAcyclicSlot::CodeNames, names_bits)
    {
        raise_exception::<u64>(
            _py,
            "SystemError",
            "code constructor violated generated code_metadata acyclic capability",
        );
        return std::ptr::null_mut();
    }
    // Slots 0..8 are CPython-visible code facts, 9..11 hold the Molt callable
    // identity, and 12..16 retain immutable signature facts used by
    // `types.FunctionType` reconstruction.
    let total = std::mem::size_of::<MoltHeader>() + 17 * std::mem::size_of::<u64>();
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
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = names_bits;
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64) = argcount;
        *(ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64) = posonlyargcount;
        *(ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64) = kwonlyargcount;
        *(ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(10 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(11 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(12 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(13 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(14 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(15 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(16 * std::mem::size_of::<u64>()) as *mut u64) = 0;
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
        if names_bits != 0 {
            inc_ref_bits(_py, names_bits);
        }
    }
    ptr
}

pub(crate) fn alloc_bound_method_obj(_py: &PyToken<'_>, func_bits: u64, self_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object_with_aux(
        _py,
        total,
        TYPE_ID_BOUND_METHOD,
        ObjectAuxPreselection::ClassInline,
    );
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
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let name_key_ptr = alloc_string(_py, b"__name__");
    if name_key_ptr.is_null() {
        dec_ref_bits(_py, dict_bits);
        return std::ptr::null_mut();
    }
    let name_key_bits = MoltObject::from_ptr(name_key_ptr).bits();
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_MODULE);
    if ptr.is_null() {
        dec_ref_bits(_py, name_key_bits);
        dec_ref_bits(_py, dict_bits);
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = name_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = dict_bits;
        inc_ref_bits(_py, name_bits);
        dict_set_in_place(_py, dict_ptr, name_key_bits, name_bits);
        dec_ref_bits(_py, name_key_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        }
    }
    ptr
}

pub(crate) fn alloc_class_obj(_py: &PyToken<'_>, name_bits: u64) -> *mut u8 {
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let bases_bits = MoltObject::none().bits();
    let mro_bits = MoltObject::none().bits();
    // Eight object-reference/layout slots, one atomic cold-policy word, then
    // an internal write-once payload-size cache. The cache is not a
    // Python attribute and cannot be forged through the class namespace.
    let total = std::mem::size_of::<MoltHeader>() + 10 * std::mem::size_of::<u64>();
    let ptr = alloc_object_with_aux(_py, total, TYPE_ID_TYPE, ObjectAuxPreselection::ClassInline);
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
        std::ptr::write(
            ptr.add(8 * std::mem::size_of::<u64>()) as *mut MoltAuxWord,
            MoltAuxWord::new(0),
        );
        std::ptr::write(
            ptr.add(9 * std::mem::size_of::<u64>()) as *mut std::sync::atomic::AtomicUsize,
            std::sync::atomic::AtomicUsize::new(0),
        );
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

fn canonical_bytes_like(
    _py: &PyToken<'_>,
    slot: &std::sync::atomic::AtomicPtr<u8>,
    bytes: &[u8],
    type_id: u32,
    interned: bool,
) -> *mut u8 {
    let cached = slot.load(std::sync::atomic::Ordering::Acquire);
    if !cached.is_null() {
        return cached;
    }
    let cache = &runtime_state(_py).canonical_objects;
    let _init = cache
        .singleton_init
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cached = slot.load(std::sync::atomic::Ordering::Acquire);
    if !cached.is_null() {
        return cached;
    }
    let ptr = alloc_bytes_like_with_len(_py, bytes.len(), type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            ptr.add(std::mem::size_of::<usize>()),
            bytes.len(),
        );
        prepare_canonical_object(ptr, interned);
    }
    slot.store(ptr, std::sync::atomic::Ordering::Release);
    ptr
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
    let slot = &runtime_state(_py).canonical_objects.ascii_chars[byte as usize];
    let raw = canonical_bytes_like(_py, slot, bytes, TYPE_ID_STRING, true);
    if raw.is_null() { None } else { Some(raw) }
}

pub(crate) fn alloc_interned_string(_py: &PyToken<'_>, bytes: &[u8]) -> *mut u8 {
    if bytes.is_empty() {
        let slot = &runtime_state(_py).canonical_objects.empty_string;
        return canonical_bytes_like(_py, slot, bytes, TYPE_ID_STRING, true);
    }
    if let Some(ptr) = try_intern_ascii_char(_py, bytes) {
        return ptr;
    }
    let cache = &runtime_state(_py).canonical_objects;
    let mut pool = cache
        .interned_strings
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(&raw) = pool.get(bytes) {
        return raw as *mut u8;
    }
    let ptr = alloc_bytes_like_with_len(_py, bytes.len(), TYPE_ID_STRING);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            ptr.add(std::mem::size_of::<usize>()),
            bytes.len(),
        );
        prepare_canonical_object(ptr, true);
    }
    pool.insert(bytes.to_vec().into_boxed_slice(), ptr as usize);
    ptr
}

pub(crate) fn alloc_string(_py: &PyToken<'_>, bytes: &[u8]) -> *mut u8 {
    if bytes.is_empty() {
        return alloc_interned_string(_py, bytes);
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
        return alloc_interned_string(_py, bytes);
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

/// Allocate a string without any interning/caching lookups.
///
/// This is the fast path for string method results (upper, lower, strip, etc.)
/// where we know the result is a freshly-computed string that is unlikely to
/// benefit from interning (it's typically discarded immediately). Skips the
/// ASCII check, identifier check, and intern pool lock that `alloc_string`
/// performs on every call.
pub(crate) fn alloc_string_nointern(_py: &PyToken<'_>, bytes: &[u8]) -> *mut u8 {
    if bytes.is_empty() {
        return alloc_string(_py, bytes);
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

pub(crate) fn alloc_bytes(_py: &PyToken<'_>, bytes: &[u8]) -> *mut u8 {
    if bytes.is_empty() {
        let slot = &runtime_state(_py).canonical_objects.empty_bytes;
        return canonical_bytes_like(_py, slot, bytes, TYPE_ID_BYTES, false);
    }
    alloc_bytes_like(_py, bytes, TYPE_ID_BYTES)
}

pub(crate) fn clear_builder_singletons(_py: &PyToken<'_>, state: &crate::RuntimeState) {
    crate::gil_assert();
    let cache = &state.canonical_objects;
    let init = cache
        .singleton_init
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut singleton_ptrs = [std::ptr::null_mut(); 131];
    for (index, slot) in [&cache.empty_tuple, &cache.empty_string, &cache.empty_bytes]
        .into_iter()
        .enumerate()
    {
        singleton_ptrs[index] =
            slot.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel);
    }
    for (index, slot) in cache.ascii_chars.iter().enumerate() {
        singleton_ptrs[index + 3] =
            slot.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel);
    }
    let mut pool = cache
        .interned_strings
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let interned = std::mem::take(&mut *pool);
    drop(pool);
    drop(init);

    // These domains are disjoint by construction: empty values have dedicated
    // slots, every one-byte ASCII string has a dedicated slot, and the pool
    // only admits remaining nonempty strings. Teardown must not hide an
    // authority collision behind deduplication.
    #[cfg(debug_assertions)]
    {
        for (index, &ptr) in singleton_ptrs.iter().enumerate() {
            if !ptr.is_null() {
                assert!(!singleton_ptrs[index + 1..].contains(&ptr));
                assert!(!interned.values().any(|&raw| raw == ptr as usize));
            }
        }
    }
    for ptr in singleton_ptrs
        .into_iter()
        .chain(interned.into_values().map(|raw| raw as *mut u8))
    {
        if !ptr.is_null() {
            crate::object::release_shutdown_owned_bits(_py, MoltObject::from_ptr(ptr).bits());
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
        let Some(vec_ptr) = crate::object::backing::tracked_vec_box_from_slice(bytes, cap) else {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
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
        let Some(vec_ptr) = crate::object::backing::tracked_vec_box_zeroed::<u8>(len) else {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
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

pub(crate) fn alloc_memoryview_from_storage(
    _py: &PyToken<'_>,
    storage: crate::object::memoryview::TypedStridedStorage,
) -> *mut u8 {
    if storage.format_bits == 0
        || (storage.base_bits == 0 && storage.data.is_null() && storage.span_len != 0)
    {
        return std::ptr::null_mut();
    }
    let data = unsafe {
        if !storage.data.is_null() {
            storage.data
        } else if storage.base_bits == 0 && storage.span_len == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            let base = obj_from_bits(storage.base_bits);
            let Some(base_ptr) = base.as_ptr() else {
                return std::ptr::null_mut();
            };
            let Some(base_slice) = bytes_like_slice_raw(base_ptr) else {
                return std::ptr::null_mut();
            };
            if !storage.fits_in_base_len(base_slice.len()) {
                return std::ptr::null_mut();
            }
            if storage.offset < 0 {
                return std::ptr::null_mut();
            }
            base_slice.as_ptr().add(storage.offset as usize).cast_mut()
        }
    };
    if !storage.data.is_null() && storage.base_bits != 0 {
        let base = obj_from_bits(storage.base_bits);
        if let Some(base_ptr) = base.as_ptr()
            && let Some(base_slice) = unsafe { bytes_like_slice_raw(base_ptr) }
            && !storage.fits_in_base_len(base_slice.len())
        {
            return std::ptr::null_mut();
        }
    }
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<MemoryView>();
    let ptr = alloc_object(_py, total, TYPE_ID_MEMORYVIEW);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let Some(shape_ptr) = crate::object::backing::tracked_vec_box_from_slice(
            storage.shape.as_slice(),
            storage.shape.len(),
        ) else {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        let Some(strides_ptr) = crate::object::backing::tracked_vec_box_from_slice(
            storage.strides.as_slice(),
            storage.strides.len(),
        ) else {
            drop(crate::object::backing::tracked_vec_box_from_raw(shape_ptr));
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        let mv_ptr = memoryview_ptr(ptr);
        (*mv_ptr).owner_bits = storage.base_bits;
        (*mv_ptr).base_bits = storage.base_bits;
        (*mv_ptr).data = data;
        (*mv_ptr).offset = storage.offset;
        (*mv_ptr).len = storage.memoryview_len_field();
        (*mv_ptr).itemsize = storage.itemsize;
        (*mv_ptr).stride = storage.memoryview_stride_field();
        (*mv_ptr).readonly = if storage.readonly { 1 } else { 0 };
        (*mv_ptr).ndim = storage.shape.len() as u8;
        (*mv_ptr).released = 0;
        (*mv_ptr)._pad = [0; 5];
        (*mv_ptr).format_bits = storage.format_bits;
        (*mv_ptr).shape_ptr = shape_ptr;
        (*mv_ptr).strides_ptr = strides_ptr;
    }
    if storage.base_bits != 0 {
        inc_ref_bits(_py, storage.base_bits);
    }
    inc_ref_bits(_py, storage.format_bits);
    ptr
}

#[cfg(test)]
mod tests {
    use super::{acyclic_slot_edge, alloc_function_obj};
    use crate::object::heap_kinds_generated::HeapAcyclicSlot;
    use crate::{
        TYPE_ID_FUNCTION, alloc_bytes, alloc_list, alloc_string, alloc_tuple, dec_ref_bits,
        function_globals_bits, object_type_id,
    };
    use molt_obj_model::MoltObject;

    extern "C" fn allocator_inert_function_target() -> u64 {
        MoltObject::none().bits()
    }

    #[test]
    fn function_allocator_does_not_eagerly_capture_globals() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = alloc_function_obj(
                _py,
                allocator_inert_function_target as *const () as usize as u64,
                0,
            );
            assert!(!ptr.is_null());
            assert_eq!(unsafe { object_type_id(ptr) }, TYPE_ID_FUNCTION);
            assert_eq!(
                unsafe { function_globals_bits(ptr) },
                0,
                "function creation must be inert; metadata or FunctionType owns globals installation"
            );
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        });
    }

    #[test]
    fn closed_acyclic_capabilities_reject_heap_backedge_domains() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            assert!(acyclic_slot_edge(
                HeapAcyclicSlot::RangeStart,
                MoltObject::from_int(7).bits(),
            ));
            assert!(!acyclic_slot_edge(
                HeapAcyclicSlot::RangeStart,
                MoltObject::from_float(7.0).bits(),
            ));

            let text_ptr = alloc_string(_py, b"name");
            let text_bits = MoltObject::from_ptr(text_ptr).bits();
            let bytes_ptr = alloc_bytes(_py, b"line-table");
            let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
            let list_ptr = alloc_list(_py, &[text_bits]);
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let valid_tuple_ptr = alloc_tuple(_py, &[text_bits]);
            let valid_tuple_bits = MoltObject::from_ptr(valid_tuple_ptr).bits();
            let invalid_tuple_ptr = alloc_tuple(_py, &[list_bits]);
            let invalid_tuple_bits = MoltObject::from_ptr(invalid_tuple_ptr).bits();

            assert!(acyclic_slot_edge(
                HeapAcyclicSlot::CodeLinetable,
                bytes_bits,
            ));
            assert!(acyclic_slot_edge(
                HeapAcyclicSlot::CodeLinetable,
                MoltObject::none().bits(),
            ));
            assert!(acyclic_slot_edge(
                HeapAcyclicSlot::CodeVarnames,
                valid_tuple_bits,
            ));
            assert!(acyclic_slot_edge(HeapAcyclicSlot::CodeVararg, text_bits));
            assert!(acyclic_slot_edge(
                HeapAcyclicSlot::CodeVararg,
                MoltObject::none().bits(),
            ));
            for bad_slot in [
                HeapAcyclicSlot::CodeLinetable,
                HeapAcyclicSlot::CodeVarnames,
                HeapAcyclicSlot::CodeVararg,
            ] {
                assert!(!acyclic_slot_edge(bad_slot, list_bits));
            }
            assert!(!acyclic_slot_edge(
                HeapAcyclicSlot::CodeVarnames,
                invalid_tuple_bits,
            ));

            dec_ref_bits(_py, invalid_tuple_bits);
            dec_ref_bits(_py, valid_tuple_bits);
            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, bytes_bits);
            dec_ref_bits(_py, text_bits);
        });
    }
}
