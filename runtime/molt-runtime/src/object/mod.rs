use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use molt_obj_model::MoltObject;
use num_bigint::BigInt;

pub(crate) mod ops;
pub(crate) mod builders;

use crate::provenance::{register_ptr, release_ptr, resolve_ptr};
use crate::{
    asyncgen_gen_bits, asyncgen_pending_bits, asyncgen_registry_remove, asyncgen_running_bits,
    bytearray_data, bytearray_len, bytearray_vec_ptr,
    bound_method_func_bits, bound_method_self_bits, call_iter_callable_bits, call_iter_sentinel_bits,
    callargs_ptr, class_annotate_bits, class_annotations_bits, class_bases_bits, class_dict_bits,
    class_mro_bits, class_name_bits, classmethod_func_bits, code_filename_bits, code_linetable_bits,
    code_name_bits, context_payload_bits, dict_order_ptr, dict_table_ptr, dict_view_dict_bits,
    enumerate_index_bits, enumerate_target_bits, exception_args_bits, exception_cause_bits,
    exception_class_bits, exception_context_bits, exception_dict_bits, exception_kind_bits,
    exception_msg_bits, exception_suppress_bits, exception_trace_bits, exception_value_bits,
    filter_func_bits, filter_iter_bits, function_annotate_bits, function_annotations_bits,
    function_closure_bits, function_code_bits, function_dict_bits, generator_exception_stack_drop,
    generic_alias_args_bits, generic_alias_origin_bits, io_wait_poll_fn_addr, io_wait_release_socket,
    iter_target_bits, map_func_bits, map_iters_ptr, module_dict_bits, module_name_bits,
    process_poll_fn_addr, process_task_drop, profile_hit, property_del_bits, property_get_bits,
    property_set_bits, reversed_target_bits, runtime_state, seq_vec_ptr, set_order_ptr,
    set_table_ptr, slice_start_bits, slice_step_bits, slice_stop_bits, staticmethod_func_bits,
    super_obj_bits, super_type_bits, task_cancel_message_clear, thread_poll_fn_addr,
    thread_task_drop, utf8_cache_remove, zip_iters_ptr, ALLOC_COUNT,
    TYPE_ID_ASYNC_GENERATOR, TYPE_ID_BIGINT, TYPE_ID_BOUND_METHOD, TYPE_ID_BUFFER2D,
    TYPE_ID_CALLARGS, TYPE_ID_CALL_ITER, TYPE_ID_CLASSMETHOD, TYPE_ID_CODE,
    TYPE_ID_CONTEXT_MANAGER, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_DICT_BUILDER,
    TYPE_ID_DICT_ITEMS_VIEW, TYPE_ID_DICT_KEYS_VIEW, TYPE_ID_DICT_VALUES_VIEW, TYPE_ID_ENUMERATE,
    TYPE_ID_EXCEPTION, TYPE_ID_FILE_HANDLE, TYPE_ID_FILTER, TYPE_ID_FROZENSET, TYPE_ID_FUNCTION,
    TYPE_ID_GENERATOR, TYPE_ID_GENERIC_ALIAS, TYPE_ID_ITER, TYPE_ID_LIST, TYPE_ID_LIST_BUILDER,
    TYPE_ID_MAP, TYPE_ID_MEMORYVIEW, TYPE_ID_MODULE, TYPE_ID_NOT_IMPLEMENTED, TYPE_ID_OBJECT,
    TYPE_ID_PROPERTY, TYPE_ID_REVERSED, TYPE_ID_SET, TYPE_ID_SET_BUILDER, TYPE_ID_SLICE,
    TYPE_ID_STATICMETHOD, TYPE_ID_STRING, TYPE_ID_SUPER, TYPE_ID_BYTEARRAY, TYPE_ID_TUPLE,
    TYPE_ID_TYPE, TYPE_ID_ZIP,
};

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
    // TODO(stdlib-compat, owner:runtime, milestone:SL1): expose closefd/buffer
    // metadata as file attributes for full parity.
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

thread_local! {
    pub(crate) static OBJECT_POOL_TLS: RefCell<Vec<Vec<PtrSlot>>> =
        RefCell::new(vec![Vec::new(); OBJECT_POOL_BUCKETS]);
}

pub(crate) fn obj_from_bits(bits: u64) -> MoltObject {
    MoltObject::from_bits(bits)
}

pub(crate) fn inc_ref_bits(bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe { molt_inc_ref(ptr) };
    }
}

pub(crate) fn dec_ref_bits(bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe { molt_dec_ref(ptr) };
    }
}

pub(crate) fn init_atomic_bits(slot: &AtomicU64, init: impl FnOnce() -> u64) -> u64 {
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
            dec_ref_bits(new_bits);
            prev
        }
    }
}

pub(crate) fn pending_bits_i64() -> i64 {
    MoltObject::pending().bits() as i64
}

fn object_pool() -> &'static Mutex<Vec<Vec<PtrSlot>>> {
    &runtime_state().object_pool
}

fn object_pool_index(total_size: usize) -> Option<usize> {
    if total_size == 0 || total_size > OBJECT_POOL_MAX_BYTES || !total_size.is_multiple_of(8) {
        return None;
    }
    Some(total_size / 8)
}

fn object_pool_take(total_size: usize) -> Option<*mut u8> {
    let idx = object_pool_index(total_size)?;
    let from_tls = OBJECT_POOL_TLS.with(|pool| {
        let mut pool = pool.borrow_mut();
        pool.get_mut(idx).and_then(|bucket| bucket.pop())
    });
    if let Some(slot) = from_tls {
        return Some(slot.0);
    }
    let mut guard = object_pool().lock().unwrap();
    guard
        .get_mut(idx)
        .and_then(|bucket| bucket.pop())
        .map(|slot| slot.0)
}

fn object_pool_put(total_size: usize, header_ptr: *mut u8) -> bool {
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
    let mut guard = object_pool().lock().unwrap();
    let bucket = &mut guard[idx];
    if bucket.len() >= OBJECT_POOL_BUCKET_LIMIT {
        return false;
    }
    bucket.push(PtrSlot(header_ptr));
    true
}

pub(crate) fn alloc_object_zeroed_with_pool(total_size: usize, type_id: u32) -> *mut u8 {
    let header_ptr = if type_id == TYPE_ID_OBJECT {
        object_pool_take(total_size)
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
    profile_hit(&ALLOC_COUNT);
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

pub(crate) fn alloc_object(total_size: usize, type_id: u32) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
    unsafe {
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        profile_hit(&ALLOC_COUNT);
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

pub(crate) unsafe fn instance_set_dict_bits(ptr: *mut u8, bits: u64) {
    *instance_dict_bits_ptr(ptr) = bits;
}

pub(crate) unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    (*header_from_obj_ptr(ptr)).state as u64
}

pub(crate) unsafe fn object_set_class_bits(ptr: *mut u8, bits: u64) {
    (*header_from_obj_ptr(ptr)).state = bits as i64;
}

pub(crate) unsafe fn object_mark_has_ptrs(ptr: *mut u8) {
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

pub(crate) unsafe fn dataclass_set_dict_bits(ptr: *mut u8, bits: u64) {
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
#[no_mangle]
pub unsafe extern "C" fn molt_dec_ref(ptr: *mut u8) {
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
                utf8_cache_remove(ptr as usize);
            }
            TYPE_ID_LIST => {
                let vec_ptr = seq_vec_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(*bits);
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
                        dec_ref_bits(*bits);
                    }
                }
            }
            TYPE_ID_DICT => {
                let order_ptr = dict_order_ptr(ptr);
                let table_ptr = dict_table_ptr(ptr);
                if !order_ptr.is_null() {
                    let order = Box::from_raw(order_ptr);
                    for bits in order.iter() {
                        dec_ref_bits(*bits);
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
                        dec_ref_bits(*bits);
                    }
                }
                if !table_ptr.is_null() {
                    drop(Box::from_raw(table_ptr));
                }
            }
            TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_VALUES_VIEW | TYPE_ID_DICT_ITEMS_VIEW => {
                let dict_bits = dict_view_dict_bits(ptr);
                dec_ref_bits(dict_bits);
            }
            TYPE_ID_ITER => {
                let target_bits = iter_target_bits(ptr);
                dec_ref_bits(target_bits);
            }
            TYPE_ID_ENUMERATE => {
                let target_bits = enumerate_target_bits(ptr);
                let index_bits = enumerate_index_bits(ptr);
                dec_ref_bits(target_bits);
                dec_ref_bits(index_bits);
            }
            TYPE_ID_CALL_ITER => {
                let call_bits = call_iter_callable_bits(ptr);
                let sentinel_bits = call_iter_sentinel_bits(ptr);
                dec_ref_bits(call_bits);
                dec_ref_bits(sentinel_bits);
            }
            TYPE_ID_REVERSED => {
                let target_bits = reversed_target_bits(ptr);
                dec_ref_bits(target_bits);
            }
            TYPE_ID_ZIP => {
                let vec_ptr = zip_iters_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(*bits);
                    }
                }
            }
            TYPE_ID_MAP => {
                let func_bits = map_func_bits(ptr);
                dec_ref_bits(func_bits);
                let vec_ptr = map_iters_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(*bits);
                    }
                }
            }
            TYPE_ID_FILTER => {
                let func_bits = filter_func_bits(ptr);
                let iter_bits = filter_iter_bits(ptr);
                dec_ref_bits(func_bits);
                dec_ref_bits(iter_bits);
            }
            TYPE_ID_LIST_BUILDER => {
                let vec_ptr = *(ptr as *mut *mut Vec<u64>);
                if !vec_ptr.is_null() {
                    drop(Box::from_raw(vec_ptr));
                }
            }
            TYPE_ID_DICT_BUILDER => {
                let vec_ptr = *(ptr as *mut *mut Vec<u64>);
                if !vec_ptr.is_null() {
                    drop(Box::from_raw(vec_ptr));
                }
            }
            TYPE_ID_SET_BUILDER => {
                let vec_ptr = *(ptr as *mut *mut Vec<u64>);
                if !vec_ptr.is_null() {
                    drop(Box::from_raw(vec_ptr));
                }
            }
            TYPE_ID_CALLARGS => {
                let args_ptr = callargs_ptr(ptr);
                if !args_ptr.is_null() {
                    drop(Box::from_raw(args_ptr));
                }
            }
            TYPE_ID_SLICE => {
                let start_bits = slice_start_bits(ptr);
                let stop_bits = slice_stop_bits(ptr);
                let step_bits = slice_step_bits(ptr);
                dec_ref_bits(start_bits);
                dec_ref_bits(stop_bits);
                dec_ref_bits(step_bits);
            }
            TYPE_ID_GENERIC_ALIAS => {
                let origin_bits = generic_alias_origin_bits(ptr);
                let args_bits = generic_alias_args_bits(ptr);
                dec_ref_bits(origin_bits);
                dec_ref_bits(args_bits);
            }
            TYPE_ID_EXCEPTION => {
                let kind_bits = exception_kind_bits(ptr);
                let msg_bits = exception_msg_bits(ptr);
                let cause_bits = exception_cause_bits(ptr);
                let context_bits = exception_context_bits(ptr);
                let suppress_bits = exception_suppress_bits(ptr);
                let trace_bits = exception_trace_bits(ptr);
                let value_bits = exception_value_bits(ptr);
                let class_bits = exception_class_bits(ptr);
                let args_bits = exception_args_bits(ptr);
                let dict_bits = exception_dict_bits(ptr);
                dec_ref_bits(kind_bits);
                dec_ref_bits(msg_bits);
                dec_ref_bits(cause_bits);
                dec_ref_bits(context_bits);
                dec_ref_bits(suppress_bits);
                dec_ref_bits(trace_bits);
                dec_ref_bits(value_bits);
                dec_ref_bits(class_bits);
                dec_ref_bits(args_bits);
                dec_ref_bits(dict_bits);
            }
            TYPE_ID_GENERATOR => {
                generator_exception_stack_drop(ptr);
                let payload = header
                    .size
                    .saturating_sub(std::mem::size_of::<MoltHeader>());
                let slots = payload / std::mem::size_of::<u64>();
                let payload_ptr = ptr as *mut u64;
                for idx in 0..slots {
                    dec_ref_bits(*payload_ptr.add(idx));
                }
            }
            TYPE_ID_ASYNC_GENERATOR => {
                asyncgen_registry_remove(ptr);
                let gen_bits = asyncgen_gen_bits(ptr);
                let running_bits = asyncgen_running_bits(ptr);
                let pending_bits = asyncgen_pending_bits(ptr);
                dec_ref_bits(gen_bits);
                dec_ref_bits(running_bits);
                dec_ref_bits(pending_bits);
            }
            TYPE_ID_CONTEXT_MANAGER => {
                let payload_bits = context_payload_bits(ptr);
                dec_ref_bits(payload_bits);
            }
            TYPE_ID_FILE_HANDLE => {
                let handle_ptr = file_handle_ptr(ptr);
                if !handle_ptr.is_null() {
                    let handle = Box::from_raw(handle_ptr);
                    if handle.owns_fd {
                        if let Ok(mut guard) = handle.state.file.lock() {
                            guard.take();
                        }
                    }
                    if handle.name_bits != 0 {
                        dec_ref_bits(handle.name_bits);
                    }
                    if handle.buffer_bits != 0 {
                        dec_ref_bits(handle.buffer_bits);
                    }
                }
            }
            TYPE_ID_DATACLASS => {
                let vec_ptr = dataclass_fields_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(*bits);
                    }
                }
                let dict_bits = dataclass_dict_bits(ptr);
                dec_ref_bits(dict_bits);
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() {
                    let class_bits = (*desc_ptr).class_bits;
                    if class_bits != 0 {
                        dec_ref_bits(class_bits);
                    }
                    drop(Box::from_raw(desc_ptr));
                }
            }
            TYPE_ID_OBJECT => {
                if header.poll_fn == 0 {
                    let has_ptrs = (header.flags & HEADER_FLAG_HAS_PTRS) != 0;
                    let payload = header
                        .size
                        .saturating_sub(std::mem::size_of::<MoltHeader>());
                    if has_ptrs {
                        let slots = payload / std::mem::size_of::<u64>();
                        let payload_ptr = ptr as *mut u64;
                        for idx in 0..slots {
                            dec_ref_bits(*payload_ptr.add(idx));
                        }
                    }
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 && (header.flags & HEADER_FLAG_SKIP_CLASS_DECREF) == 0 {
                        dec_ref_bits(class_bits);
                    }
                } else {
                    task_cancel_message_clear(ptr);
                    if header.poll_fn == io_wait_poll_fn_addr() {
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            runtime_state().io_poller().cancel_waiter(ptr);
                            io_wait_release_socket(ptr);
                        }
                    }
                    if header.poll_fn == thread_poll_fn_addr() {
                        #[cfg(not(target_arch = "wasm32"))]
                        thread_task_drop(ptr);
                    }
                    if header.poll_fn == process_poll_fn_addr() {
                        #[cfg(not(target_arch = "wasm32"))]
                        process_task_drop(ptr);
                    }
                    let payload = header
                        .size
                        .saturating_sub(std::mem::size_of::<MoltHeader>());
                    let slots = payload / std::mem::size_of::<u64>();
                    let payload_ptr = ptr as *mut u64;
                    for idx in 0..slots {
                        dec_ref_bits(*payload_ptr.add(idx));
                    }
                }
            }
            TYPE_ID_BUFFER2D => {
                let buf_ptr = buffer2d_ptr(ptr);
                if !buf_ptr.is_null() {
                    drop(Box::from_raw(buf_ptr));
                }
            }
            TYPE_ID_MEMORYVIEW => {
                let owner_bits = memoryview_owner_bits(ptr);
                let format_bits = memoryview_format_bits(ptr);
                let shape_ptr = memoryview_shape_ptr(ptr);
                let strides_ptr = memoryview_strides_ptr(ptr);
                dec_ref_bits(owner_bits);
                dec_ref_bits(format_bits);
                if !shape_ptr.is_null() {
                    drop(Box::from_raw(shape_ptr));
                }
                if !strides_ptr.is_null() {
                    drop(Box::from_raw(strides_ptr));
                }
            }
            TYPE_ID_BIGINT => {
                std::ptr::drop_in_place(ptr as *mut BigInt);
            }
            TYPE_ID_BOUND_METHOD => {
                let func_bits = bound_method_func_bits(ptr);
                let self_bits = bound_method_self_bits(ptr);
                dec_ref_bits(func_bits);
                dec_ref_bits(self_bits);
            }
            TYPE_ID_MODULE => {
                let name_bits = module_name_bits(ptr);
                let dict_bits = module_dict_bits(ptr);
                dec_ref_bits(name_bits);
                dec_ref_bits(dict_bits);
            }
            TYPE_ID_TYPE => {
                let name_bits = class_name_bits(ptr);
                let dict_bits = class_dict_bits(ptr);
                let bases_bits = class_bases_bits(ptr);
                let mro_bits = class_mro_bits(ptr);
                let annotations_bits = class_annotations_bits(ptr);
                let annotate_bits = class_annotate_bits(ptr);
                dec_ref_bits(name_bits);
                dec_ref_bits(dict_bits);
                dec_ref_bits(bases_bits);
                dec_ref_bits(mro_bits);
                if annotations_bits != 0 {
                    dec_ref_bits(annotations_bits);
                }
                if annotate_bits != 0 {
                    dec_ref_bits(annotate_bits);
                }
            }
            TYPE_ID_FUNCTION => {
                let dict_bits = function_dict_bits(ptr);
                let closure_bits = function_closure_bits(ptr);
                let code_bits = function_code_bits(ptr);
                let annotations_bits = function_annotations_bits(ptr);
                let annotate_bits = function_annotate_bits(ptr);
                dec_ref_bits(dict_bits);
                if closure_bits != 0 {
                    dec_ref_bits(closure_bits);
                }
                if code_bits != 0 {
                    dec_ref_bits(code_bits);
                }
                if annotations_bits != 0 {
                    dec_ref_bits(annotations_bits);
                }
                if annotate_bits != 0 {
                    dec_ref_bits(annotate_bits);
                }
            }
            TYPE_ID_CODE => {
                let filename_bits = code_filename_bits(ptr);
                let name_bits = code_name_bits(ptr);
                let linetable_bits = code_linetable_bits(ptr);
                if filename_bits != 0 {
                    dec_ref_bits(filename_bits);
                }
                if name_bits != 0 {
                    dec_ref_bits(name_bits);
                }
                if linetable_bits != 0 {
                    dec_ref_bits(linetable_bits);
                }
            }
            TYPE_ID_CLASSMETHOD => {
                let func_bits = classmethod_func_bits(ptr);
                dec_ref_bits(func_bits);
            }
            TYPE_ID_STATICMETHOD => {
                let func_bits = staticmethod_func_bits(ptr);
                dec_ref_bits(func_bits);
            }
            TYPE_ID_PROPERTY => {
                let get_bits = property_get_bits(ptr);
                let set_bits = property_set_bits(ptr);
                let del_bits = property_del_bits(ptr);
                dec_ref_bits(get_bits);
                dec_ref_bits(set_bits);
                dec_ref_bits(del_bits);
            }
            TYPE_ID_SUPER => {
                let type_bits = super_type_bits(ptr);
                let obj_bits = super_obj_bits(ptr);
                dec_ref_bits(type_bits);
                dec_ref_bits(obj_bits);
            }
            _ => {}
        }
        release_ptr(ptr);
        let size = header.size;
        if header.type_id == TYPE_ID_OBJECT
            && header.poll_fn == 0
            && object_pool_put(size, header_ptr as *mut u8)
        {
            return;
        }
        let layout = std::alloc::Layout::from_size_align(size, 8).unwrap();
        std::alloc::dealloc(header_ptr as *mut u8, layout);
    }
}
