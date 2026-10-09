use crate::PyToken;
use crate::object::layout::{
    CodeExecutionKind, code_callable_identity, code_cellvars_bits, code_frame_slot_id,
    code_freevars_bits, code_protocol_flags, code_publish_callable_identity,
    code_publish_lexical_metadata, code_publish_policy, code_published_execution_kind,
    code_set_frame_slot_id, code_set_signature_bits,
};
use crate::object::{ClassEdgeOwnership, MoltAuxWord, object_init_class_edge_unpublished};
use crate::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_header_size() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { std::mem::size_of::<MoltHeader>() as u64 })
}

#[unsafe(no_mangle)]
/// Allocate compiler-owned boxed fields, not arbitrary raw bytes. The same
/// generated shape owns field traversal, cycle detachment and terminal release.
pub extern "C" fn molt_alloc(size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(total_size) = crate::object::boxed_object_total_size(_py, size_bits) else {
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
        let _ = crate::object::ops_slice::dataclass_finish_construction_unpublished(
            _py, inst_ptr, class_bits,
        );
        if exception_pending(_py) {
            dec_ref_bits(_py, inst_bits);
            return Some(MoltObject::none().bits());
        }
        Some(inst_bits)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_alloc_class(size_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // The ABI carries a raw byte extent, never a boxed Python integer.
        let Some(size) = usize_from_bits(size_bits) else {
            return MoltObject::none().bits();
        };
        alloc_class_instance(_py, size, class_bits)
    })
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

/// Allocate with a native byte extent; only the external ABI decodes raw u64 sizes.
pub(crate) fn alloc_class_instance(_py: &PyToken<'_>, size: usize, class_bits: u64) -> u64 {
    let mut type_id = TYPE_ID_OBJECT;
    if class_bits != 0 {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
            if crate::object::class_finish_definition(_py, class_ptr).is_err() {
                return MoltObject::none().bits();
            }
            type_id = crate::object::class_instance_type_id(class_ptr);
        }
    }
    if !super::heap_kind_has_class_shape(type_id) {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "native payload requires its declaring constructor",
        );
    }
    if let Some(class) = obj_from_bits(class_bits).as_ptr()
        && unsafe { super::layout::class_cached_layout_size(class) }
            .is_some_and(|required| size < required)
    {
        return raise_exception::<u64>(
            _py,
            "SystemError",
            "instance allocation is smaller than sealed class layout",
        );
    }
    let Some(total_size) = crate::object::checked_object_total_size(size) else {
        record_memory_error_without_allocation(_py);
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
        if !super::layout::class_initialize_native_prefix_unpublished(_py, obj_ptr) {
            dec_ref_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<u64>(
                _py,
                "SystemError",
                "instance allocation cannot initialize its native prefix",
            );
        }
        if let Some(class) = obj_from_bits(class_bits).as_ptr()
            && super::field_storage::initialize_fields(_py, obj_ptr, class, size).is_err()
        {
            dec_ref_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            return MoltObject::none().bits();
        }
        // Prepare eager dictionary backing under local ownership. Until the
        // class edge exists, ordinary dictionary access and lifecycle traversal
        // deliberately reject this classless allocation's trailing word.
        let dictionary = if type_id == TYPE_ID_MODULE {
            let dictionary = alloc_dict_with_pairs(_py, &[]);
            if dictionary.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(dictionary).bits()
        } else {
            0
        };
        // Commit the class and prepared dictionary without allocation or Python
        // callbacks between the stores. Rollback before this point must never
        // invoke a user finalizer against incomplete native backing.
        if class_bits != 0
            && !object_init_class_edge_unpublished(
                _py,
                obj_ptr,
                class_bits,
                ClassEdgeOwnership::Owned,
            )
        {
            if dictionary != 0 {
                dec_ref_bits(_py, dictionary);
            }
            dec_ref_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            return raise_exception::<u64>(
                _py,
                "SystemError",
                "instance allocation cannot initialize its class edge",
            );
        }
        if dictionary != 0 {
            super::instance_set_dict_bits(_py, obj_ptr, dictionary);
        }
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

/// Allocate the shared dict/set/frozenset storage transaction, still unpublished.
/// Each admitted buffer immediately belongs to the zeroed object, so the normal
/// lifecycle is the single rollback authority even after partial allocation.
fn alloc_hash_aggregate_unpublished(_py: &PyToken<'_>, capacity: usize, type_id: u32) -> *mut u8 {
    alloc_hash_aggregate_for_class_unpublished(_py, capacity, type_id, None)
}

fn alloc_hash_aggregate_for_class_unpublished(
    _py: &PyToken<'_>,
    capacity: usize,
    type_id: u32,
    class: Option<u64>,
) -> *mut u8 {
    debug_assert!(matches!(
        type_id,
        TYPE_ID_DICT | TYPE_ID_SET | TYPE_ID_FROZENSET
    ));
    if exception_pending(_py) {
        return std::ptr::null_mut();
    }
    let table_capacity = if capacity == 0 {
        0
    } else {
        let Some(capacity) = crate::object::ops::checked_dict_table_capacity(capacity) else {
            record_memory_error_without_allocation(_py);
            return std::ptr::null_mut();
        };
        capacity
    };
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<HashStorage<DictEntry>>();
    let ptr = if let Some(class) = class {
        let kind = match type_id {
            TYPE_ID_SET => super::native_instance::NativePayload::Set,
            TYPE_ID_FROZENSET => super::native_instance::NativePayload::Frozenset,
            _ => unreachable!("dict subclasses use their established class shape"),
        };
        unsafe { super::native_instance::alloc_unpublished(_py, class, kind, 0) }
    } else {
        crate::object::alloc_object_zeroed_unpublished_with_aux(
            _py,
            total,
            type_id,
            if type_id == TYPE_ID_FROZENSET {
                ObjectAuxPreselection::StateInline
            } else {
                ObjectAuxPreselection::Default
            },
        )
    };
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let initialized = (|| {
            if type_id == TYPE_ID_DICT {
                dict_storage(ptr).entries =
                    crate::object::backing::tracked_vec_box_with_capacity::<DictEntry>(capacity)?;
                dict_storage(ptr).table =
                    crate::object::backing::tracked_vec_box_zeroed::<usize>(table_capacity)?;
            } else {
                set_storage(ptr).entries =
                    crate::object::backing::tracked_vec_box_with_capacity::<SetEntry>(capacity)?;
                set_storage(ptr).table =
                    crate::object::backing::tracked_vec_box_zeroed::<usize>(table_capacity)?;
            }
            Some(())
        })();
        if initialized.is_none() {
            record_memory_error_without_allocation(_py);
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        }
    }
    ptr
}

/// Storage and initial edges remain unpublished until complete. Each write
/// applies the shared sticky GC tracking law; publication exposes the complete
/// payload without scanning it again or forgetting a replaced trackable edge.
pub(crate) fn alloc_dict_with_capacity_and_pairs(
    _py: &PyToken<'_>,
    capacity_hint: usize,
    pairs: &[u64],
) -> *mut u8 {
    let ptr =
        alloc_hash_aggregate_unpublished(_py, capacity_hint.max(pairs.len() / 2), TYPE_ID_DICT);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        for pair in pairs.chunks(2) {
            if pair.len() == 2 {
                dict_set_in_place(_py, ptr, pair[0], pair[1]);
                if exception_pending(_py) {
                    dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                    return std::ptr::null_mut();
                }
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
    alloc_set_like_with_capacity_and_entries(_py, entries.len(), entries, type_id)
}

pub(crate) fn alloc_set_like_with_capacity_and_entries(
    _py: &PyToken<'_>,
    capacity_hint: usize,
    entries: &[u64],
    type_id: u32,
) -> *mut u8 {
    let ptr = alloc_hash_aggregate_unpublished(_py, capacity_hint.max(entries.len()), type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        for &entry in entries {
            set_add_in_place(_py, ptr, entry, HashContext::SetElement);
            if exception_pending(_py) {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return std::ptr::null_mut();
            }
        }
        crate::object::gc::gc_publish_initialized(_py, ptr);
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
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
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
    preserve_error: bool,
}

impl PtrDropGuard {
    pub(crate) fn new(ptr: *mut u8) -> Self {
        Self {
            ptr,
            active: !ptr.is_null(),
            preserve_error: false,
        }
    }

    /// Callback-capable temporary owners must not replace a pending operation
    /// error when their release reenters Python or a foreign deallocator.
    pub(crate) fn preserving(ptr: *mut u8) -> Self {
        let mut guard = Self::new(ptr);
        guard.preserve_error = true;
        guard
    }

    pub(crate) fn release(&mut self) {
        self.active = false;
    }
}

impl Drop for PtrDropGuard {
    fn drop(&mut self) {
        if self.active && !self.ptr.is_null() {
            let release = || unsafe {
                if trace_callargs_enabled() && object_type_id(self.ptr) == TYPE_ID_CALLARGS {
                    let args_ptr = crate::call::bind::callargs_ptr(self.ptr);
                    eprintln!(
                        "[molt callargs] guard_drop builder_ptr=0x{:x} args_ptr=0x{:x}",
                        self.ptr as usize, args_ptr as usize,
                    );
                }
                molt_dec_ref(self.ptr);
            };
            if self.preserve_error {
                molt_cpython_abi::api::errors::with_preserved_error(release);
            } else {
                release();
            }
        }
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` points to a live list builder. `val` is
/// borrowed. Return 0 after retaining it into owned storage, or 1 on failure
/// without retaining it. A pending exception must abort the builder, not publish
/// its partial contents.
pub unsafe extern "C" fn molt_list_builder_append(builder_bits: u64, val: u64) -> i32 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                return 1;
            }
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                raise_exception::<()>(_py, "RuntimeError", "invalid list builder");
                return 1;
            }
            let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
            if vec_ptr.is_null() {
                raise_exception::<()>(_py, "RuntimeError", "consumed list builder");
                return 1;
            }
            let vec = &mut *vec_ptr;
            if !crate::object::backing::tracked_vec_reserve_or_raise(
                _py,
                vec_ptr,
                vec.len().saturating_add(1),
                "list allocation failed",
            ) {
                return 1;
            }
            inc_ref_bits(_py, val);
            vec.push(val);
            0
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must transfer one live list-builder owner. Success transfers all
/// retained elements into the list; failure releases them and returns None.
pub unsafe extern "C" fn molt_list_builder_finish(builder_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            finish_sequence_builder(_py, builder_bits, |py, values, capacity| {
                alloc_list_with_capacity_owned(py, values, capacity.max(MAX_SMALL_LIST))
            })
        })
    }
}

/// The list finish export consumes the builder and its owned element references.
/// A preexisting failure drops the partial builder without allocating a result.
/// After detaching storage, allocator failure releases each element exactly once.
unsafe fn finish_sequence_builder(
    py: &PyToken<'_>,
    builder_bits: u64,
    allocate: impl FnOnce(&PyToken<'_>, &[u64], usize) -> *mut u8,
) -> u64 {
    unsafe {
        let builder_ptr = ptr_from_bits(builder_bits);
        if builder_ptr.is_null() {
            if !exception_pending(py) {
                raise_exception::<()>(py, "RuntimeError", "invalid list builder");
            }
            return MoltObject::none().bits();
        }
        let _guard = PtrDropGuard::new(builder_ptr);
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
        if vec_ptr.is_null() {
            return raise_exception::<_>(py, "RuntimeError", "consumed list builder");
        }
        *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();
        let vec = crate::object::backing::tracked_vec_box_from_raw(vec_ptr);
        let result = allocate(py, vec.as_slice(), vec.capacity());
        if result.is_null() {
            if !exception_pending(py) {
                crate::record_memory_error_without_allocation(py);
            }
            for &elem in vec.iter() {
                dec_ref_bits(py, elem);
            }
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(result).bits()
        }
    }
}

/// Decode a compiler-owned borrowed word range. All fixed aggregate consumers
/// preserve an existing exception and use the same target-width/range checks.
///
/// # Safety
/// A nonempty range must name initialized words that remain live for the borrow.
pub(crate) unsafe fn borrowed_constructor_values<'a>(
    py: &PyToken<'_>,
    address: u64,
    len: u64,
) -> Option<&'a [u64]> {
    if exception_pending(py) {
        return None;
    }
    let Some(ptr) = crate::provenance::abi::const_ptr::<u64>(address) else {
        return raise_exception(
            py,
            "MemoryError",
            "constructor values address exceeds the active address space",
        );
    };
    let Some(values) = (unsafe { crate::provenance::abi::slice(ptr, len) }) else {
        return raise_exception(
            py,
            "RuntimeError",
            "constructor values range is invalid for the active target",
        );
    };
    Some(values)
}

#[derive(Clone, Copy)]
enum FixedSequenceKind {
    List,
    Tuple,
}

unsafe fn sequence_from_values(
    py: &PyToken<'_>,
    address: u64,
    len: u64,
    kind: FixedSequenceKind,
) -> u64 {
    let Some(values) = (unsafe { borrowed_constructor_values(py, address, len) }) else {
        return MoltObject::none().bits();
    };
    let ptr = match kind {
        FixedSequenceKind::List => alloc_list(py, values),
        FixedSequenceKind::Tuple => alloc_tuple(py, values),
    };
    if ptr.is_null() {
        if !exception_pending(py) {
            crate::record_memory_error_without_allocation(py);
        }
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

/// Construct a tuple by copying and retaining a borrowed word range.
/// # Safety
/// The address must encode `len` initialized, live NaN-boxed words, or null for zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_tuple_from_values(address: u64, len: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { sequence_from_values(py, address, len, FixedSequenceKind::Tuple) }
    })
}

/// Construct a fresh mutable list by copying and retaining a borrowed word range.
/// # Safety
/// The address must encode `len` initialized, live NaN-boxed words, or null for zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_list_from_values(address: u64, len: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { sequence_from_values(py, address, len, FixedSequenceKind::List) }
    })
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

/// Adopt retained, resource-accounted snapshot backing without copying its
/// elements or repeating INCREF/DECREF traffic. Failure leaves the snapshot's
/// ordinary owner responsible for releasing every element and backing charge.
pub(crate) fn alloc_list_from_snapshot(
    py: &PyToken<'_>,
    snapshot: super::seq_access::PinnedSequenceSnapshot<'_, '_>,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut DataclassDesc>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let object = alloc_object_with_aux(py, total, TYPE_ID_LIST, ObjectAuxPreselection::ClassInline);
    if object.is_null() {
        return object;
    }
    let heap_edges = super::refcount_opt::slice_heap_ref_count(&snapshot);
    let storage = snapshot.into_owned_values().into_raw();
    unsafe {
        super::backing::tracked_vec_set_heap_edge_count(storage, heap_edges);
        *(object as *mut *mut Vec<u64>) = storage;
        if heap_edges != 0 {
            (*header_from_obj_ptr(object)).fetch_or_flags(super::HEADER_FLAG_CONTAINS_REFS);
        }
    }
    object
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
    py: &PyToken<'_>,
    elems: &[i64],
) -> Result<*mut u8, u64> {
    alloc_list_int_from_raw_iter(py, elems.len(), |index| elems[index])
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

/// Retain a supplied Python fill owner whenever it is not an inline integer.
pub(crate) fn alloc_list_int_from_fill(
    py: &PyToken<'_>,
    len: usize,
    fill: u64,
) -> Result<*mut u8, u64> {
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    if let Some(value) = crate::object::layout::InlineListInt::from_bits(fill) {
        let Some(storage) = crate::object::layout::ListIntStorage::filled(len, value) else {
            return Err(raise_exception::<u64>(
                py,
                "MemoryError",
                "list allocation failed",
            ));
        };
        return unsafe { alloc_list_int_with_storage(py, storage) };
    }
    let ptr = alloc_list_filled(py, len, obj_from_bits(fill));
    if ptr.is_null() {
        if !exception_pending(py) {
            crate::record_memory_error_without_allocation(py);
        }
        Err(MoltObject::none().bits())
    } else {
        Ok(ptr)
    }
}

#[cfg(test)]
pub(crate) fn alloc_list_int_filled(
    py: &PyToken<'_>,
    len: usize,
    value: i64,
) -> Result<*mut u8, u64> {
    if len == 0 {
        return alloc_list_int_from_raw_slice(py, &[]);
    }
    let fill = int_bits_from_i64(py, value);
    let result = alloc_list_int_from_fill(py, len, fill);
    dec_ref_bits(py, fill);
    result
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
    py: &PyToken<'_>,
    elems: &[i64],
    times: usize,
) -> Result<*mut u8, u64> {
    let Some(len) = elems.len().checked_mul(times) else {
        return Err(raise_exception::<u64>(
            py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    if len == 0 {
        return alloc_list_int_from_raw_slice(py, &[]);
    }
    if elems
        .iter()
        .all(|&value| crate::object::layout::InlineListInt::from_raw(value).is_some())
    {
        return alloc_list_int_from_raw_iter(py, len, |index| elems[index % elems.len()]);
    }
    // Materialize source occurrences once, then repeat their retained owners.
    let source = alloc_list_int_from_raw_slice(py, elems)?;
    let guard = PtrDropGuard::new(source);
    let count = match i64::try_from(times) {
        Ok(count) => count,
        Err(_) => {
            return Err(raise_exception::<u64>(
                py,
                "MemoryError",
                "list allocation failed",
            ));
        }
    };
    let repeated = crate::object::ops_arith::repeat_sequence(py, source, count);
    drop(guard);
    repeated
        .and_then(|bits| obj_from_bits(bits).as_ptr())
        .ok_or_else(|| MoltObject::none().bits())
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
    py: &PyToken<'_>,
    len: usize,
    mut raw_at: F,
) -> Result<*mut u8, u64>
where
    F: FnMut(usize) -> i64,
{
    use crate::object::layout::{InlineListInt, ListIntStorage};
    let none = MoltObject::none().bits();
    if exception_pending(py) {
        return Err(none);
    }
    let Some(storage_ptr) = ListIntStorage::with_capacity(len) else {
        return Err(raise_exception::<u64>(
            py,
            "MemoryError",
            "list allocation failed",
        ));
    };
    // Build the inline prefix without Python owners. The first heap-sized
    // value moves that prefix through canonical promotion exactly once.
    for index in 0..len {
        let value = raw_at(index);
        if let Some(value) = InlineListInt::from_raw(value) {
            if unsafe { !(*storage_ptr).push(value) } {
                unsafe { drop_list_int_storage(storage_ptr) };
                return Err(raise_exception::<u64>(
                    py,
                    "MemoryError",
                    "list allocation failed",
                ));
            }
            continue;
        }
        let ptr = unsafe { alloc_list_int_with_storage(py, storage_ptr)? };
        let mut guard = PtrDropGuard::new(ptr);
        unsafe { crate::object::ops_list::promote_list_int_to_list(py, ptr) };
        if exception_pending(py) {
            return Err(none);
        }
        for current in index..len {
            let raw = if current == index {
                value
            } else {
                raw_at(current)
            };
            let bits = int_bits_from_i64(py, raw);
            if exception_pending(py) {
                dec_ref_bits(py, bits);
                return Err(none);
            }
            let appended = unsafe { crate::object::list_mutation::append(py, ptr, bits) };
            dec_ref_bits(py, bits);
            if !appended {
                return Err(none);
            }
        }
        guard.release();
        return Ok(ptr);
    }
    unsafe { alloc_list_int_with_storage(py, storage_ptr) }
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

fn alloc_tuple_unpublished(
    _py: &PyToken<'_>,
    elems: &[u64],
    owned: bool,
    class: Option<u64>,
) -> *mut u8 {
    let Some(total) = crate::object::layout::TupleStorage::object_size(elems.len()) else {
        record_memory_error_without_allocation(_py);
        return std::ptr::null_mut();
    };
    let ptr = if let Some(class) = class {
        unsafe {
            super::native_instance::alloc_unpublished(
                _py,
                class,
                super::native_instance::NativePayload::Tuple,
                elems.len(),
            )
        }
    } else {
        crate::object::alloc_object_zeroed_unpublished_with_aux(
            _py,
            total,
            TYPE_ID_TUPLE,
            ObjectAuxPreselection::ClassInline,
        )
    };
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

fn alloc_tuple_exact(_py: &PyToken<'_>, elems: &[u64], owned: bool) -> *mut u8 {
    let ptr = alloc_tuple_unpublished(_py, elems, owned, None);
    if !ptr.is_null() {
        unsafe {
            crate::object::gc::gc_publish_initialized(_py, ptr);
        }
    }
    ptr
}

/// Fresh subtype tuples share native-extension admission, field addressing and
/// lifecycle with every other native subtype. Exact tuple owners never move.
pub(crate) unsafe fn alloc_tuple_subclass(py: &PyToken<'_>, class: u64, elems: &[u64]) -> u64 {
    let ptr = alloc_tuple_unpublished(py, elems, false, Some(class));
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let ptr = unsafe { super::native_instance::publish(py, ptr, class) };
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// Allocate a fixed-length tuple whose slots begin as the canonical internal
/// missing singleton, never a Python value such as float +0.0. This is the
/// construction-only authority for `PyTuple_New`; later writes cannot grow it.
pub(crate) fn alloc_tuple_uninitialized(_py: &PyToken<'_>, len: usize) -> *mut u8 {
    if len == 0 {
        return alloc_tuple(_py, &[]);
    }
    let missing = missing_bits(_py);
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
            items.add(index).write(missing);
        }
        (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
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
    missing: std::sync::atomic::AtomicPtr<u8>,
    not_implemented: std::sync::atomic::AtomicPtr<u8>,
    ellipsis: std::sync::atomic::AtomicPtr<u8>,
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
            missing: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            not_implemented: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            ellipsis: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            ascii_chars: [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 128],
            interned_strings: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[inline]
unsafe fn prepare_canonical_object(ptr: *mut u8, interned: bool) {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        if interned {
            (*header).fetch_or_flags(crate::object::HEADER_FLAG_INTERNED);
        }
        (*header).make_immortal();
    }
}

/// One publication protocol for every fixed canonical singleton. The object is
/// fully initialized and immortal before a lock-free reader can observe it.
fn canonical_singleton(
    py: &PyToken<'_>,
    slot: &std::sync::atomic::AtomicPtr<u8>,
    interned: bool,
    allocate: impl FnOnce() -> *mut u8,
) -> *mut u8 {
    let cached = slot.load(std::sync::atomic::Ordering::Acquire);
    if !cached.is_null() {
        return cached;
    }
    let _init = runtime_state(py)
        .canonical_objects
        .singleton_init
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cached = slot.load(std::sync::atomic::Ordering::Acquire);
    if !cached.is_null() {
        return cached;
    }
    let ptr = allocate();
    if !ptr.is_null() {
        unsafe { prepare_canonical_object(ptr, interned) };
        slot.store(ptr, std::sync::atomic::Ordering::Release);
    }
    ptr
}

#[derive(Clone, Copy)]
pub(crate) enum CanonicalSpecialSingleton {
    Missing,
    NotImplemented,
    Ellipsis,
}

pub(crate) fn canonical_special_singleton_bits(
    py: &PyToken<'_>,
    kind: CanonicalSpecialSingleton,
) -> u64 {
    let cache = &runtime_state(py).canonical_objects;
    let (slot, type_id) = match kind {
        CanonicalSpecialSingleton::Missing => (&cache.missing, TYPE_ID_OBJECT),
        CanonicalSpecialSingleton::NotImplemented => {
            (&cache.not_implemented, TYPE_ID_NOT_IMPLEMENTED)
        }
        CanonicalSpecialSingleton::Ellipsis => (&cache.ellipsis, TYPE_ID_ELLIPSIS),
    };
    let ptr = canonical_singleton(py, slot, false, || {
        alloc_object(py, std::mem::size_of::<MoltHeader>(), type_id)
    });
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

pub(crate) fn alloc_tuple(_py: &PyToken<'_>, elems: &[u64]) -> *mut u8 {
    // Fast path: return the immortal empty tuple singleton.
    if elems.is_empty() {
        let slot = &runtime_state(_py).canonical_objects.empty_tuple;
        return canonical_singleton(_py, slot, true, || {
            let candidate = alloc_tuple_exact(_py, &[], false);
            if !candidate.is_null() {
                unsafe {
                    crate::object::gc::gc_untrack(
                        _py,
                        candidate,
                        TYPE_ID_TUPLE,
                        crate::object::gc::GcUntrackReason::DynamicProjection,
                    );
                }
            }
            candidate
        });
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
    // code, trampoline, annotations, annotate, call_target, globals).
    // Slot 10 is a plain defaults-version stamp, slot 11 owns captured builtins,
    // and slot 12 is the plain immutable FunctionCallAbi discriminant. The
    // scalar slots are not refcounted; dealloc leaves them alone. The typed
    // metadata tail owns execution/introspection fields independently of dict.
    let total = std::mem::size_of::<MoltHeader>()
        + crate::object::function_metadata::FUNCTION_PAYLOAD_WORDS * std::mem::size_of::<u64>();
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
        *(ptr.add(12 * std::mem::size_of::<u64>()) as *mut u64) =
            crate::object::layout::FunctionCallAbi::Positional as u64;
        crate::object::function_metadata::initialize(ptr);
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
    // identity, 12..16 retain immutable signature facts used by
    // `types.FunctionType` reconstruction, 17 binds the compiled frame slot,
    // 18 selects the generated execution trampoline policy, 19..20 own the
    // immutable freevar/cellvar positional-name contracts, and 21 is the
    // immutable callable-context provenance paired with slots 9..11. Slot 22
    // owns optional immutable GPU body metadata; no function payload grows.
    let empty_lexical_ptr = alloc_tuple(_py, &[]);
    if empty_lexical_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let empty_lexical_bits = MoltObject::from_ptr(empty_lexical_ptr).bits();
    let total = std::mem::size_of::<MoltHeader>() + 23 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_CODE);
    if ptr.is_null() {
        dec_ref_bits(_py, empty_lexical_bits);
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
        *(ptr.add(17 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        // Execution kind is published exactly once with the packed function
        // metadata. Until then readers conservatively expose Direct behavior.
        *(ptr.add(18 * std::mem::size_of::<u64>()) as *mut u64) = u64::MAX;
        *(ptr.add(19 * std::mem::size_of::<u64>()) as *mut u64) = empty_lexical_bits;
        *(ptr.add(20 * std::mem::size_of::<u64>()) as *mut u64) = empty_lexical_bits;
        *(ptr.add(21 * std::mem::size_of::<u64>()) as *mut u64) = u64::MAX;
        *(ptr.add(22 * std::mem::size_of::<u64>()) as *mut u64) = 0;
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
        inc_ref_bits(_py, empty_lexical_bits);
        inc_ref_bits(_py, empty_lexical_bits);
    }
    dec_ref_bits(_py, empty_lexical_bits);
    ptr
}

/// Clone every immutable compiled-code fact while adding protocol flags to the
/// new code object's publication. Object-valued slots are retained through the
/// ordinary constructor/signature setters; the source remains untouched.
pub(crate) unsafe fn clone_code_obj_with_protocol_flags(
    _py: &PyToken<'_>,
    source: *mut u8,
    added_protocol_flags: u64,
) -> *mut u8 {
    unsafe {
        let clone = alloc_code_obj(
            _py,
            code_filename_bits(source),
            code_name_bits(source),
            code_firstlineno(source),
            code_linetable_bits(source),
            code_varnames_bits(source),
            code_names_bits(source),
            code_argcount(source),
            code_posonlyargcount(source),
            code_kwonlyargcount(source),
        );
        if clone.is_null() {
            return clone;
        }
        let signature = [
            code_arg_names_bits(source),
            code_signature_posonly_bits(source),
            code_kwonly_names_bits(source),
            code_vararg_bits(source),
            code_varkw_bits(source),
        ];
        // A freshly allocated or synthetic code object can have no signature.
        // Preserve that state; a partially populated signature must still fail
        // the same generated-capability validation as ordinary publication.
        if signature != [0; 5]
            && code_set_signature_bits(
                _py,
                clone,
                signature[0],
                signature[1],
                signature[2],
                signature[3],
                signature[4],
            )
            .is_err()
        {
            dec_ref_bits(_py, MoltObject::from_ptr(clone).bits());
            return std::ptr::null_mut();
        }
        if !code_publish_lexical_metadata(
            _py,
            clone,
            code_freevars_bits(source),
            code_cellvars_bits(source),
        ) {
            dec_ref_bits(_py, MoltObject::from_ptr(clone).bits());
            return std::ptr::null_mut();
        }
        let descriptor = crate::object::layout::code_gpu_descriptor_bits(source);
        if descriptor != 0
            && !crate::object::layout::code_publish_gpu_descriptor(_py, clone, descriptor)
        {
            dec_ref_bits(_py, MoltObject::from_ptr(clone).bits());
            return std::ptr::null_mut();
        }
        if let Some(frame_slot) = code_frame_slot_id(source) {
            code_set_frame_slot_id(clone, frame_slot);
        }
        if let Some(identity) = code_callable_identity(source) {
            code_publish_callable_identity(clone, identity)
                .expect("fresh code clone has unpublished callable identity");
        }
        let kind = code_published_execution_kind(source).unwrap_or(CodeExecutionKind::Direct);
        let protocol_flags = code_protocol_flags(source) | added_protocol_flags;
        code_publish_policy(clone, kind, protocol_flags)
            .expect("fresh code clone has unpublished execution policy");
        clone
    }
}

pub(crate) fn alloc_bound_method_obj(_py: &PyToken<'_>, func_bits: u64, self_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<super::layout::BoundMethodPayload>();
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
        ptr.cast::<super::layout::BoundMethodPayload>()
            .write(super::layout::BoundMethodPayload {
                function: func_bits,
                receiver: self_bits,
                module: MoltObject::none().bits(),
            });
        inc_ref_bits(_py, func_bits);
        inc_ref_bits(_py, self_bits);
    }
    ptr
}

pub(crate) fn alloc_module_obj(_py: &PyToken<'_>, name_bits: u64) -> *mut u8 {
    let class = obj_from_bits(builtin_classes(_py).module)
        .as_ptr()
        .expect("module builtin class");
    let bits = unsafe { crate::call::class_init::alloc_instance_for_class(_py, class) };
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return std::ptr::null_mut();
    };
    crate::builtins::modules::initialize_module_namespace(
        _py,
        bits,
        name_bits,
        MoltObject::none().bits(),
    );
    if exception_pending(_py) {
        dec_ref_bits(_py, bits);
        return std::ptr::null_mut();
    }
    ptr
}

pub(crate) fn alloc_class_obj(_py: &PyToken<'_>, name_bits: u64) -> *mut u8 {
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let ptr = alloc_class_obj_with_namespace(_py, name_bits, dict_bits);
    dec_ref_bits(_py, dict_bits);
    ptr
}

/// Allocate the fully initialized class payload around an already copied
/// namespace. The caller lends the dictionary; the class owns its own edge.
pub(crate) fn alloc_class_obj_with_namespace(
    _py: &PyToken<'_>,
    name_bits: u64,
    dict_bits: u64,
) -> *mut u8 {
    if !super::layout::validate_class_name(_py, name_bits) {
        return std::ptr::null_mut();
    }
    inc_ref_bits(_py, dict_bits);
    // Typed reference slots, layout epoch, atomic cold policy and write-once
    // payload size. The slot authority also owns private layout provenance.
    let total = std::mem::size_of::<MoltHeader>()
        + super::layout::CLASS_PAYLOAD_WORDS * std::mem::size_of::<u64>();
    let ptr = alloc_object_with_aux(_py, total, TYPE_ID_TYPE, ObjectAuxPreselection::ClassInline);
    if ptr.is_null() {
        dec_ref_bits(_py, dict_bits);
        return ptr;
    }
    unsafe {
        use super::class_storage::ClassReferenceSlot;
        super::class_storage::initialize_class_declarations(ptr);
        inc_ref_bits(_py, name_bits);
        inc_ref_bits(_py, name_bits); // independent name and qualname owners
        for slot in ClassReferenceSlot::ALL {
            let bits = match slot {
                ClassReferenceSlot::Name | ClassReferenceSlot::Qualname => name_bits,
                ClassReferenceSlot::Dictionary => dict_bits,
                ClassReferenceSlot::Bases | ClassReferenceSlot::Mro => MoltObject::none().bits(),
                ClassReferenceSlot::SlotDeclaration
                | ClassReferenceSlot::FieldLayout
                | ClassReferenceSlot::InstanceDictionary
                | ClassReferenceSlot::CreationDoc => 0,
            };
            slot.initialize_owned(ptr, bits);
        }
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        std::ptr::write(
            ptr.add(8 * std::mem::size_of::<u64>()) as *mut MoltAuxWord,
            MoltAuxWord::new(0),
        );
        std::ptr::write(
            ptr.add(9 * std::mem::size_of::<u64>()) as *mut std::sync::atomic::AtomicUsize,
            std::sync::atomic::AtomicUsize::new(0),
        );
    }
    ptr
}

fn alloc_exact_wrapper(_py: &PyToken<'_>, kind: super::layout::WrapperKind) -> *mut u8 {
    // Exact wrapper allocation can occur only after the builtin class anchor is
    // published. A recursive `builtin_classes()` call while the anchor is being
    // built would deadlock its initialization lock, so fail closed instead.
    let Some(classes) = crate::builtins::classes::builtin_classes_for_wrapper_allocation(_py)
    else {
        return std::ptr::null_mut();
    };
    let class_bits = match kind {
        super::layout::WrapperKind::Classmethod => classes.classmethod,
        super::layout::WrapperKind::Staticmethod => classes.staticmethod,
        super::layout::WrapperKind::Property => classes.property,
    };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return std::ptr::null_mut();
    };
    let Some(size) = (unsafe { super::layout::class_cached_layout_size(class_ptr) }) else {
        return std::ptr::null_mut();
    };
    debug_assert_eq!(
        unsafe { crate::object::class_instance_type_id(class_ptr) },
        kind.type_id()
    );
    debug_assert!(size >= kind.prefix_size() + std::mem::size_of::<u64>());
    obj_from_bits(alloc_class_instance(_py, size, class_bits))
        .as_ptr()
        .unwrap_or(std::ptr::null_mut())
}

#[cfg(test)]
pub(crate) fn alloc_classmethod_obj(_py: &PyToken<'_>, func_bits: u64) -> *mut u8 {
    let ptr = alloc_exact_wrapper(_py, super::layout::WrapperKind::Classmethod);
    if !ptr.is_null() {
        unsafe {
            if !super::layout::classmethod_replace_func_bits(_py, ptr, func_bits) {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return std::ptr::null_mut();
            }
            crate::object::gc::gc_publish_initialized(_py, ptr);
        }
    }
    ptr
}

#[cfg(test)]
pub(crate) fn alloc_staticmethod_obj(_py: &PyToken<'_>, func_bits: u64) -> *mut u8 {
    let ptr = alloc_exact_wrapper(_py, super::layout::WrapperKind::Staticmethod);
    if !ptr.is_null() {
        unsafe {
            if !super::layout::staticmethod_replace_func_bits(_py, ptr, func_bits) {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return std::ptr::null_mut();
            }
            crate::object::gc::gc_publish_initialized(_py, ptr);
        }
    }
    ptr
}

pub(crate) fn alloc_property_obj(
    _py: &PyToken<'_>,
    get_bits: u64,
    set_bits: u64,
    del_bits: u64,
) -> *mut u8 {
    let ptr = alloc_exact_wrapper(_py, super::layout::WrapperKind::Property);
    if !ptr.is_null() {
        unsafe {
            if !super::layout::property_replace_get_bits(_py, ptr, get_bits)
                || !super::layout::property_replace_set_bits(_py, ptr, set_bits)
                || !super::layout::property_replace_del_bits(_py, ptr, del_bits)
            {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return std::ptr::null_mut();
            }
            // `doc`, `name`, and `getter_doc` retain the prefix defaults here.
            // The shared property initializer derives and publishes metadata.
            crate::object::gc::gc_publish_initialized(_py, ptr);
        }
    }
    ptr
}

/// Allocate one immutable builtin member/getset descriptor. Public type
/// identity is an owned class edge; every portable callback is a retained
/// Python callable rather than a host function pointer in object storage.
pub(crate) fn alloc_native_descriptor_obj(
    _py: &PyToken<'_>,
    class_bits: u64,
    spec: crate::builtins::types::NativeDescriptorSpec,
) -> *mut u8 {
    let crate::builtins::types::NativeDescriptorSpec {
        flavor,
        operation,
        owner: owner_bits,
        name: name_bits,
        doc: doc_bits,
        getter: getter_bits,
        setter: setter_bits,
        deleter: deleter_bits,
    } = spec;
    let references = [
        owner_bits,
        name_bits,
        doc_bits,
        getter_bits,
        setter_bits,
        deleter_bits,
    ];
    if references.contains(&0) {
        return std::ptr::null_mut();
    }
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return std::ptr::null_mut();
    };
    if unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return std::ptr::null_mut();
    }
    let Some(total) =
        crate::object::checked_object_total_size(super::layout::NATIVE_DESCRIPTOR_PREFIX_SIZE)
    else {
        return std::ptr::null_mut();
    };
    let ptr = crate::object::alloc_object_zeroed_unpublished_with_aux(
        _py,
        total,
        TYPE_ID_NATIVE_DESCRIPTOR,
        ObjectAuxPreselection::ClassInline,
    );
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        if !object_init_class_edge_unpublished(_py, ptr, class_bits, ClassEdgeOwnership::Owned) {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        }
        for (index, bits) in references.into_iter().enumerate() {
            inc_ref_bits(_py, bits);
            *ptr.cast::<u64>().add(index) = bits;
        }
        *ptr.cast::<u64>()
            .add(super::layout::NATIVE_DESCRIPTOR_REFERENCE_WORDS) =
            super::layout::native_descriptor_control(flavor, operation);
        crate::object::gc::gc_publish_initialized(_py, ptr);
    }
    ptr
}

pub(crate) fn alloc_super_obj(
    _py: &PyToken<'_>,
    type_bits: u64,
    obj_bits: u64,
    receiver_class_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(_py, total, TYPE_ID_SUPER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = type_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = obj_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = receiver_class_bits;
        inc_ref_bits(_py, type_bits);
        inc_ref_bits(_py, obj_bits);
        inc_ref_bits(_py, receiver_class_bits);
    }
    ptr
}

// Context stack helpers moved to runtime/molt-runtime/src/builtins/context.rs.

// Frame stack helpers moved to runtime/molt-runtime/src/builtins/exceptions.rs.

/// The only heap kinds whose payload is InlineBytesStorage, including its NUL.
/// Bytearray owns a Vec and must use `alloc_bytearray[_with_len]` instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InlineBytesKind {
    String,
    Bytes,
}

impl InlineBytesKind {
    fn type_id(self) -> u32 {
        match self {
            Self::String => TYPE_ID_STRING,
            Self::Bytes => TYPE_ID_BYTES,
        }
    }
}

pub(crate) fn alloc_inline_bytes_with_len(
    _py: &PyToken<'_>,
    len: usize,
    kind: InlineBytesKind,
) -> *mut u8 {
    let Some(total) = super::layout::InlineBytesStorage::object_size(len) else {
        record_memory_error_without_allocation(_py);
        return std::ptr::null_mut();
    };
    let ptr = alloc_object(_py, total, kind.type_id());
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        super::layout::InlineBytesStorage::set_len(ptr, len);
    }
    ptr
}

fn canonical_inline_bytes(
    _py: &PyToken<'_>,
    slot: &std::sync::atomic::AtomicPtr<u8>,
    bytes: &[u8],
    kind: InlineBytesKind,
    interned: bool,
) -> *mut u8 {
    canonical_singleton(_py, slot, interned, || {
        let ptr = alloc_inline_bytes_with_len(_py, bytes.len(), kind);
        if !ptr.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    super::layout::InlineBytesStorage::data(ptr),
                    bytes.len(),
                );
            }
        }
        ptr
    })
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
    let raw = canonical_inline_bytes(_py, slot, bytes, InlineBytesKind::String, true);
    if raw.is_null() { None } else { Some(raw) }
}

pub(crate) fn alloc_interned_string(_py: &PyToken<'_>, bytes: &[u8]) -> *mut u8 {
    if bytes.is_empty() {
        let slot = &runtime_state(_py).canonical_objects.empty_string;
        return canonical_inline_bytes(_py, slot, bytes, InlineBytesKind::String, true);
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
    let ptr = alloc_inline_bytes_with_len(_py, bytes.len(), InlineBytesKind::String);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            super::layout::InlineBytesStorage::data(ptr),
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

    let ptr = alloc_inline_bytes_with_len(_py, bytes.len(), InlineBytesKind::String);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let data_ptr = super::layout::InlineBytesStorage::data(ptr);
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
    let ptr = alloc_inline_bytes_with_len(_py, bytes.len(), InlineBytesKind::String);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let data_ptr = super::layout::InlineBytesStorage::data(ptr);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
    }
    ptr
}

fn alloc_inline_bytes(_py: &PyToken<'_>, bytes: &[u8], kind: InlineBytesKind) -> *mut u8 {
    let ptr = alloc_inline_bytes_with_len(_py, bytes.len(), kind);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let data_ptr = super::layout::InlineBytesStorage::data(ptr);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
    }
    ptr
}

pub(crate) fn alloc_bytes(_py: &PyToken<'_>, bytes: &[u8]) -> *mut u8 {
    if bytes.is_empty() {
        let slot = &runtime_state(_py).canonical_objects.empty_bytes;
        return canonical_inline_bytes(_py, slot, bytes, InlineBytesKind::Bytes, false);
    }
    alloc_inline_bytes(_py, bytes, InlineBytesKind::Bytes)
}

pub(crate) fn clear_builder_singletons(_py: &PyToken<'_>, state: &crate::RuntimeState) {
    crate::gil_assert();
    let cache = &state.canonical_objects;
    let init = cache
        .singleton_init
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let singleton_slots = [
        &cache.empty_tuple,
        &cache.empty_string,
        &cache.empty_bytes,
        &cache.missing,
        &cache.not_implemented,
        &cache.ellipsis,
    ];
    let singleton_ptrs = singleton_slots
        .map(|slot| slot.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel));
    let ascii_ptrs = cache
        .ascii_chars
        .each_ref()
        .map(|slot| slot.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel));
    let mut pool = cache
        .interned_strings
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let interned = std::mem::take(&mut *pool);
    drop(pool);
    drop(init);

    // These domains are disjoint by construction: fixed values and every
    // one-byte ASCII string have dedicated slots, and the pool only admits
    // remaining nonempty strings. Do not hide collisions by deduplicating.
    #[cfg(debug_assertions)]
    {
        let singleton_ptrs: Vec<_> = singleton_ptrs.into_iter().chain(ascii_ptrs).collect();
        for (index, &ptr) in singleton_ptrs.iter().enumerate() {
            if !ptr.is_null() {
                assert!(!singleton_ptrs[index + 1..].contains(&ptr));
                assert!(!interned.values().any(|&raw| raw == ptr as usize));
            }
        }
    }
    for ptr in singleton_ptrs
        .into_iter()
        .chain(ascii_ptrs)
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

/// Constructor-owned native payloads share their ordinary backing transaction;
/// only heap subtypes acquire the sealed extension, dictionary and class edge.
pub(crate) unsafe fn alloc_native_set(
    py: &PyToken<'_>,
    class: u64,
    kind: super::native_instance::NativePayload,
    source: Option<*mut u8>,
) -> *mut u8 {
    assert!(matches!(
        kind,
        super::native_instance::NativePayload::Set
            | super::native_instance::NativePayload::Frozenset
    ));
    let ptr = alloc_hash_aggregate_for_class_unpublished(py, 0, kind.type_id(), Some(class));
    if !ptr.is_null()
        && let Some(source) = source
    {
        unsafe {
            super::ops::set_copy_into_empty(py, source, ptr);
        }
        if exception_pending(py) {
            dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        }
    }
    unsafe { super::native_instance::publish(py, ptr, class) }
}

pub(crate) unsafe fn alloc_native_bytearray(py: &PyToken<'_>, class: u64) -> *mut u8 {
    let ptr = unsafe {
        super::native_instance::alloc_unpublished(
            py,
            class,
            super::native_instance::NativePayload::Bytearray,
            0,
        )
    };
    if ptr.is_null() {
        return ptr;
    }
    let Some(backing) = super::buffer_exports::bytearray_backing_from_slice(&[], 0) else {
        dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
        record_memory_error_without_allocation(py);
        return std::ptr::null_mut();
    };
    unsafe {
        ptr.cast::<*mut Vec<u8>>().write(backing);
        super::native_instance::publish(py, ptr, class)
    }
}

pub(crate) unsafe fn alloc_native_inline_bytes(
    py: &PyToken<'_>,
    class: u64,
    kind: super::native_instance::NativePayload,
    bytes: &[u8],
) -> *mut u8 {
    assert!(matches!(
        kind,
        super::native_instance::NativePayload::String
            | super::native_instance::NativePayload::Bytes
    ));
    let ptr = unsafe { super::native_instance::alloc_unpublished(py, class, kind, bytes.len()) };
    if !ptr.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                super::layout::InlineBytesStorage::data(ptr),
                bytes.len(),
            );
            super::layout::InlineBytesStorage::set_len(ptr, bytes.len());
        }
    }
    unsafe { super::native_instance::publish(py, ptr, class) }
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
        let Some(vec_ptr) = super::buffer_exports::bytearray_backing_from_slice(bytes, cap) else {
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
        let Some(vec_ptr) = super::buffer_exports::bytearray_backing_zeroed(len) else {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return std::ptr::null_mut();
        };
        *(ptr as *mut *mut Vec<u8>) = vec_ptr;
    }
    ptr
}

/// Own the descriptor edges and counted export before callbacks or allocation.
/// Geometry may change while pinned; ownership can only transfer into a view.
pub(crate) struct PinnedMemoryViewStorage<'a, 'py> {
    py: &'a PyToken<'py>,
    storage: crate::object::memoryview::TypedStridedStorage,
    base_guard: PtrDropGuard,
    format_guard: PtrDropGuard,
    owner: Option<super::buffer_exports::ScopedBufferExport<'a, 'py>>,
}

impl<'a, 'py> PinnedMemoryViewStorage<'a, 'py> {
    pub(crate) fn new(
        py: &'a PyToken<'py>,
        storage: crate::object::memoryview::TypedStridedStorage,
    ) -> Result<Self, ()> {
        if storage.format_bits == 0
            || (storage.base_bits == 0 && storage.data.is_null() && storage.span_len != 0)
        {
            memoryview_construction_failure(py, "BufferError", "invalid memoryview storage");
            return Err(());
        }
        inc_ref_bits(py, storage.base_bits);
        let base_guard = PtrDropGuard::preserving(
            obj_from_bits(storage.base_bits)
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        );
        inc_ref_bits(py, storage.format_bits);
        let format_guard = PtrDropGuard::preserving(
            obj_from_bits(storage.format_bits)
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        );
        let owner = super::buffer_exports::ScopedBufferExport::new(py, storage.owner_bits)?;
        Ok(Self {
            py,
            storage,
            base_guard,
            format_guard,
            owner: Some(owner),
        })
    }

    pub(crate) fn storage(&self) -> &crate::object::memoryview::TypedStridedStorage {
        &self.storage
    }

    /// Derive geometry through the shared checked storage authority.
    pub(crate) fn slice_first_axis(
        &mut self,
        start: isize,
        stop: isize,
        step: isize,
    ) -> Option<()> {
        self.storage.slice_first_axis(self.py, start, stop, step)
    }

    pub(crate) fn allocate(mut self) -> *mut u8 {
        let py = self.py;
        let storage = &mut self.storage;
        let invalid =
            || memoryview_construction_failure(py, "BufferError", "invalid memoryview storage");
        let no_memory =
            || memoryview_construction_failure(py, "MemoryError", "cannot allocate memoryview");
        let data = unsafe {
            if !storage.data.is_null() {
                storage.data
            } else if storage.base_bits == 0 && storage.span_len == 0 {
                std::ptr::NonNull::<u8>::dangling().as_ptr()
            } else {
                let Some(base_ptr) = obj_from_bits(storage.base_bits).as_ptr() else {
                    return invalid();
                };
                let Some(base_slice) = bytes_like_slice_raw(base_ptr) else {
                    return invalid();
                };
                if !storage.fits_in_base_len(base_slice.len()) || storage.offset < 0 {
                    return invalid();
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
                return invalid();
            }
        }
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<MemoryView>();
        let ptr = alloc_object(py, total, TYPE_ID_MEMORYVIEW);
        if ptr.is_null() {
            return no_memory();
        }
        unsafe {
            let Some(shape_ptr) = crate::object::backing::tracked_vec_box_from_slice(
                storage.shape.as_slice(),
                storage.shape.len(),
            ) else {
                dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
                return no_memory();
            };
            let Some(strides_ptr) = crate::object::backing::tracked_vec_box_from_slice(
                storage.strides.as_slice(),
                storage.strides.len(),
            ) else {
                drop(crate::object::backing::tracked_vec_box_from_raw(shape_ptr));
                dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
                return no_memory();
            };
            let mv_ptr = memoryview_ptr(ptr);
            (*mv_ptr).owner_bits = 0;
            (*mv_ptr).base_bits = 0;
            (*mv_ptr).data = data;
            (*mv_ptr).offset = storage.offset;
            (*mv_ptr).len = storage.memoryview_len_field();
            (*mv_ptr).itemsize = storage.itemsize;
            (*mv_ptr).stride = storage.memoryview_stride_field();
            (*mv_ptr).readonly = if storage.readonly { 1 } else { 0 };
            (*mv_ptr).ndim = storage.shape.len() as u8;
            (*mv_ptr).released = 0;
            (*mv_ptr).restricted = 0;
            (*mv_ptr)._pad = [0; 4];
            (*mv_ptr).format_bits = storage.format_bits;
            (*mv_ptr).shape_ptr = shape_ptr;
            (*mv_ptr).strides_ptr = strides_ptr;
            (*mv_ptr).exports = super::buffer_exports::BufferExports::new();
            (*mv_ptr).native_lease = storage.native_lease.take();
        }
        self.format_guard.release();
        // Transfer the single pre-acquired export only after initialization.
        let owner = self
            .owner
            .take()
            .expect("pinned memoryview owner")
            .into_owner();
        unsafe {
            (*memoryview_ptr(ptr)).owner_bits = owner;
            (*memoryview_ptr(ptr)).base_bits = storage.base_bits;
        }
        self.base_guard.release();
        if storage.base_bits != 0 && storage.base_bits == owner {
            // The initialized view now owns the same object through its export.
            // Dropping this redundant strong edge cannot run a finalizer.
            dec_ref_bits(py, storage.base_bits);
        }
        ptr
    }
}

impl Drop for PinnedMemoryViewStorage<'_, '_> {
    fn drop(&mut self) {
        // Native leases and pointer guards already preserve pending errors.
        // The counted owner needs the same rule when a callback aborted slicing.
        if let Some(owner) = self.owner.take()
            && owner.owner_bits() != 0
        {
            molt_cpython_abi::api::errors::with_preserved_error(|| drop(owner));
        }
    }
}

fn memoryview_construction_failure(py: &PyToken<'_>, kind: &str, message: &str) -> *mut u8 {
    if !crate::exception_pending(py) {
        let _ = crate::raise_exception::<u64>(py, kind, message);
    }
    std::ptr::null_mut()
}

pub(crate) fn alloc_memoryview_from_storage(
    py: &PyToken<'_>,
    storage: crate::object::memoryview::TypedStridedStorage,
) -> *mut u8 {
    match PinnedMemoryViewStorage::new(py, storage) {
        Ok(pinned) => pinned.allocate(),
        Err(()) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod sequence_builder_tests {
    use super::*;
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};

    struct RestoreBudget;
    impl Drop for RestoreBudget {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    fn deny_allocations() -> RestoreBudget {
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(0),
            max_allocations: Some(0),
            ..Default::default()
        })));
        RestoreBudget
    }

    fn refs(bits: u64) -> u32 {
        unsafe { (*header_from_obj_ptr(ptr_from_bits(bits))).ref_count_snapshot() }
    }

    #[test]
    fn native_prefix_construction_commits_namespace_and_rolls_back_every_owner() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let classes = builtin_classes(py);
            let name = bits_from_ptr(alloc_string(py, b"NamespaceWithSlot"));
            let subtype = crate::molt_class_new(name);
            dec_ref_bits(py, name);
            crate::molt_class_set_base(subtype, classes.module);
            let field = bits_from_ptr(alloc_string(py, b"field"));
            let slots = bits_from_ptr(alloc_tuple(py, &[field]));
            let key = attr_name_bits_from_bytes(py, b"__slots__").unwrap();
            crate::molt_set_attr_name(subtype, key, slots);
            for bits in [key, slots, field] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));

            for class in [
                classes.module,
                subtype,
                classes.list,
                classes.property,
                classes.classmethod,
                classes.staticmethod,
            ] {
                let class_ptr = ptr_from_bits(class);
                assert!(unsafe { crate::object::class_finish_definition(py, class_ptr) }.is_ok());
                let size =
                    unsafe { crate::object::layout::class_cached_layout_size(class_ptr) }.unwrap();
                let construct = || alloc_class_instance(py, size, class);
                // Warm singleton/layout and terminal cleanup resources before
                // denying each successive allocation in this transaction.
                let warm = construct();
                assert!(!obj_from_bits(warm).is_none());
                dec_ref_bits(py, warm);
                let baseline = refs(class);
                let mut succeeded = false;
                for limit in 0..=12 {
                    set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                        max_allocations: Some(limit + 1),
                        ..Default::default()
                    })));
                    let budget = RestoreBudget;
                    let sentinel = crate::object::backing::tracked_vec_box_with_capacity::<u64>(0)
                        .expect("one reserved sentinel allocation");
                    let value = construct();
                    if let Some(ptr) = obj_from_bits(value).as_ptr() {
                        assert!(!exception_pending(py));
                        assert_eq!(unsafe { crate::object::object_class_bits(ptr) }, class);
                        if unsafe { object_type_id(ptr) } == TYPE_ID_MODULE {
                            let dictionary = unsafe { instance_dict_bits(ptr) };
                            assert_ne!(
                                dictionary, 0,
                                "ModuleType.__new__ must own its empty namespace"
                            );
                            assert_eq!(
                                unsafe { object_type_id(ptr_from_bits(dictionary)) },
                                TYPE_ID_DICT
                            );
                            assert_eq!(refs(dictionary), 1);
                            let mut edges = Vec::new();
                            unsafe {
                                crate::object::heap_lifecycle::visit_owned_edges(
                                    py,
                                    ptr,
                                    &mut |edge| edges.push(edge),
                                );
                            }
                            assert_eq!(
                                edges
                                    .iter()
                                    .filter(|&&edge| edge == ptr_from_bits(dictionary))
                                    .count(),
                                1
                            );
                            inc_ref_bits(py, dictionary);
                            dec_ref_bits(py, value);
                            assert_eq!(
                                refs(dictionary),
                                1,
                                "terminal cleanup must release the namespace"
                            );
                            dec_ref_bits(py, dictionary);
                        } else {
                            dec_ref_bits(py, value);
                        }
                        succeeded = true;
                    } else {
                        assert!(
                            exception_pending(py),
                            "allocation denial must report MemoryError"
                        );
                    }
                    assert_eq!(
                        refs(class),
                        baseline,
                        "class ownership must balance on every exit"
                    );
                    crate::resource::with_tracker(|tracker| {
                        for _ in 0..limit {
                            assert!(
                                tracker.on_allocate(1).is_ok(),
                                "construction or rollback leaked an allocation"
                            );
                        }
                        assert!(
                            tracker.on_allocate(1).is_err(),
                            "cleanup stole another owner's allocation"
                        );
                        for _ in 0..limit {
                            tracker.on_free(1);
                        }
                    });
                    unsafe { drop(crate::object::backing::tracked_vec_box_from_raw(sentinel)) };
                    drop(budget);
                    clear_exception(py);
                    if succeeded {
                        break;
                    }
                }
                assert!(
                    succeeded,
                    "native construction never completed under bounded admission"
                );
            }
            dec_ref_bits(py, subtype);
        });
    }

    #[test]
    fn raw_object_allocators_share_nonallocating_failure_custody() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for mode in 0..3 {
                let allocate = |size| match mode {
                    0 => alloc_object(py, size, TYPE_ID_OBJECT),
                    1 => crate::object::alloc_object_zeroed_with_aux(
                        py,
                        size,
                        TYPE_ID_OBJECT,
                        ObjectAuxPreselection::Default,
                    ),
                    _ => crate::object::alloc_object_zeroed_unpublished_with_aux(
                        py,
                        size,
                        TYPE_ID_OBJECT,
                        ObjectAuxPreselection::Default,
                    ),
                };
                for size in [0, usize::MAX, std::mem::size_of::<MoltHeader>() + 8] {
                    let budget = deny_allocations();
                    assert!(allocate(size).is_null());
                    assert!(exception_pending(py), "mode={mode}, size={size}");
                    drop(budget);
                    clear_exception(py);
                    let _ = raise_exception::<u64>(py, "ValueError", "keep the caller error");
                    let pending = crate::builtins::exceptions::molt_exception_last_pending();
                    let budget = deny_allocations();
                    assert!(allocate(size).is_null());
                    drop(budget);
                    let still_pending = crate::builtins::exceptions::molt_exception_last_pending();
                    assert_eq!(still_pending, pending);
                    dec_ref_bits(py, still_pending);
                    dec_ref_bits(py, pending);
                    clear_exception(py);
                }
            }
        });
    }

    #[test]
    fn hash_storage_failure_rolls_back_every_allocation_and_borrowed_edge() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let item = bits_from_ptr(alloc_string(py, b"hash-storage-borrowed-owner"));
            let baseline = refs(item);
            for type_id in [TYPE_ID_DICT, TYPE_ID_SET, TYPE_ID_FROZENSET] {
                for populated in [false, true] {
                    let construct = || {
                        if type_id == TYPE_ID_DICT {
                            let pairs = [MoltObject::from_int(1).bits(), item];
                            alloc_dict_with_capacity_and_pairs(
                                py,
                                7,
                                if populated { &pairs } else { &[] },
                            )
                        } else {
                            let entries = [item];
                            alloc_set_like_with_capacity_and_entries(
                                py,
                                7,
                                if populated { &entries } else { &[] },
                                type_id,
                            )
                        }
                    };
                    // Warm runtime metadata/hash caches before deterministic denial.
                    let warm = construct();
                    assert!(!warm.is_null());
                    dec_ref_bits(py, bits_from_ptr(warm));
                    let mut failures = 0;
                    let mut completed = false;
                    for limit in 0..=8 {
                        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                            max_allocations: Some(limit + 1),
                            ..Default::default()
                        })));
                        let budget = RestoreBudget;
                        let sentinel =
                            crate::object::backing::tracked_vec_box_with_capacity::<u64>(0)
                                .expect("one reserved sentinel allocation");
                        let ptr = construct();
                        if ptr.is_null() {
                            failures += 1;
                            assert!(exception_pending(py), "type={type_id}, limit={limit}");
                            assert_eq!(refs(item), baseline);
                            // Every admitted object/buffer charge must be gone,
                            // but rollback must not steal the sentinel's count.
                            crate::resource::with_tracker(|tracker| {
                                for _ in 0..limit {
                                    assert!(
                                        tracker.on_allocate(1).is_ok(),
                                        "rollback leaked an allocation"
                                    );
                                }
                                assert!(tracker.on_allocate(1).is_err());
                                for _ in 0..limit {
                                    tracker.on_free(1);
                                }
                            });
                        } else {
                            assert!(!exception_pending(py));
                            assert!(unsafe { (*header_from_obj_ptr(ptr)).gc_is_published() });
                            assert_eq!(refs(item), baseline + u32::from(populated));
                            dec_ref_bits(py, bits_from_ptr(ptr));
                            assert_eq!(refs(item), baseline);
                            completed = true;
                        }
                        unsafe { drop(crate::object::backing::tracked_vec_box_from_raw(sentinel)) };
                        drop(budget);
                        clear_exception(py);
                        if completed {
                            break;
                        }
                    }
                    assert!(completed, "bounded admission never succeeded for {type_id}");
                    assert!(
                        failures >= 3,
                        "must exercise object, typed entries and index owners"
                    );
                }
            }
            dec_ref_bits(py, item);
        });
    }

    #[test]
    fn hash_storage_capacity_overflow_raises_and_preserves_first_error() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for type_id in [TYPE_ID_DICT, TYPE_ID_SET, TYPE_ID_FROZENSET] {
                for capacity in [usize::MAX, usize::MAX / 2] {
                    assert!(alloc_hash_aggregate_unpublished(py, capacity, type_id).is_null());
                    assert!(exception_pending(py));
                    clear_exception(py);
                    let _ = raise_exception::<u64>(
                        py,
                        "ValueError",
                        "original allocation caller error",
                    );
                    let pending = crate::builtins::exceptions::molt_exception_last_pending();
                    let budget = deny_allocations();
                    assert!(alloc_hash_aggregate_unpublished(py, capacity, type_id).is_null());
                    drop(budget);
                    let still_pending = crate::builtins::exceptions::molt_exception_last_pending();
                    assert_eq!(still_pending, pending);
                    dec_ref_bits(py, still_pending);
                    dec_ref_bits(py, pending);
                    clear_exception(py);
                }
            }
        });
    }

    #[test]
    fn direct_hash_aggregate_abort_releases_only_admitted_borrowed_edges() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for dict in [false, true] {
                let item = bits_from_ptr(alloc_string(py, b"aggregate-borrowed-owner"));
                let baseline = refs(item);
                let invalid = bits_from_ptr(alloc_list(py, &[]));
                let aggregate = if dict {
                    molt_dict_new(2)
                } else {
                    molt_set_new(2)
                };
                assert!(!obj_from_bits(aggregate).is_none());
                if dict {
                    molt_dict_set(aggregate, MoltObject::from_int(1).bits(), item);
                    molt_dict_set(aggregate, MoltObject::from_int(2).bits(), item);
                    assert_eq!(refs(item), baseline + 2);
                    molt_dict_set(aggregate, invalid, item);
                } else {
                    molt_set_add(aggregate, item);
                    molt_set_add(aggregate, item);
                    assert_eq!(refs(item), baseline + 1);
                    molt_set_add(aggregate, invalid);
                }
                assert!(exception_pending(py));
                dec_ref_bits(py, aggregate);
                assert_eq!(refs(item), baseline, "only admitted edges were retained");
                assert_eq!(
                    refs(invalid),
                    1,
                    "failed hashing never acquired the operand"
                );
                clear_exception(py);
                dec_ref_bits(py, invalid);
                dec_ref_bits(py, item);
            }
        });
    }

    #[test]
    fn dict_conversion_failure_releases_partial_aggregate_edges() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for malformed_pair in [false, true] {
                let item = bits_from_ptr(alloc_string(py, b"dict-conversion-borrowed-owner"));
                let invalid = bits_from_ptr(alloc_list(py, &[]));
                let first = bits_from_ptr(alloc_tuple(py, &[MoltObject::from_int(1).bits(), item]));
                let second_items = [invalid, item];
                let second = bits_from_ptr(alloc_tuple(
                    py,
                    if malformed_pair {
                        &second_items[1..]
                    } else {
                        &second_items
                    },
                ));
                let source = bits_from_ptr(alloc_list(py, &[first, second]));
                let baseline = refs(item);
                let result = molt_dict_from_obj(source);
                assert!(obj_from_bits(result).is_none());
                assert!(exception_pending(py));
                assert_eq!(
                    refs(item),
                    baseline,
                    "partial dict must release every admitted edge"
                );
                clear_exception(py);
                dec_ref_bits(py, source);
                dec_ref_bits(py, second);
                dec_ref_bits(py, first);
                dec_ref_bits(py, invalid);
                dec_ref_bits(py, item);
            }
        });
    }

    #[test]
    fn hash_aggregate_constructors_fail_with_pending_error_and_never_publish() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (constructor, capacity) in [
                (molt_dict_new as extern "C" fn(u64) -> u64, 1),
                (molt_set_new, 1),
                (molt_frozenset_new, 1),
            ] {
                let budget = deny_allocations();
                assert!(obj_from_bits(constructor(capacity)).is_none());
                drop(budget);
                assert!(exception_pending(py));
                let pending = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(obj_from_bits(constructor(capacity)).is_none());
                let still_pending = crate::builtins::exceptions::molt_exception_last_pending();
                assert_eq!(
                    still_pending, pending,
                    "constructor must preserve first failure"
                );
                dec_ref_bits(py, still_pending);
                dec_ref_bits(py, pending);
                clear_exception(py);
            }
        });
    }

    #[test]
    fn internal_hash_aggregate_construction_aborts_on_hash_failure() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let item = bits_from_ptr(alloc_string(py, b"partial-hash-aggregate-owner"));
            let invalid = bits_from_ptr(alloc_list(py, &[]));
            let baseline = refs(item);
            for type_id in [TYPE_ID_DICT, TYPE_ID_SET, TYPE_ID_FROZENSET] {
                let construct = || {
                    if type_id == TYPE_ID_DICT {
                        alloc_dict_with_pairs(
                            py,
                            &[
                                item,
                                item,
                                invalid,
                                item,
                                MoltObject::from_int(1).bits(),
                                item,
                            ],
                        )
                    } else {
                        alloc_set_like_with_entries(py, &[item, invalid, item], type_id)
                    }
                };
                assert!(construct().is_null());
                assert!(exception_pending(py));
                assert_eq!(refs(item), baseline);
                assert_eq!(refs(invalid), 1);
                let pending = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(
                    construct().is_null(),
                    "pending failure rejects construction"
                );
                let still_pending = crate::builtins::exceptions::molt_exception_last_pending();
                assert_eq!(still_pending, pending);
                assert_eq!(refs(item), baseline);
                dec_ref_bits(py, still_pending);
                dec_ref_bits(py, pending);
                clear_exception(py);
            }
            dec_ref_bits(py, invalid);
            dec_ref_bits(py, item);
        });
    }

    #[test]
    fn sequence_builder_append_borrows_and_abort_releases_every_admitted_owner() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let item = bits_from_ptr(alloc_string(py, b"builder-abort-owner"));
            let baseline = refs(item);
            let builder = molt_list_builder_new(MoltObject::from_int(2).bits());
            assert!(!obj_from_bits(builder).is_none());
            unsafe {
                assert_eq!(molt_list_builder_append(builder, item), 0);
                assert_eq!(molt_list_builder_append(builder, item), 0);
            }
            assert_eq!(refs(item), baseline + 2);
            dec_ref_bits(py, builder);
            assert_eq!(
                refs(item),
                baseline,
                "lifecycle detaches each owned edge before dropping storage"
            );
            dec_ref_bits(py, item);
        });
    }

    #[test]
    fn rejected_append_never_retains_or_publishes_partial_contents() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let item = bits_from_ptr(alloc_string(py, b"builder-rejected-owner"));
            let baseline = refs(item);
            let builder = molt_list_builder_new(MoltObject::from_int(0).bits());
            assert!(!obj_from_bits(builder).is_none());
            let budget = deny_allocations();
            assert_eq!(unsafe { molt_list_builder_append(builder, item) }, 1);
            drop(budget);
            assert!(exception_pending(py));
            assert_eq!(refs(item), baseline);
            assert_eq!(unsafe { molt_list_builder_append(builder, item) }, 1);
            assert_eq!(
                refs(item),
                baseline,
                "pending failures reject without admitting a new owner"
            );
            dec_ref_bits(py, builder);
            clear_exception(py);
            dec_ref_bits(py, item);
        });
    }

    #[test]
    fn list_finish_transfers_or_releases_owned_storage() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for failure in ["none", "pending", "allocation"] {
                let item = bits_from_ptr(alloc_string(py, b"builder-finish-owner"));
                let baseline = refs(item);
                let builder = molt_list_builder_new(MoltObject::from_int(1).bits());
                assert!(!obj_from_bits(builder).is_none());
                assert_eq!(unsafe { molt_list_builder_append(builder, item) }, 0);
                assert_eq!(refs(item), baseline + 1);
                if failure == "pending" {
                    crate::record_memory_error_without_allocation(py);
                }
                let budget = (failure == "allocation").then(deny_allocations);
                let result = unsafe { molt_list_builder_finish(builder) };
                drop(budget);
                if failure == "none" {
                    assert!(!obj_from_bits(result).is_none());
                    assert_eq!(
                        refs(item),
                        baseline + 1,
                        "finish transfers, never retains again"
                    );
                    dec_ref_bits(py, result);
                } else {
                    assert!(obj_from_bits(result).is_none());
                    assert!(exception_pending(py));
                    clear_exception(py);
                }
                assert_eq!(refs(item), baseline, "failure={failure}");
                dec_ref_bits(py, item);
            }
        });
    }

    #[test]
    fn tuple_from_values_borrows_words_and_fails_without_retaining() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let item = bits_from_ptr(alloc_string(py, b"tuple-from-values-owner"));
            let baseline = refs(item);
            let words = [item, MoltObject::from_int(7).bits(), item];
            let address = crate::provenance::abi::expose_address(words.as_ptr());
            let len = words.len() as u64;

            let tuple = unsafe { molt_tuple_from_values(address, len) };
            assert!(!obj_from_bits(tuple).is_none());
            assert_eq!(
                refs(item),
                baseline + 2,
                "the tuple retains each borrowed word"
            );
            let tuple_ptr = ptr_from_bits(tuple);
            let items = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(tuple_ptr, <[u64]>::to_vec)
            };
            assert_eq!(items.as_deref(), Some(&words[..]));
            dec_ref_bits(py, tuple);
            assert_eq!(refs(item), baseline);

            // An empty range needs no address and yields the canonical singleton.
            let empty = unsafe { molt_tuple_from_values(0, 0) };
            assert_eq!(empty, bits_from_ptr(alloc_tuple(py, &[])));
            assert!(!exception_pending(py));

            for failure in ["pending", "allocation", "null_range"] {
                if failure == "pending" {
                    crate::record_memory_error_without_allocation(py);
                }
                let budget = (failure == "allocation").then(deny_allocations);
                let address = if failure == "null_range" { 0 } else { address };
                let result = unsafe { molt_tuple_from_values(address, len) };
                drop(budget);
                assert!(obj_from_bits(result).is_none(), "failure={failure}");
                assert!(
                    exception_pending(py),
                    "failure={failure}: None always carries an exception"
                );
                assert_eq!(refs(item), baseline, "failure={failure}: nothing retained");
                clear_exception(py);
            }
            dec_ref_bits(py, item);
        });
    }

    #[test]
    fn list_from_values_owns_each_word_and_never_interns_mutable_storage() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let item = bits_from_ptr(alloc_string(py, b"list-from-values-owner"));
            let baseline = refs(item);
            let words = [item, MoltObject::from_int(7).bits(), item];
            let address = crate::provenance::abi::expose_address(words.as_ptr());
            let list = unsafe { molt_list_from_values(address, words.len() as u64) };
            assert!(!obj_from_bits(list).is_none());
            assert_eq!(refs(item), baseline + 2);
            {
                let snapshot = unsafe {
                    crate::object::seq_access::snapshot(py, ptr_from_bits(list), "test snapshot")
                }
                .unwrap();
                assert_eq!(&*snapshot, &words);
            }
            dec_ref_bits(py, list);
            assert_eq!(refs(item), baseline);
            let empty = unsafe { molt_list_from_values(0, 0) };
            let another = unsafe { molt_list_from_values(0, 0) };
            assert!(!obj_from_bits(empty).is_none() && !obj_from_bits(another).is_none());
            assert_ne!(
                empty, another,
                "mutable empty lists must have independent storage"
            );
            dec_ref_bits(py, empty);
            dec_ref_bits(py, another);
            for failure in ["pending", "allocation", "null_range"] {
                if failure == "pending" {
                    crate::record_memory_error_without_allocation(py);
                }
                let budget = (failure == "allocation").then(deny_allocations);
                let pointer = if failure == "null_range" { 0 } else { address };
                let result = unsafe { molt_list_from_values(pointer, words.len() as u64) };
                drop(budget);
                assert!(obj_from_bits(result).is_none(), "failure={failure}");
                assert!(exception_pending(py), "failure={failure}");
                assert_eq!(
                    refs(item),
                    baseline,
                    "failure={failure}: no retained partial state"
                );
                clear_exception(py);
            }
            dec_ref_bits(py, item);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        acyclic_slot_edge, alloc_code_obj, alloc_function_obj, clone_code_obj_with_protocol_flags,
    };
    use crate::object::heap_kinds_generated::HeapAcyclicSlot;
    use crate::object::layout::{
        CodeCallableIdentity, FunctionCallAbi, code_callable_identity, code_cellvars_bits,
        code_freevars_bits, code_publish_callable_identity, code_publish_lexical_metadata,
    };
    use crate::{
        TYPE_ID_FUNCTION, alloc_bytes, alloc_list, alloc_string, alloc_tuple, dec_ref_bits,
        function_globals_bits, object_type_id,
    };
    use molt_obj_model::MoltObject;

    extern "C" fn allocator_inert_function_target() -> u64 {
        MoltObject::none().bits()
    }

    /// An instance owns one strong edge to its heap class and releases it
    /// with the instance. Allocation takes the class's sealed layout size: an
    /// extent smaller than the sealed layout is a SystemError.
    #[test]
    fn alloc_class_owns_one_heap_class_edge_released_with_the_instance() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let name = crate::builtins::attr::attr_name_bits_from_bytes(py, b"HeapClassRef")
                .expect("class name");
            let class = crate::molt_class_new(name);
            dec_ref_bits(py, name);
            let class_ptr = MoltObject::from_bits(class).as_ptr().expect("heap class");
            unsafe { crate::object::class_finish_definition(py, class_ptr) }.expect("seal class");
            let size = unsafe { crate::object::layout::class_cached_layout_size(class_ptr) }
                .expect("sealed layout size");
            let refs = || unsafe { (*crate::header_from_obj_ptr(class_ptr)).ref_count_snapshot() };
            let before = refs();

            let object = super::molt_alloc_class(size as u64, class);
            assert_ne!(object, MoltObject::none().bits());
            assert_eq!(refs(), before + 1);
            let actual_type = crate::molt_type_of(object);
            assert_eq!(actual_type, class);
            dec_ref_bits(py, actual_type);
            assert_eq!(refs(), before + 1);

            dec_ref_bits(py, object);
            assert_eq!(refs(), before);
            dec_ref_bits(py, class);
        });
    }

    #[test]
    fn pointer_guard_adopts_once_and_releases_only_at_scope_exit() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for make_guard in [super::PtrDropGuard::new, super::PtrDropGuard::preserving] {
                let ptr = alloc_tuple(py, &[MoltObject::from_int(17).bits()]);
                assert!(!ptr.is_null());
                let bits = MoltObject::from_ptr(ptr).bits();
                crate::inc_ref_bits(py, bits);
                let count = || unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() };
                let guard = make_guard(ptr);
                assert_eq!(
                    count(),
                    2,
                    "constructing an owner must not retire its reference"
                );
                drop(guard);
                assert_eq!(count(), 1, "scope exit must retire exactly one reference");
                crate::inc_ref_bits(py, bits);
                let mut guard = make_guard(ptr);
                guard.release();
                drop(guard);
                assert_eq!(count(), 2, "released custody belongs to the caller");
                dec_ref_bits(py, bits);
                dec_ref_bits(py, bits);
                drop(make_guard(std::ptr::null_mut()));
            }
        });
    }

    #[test]
    fn code_clone_retains_exact_lexical_metadata_tuples() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let name = alloc_string(py, b"clone_lexical");
            let freevar = alloc_string(py, b"captured");
            let cellvar = alloc_string(py, b"local_cell");
            let _name_owner = super::PtrDropGuard::preserving(name);
            let _freevar_owner = super::PtrDropGuard::preserving(freevar);
            let _cellvar_owner = super::PtrDropGuard::preserving(cellvar);
            assert!(!name.is_null() && !freevar.is_null() && !cellvar.is_null());
            let name_bits = MoltObject::from_ptr(name).bits();
            let freevar_bits = MoltObject::from_ptr(freevar).bits();
            let cellvar_bits = MoltObject::from_ptr(cellvar).bits();
            let empty = alloc_tuple(py, &[]);
            let freevars = alloc_tuple(py, &[freevar_bits]);
            let cellvars = alloc_tuple(py, &[cellvar_bits]);
            let _empty_owner = super::PtrDropGuard::preserving(empty);
            let _freevars_owner = super::PtrDropGuard::preserving(freevars);
            let _cellvars_owner = super::PtrDropGuard::preserving(cellvars);
            assert!(!empty.is_null() && !freevars.is_null() && !cellvars.is_null());
            let empty_bits = MoltObject::from_ptr(empty).bits();
            let freevars_bits = MoltObject::from_ptr(freevars).bits();
            let cellvars_bits = MoltObject::from_ptr(cellvars).bits();
            let source = alloc_code_obj(
                py,
                name_bits,
                name_bits,
                1,
                MoltObject::none().bits(),
                empty_bits,
                empty_bits,
                0,
                0,
                0,
            );
            let source_owner = super::PtrDropGuard::preserving(source);
            assert!(!source.is_null());
            unsafe {
                assert!(code_publish_lexical_metadata(
                    py,
                    source,
                    freevars_bits,
                    cellvars_bits,
                ));
                let identity = CodeCallableIdentity {
                    fn_ptr: 0x101,
                    trampoline_ptr: 0x202,
                    arity: 0,
                    call_abi: FunctionCallAbi::LexicalClosureFirst,
                    custody: crate::object::layout::EntryCustody::Adopting,
                };
                assert_eq!(code_publish_callable_identity(source, identity), Ok(()));
                let descriptor = alloc_string(py, b"{\"kind\":\"molt_gpu_kernel\"}");
                assert!(!descriptor.is_null());
                let descriptor_owner = super::PtrDropGuard::preserving(descriptor);
                let descriptor_bits = MoltObject::from_ptr(descriptor).bits();
                assert!(crate::object::layout::code_publish_gpu_descriptor(
                    py,
                    source,
                    descriptor_bits
                ));
                let refs = || (*crate::header_from_obj_ptr(descriptor)).ref_count_snapshot();
                assert_eq!(refs(), 2);
                let clone = clone_code_obj_with_protocol_flags(py, source, 0);
                let clone_owner = super::PtrDropGuard::preserving(clone);
                assert!(!clone.is_null());
                assert_eq!(code_freevars_bits(clone), freevars_bits);
                assert_eq!(code_cellvars_bits(clone), cellvars_bits);
                assert_eq!(code_callable_identity(clone), Some(identity));
                assert_eq!(
                    crate::object::layout::code_gpu_descriptor_bits(clone),
                    descriptor_bits
                );
                assert_eq!(refs(), 3);
                drop(clone_owner);
                drop(source_owner);
                assert_eq!(
                    refs(),
                    1,
                    "both code owners must retire their descriptor edge"
                );
                drop(descriptor_owner);
            }
        });
    }

    #[test]
    fn byte_storage_constructors_admit_only_their_physical_payload_kind() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for kind in [
                super::InlineBytesKind::String,
                super::InlineBytesKind::Bytes,
            ] {
                let ptr = super::alloc_inline_bytes(py, b"storage payload", kind);
                assert!(!ptr.is_null());
                unsafe {
                    assert_eq!(object_type_id(ptr), kind.type_id());
                    assert_eq!(*(ptr as *const usize), b"storage payload".len());
                    assert_eq!(
                        std::slice::from_raw_parts(
                            crate::object::layout::InlineBytesStorage::data(ptr),
                            b"storage payload".len(),
                        ),
                        b"storage payload",
                    );
                }
                dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
                assert!(super::alloc_inline_bytes_with_len(py, usize::MAX, kind).is_null());
                assert!(crate::exception_pending(py));
                crate::clear_exception(py);
            }
            let ptr = super::alloc_bytearray(py, b"storage payload");
            assert!(!ptr.is_null());
            unsafe {
                assert_eq!(object_type_id(ptr), crate::TYPE_ID_BYTEARRAY);
                assert_eq!(
                    (*crate::bytearray_vec_ptr(ptr)).as_slice(),
                    b"storage payload"
                );
            }
            dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
            assert!(!crate::exception_pending(py));
        });
    }

    unsafe fn assert_inline_content_and_terminator(ptr: *mut u8, expected: &[u8]) {
        assert!(!ptr.is_null());
        unsafe {
            // Check the allocation bound before reading the byte beyond content.
            assert!(
                crate::object::object_payload_size(ptr)
                    > std::mem::size_of::<usize>() + expected.len()
            );
            assert_eq!(crate::string_len(ptr), expected.len());
            let data = crate::string_bytes(ptr);
            assert_eq!(std::slice::from_raw_parts(data, expected.len()), expected);
            assert_eq!(*data.add(expected.len()), 0);
        }
    }

    #[test]
    fn inline_bytes_terminator_is_owned_beyond_logical_content_and_c_abi_length() {
        use molt_cpython_abi::api::{refcount, strings};
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for length in [0, 1, 7, 8, 9, 15, 16, 17, 255, 256] {
                let mut content = vec![b'x'; length];
                if length > 2 {
                    content[1] = 0;
                }
                let text = alloc_string(py, &content);
                unsafe {
                    assert_inline_content_and_terminator(text, &content);
                }
                dec_ref_bits(py, MoltObject::from_ptr(text).bits());
                unsafe {
                    let view = strings::PyBytes_FromStringAndSize(
                        content.as_ptr().cast(),
                        length as isize,
                    );
                    assert!(!view.is_null());
                    let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .observed_handle_for_pyobj(view)
                        .unwrap()
                        .bits();
                    let ptr = MoltObject::from_bits(bits).as_ptr().unwrap();
                    assert_inline_content_and_terminator(ptr, &content);
                    let mut data = std::ptr::null_mut();
                    let mut c_length = -1;
                    assert_eq!(
                        strings::PyBytes_AsStringAndSize(view, &raw mut data, &raw mut c_length),
                        0
                    );
                    assert_eq!(c_length, length as isize);
                    assert_eq!(data.cast::<u8>().cast_const(), crate::bytes_data(ptr));
                    assert_eq!(strings::PyBytes_AsString(view), data);
                    assert_eq!(strings::PyBytes_AS_STRING(view), data);
                    assert_eq!(*data.add(length), 0);
                    refcount::Py_DECREF(view);
                }
            }
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    fn unique_string_append_restores_terminator_after_growth_and_capacity_reuse() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let left = alloc_string(py, b"left");
            let right = alloc_string(py, &[b'x'; 256]);
            let left_bits = MoltObject::from_ptr(left).bits();
            let right_bits = MoltObject::from_ptr(right).bits();
            let grown = crate::molt_inplace_add(left_bits, right_bits);
            assert!(!crate::exception_pending(py));
            let grown_ptr = MoltObject::from_bits(grown).as_ptr().unwrap();
            assert_ne!(
                grown_ptr, left,
                "the initial allocation cannot hold this append"
            );
            let mut expected = b"left".to_vec();
            expected.extend_from_slice(&[b'x'; 256]);
            unsafe {
                assert_inline_content_and_terminator(grown_ptr, &expected);
            }
            dec_ref_bits(py, left_bits);
            dec_ref_bits(py, right_bits);

            // Poison the next sentinel position so capacity reuse must write it.
            unsafe {
                assert!(
                    crate::object::object_payload_size(grown_ptr)
                        >= std::mem::size_of::<usize>() + expected.len() + 2
                );
                (crate::string_bytes(grown_ptr) as *mut u8)
                    .add(expected.len() + 1)
                    .write(0x7f);
            }
            let suffix = MoltObject::from_ptr(alloc_string(py, b"!")).bits();
            let appended = crate::molt_inplace_add(grown, suffix);
            assert!(!crate::exception_pending(py));
            assert_eq!(appended, grown, "amortized capacity is reused");
            expected.push(b'!');
            unsafe {
                assert_inline_content_and_terminator(grown_ptr, &expected);
            }
            for bits in [suffix, grown, appended] {
                dec_ref_bits(py, bits);
            }
        });
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
