use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use molt_obj_model::MoltObject;
use num_bigint::BigInt;

pub(crate) mod accessors;
pub(crate) mod buffer2d;
pub(crate) mod builders;
pub(crate) mod layout;
pub(crate) mod memoryview;
pub(crate) mod ops;
pub(crate) mod type_ids;
pub(crate) mod utf8_cache;

#[allow(unused_imports)]
pub(crate) use type_ids::*;

use crate::call::bind::callargs_drop;
use crate::provenance::{register_ptr, release_ptr, resolve_ptr};
use crate::{
    asyncgen_gen_bits, asyncgen_pending_bits, asyncgen_registry_remove, asyncgen_running_bits,
    bound_method_func_bits, bound_method_self_bits, bytearray_data, bytearray_len,
    bytearray_vec_ptr, call_iter_callable_bits, call_iter_sentinel_bits, callargs_ptr,
    classmethod_func_bits, code_filename_bits, code_linetable_bits, code_name_bits,
    context_payload_bits, dict_order_ptr, dict_table_ptr, dict_view_dict_bits,
    enumerate_index_bits, enumerate_target_bits, exception_args_bits, exception_cause_bits,
    exception_class_bits, exception_context_bits, exception_kind_bits, exception_msg_bits,
    exception_suppress_bits, exception_trace_bits, exception_value_bits, filter_func_bits,
    filter_iter_bits, function_annotate_bits, function_annotations_bits, function_closure_bits,
    function_code_bits, function_dict_bits, generator_exception_stack_drop,
    generic_alias_args_bits, generic_alias_origin_bits, io_wait_poll_fn_addr,
    io_wait_release_socket, iter_target_bits, map_func_bits, map_iters_ptr, module_dict_bits,
    module_name_bits, process_poll_fn_addr, profile_hit, property_del_bits,
    property_get_bits, property_set_bits, reversed_target_bits, runtime_state, seq_vec_ptr,
    set_order_ptr, set_table_ptr, slice_start_bits, slice_step_bits, slice_stop_bits,
    staticmethod_func_bits, task_cancel_message_clear, thread_poll_fn_addr,
    utf8_cache_remove, zip_iters_ptr, PyToken, ALLOC_COUNT, GEN_CLOSED_OFFSET,
    GEN_EXC_DEPTH_OFFSET, GEN_SEND_OFFSET, GEN_THROW_OFFSET, TYPE_ID_ASYNC_GENERATOR,
    TYPE_ID_BIGINT, TYPE_ID_BOUND_METHOD, TYPE_ID_BUFFER2D, TYPE_ID_BYTEARRAY, TYPE_ID_CALLARGS,
    TYPE_ID_CALL_ITER, TYPE_ID_CLASSMETHOD, TYPE_ID_CODE, TYPE_ID_CONTEXT_MANAGER,
    TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_DICT_ITEMS_VIEW, TYPE_ID_DICT_KEYS_VIEW,
    TYPE_ID_DICT_VALUES_VIEW, TYPE_ID_ENUMERATE, TYPE_ID_EXCEPTION, TYPE_ID_FILE_HANDLE,
    TYPE_ID_FILTER, TYPE_ID_FROZENSET, TYPE_ID_FUNCTION, TYPE_ID_GENERATOR, TYPE_ID_GENERIC_ALIAS,
    TYPE_ID_ITER, TYPE_ID_LIST, TYPE_ID_MAP, TYPE_ID_MEMORYVIEW, TYPE_ID_MODULE,
    TYPE_ID_NOT_IMPLEMENTED, TYPE_ID_OBJECT, TYPE_ID_PROPERTY, TYPE_ID_REVERSED, TYPE_ID_SET,
    TYPE_ID_SLICE, TYPE_ID_STATICMETHOD, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_ZIP,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::{process_task_drop, thread_task_drop};

#[repr(C)]
pub struct MoltHeader {
    pub type_id: u32,
    pub ref_count: AtomicU32,
    pub poll_fn: u64, // Function pointer for polling
    pub state: i64,   // State machine state
    pub size: usize,  // Total size of allocation
    pub flags: u64,   // Header flags (object metadata)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PtrSlot(pub(crate) *mut u8);

// Raw pointers are guarded by locks; it is safe to share slots across threads.
unsafe impl Send for PtrSlot {}
unsafe impl Sync for PtrSlot {}

pub(crate) struct DataclassDesc {
    pub(crate) name: String,
    pub(crate) field_names: Vec<String>,
    pub(crate) frozen: bool,
    pub(crate) eq: bool,
    pub(crate) repr: bool,
    pub(crate) slots: bool,
    pub(crate) class_bits: u64,
}

pub(crate) struct Buffer2D {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) data: Vec<i64>,
}

#[repr(C)]
pub(crate) struct MemoryView {
    pub(crate) owner_bits: u64,
    pub(crate) offset: isize,
    pub(crate) len: usize,
    pub(crate) itemsize: usize,
    pub(crate) stride: isize,
    pub(crate) readonly: u8,
    pub(crate) ndim: u8,
    pub(crate) _pad: [u8; 6],
    pub(crate) format_bits: u64,
    pub(crate) shape_ptr: *mut Vec<isize>,
    pub(crate) strides_ptr: *mut Vec<isize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryViewFormatKind {
    Signed,
    Unsigned,
    Float,
    Bool,
    Char,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MemoryViewFormat {
    pub(crate) code: u8,
    pub(crate) itemsize: usize,
    pub(crate) kind: MemoryViewFormatKind,
}

pub(crate) struct MoltFileState {
    pub(crate) file: Mutex<Option<std::fs::File>>,
}

pub(crate) struct MoltFileHandle {
    pub(crate) state: Arc<MoltFileState>,
    pub(crate) readable: bool,
    pub(crate) writable: bool,
    pub(crate) text: bool,
    // TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:missing):
    // expose closefd/buffer metadata as file attributes for full parity.
    #[allow(dead_code)]
    pub(crate) closefd: bool,
    pub(crate) owns_fd: bool,
    pub(crate) closed: bool,
    pub(crate) detached: bool,
    pub(crate) line_buffering: bool,
    pub(crate) write_through: bool,
    #[allow(dead_code)]
    pub(crate) buffer_size: i64,
    #[allow(dead_code)]
    pub(crate) class_bits: u64,
    pub(crate) name_bits: u64,
    pub(crate) mode: String,
    pub(crate) encoding: Option<String>,
    pub(crate) errors: Option<String>,
    pub(crate) newline: Option<String>,
    pub(crate) buffer_bits: u64,
    pub(crate) pending_byte: Option<u8>,
}

const OBJECT_POOL_MAX_BYTES: usize = 1024;
const OBJECT_POOL_BUCKET_LIMIT: usize = 4096;
const OBJECT_POOL_TLS_BUCKET_LIMIT: usize = 1024;
pub(crate) const OBJECT_POOL_BUCKETS: usize = OBJECT_POOL_MAX_BYTES / 8 + 1;
pub(crate) const HEADER_FLAG_HAS_PTRS: u64 = 1;
pub(crate) const HEADER_FLAG_SKIP_CLASS_DECREF: u64 = 1 << 1;
pub(crate) const HEADER_FLAG_GEN_RUNNING: u64 = 1 << 2;
pub(crate) const HEADER_FLAG_GEN_STARTED: u64 = 1 << 3;
pub(crate) const HEADER_FLAG_SPAWN_RETAIN: u64 = 1 << 4;
pub(crate) const HEADER_FLAG_CANCEL_PENDING: u64 = 1 << 5;
pub(crate) const HEADER_FLAG_BLOCK_ON: u64 = 1 << 6;

thread_local! {
    pub(crate) static OBJECT_POOL_TLS: RefCell<Vec<Vec<PtrSlot>>> =
        RefCell::new(vec![Vec::new(); OBJECT_POOL_BUCKETS]);
}

pub(crate) fn obj_from_bits(bits: u64) -> MoltObject {
    MoltObject::from_bits(bits)
}

pub(crate) fn inc_ref_bits(_py: &PyToken<'_>, bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe { inc_ref_ptr(_py, ptr) };
    }
}

pub(crate) fn dec_ref_bits(_py: &PyToken<'_>, bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe { dec_ref_ptr(_py, ptr) };
    }
}

pub(crate) fn init_atomic_bits(
    py: &PyToken<'_>,
    slot: &AtomicU64,
    init: impl FnOnce() -> u64,
) -> u64 {
    let existing = slot.load(AtomicOrdering::Acquire);
    if existing != 0 {
        return existing;
    }
    let new_bits = init();
    if new_bits == 0 {
        return 0;
    }
    match slot.compare_exchange(0, new_bits, AtomicOrdering::AcqRel, AtomicOrdering::Acquire) {
        Ok(_) => new_bits,
        Err(prev) => {
            dec_ref_bits(py, new_bits);
            prev
        }
    }
}

pub(crate) fn pending_bits_i64() -> i64 {
    MoltObject::pending().bits() as i64
}

fn object_pool(_py: &PyToken<'_>) -> &'static Mutex<Vec<Vec<PtrSlot>>> {
    &runtime_state(_py).object_pool
}

fn object_pool_index(total_size: usize) -> Option<usize> {
    if total_size == 0 || total_size > OBJECT_POOL_MAX_BYTES || !total_size.is_multiple_of(8) {
        return None;
    }
    Some(total_size / 8)
}

fn object_pool_take(_py: &PyToken<'_>, total_size: usize) -> Option<*mut u8> {
    crate::gil_assert();
    let idx = object_pool_index(total_size)?;
    let from_tls = OBJECT_POOL_TLS.with(|pool| {
        let mut pool = pool.borrow_mut();
        pool.get_mut(idx).and_then(|bucket| bucket.pop())
    });
    if let Some(slot) = from_tls {
        return Some(slot.0);
    }
    let mut guard = object_pool(_py).lock().unwrap();
    guard
        .get_mut(idx)
        .and_then(|bucket| bucket.pop())
        .map(|slot| slot.0)
}

fn object_pool_put(_py: &PyToken<'_>, total_size: usize, header_ptr: *mut u8) -> bool {
    crate::gil_assert();
    if header_ptr.is_null() {
        return false;
    }
    let Some(idx) = object_pool_index(total_size) else {
        return false;
    };
    unsafe {
        std::ptr::write_bytes(header_ptr, 0, total_size);
    }
    let stored_tls = OBJECT_POOL_TLS.with(|pool| {
        let mut pool = pool.borrow_mut();
        let bucket = &mut pool[idx];
        if bucket.len() >= OBJECT_POOL_TLS_BUCKET_LIMIT {
            return false;
        }
        bucket.push(PtrSlot(header_ptr));
        true
    });
    if stored_tls {
        return true;
    }
    let mut guard = object_pool(_py).lock().unwrap();
    let bucket = &mut guard[idx];
    if bucket.len() >= OBJECT_POOL_BUCKET_LIMIT {
        return false;
    }
    bucket.push(PtrSlot(header_ptr));
    true
}

pub(crate) fn alloc_object_zeroed_with_pool(
    _py: &PyToken<'_>,
    total_size: usize,
    type_id: u32,
) -> *mut u8 {
    crate::gil_assert();
    let header_ptr = if type_id == TYPE_ID_OBJECT {
        object_pool_take(_py, total_size)
    } else {
        None
    };
    let header_ptr = header_ptr.unwrap_or_else(|| {
        let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
        unsafe { std::alloc::alloc_zeroed(layout) }
    });
    if header_ptr.is_null() {
        return std::ptr::null_mut();
    }
    profile_hit(_py, &ALLOC_COUNT);
    unsafe {
        let header = header_ptr as *mut MoltHeader;
        (*header).type_id = type_id;
        (*header).ref_count.store(1, AtomicOrdering::Relaxed);
        (*header).poll_fn = 0;
        (*header).state = 0;
        (*header).size = total_size;
        (*header).flags = 0;
        header_ptr.add(std::mem::size_of::<MoltHeader>())
    }
}

pub(crate) fn alloc_object(_py: &PyToken<'_>, total_size: usize, type_id: u32) -> *mut u8 {
    crate::gil_assert();
    let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
    unsafe {
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        profile_hit(_py, &ALLOC_COUNT);
        let header = ptr as *mut MoltHeader;
        (*header).type_id = type_id;
        (*header).ref_count.store(1, AtomicOrdering::Relaxed);
        (*header).poll_fn = 0;
        (*header).state = 0;
        (*header).size = total_size;
        (*header).flags = 0;
        ptr.add(std::mem::size_of::<MoltHeader>())
    }
}

pub(crate) unsafe fn header_from_obj_ptr(ptr: *mut u8) -> *mut MoltHeader {
    ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader
}

pub(crate) unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    (*header_from_obj_ptr(ptr)).type_id
}

pub(crate) unsafe fn object_payload_size(ptr: *mut u8) -> usize {
    (*header_from_obj_ptr(ptr)).size - std::mem::size_of::<MoltHeader>()
}

pub(crate) unsafe fn instance_dict_bits_ptr(ptr: *mut u8) -> *mut u64 {
    let payload = object_payload_size(ptr);
    ptr.add(payload - std::mem::size_of::<u64>()) as *mut u64
}

pub(crate) unsafe fn instance_dict_bits(ptr: *mut u8) -> u64 {
    *instance_dict_bits_ptr(ptr)
}

pub(crate) unsafe fn instance_set_dict_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    *instance_dict_bits_ptr(ptr) = bits;
}

pub(crate) unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    (*header_from_obj_ptr(ptr)).state as u64
}

pub(crate) unsafe fn object_set_class_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    (*header_from_obj_ptr(ptr)).state = bits as i64;
}

pub(crate) unsafe fn object_mark_has_ptrs(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    (*header_from_obj_ptr(ptr)).flags |= HEADER_FLAG_HAS_PTRS;
}

pub(crate) unsafe fn string_len(ptr: *mut u8) -> usize {
    *(ptr as *const usize)
}

pub(crate) unsafe fn string_bytes(ptr: *mut u8) -> *const u8 {
    ptr.add(std::mem::size_of::<usize>())
}

pub(crate) unsafe fn bytes_len(ptr: *mut u8) -> usize {
    if object_type_id(ptr) == TYPE_ID_BYTEARRAY {
        return bytearray_len(ptr);
    }
    string_len(ptr)
}

pub(crate) unsafe fn intarray_len(ptr: *mut u8) -> usize {
    *(ptr as *const usize)
}

pub(crate) unsafe fn intarray_data(ptr: *mut u8) -> *const i64 {
    ptr.add(std::mem::size_of::<usize>()) as *const i64
}

pub(crate) unsafe fn intarray_slice(ptr: *mut u8) -> &'static [i64] {
    std::slice::from_raw_parts(intarray_data(ptr), intarray_len(ptr))
}

pub(crate) unsafe fn bytes_data(ptr: *mut u8) -> *const u8 {
    if object_type_id(ptr) == TYPE_ID_BYTEARRAY {
        return bytearray_data(ptr);
    }
    string_bytes(ptr)
}

pub(crate) unsafe fn memoryview_ptr(ptr: *mut u8) -> *mut MemoryView {
    ptr as *mut MemoryView
}

pub(crate) unsafe fn memoryview_owner_bits(ptr: *mut u8) -> u64 {
    (*memoryview_ptr(ptr)).owner_bits
}

pub(crate) unsafe fn memoryview_offset(ptr: *mut u8) -> isize {
    (*memoryview_ptr(ptr)).offset
}

pub(crate) unsafe fn memoryview_len(ptr: *mut u8) -> usize {
    (*memoryview_ptr(ptr)).len
}

pub(crate) unsafe fn memoryview_itemsize(ptr: *mut u8) -> usize {
    (*memoryview_ptr(ptr)).itemsize
}

pub(crate) unsafe fn memoryview_stride(ptr: *mut u8) -> isize {
    (*memoryview_ptr(ptr)).stride
}

pub(crate) unsafe fn memoryview_readonly(ptr: *mut u8) -> bool {
    (*memoryview_ptr(ptr)).readonly != 0
}

pub(crate) unsafe fn memoryview_ndim(ptr: *mut u8) -> usize {
    (*memoryview_ptr(ptr)).ndim as usize
}

pub(crate) unsafe fn memoryview_format_bits(ptr: *mut u8) -> u64 {
    (*memoryview_ptr(ptr)).format_bits
}

pub(crate) unsafe fn memoryview_shape_ptr(ptr: *mut u8) -> *mut Vec<isize> {
    (*memoryview_ptr(ptr)).shape_ptr
}

pub(crate) unsafe fn memoryview_strides_ptr(ptr: *mut u8) -> *mut Vec<isize> {
    (*memoryview_ptr(ptr)).strides_ptr
}

pub(crate) unsafe fn memoryview_shape(ptr: *mut u8) -> Option<&'static [isize]> {
    let shape_ptr = memoryview_shape_ptr(ptr);
    if shape_ptr.is_null() {
        return None;
    }
    Some(&*shape_ptr)
}

pub(crate) unsafe fn memoryview_strides(ptr: *mut u8) -> Option<&'static [isize]> {
    let strides_ptr = memoryview_strides_ptr(ptr);
    if strides_ptr.is_null() {
        return None;
    }
    Some(&*strides_ptr)
}

pub(crate) unsafe fn dataclass_desc_ptr(ptr: *mut u8) -> *mut DataclassDesc {
    *(ptr as *const *mut DataclassDesc)
}

pub(crate) unsafe fn dataclass_fields_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr.add(std::mem::size_of::<*mut DataclassDesc>()) as *const *mut Vec<u64>)
}

pub(crate) unsafe fn dataclass_fields_ref(ptr: *mut u8) -> &'static Vec<u64> {
    &*dataclass_fields_ptr(ptr)
}

pub(crate) unsafe fn dataclass_fields_mut(ptr: *mut u8) -> &'static mut Vec<u64> {
    &mut *dataclass_fields_ptr(ptr)
}

pub(crate) unsafe fn dataclass_dict_bits_ptr(ptr: *mut u8) -> *mut u64 {
    ptr.add(std::mem::size_of::<*mut DataclassDesc>() + std::mem::size_of::<*mut Vec<u64>>())
        as *mut u64
}

pub(crate) unsafe fn dataclass_dict_bits(ptr: *mut u8) -> u64 {
    *dataclass_dict_bits_ptr(ptr)
}

pub(crate) unsafe fn dataclass_set_dict_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    *dataclass_dict_bits_ptr(ptr) = bits;
}

pub(crate) unsafe fn buffer2d_ptr(ptr: *mut u8) -> *mut Buffer2D {
    *(ptr as *const *mut Buffer2D)
}

pub(crate) unsafe fn file_handle_ptr(ptr: *mut u8) -> *mut MoltFileHandle {
    *(ptr as *const *mut MoltFileHandle)
}

pub(crate) fn maybe_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let obj = obj_from_bits(bits);
    obj.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::{
        alloc_object_zeroed_with_pool, dec_ref_ptr, object_pool, object_pool_index,
        object_pool_take, MoltHeader, OBJECT_POOL_TLS, TYPE_ID_OBJECT, TYPE_ID_TUPLE,
    };
    use crate::PyToken;
    use std::alloc::Layout;

    fn drain_pool(_py: &PyToken<'_>, total_size: usize) {
        let Some(idx) = object_pool_index(total_size) else {
            return;
        };
        let layout = Layout::from_size_align(total_size, 8).unwrap();
        while let Some(ptr) = object_pool_take(_py, total_size) {
            unsafe { std::alloc::dealloc(ptr, layout) };
        }
        OBJECT_POOL_TLS.with(|pool| {
            if let Some(bucket) = pool.borrow_mut().get_mut(idx) {
                bucket.clear();
            }
        });
        let mut guard = object_pool(_py).lock().unwrap();
        if let Some(bucket) = guard.get_mut(idx) {
            bucket.clear();
        }
    }

    #[test]
    fn object_pool_reuses_object_allocations() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let total_size = std::mem::size_of::<MoltHeader>() + 16;
            drain_pool(_py, total_size);
            let ptr1 = alloc_object_zeroed_with_pool(_py, total_size, TYPE_ID_OBJECT);
            assert!(!ptr1.is_null());
            unsafe { dec_ref_ptr(_py, ptr1) };
            let ptr2 = alloc_object_zeroed_with_pool(_py, total_size, TYPE_ID_OBJECT);
            assert_eq!(ptr1, ptr2);
            unsafe { dec_ref_ptr(_py, ptr2) };
        });
    }

    #[test]
    fn non_object_allocations_do_not_fill_pool() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let total_size = std::mem::size_of::<MoltHeader>() + 16;
            drain_pool(_py, total_size);
            let idx = object_pool_index(total_size).expect("pool index should be valid");
            let tls_before = OBJECT_POOL_TLS.with(|pool| pool.borrow()[idx].len());
            let global_before = object_pool(_py).lock().unwrap()[idx].len();
            let ptr = alloc_object_zeroed_with_pool(_py, total_size, TYPE_ID_TUPLE);
            assert!(!ptr.is_null());
            unsafe { dec_ref_ptr(_py, ptr) };
            let tls_after = OBJECT_POOL_TLS.with(|pool| pool.borrow()[idx].len());
            let global_after = object_pool(_py).lock().unwrap()[idx].len();
            assert_eq!(tls_after, tls_before);
            assert_eq!(global_after, global_before);
        });
    }
}

#[inline]
pub(crate) fn ptr_from_bits(bits: u64) -> *mut u8 {
    let obj = obj_from_bits(bits);
    if obj.is_ptr() {
        return obj.as_ptr().unwrap_or(std::ptr::null_mut());
    }
    resolve_ptr(bits).unwrap_or(std::ptr::null_mut())
}

#[inline]
pub(crate) fn bits_from_ptr(ptr: *mut u8) -> u64 {
    register_ptr(ptr)
}

/// # Safety
/// Dereferences raw pointer to increment ref count.
#[no_mangle]
pub unsafe extern "C" fn molt_inc_ref(ptr: *mut u8) {
    crate::with_gil_entry!(_py, {
        inc_ref_ptr(_py, ptr);
    })
}

/// # Safety
/// Dereferences raw pointer to decrement ref count. Frees memory if count reaches 0.
#[no_mangle]
pub unsafe extern "C" fn molt_dec_ref(ptr: *mut u8) {
    crate::with_gil_entry!(_py, {
        dec_ref_ptr(_py, ptr);
    })
}

/// # Safety
/// Dereferences raw pointer to increment ref count.
pub(crate) unsafe fn inc_ref_ptr(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    if ptr.is_null() {
        return;
    }
    let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    (*header_ptr)
        .ref_count
        .fetch_add(1, AtomicOrdering::Relaxed);
}

/// # Safety
/// Dereferences raw pointer to decrement ref count. Frees memory if count reaches 0.
pub(crate) unsafe fn dec_ref_ptr(py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    if ptr.is_null() {
        return;
    }
    let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    let header = &mut *header_ptr;
    if header.type_id == TYPE_ID_NOT_IMPLEMENTED {
        return;
    }
    if header.ref_count.fetch_sub(1, AtomicOrdering::AcqRel) == 1 {
        std::sync::atomic::fence(AtomicOrdering::Acquire);
        match header.type_id {
            TYPE_ID_STRING => {
                utf8_cache_remove(py, ptr as usize);
            }
            TYPE_ID_LIST => {
                let vec_ptr = seq_vec_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(py, *bits);
                    }
                }
            }
            TYPE_ID_BYTEARRAY => {
                let vec_ptr = bytearray_vec_ptr(ptr);
                if !vec_ptr.is_null() {
                    drop(Box::from_raw(vec_ptr));
                }
            }
            TYPE_ID_TUPLE => {
                let vec_ptr = seq_vec_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(py, *bits);
                    }
                }
            }
            TYPE_ID_DICT => {
                let order_ptr = dict_order_ptr(ptr);
                let table_ptr = dict_table_ptr(ptr);
                if !order_ptr.is_null() {
                    let order = Box::from_raw(order_ptr);
                    for bits in order.iter() {
                        dec_ref_bits(py, *bits);
                    }
                }
                if !table_ptr.is_null() {
                    drop(Box::from_raw(table_ptr));
                }
            }
            TYPE_ID_SET | TYPE_ID_FROZENSET => {
                let order_ptr = set_order_ptr(ptr);
                let table_ptr = set_table_ptr(ptr);
                if !order_ptr.is_null() {
                    let order = Box::from_raw(order_ptr);
                    for bits in order.iter() {
                        dec_ref_bits(py, *bits);
                    }
                }
                if !table_ptr.is_null() {
                    drop(Box::from_raw(table_ptr));
                }
            }
            TYPE_ID_MEMORYVIEW => {
                let owner_bits = memoryview_owner_bits(ptr);
                if owner_bits != 0 && !obj_from_bits(owner_bits).is_none() {
                    dec_ref_bits(py, owner_bits);
                }
            }
            TYPE_ID_RANGE => {}
            TYPE_ID_SLICE => {
                let start_bits = slice_start_bits(ptr);
                let stop_bits = slice_stop_bits(ptr);
                let step_bits = slice_step_bits(ptr);
                if start_bits != 0 && !obj_from_bits(start_bits).is_none() {
                    dec_ref_bits(py, start_bits);
                }
                if stop_bits != 0 && !obj_from_bits(stop_bits).is_none() {
                    dec_ref_bits(py, stop_bits);
                }
                if step_bits != 0 && !obj_from_bits(step_bits).is_none() {
                    dec_ref_bits(py, step_bits);
                }
            }
            TYPE_ID_DATACLASS => {
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() {
                    drop(Box::from_raw(desc_ptr));
                }
            }
            TYPE_ID_CODE => {
                let filename_bits = code_filename_bits(ptr);
                let name_bits = code_name_bits(ptr);
                let linetable_bits = code_linetable_bits(ptr);
                if filename_bits != 0 && !obj_from_bits(filename_bits).is_none() {
                    dec_ref_bits(py, filename_bits);
                }
                if name_bits != 0 && !obj_from_bits(name_bits).is_none() {
                    dec_ref_bits(py, name_bits);
                }
                if linetable_bits != 0 && !obj_from_bits(linetable_bits).is_none() {
                    dec_ref_bits(py, linetable_bits);
                }
            }
            TYPE_ID_FUNCTION => {
                let dict_bits = function_dict_bits(ptr);
                if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                    dec_ref_bits(py, dict_bits);
                }
                let annotations_bits = function_annotations_bits(ptr);
                if annotations_bits != 0 && !obj_from_bits(annotations_bits).is_none() {
                    dec_ref_bits(py, annotations_bits);
                }
                let annotate_bits = function_annotate_bits(ptr);
                if annotate_bits != 0 && !obj_from_bits(annotate_bits).is_none() {
                    dec_ref_bits(py, annotate_bits);
                }
                let code_bits = function_code_bits(ptr);
                if code_bits != 0 && !obj_from_bits(code_bits).is_none() {
                    dec_ref_bits(py, code_bits);
                }
                let closure_bits = function_closure_bits(ptr);
                if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
                    dec_ref_bits(py, closure_bits);
                }
            }
            TYPE_ID_BOUND_METHOD => {
                let func_bits = bound_method_func_bits(ptr);
                let self_bits = bound_method_self_bits(ptr);
                if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                    dec_ref_bits(py, func_bits);
                }
                if self_bits != 0 && !obj_from_bits(self_bits).is_none() {
                    dec_ref_bits(py, self_bits);
                }
            }
            TYPE_ID_PROPERTY => {
                let get_bits = property_get_bits(ptr);
                let set_bits = property_set_bits(ptr);
                let del_bits = property_del_bits(ptr);
                if get_bits != 0 && !obj_from_bits(get_bits).is_none() {
                    dec_ref_bits(py, get_bits);
                }
                if set_bits != 0 && !obj_from_bits(set_bits).is_none() {
                    dec_ref_bits(py, set_bits);
                }
                if del_bits != 0 && !obj_from_bits(del_bits).is_none() {
                    dec_ref_bits(py, del_bits);
                }
            }
            TYPE_ID_CLASSMETHOD => {
                let func_bits = classmethod_func_bits(ptr);
                if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                    dec_ref_bits(py, func_bits);
                }
            }
            TYPE_ID_STATICMETHOD => {
                let func_bits = staticmethod_func_bits(ptr);
                if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                    dec_ref_bits(py, func_bits);
                }
            }
            TYPE_ID_GENERIC_ALIAS => {
                let origin_bits = generic_alias_origin_bits(ptr);
                let args_bits = generic_alias_args_bits(ptr);
                if origin_bits != 0 && !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(py, origin_bits);
                }
                if args_bits != 0 && !obj_from_bits(args_bits).is_none() {
                    dec_ref_bits(py, args_bits);
                }
            }
            TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_VALUES_VIEW | TYPE_ID_DICT_ITEMS_VIEW => {
                let dict_bits = dict_view_dict_bits(ptr);
                if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                    dec_ref_bits(py, dict_bits);
                }
            }
            TYPE_ID_CALLARGS => {
                let args_ptr = callargs_ptr(ptr);
                callargs_drop(py, args_ptr);
            }
            TYPE_ID_EXCEPTION => {
                let exc_kind_bits = exception_kind_bits(ptr);
                if exc_kind_bits != 0 && !obj_from_bits(exc_kind_bits).is_none() {
                    dec_ref_bits(py, exc_kind_bits);
                }
                let exc_msg_bits = exception_msg_bits(ptr);
                if exc_msg_bits != 0 && !obj_from_bits(exc_msg_bits).is_none() {
                    dec_ref_bits(py, exc_msg_bits);
                }
                let exc_type_bits = exception_class_bits(ptr);
                if exc_type_bits != 0 && !obj_from_bits(exc_type_bits).is_none() {
                    dec_ref_bits(py, exc_type_bits);
                }
                let exc_args_bits = exception_args_bits(ptr);
                if exc_args_bits != 0 && !obj_from_bits(exc_args_bits).is_none() {
                    dec_ref_bits(py, exc_args_bits);
                }
                let exc_cause_bits = exception_cause_bits(ptr);
                if exc_cause_bits != 0 && !obj_from_bits(exc_cause_bits).is_none() {
                    dec_ref_bits(py, exc_cause_bits);
                }
                let exc_ctx_bits = exception_context_bits(ptr);
                if exc_ctx_bits != 0 && !obj_from_bits(exc_ctx_bits).is_none() {
                    dec_ref_bits(py, exc_ctx_bits);
                }
                let exc_trace_bits = exception_trace_bits(ptr);
                if exc_trace_bits != 0 && !obj_from_bits(exc_trace_bits).is_none() {
                    dec_ref_bits(py, exc_trace_bits);
                }
                let exc_suppress_bits = exception_suppress_bits(ptr);
                if exc_suppress_bits != 0 && !obj_from_bits(exc_suppress_bits).is_none() {
                    dec_ref_bits(py, exc_suppress_bits);
                }
                let exc_val_bits = exception_value_bits(ptr);
                if exc_val_bits != 0 && !obj_from_bits(exc_val_bits).is_none() {
                    dec_ref_bits(py, exc_val_bits);
                }
            }
            TYPE_ID_CONTEXT_MANAGER => {
                let payload_bits = context_payload_bits(ptr);
                if payload_bits != 0 && !obj_from_bits(payload_bits).is_none() {
                    dec_ref_bits(py, payload_bits);
                }
            }
            TYPE_ID_MODULE => {
                let dict_bits = module_dict_bits(ptr);
                if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                    dec_ref_bits(py, dict_bits);
                }
                let name_bits = module_name_bits(ptr);
                if name_bits != 0 && !obj_from_bits(name_bits).is_none() {
                    dec_ref_bits(py, name_bits);
                }
            }
            TYPE_ID_ENUMERATE => {
                let target_bits = enumerate_target_bits(ptr);
                if target_bits != 0 && !obj_from_bits(target_bits).is_none() {
                    dec_ref_bits(py, target_bits);
                }
                let idx_bits = enumerate_index_bits(ptr);
                if idx_bits != 0 && !obj_from_bits(idx_bits).is_none() {
                    dec_ref_bits(py, idx_bits);
                }
            }
            TYPE_ID_FILTER => {
                let func_bits = filter_func_bits(ptr);
                let iter_bits = filter_iter_bits(ptr);
                if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                    dec_ref_bits(py, func_bits);
                }
                if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
                    dec_ref_bits(py, iter_bits);
                }
            }
            TYPE_ID_MAP => {
                let func_bits = map_func_bits(ptr);
                let iters_ptr = map_iters_ptr(ptr);
                if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
                    dec_ref_bits(py, func_bits);
                }
                if !iters_ptr.is_null() {
                    let iters = Box::from_raw(iters_ptr);
                    for bits in iters.iter() {
                        dec_ref_bits(py, *bits);
                    }
                }
            }
            TYPE_ID_ITER => {
                let target_bits = iter_target_bits(ptr);
                if target_bits != 0 && !obj_from_bits(target_bits).is_none() {
                    dec_ref_bits(py, target_bits);
                }
            }
            TYPE_ID_REVERSED => {
                let target_bits = reversed_target_bits(ptr);
                if target_bits != 0 && !obj_from_bits(target_bits).is_none() {
                    dec_ref_bits(py, target_bits);
                }
            }
            TYPE_ID_ZIP => {
                let iters_ptr = zip_iters_ptr(ptr);
                if !iters_ptr.is_null() {
                    let iters = Box::from_raw(iters_ptr);
                    for bits in iters.iter() {
                        dec_ref_bits(py, *bits);
                    }
                }
            }
            TYPE_ID_GENERATOR => {
                let send_bits = *(ptr.add(GEN_SEND_OFFSET) as *const u64);
                let throw_bits = *(ptr.add(GEN_THROW_OFFSET) as *const u64);
                let closed_bits = *(ptr.add(GEN_CLOSED_OFFSET) as *const u64);
                let depth_bits = *(ptr.add(GEN_EXC_DEPTH_OFFSET) as *const u64);
                dec_ref_bits(py, send_bits);
                dec_ref_bits(py, throw_bits);
                dec_ref_bits(py, closed_bits);
                dec_ref_bits(py, depth_bits);
                generator_exception_stack_drop(py, ptr);
            }
            TYPE_ID_ASYNC_GENERATOR => {
                let pending_bits = asyncgen_pending_bits(ptr);
                let running_bits = asyncgen_running_bits(ptr);
                let gen_bits = asyncgen_gen_bits(ptr);
                if pending_bits != 0 && !obj_from_bits(pending_bits).is_none() {
                    dec_ref_bits(py, pending_bits);
                }
                if running_bits != 0 && !obj_from_bits(running_bits).is_none() {
                    dec_ref_bits(py, running_bits);
                }
                if gen_bits != 0 && !obj_from_bits(gen_bits).is_none() {
                    dec_ref_bits(py, gen_bits);
                }
                asyncgen_registry_remove(py, ptr);
            }
            TYPE_ID_BUFFER2D => {
                let buffer_ptr = buffer2d_ptr(ptr);
                if !buffer_ptr.is_null() {
                    drop(Box::from_raw(buffer_ptr));
                }
            }
            TYPE_ID_FILE_HANDLE => {
                let handle_ptr = file_handle_ptr(ptr);
                if !handle_ptr.is_null() {
                    drop(Box::from_raw(handle_ptr));
                }
            }
            TYPE_ID_CALL_ITER => {
                let sentinel_bits = call_iter_sentinel_bits(ptr);
                let callable_bits = call_iter_callable_bits(ptr);
                if sentinel_bits != 0 && !obj_from_bits(sentinel_bits).is_none() {
                    dec_ref_bits(py, sentinel_bits);
                }
                if callable_bits != 0 && !obj_from_bits(callable_bits).is_none() {
                    dec_ref_bits(py, callable_bits);
                }
            }
            TYPE_ID_OBJECT => {
                let poll_fn = header.poll_fn;
                if poll_fn == thread_poll_fn_addr() {
                    #[cfg(not(target_arch = "wasm32"))]
                    thread_task_drop(py, ptr);
                } else if poll_fn == process_poll_fn_addr() {
                    #[cfg(not(target_arch = "wasm32"))]
                    process_task_drop(py, ptr);
                } else if poll_fn == io_wait_poll_fn_addr() {
                    io_wait_release_socket(py, ptr);
                }
                if poll_fn != 0 {
                    task_cancel_message_clear(py, ptr);
                }
            }
            TYPE_ID_BIGINT => {
                std::ptr::drop_in_place(ptr as *mut BigInt);
            }
            _ => {}
        }
        release_ptr(ptr);
        let total_size = header.size;
        let should_pool = header.type_id == TYPE_ID_OBJECT
            && object_pool_put(py, total_size, header_ptr as *mut u8);
        if should_pool {
            return;
        }
        if total_size == 0 {
            return;
        }
        let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
        std::alloc::dealloc(header_ptr as *mut u8, layout);
    }
}
