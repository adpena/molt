use crate::builtins::functions::native_callable::NativeCallableKind;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use molt_obj_model::MoltObject;

use crate::builtins::methods::not_implemented_bits;
use crate::builtins::numbers::index_i64_from_obj;
use crate::builtins::types::{
    ClassSemanticPolicy, RuntimeClassMethodSpec, RuntimeMethodSignature,
    SELF_RUNTIME_ARGUMENT_NAMES, init_cached_runtime_class,
};
use crate::{
    PyToken, TYPE_ID_DICT, TYPE_ID_TUPLE, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    builtin_classes, call_callable2, class_dict_bits, dec_ref_bits, dict_find_entry_kv_in_place,
    dict_get_in_place, dict_set_in_place, dict_update_apply, dict_update_set_in_place,
    exception_pending, inc_ref_bits, init_atomic_bits, intern_static_name, is_truthy,
    issubclass_runtime, molt_is_callable, molt_repr_from_obj, molt_set_attr_name, obj_from_bits,
    object_class_bits, object_type_id, raise_exception, string_obj_to_owned, to_i64, type_of_bits,
};

/// The runtime-owned functools objects: the keyword marker and the five
/// native classes. Their methods are published by the class specs, so no
/// per-method slot exists here.
const FUNCTOOLS_OBJECT_SLOT_COUNT: usize = 6;

pub(crate) struct FunctoolsRuntimeState {
    kwd_mark_bits: AtomicU64,
    partial_class: AtomicU64,
    cmpkey_class: AtomicU64,
    lru_wrapper_class: AtomicU64,
    lru_factory_class: AtomicU64,
    cacheinfo_class: AtomicU64,
    next_singledispatch_handle: AtomicI64,
    singledispatch_registry: Mutex<HashMap<i64, SingleDispatchState>>,
}

impl FunctoolsRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            kwd_mark_bits: AtomicU64::new(0),
            partial_class: AtomicU64::new(0),
            cmpkey_class: AtomicU64::new(0),
            lru_wrapper_class: AtomicU64::new(0),
            lru_factory_class: AtomicU64::new(0),
            cacheinfo_class: AtomicU64::new(0),
            next_singledispatch_handle: AtomicI64::new(1),
            singledispatch_registry: Mutex::new(HashMap::new()),
        }
    }

    fn object_slots(&self) -> [&AtomicU64; FUNCTOOLS_OBJECT_SLOT_COUNT] {
        [
            &self.kwd_mark_bits,
            &self.partial_class,
            &self.cmpkey_class,
            &self.lru_wrapper_class,
            &self.lru_factory_class,
            &self.cacheinfo_class,
        ]
    }
}

pub(crate) fn functools_runtime_class_roots(
    py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> Vec<u64> {
    crate::state::cache::cached_runtime_class_roots(py, &state.functools.object_slots())
}

pub(crate) fn functools_clear_runtime_state(
    py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    let changed = functools_clear_runtime_callbacks(py, state);
    changed | crate::state::cache::clear_atomic_slots(py, &state.functools.object_slots())
}

pub(crate) fn functools_clear_runtime_callbacks(
    _py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    crate::gil_assert();
    let dispatches = {
        let mut registry = state.functools.singledispatch_registry.lock().unwrap();
        let detached = std::mem::take(&mut *registry);
        // Reset before any displaced owner can publish a new dispatch state.
        state
            .functools
            .next_singledispatch_handle
            .store(1, Ordering::Release);
        detached
    };
    let changed = !dispatches.is_empty();
    let slots = state.functools.object_slots();
    let slots_changed = crate::state::cache::clear_cached_runtime_callbacks(_py, &slots);
    for dispatch in dispatches.into_values() {
        dispatch.release(_py);
    }
    changed | slots_changed
}

#[derive(Default)]
struct LruOrderState {
    tick: u64,
    latest: HashMap<u64, u64>,
    heap: BinaryHeap<Reverse<(u64, u64)>>,
}

impl LruOrderState {
    fn touch(&mut self, key_bits: u64) {
        self.tick = self.tick.wrapping_add(1);
        let stamp = self.tick;
        self.latest.insert(key_bits, stamp);
        self.heap.push(Reverse((stamp, key_bits)));
    }

    fn clear(&mut self) {
        self.tick = 0;
        self.latest.clear();
        self.heap.clear();
    }

    fn evict_over_limit(&mut self, _py: &PyToken<'_>, cache_ptr: *mut u8, maxsize: usize) {
        while self.latest.len() > maxsize {
            let Some(Reverse((stamp, key_bits))) = self.heap.pop() else {
                break;
            };
            if self.latest.get(&key_bits).copied() != Some(stamp) {
                continue;
            }
            self.latest.remove(&key_bits);
            unsafe {
                let _ = crate::dict_del_in_place(_py, cache_ptr, key_bits);
            }
        }
        // Compact stale heap entries after sufficient churn.
        if self.heap.len() > self.latest.len().saturating_mul(8).saturating_add(32) {
            self.compact_heap();
        }
    }

    fn compact_heap(&mut self) {
        let mut compacted = BinaryHeap::with_capacity(self.latest.len());
        for (&key_bits, &stamp) in &self.latest {
            compacted.push(Reverse((stamp, key_bits)));
        }
        self.heap = compacted;
    }
}

fn kwd_mark_bits(_py: &PyToken<'_>) -> u64 {
    if exception_pending(_py) {
        return 0;
    }
    let functools = &crate::runtime_state(_py).functools;
    init_atomic_bits(_py, &functools.kwd_mark_bits, || {
        crate::state::cache::alloc_kwd_mark(_py)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_kwd_mark() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        crate::state::cache::retain_cached_result(_py, kwd_mark_bits(_py))
    })
}

fn partial_class(_py: &PyToken<'_>) -> u64 {
    let functools = &crate::runtime_state(_py).functools;
    let methods = [
        RuntimeClassMethodSpec::with_signature(
            "__call__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_partial_call as *const () as usize as u64,
            3,
            RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, true),
        ),
        RuntimeClassMethodSpec::fixed(
            "__repr__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_partial_repr as *const () as usize as u64,
            1,
        ),
    ];
    init_cached_runtime_class(
        _py,
        &functools.partial_class,
        "partial",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(true, true),
            layout_size: 32,
            instance_shape: Some(crate::object::ObjectShapeId::FunctoolsPartial),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy {
                allows_dict: true,
                allows_weakref: true,
                variable_sized: false,
            }),
        },
        &methods,
    )
}

fn cmpkey_class(_py: &PyToken<'_>) -> u64 {
    let functools = &crate::runtime_state(_py).functools;
    let methods = [
        RuntimeClassMethodSpec::fixed(
            "__lt__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_cmpkey_lt as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            "__le__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_cmpkey_le as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            "__gt__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_cmpkey_gt as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            "__ge__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_cmpkey_ge as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            "__eq__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_cmpkey_eq as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::fixed(
            "__ne__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_cmpkey_ne as *const () as usize as u64,
            2,
        ),
    ];
    init_cached_runtime_class(
        _py,
        &functools.cmpkey_class,
        "_CmpKey",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(true, false),
            layout_size: 24,
            instance_shape: Some(crate::object::ObjectShapeId::FunctoolsCmpKey),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::default()),
        },
        &methods,
    )
}

fn lru_wrapper_class(_py: &PyToken<'_>) -> u64 {
    let functools = &crate::runtime_state(_py).functools;
    let methods = [
        RuntimeClassMethodSpec::with_signature(
            "__call__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_lru_call as *const () as usize as u64,
            3,
            RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, true),
        ),
        RuntimeClassMethodSpec::fixed(
            "__get__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_lru_descriptor_get as *const () as usize as u64,
            3,
        ),
        RuntimeClassMethodSpec::fixed(
            "cache_info",
            NativeCallableKind::MethodDescriptor,
            crate::molt_functools_lru_cache_info as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            "cache_clear",
            NativeCallableKind::MethodDescriptor,
            crate::molt_functools_lru_cache_clear as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            "cache_parameters",
            NativeCallableKind::MethodDescriptor,
            crate::molt_functools_lru_cache_params as *const () as usize as u64,
            1,
        ),
    ];
    init_cached_runtime_class(
        _py,
        &functools.lru_wrapper_class,
        "_lru_cache_wrapper",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(true, false),
            layout_size: 64,
            instance_shape: Some(crate::object::ObjectShapeId::FunctoolsLruWrapper),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy {
                allows_dict: true,
                allows_weakref: true,
                variable_sized: false,
            }),
        },
        &methods,
    )
}

fn lru_factory_class(_py: &PyToken<'_>) -> u64 {
    let functools = &crate::runtime_state(_py).functools;
    let methods = [RuntimeClassMethodSpec::fixed(
        "__call__",
        NativeCallableKind::WrapperDescriptor,
        crate::molt_functools_lru_factory_call as *const () as usize as u64,
        2,
    )];
    init_cached_runtime_class(
        _py,
        &functools.lru_factory_class,
        "_LruCacheFactory",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(false, true),
            layout_size: 24,
            instance_shape: Some(crate::object::ObjectShapeId::FunctoolsLruFactory),
            native_slots: None,
        },
        &methods,
    )
}

fn cacheinfo_class(_py: &PyToken<'_>) -> u64 {
    let functools = &crate::runtime_state(_py).functools;
    let methods = [
        RuntimeClassMethodSpec::fixed(
            "__iter__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_cacheinfo_iter as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            "__repr__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_functools_cacheinfo_repr as *const () as usize as u64,
            1,
        ),
        RuntimeClassMethodSpec::fixed(
            "__getattr__",
            NativeCallableKind::MethodDescriptor,
            crate::molt_functools_cacheinfo_getattr as *const () as usize as u64,
            2,
        ),
    ];
    init_cached_runtime_class(
        _py,
        &functools.cacheinfo_class,
        "CacheInfo",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(false, true),
            layout_size: 40,
            instance_shape: Some(crate::object::ObjectShapeId::FunctoolsCacheInfo),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy {
                allows_dict: false,
                allows_weakref: false,
                variable_sized: true,
            }),
        },
        &methods,
    )
}

unsafe fn partial_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}
unsafe fn partial_args_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn partial_kwargs_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn partial_set_func_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}
unsafe fn partial_set_args_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn partial_set_kwargs_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn cmpkey_obj_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}
unsafe fn cmpkey_cmp_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn cmpkey_set_obj_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}
unsafe fn cmpkey_set_cmp_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn lru_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}
unsafe fn lru_maxsize_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn lru_typed_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn lru_cache_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn lru_order_ptr(ptr: *mut u8) -> *mut LruOrderState {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut *mut LruOrderState) }
}
unsafe fn lru_hits(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(5 * std::mem::size_of::<u64>()) as *const i64) }
}
unsafe fn lru_misses(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(6 * std::mem::size_of::<u64>()) as *const i64) }
}
unsafe fn lru_set_func_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}
unsafe fn lru_set_maxsize_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn lru_set_typed_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn lru_set_cache_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn lru_set_order_ptr(ptr: *mut u8, order: *mut LruOrderState) {
    unsafe {
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut *mut LruOrderState) = order;
    }
}
unsafe fn lru_set_hits(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}
unsafe fn lru_set_misses(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn lru_factory_maxsize_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}
unsafe fn lru_factory_typed_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn lru_factory_set_maxsize_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}
unsafe fn lru_factory_set_typed_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn cacheinfo_hits(ptr: *mut u8) -> i64 {
    unsafe { *(ptr as *const i64) }
}
unsafe fn cacheinfo_misses(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const i64) }
}
unsafe fn cacheinfo_maxsize_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn cacheinfo_currsize(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const i64) }
}
unsafe fn cacheinfo_set_hits(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr as *mut i64) = val;
    }
}
unsafe fn cacheinfo_set_misses(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}
unsafe fn cacheinfo_set_maxsize_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn cacheinfo_set_currsize(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

fn extend_positional_from_call_arg(arg_bits: u64, out: &mut Vec<u64>) {
    let Some(arg_ptr) = obj_from_bits(arg_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(arg_ptr) == TYPE_ID_TUPLE {
            let _ = crate::object::seq_access::with_immutable_tuple_slice(arg_ptr, |items| {
                out.extend_from_slice(items);
            });
            return;
        }
    }
    out.push(arg_bits);
}

fn push_owned_lru_key_part(_py: &PyToken<'_>, out: &mut Vec<u64>, bits: u64) {
    inc_ref_bits(_py, bits);
    out.push(bits);
}

fn extend_owned_lru_key_parts_from_call_arg(_py: &PyToken<'_>, arg_bits: u64, out: &mut Vec<u64>) {
    let Some(arg_ptr) = obj_from_bits(arg_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(arg_ptr) == TYPE_ID_TUPLE {
            let _ = crate::object::seq_access::with_immutable_tuple_slice(arg_ptr, |items| {
                for bits in items.iter().copied() {
                    push_owned_lru_key_part(_py, out, bits);
                }
            });
            return;
        }
    }
    push_owned_lru_key_part(_py, out, arg_bits);
}

fn release_owned_lru_key_parts(_py: &PyToken<'_>, parts: &[u64]) {
    for bits in parts.iter().copied() {
        dec_ref_bits(_py, bits);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_partial(func_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let callable = is_truthy(_py, obj_from_bits(molt_is_callable(func_bits)));
        if !callable {
            return raise_exception::<_>(_py, "TypeError", "partial() requires a callable");
        }
        let class_bits = partial_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            partial_set_func_bits(inst_ptr, func_bits);
            partial_set_args_bits(inst_ptr, args_bits);
            partial_set_kwargs_bits(inst_ptr, kwargs_bits);
        }
        inc_ref_bits(_py, func_bits);
        if args_bits != 0 && !obj_from_bits(args_bits).is_none() {
            inc_ref_bits(_py, args_bits);
        }
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            inc_ref_bits(_py, kwargs_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_partial_call(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let func_bits = unsafe { partial_func_bits(self_ptr) };
        let stored_args_bits = unsafe { partial_args_bits(self_ptr) };
        let stored_kwargs_bits = unsafe { partial_kwargs_bits(self_ptr) };
        let mut pos: Vec<u64> = Vec::new();
        extend_positional_from_call_arg(stored_args_bits, &mut pos);
        extend_positional_from_call_arg(args_bits, &mut pos);
        let merged_kwargs_bits =
            if stored_kwargs_bits != 0 && !obj_from_bits(stored_kwargs_bits).is_none() {
                let copy_bits = crate::dict_copy_method(stored_kwargs_bits) as u64;
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
                    let _ = unsafe {
                        dict_update_apply(_py, copy_bits, dict_update_set_in_place, kwargs_bits)
                    };
                    if exception_pending(_py) {
                        dec_ref_bits(_py, copy_bits);
                        return MoltObject::none().bits();
                    }
                }
                copy_bits
            } else if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
                inc_ref_bits(_py, kwargs_bits);
                kwargs_bits
            } else {
                MoltObject::none().bits()
            };
        let result = unsafe {
            crate::call::bind::call_bind_capi(_py, func_bits, None, &pos, merged_kwargs_bits)
        };
        dec_ref_bits(_py, merged_kwargs_bits);
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_partial_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let func_bits = unsafe { partial_func_bits(self_ptr) };
        let args_bits = unsafe { partial_args_bits(self_ptr) };
        let kwargs_bits = unsafe { partial_kwargs_bits(self_ptr) };
        let func_repr_bits = molt_repr_from_obj(func_bits);
        let func_repr = string_obj_to_owned(obj_from_bits(func_repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, func_repr_bits);
        let args_repr_bits = molt_repr_from_obj(args_bits);
        let args_repr = string_obj_to_owned(obj_from_bits(args_repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, args_repr_bits);
        let mut out = String::new();
        out.push_str("functools.partial(");
        out.push_str(&func_repr);
        out.push_str(", ");
        out.push_str(&args_repr);
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            let kw_repr_bits = molt_repr_from_obj(kwargs_bits);
            let kw_repr = string_obj_to_owned(obj_from_bits(kw_repr_bits)).unwrap_or_default();
            dec_ref_bits(_py, kw_repr_bits);
            out.push_str(", ");
            out.push_str(&kw_repr);
        }
        out.push(')');
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_reduce(
    func_bits: u64,
    iterable_bits: u64,
    initializer_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        use molt_runtime_core::OwnedRuntimeValue;

        let missing = kwd_mark_bits(_py);
        if missing == 0 || exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(mut iterator) = crate::object::iterable::OwnedIterator::new(_py, iterable_bits)
        else {
            return MoltObject::none().bits();
        };
        let core_py = _py.core_token();
        let mut value = if initializer_bits == missing {
            match iterator.next() {
                Ok(Some(bits)) => unsafe { OwnedRuntimeValue::from_owned_bits(core_py, bits) },
                Ok(None) => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "reduce() of empty sequence with no initial value",
                    );
                }
                Err(molt_runtime_core::ErrorIndicatorSet) => return MoltObject::none().bits(),
            }
        } else {
            OwnedRuntimeValue::retain(core_py, initializer_bits)
        };
        loop {
            let item = match iterator.next() {
                Ok(Some(bits)) => unsafe { OwnedRuntimeValue::from_owned_bits(core_py, bits) },
                Ok(None) => return value.into_bits(),
                Err(molt_runtime_core::ErrorIndicatorSet) => return MoltObject::none().bits(),
            };
            let next_bits = unsafe { call_callable2(_py, func_bits, value.bits(), item.bits()) };
            let next = unsafe { OwnedRuntimeValue::from_owned_bits(core_py, next_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let previous = std::mem::replace(&mut value, next);
            drop(previous);
        }
    })
}

fn update_wrapper_members(
    py: &PyToken<'_>,
    wrapper: u64,
    wrapped: u64,
    names: u64,
    update: bool,
) -> Result<(), molt_runtime_core::ErrorIndicatorSet> {
    let mut iterator = crate::object::iterable::OwnedIterator::new(py, names)
        .ok_or(molt_runtime_core::ErrorIndicatorSet)?;
    let outcome = (|| {
        while let Some(name) = iterator.next()? {
            let outcome = (|| {
                if !update {
                    let value = crate::molt_get_attr_name(wrapped, name);
                    if exception_pending(py) {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(py, value)
                        });
                        if crate::builtins::attr::clear_attribute_error_if_pending(py) {
                            return Ok(());
                        }
                        return Err(molt_runtime_core::ErrorIndicatorSet);
                    }
                    let result = molt_set_attr_name(wrapper, name, value);
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(py, result);
                        dec_ref_bits(py, value);
                    });
                } else {
                    let target = crate::molt_get_attr_name(wrapper, name);
                    if exception_pending(py) {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(py, target)
                        });
                        return Err(molt_runtime_core::ErrorIndicatorSet);
                    }
                    let Some(update_name) = attr_name_bits_from_bytes(py, b"update") else {
                        dec_ref_bits(py, target);
                        return Err(molt_runtime_core::ErrorIndicatorSet);
                    };
                    let method = crate::molt_get_attr_name(target, update_name);
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(py, update_name);
                        dec_ref_bits(py, target);
                    });
                    if exception_pending(py) {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(py, method)
                        });
                        return Err(molt_runtime_core::ErrorIndicatorSet);
                    }
                    let missing = crate::missing_bits(py);
                    let mut source = crate::molt_getattr_builtin(wrapped, name, missing);
                    if source == missing && !exception_pending(py) {
                        source = crate::molt_dict_new(0);
                    }
                    if !exception_pending(py) {
                        let result = unsafe { crate::call_callable1(py, method, source) };
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(py, result)
                        });
                    }
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(py, source);
                        dec_ref_bits(py, method);
                    });
                }
                if exception_pending(py) {
                    Err(molt_runtime_core::ErrorIndicatorSet)
                } else {
                    Ok(())
                }
            })();
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, name));
            outcome?;
        }
        Ok(())
    })();
    molt_cpython_abi::api::errors::with_preserved_error(|| drop(iterator));
    outcome
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_update_wrapper(
    wrapper_bits: u64,
    wrapped_bits: u64,
    assigned_bits: u64,
    updated_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if update_wrapper_members(py, wrapper_bits, wrapped_bits, assigned_bits, false).is_err()
            || update_wrapper_members(py, wrapper_bits, wrapped_bits, updated_bits, true).is_err()
        {
            return MoltObject::none().bits();
        }
        let name = intern_static_name(
            py,
            &crate::runtime_state(py).interned.wrapped_name,
            b"__wrapped__",
        );
        let result = molt_set_attr_name(wrapper_bits, name, wrapped_bits);
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, result));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        inc_ref_bits(py, wrapper_bits);
        wrapper_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_wraps(
    wrapped_bits: u64,
    assigned_bits: u64,
    updated_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let tuple_ptr = alloc_tuple(_py, &[wrapped_bits, assigned_bits, updated_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let closure_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::molt_functools_wraps_call as *const () as usize as u64,
            1,
        );
        if func_ptr.is_null() {
            dec_ref_bits(_py, closure_bits);
            return MoltObject::none().bits();
        }
        unsafe {
            crate::function_set_closure_bits(
                _py,
                func_ptr,
                closure_bits,
                crate::FunctionCallAbi::OpaqueContextFirst,
            )
        };
        dec_ref_bits(_py, closure_bits);
        MoltObject::from_ptr(func_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_wraps_call(closure_bits: u64, wrapper_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let closure_ptr = obj_from_bits(closure_bits).as_ptr();
        let Some(closure_ptr) = closure_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(closure_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            if crate::object::seq_access::len(closure_ptr) < 3 {
                return MoltObject::none().bits();
            }
            let mut wrapped_bits = 0;
            let mut assigned_bits = 0;
            let mut updated_bits = 0;
            if crate::object::seq_access::read_item_gil_borrowed(closure_ptr, 0, &mut wrapped_bits)
                == 0
                || crate::object::seq_access::read_item_gil_borrowed(
                    closure_ptr,
                    1,
                    &mut assigned_bits,
                ) == 0
                || crate::object::seq_access::read_item_gil_borrowed(
                    closure_ptr,
                    2,
                    &mut updated_bits,
                ) == 0
            {
                return MoltObject::none().bits();
            }
            molt_functools_update_wrapper(wrapper_bits, wrapped_bits, assigned_bits, updated_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmp_to_key(cmp_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_bits = cmpkey_class(_py);
        if class_bits == 0 {
            return MoltObject::none().bits();
        }
        let tuple_ptr = alloc_tuple(_py, &[cmp_bits, class_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let closure_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::molt_functools_cmp_key_func as *const () as usize as u64,
            1,
        );
        if func_ptr.is_null() {
            dec_ref_bits(_py, closure_bits);
            return MoltObject::none().bits();
        }
        unsafe {
            crate::function_set_closure_bits(
                _py,
                func_ptr,
                closure_bits,
                crate::FunctionCallAbi::OpaqueContextFirst,
            )
        };
        dec_ref_bits(_py, closure_bits);
        MoltObject::from_ptr(func_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmp_key_func(closure_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let closure_ptr = obj_from_bits(closure_bits).as_ptr();
        let Some(closure_ptr) = closure_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(closure_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            if crate::object::seq_access::len(closure_ptr) < 2 {
                return MoltObject::none().bits();
            }
            let mut cmp_bits = 0;
            let mut class_bits = 0;
            if crate::object::seq_access::read_item_gil_borrowed(closure_ptr, 0, &mut cmp_bits) == 0
                || crate::object::seq_access::read_item_gil_borrowed(
                    closure_ptr,
                    1,
                    &mut class_bits,
                ) == 0
            {
                return MoltObject::none().bits();
            }
            let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            let inst_bits = crate::alloc_instance_for_class(_py, class_ptr);
            if obj_from_bits(inst_bits).is_none() {
                return MoltObject::none().bits();
            }
            let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
            cmpkey_set_obj_bits(inst_ptr, obj_bits);
            cmpkey_set_cmp_bits(inst_ptr, cmp_bits);
            inc_ref_bits(_py, obj_bits);
            inc_ref_bits(_py, cmp_bits);
            inst_bits
        }
    })
}

fn cmpkey_compare(_py: &PyToken<'_>, self_bits: u64, other_bits: u64) -> Option<i64> {
    let self_ptr = obj_from_bits(self_bits).as_ptr()?;
    let other_ptr = obj_from_bits(other_bits).as_ptr()?;
    let self_class = unsafe { object_class_bits(self_ptr) };
    let other_class = unsafe { object_class_bits(other_ptr) };
    if self_class == 0 || other_class == 0 || self_class != other_class {
        return None;
    }
    let obj_bits = unsafe { cmpkey_obj_bits(self_ptr) };
    let cmp_bits = unsafe { cmpkey_cmp_bits(self_ptr) };
    let other_obj_bits = unsafe { cmpkey_obj_bits(other_ptr) };
    let res_bits = unsafe { call_callable2(_py, cmp_bits, obj_bits, other_obj_bits) };
    if exception_pending(_py) {
        return Some(0);
    }
    let val = index_i64_from_obj(_py, res_bits, "cmp_to_key comparison must be int");
    if exception_pending(_py) {
        return Some(0);
    }
    Some(val)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_lt(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return not_implemented_bits(_py);
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val < 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_le(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return not_implemented_bits(_py);
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val <= 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_gt(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return not_implemented_bits(_py);
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val > 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_ge(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return not_implemented_bits(_py);
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val >= 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val == 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_ne(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return MoltObject::from_bool(true).bits();
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val != 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_total_ordering(cls_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_ptr = obj_from_bits(cls_bits).as_ptr();
        let Some(cls_ptr) = cls_ptr else {
            return raise_exception::<_>(_py, "TypeError", "total_ordering expects a class");
        };
        unsafe {
            if object_type_id(cls_ptr) != crate::TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "total_ordering expects a class");
            }
        }
        let dict_bits = unsafe { class_dict_bits(cls_ptr) };
        let dict_ptr = obj_from_bits(dict_bits).as_ptr();
        let Some(dict_ptr) = dict_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return MoltObject::none().bits();
            }
        }
        let lt_name =
            intern_static_name(_py, &crate::runtime_state(_py).interned.lt_name, b"__lt__");
        let le_name =
            intern_static_name(_py, &crate::runtime_state(_py).interned.le_name, b"__le__");
        let gt_name =
            intern_static_name(_py, &crate::runtime_state(_py).interned.gt_name, b"__gt__");
        let ge_name =
            intern_static_name(_py, &crate::runtime_state(_py).interned.ge_name, b"__ge__");
        let root = if unsafe { dict_get_in_place(_py, dict_ptr, lt_name).is_some() } {
            "lt"
        } else if unsafe { dict_get_in_place(_py, dict_ptr, le_name).is_some() } {
            "le"
        } else if unsafe { dict_get_in_place(_py, dict_ptr, gt_name).is_some() } {
            "gt"
        } else if unsafe { dict_get_in_place(_py, dict_ptr, ge_name).is_some() } {
            "ge"
        } else {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "total_ordering requires at least one ordering operation: < <= > >=",
            );
        };
        let mut missing: Vec<(&'static str, i64, i64, i64)> = Vec::new();
        // op_code: 0=lt,1=le,2=gt,3=ge; swap, negate
        match root {
            "lt" => {
                if unsafe { dict_get_in_place(_py, dict_ptr, gt_name).is_none() } {
                    missing.push(("__gt__", 0, 1, 0));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, le_name).is_none() } {
                    missing.push(("__le__", 0, 1, 1));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, ge_name).is_none() } {
                    missing.push(("__ge__", 0, 0, 1));
                }
            }
            "le" => {
                if unsafe { dict_get_in_place(_py, dict_ptr, ge_name).is_none() } {
                    missing.push(("__ge__", 1, 1, 0));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, lt_name).is_none() } {
                    missing.push(("__lt__", 1, 1, 1));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, gt_name).is_none() } {
                    missing.push(("__gt__", 1, 0, 1));
                }
            }
            "gt" => {
                if unsafe { dict_get_in_place(_py, dict_ptr, lt_name).is_none() } {
                    missing.push(("__lt__", 2, 1, 0));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, ge_name).is_none() } {
                    missing.push(("__ge__", 2, 1, 1));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, le_name).is_none() } {
                    missing.push(("__le__", 2, 0, 1));
                }
            }
            _ => {
                if unsafe { dict_get_in_place(_py, dict_ptr, le_name).is_none() } {
                    missing.push(("__le__", 3, 1, 0));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, gt_name).is_none() } {
                    missing.push(("__gt__", 3, 1, 1));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, lt_name).is_none() } {
                    missing.push(("__lt__", 3, 0, 1));
                }
            }
        }
        for (name, op_code, swap, negate) in missing {
            let closure_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(op_code).bits(),
                    MoltObject::from_int(swap).bits(),
                    MoltObject::from_int(negate).bits(),
                ],
            );
            if closure_ptr.is_null() {
                continue;
            }
            let closure_bits = MoltObject::from_ptr(closure_ptr).bits();
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::molt_functools_total_ordering_op as *const () as usize as u64,
                2,
            );
            if func_ptr.is_null() {
                dec_ref_bits(_py, closure_bits);
                continue;
            }
            unsafe {
                crate::function_set_closure_bits(
                    _py,
                    func_ptr,
                    closure_bits,
                    crate::FunctionCallAbi::OpaqueContextFirst,
                )
            };
            dec_ref_bits(_py, closure_bits);
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let Some(name_bits) = attr_name_bits_from_bytes(_py, name.as_bytes()) else {
                dec_ref_bits(_py, func_bits);
                continue;
            };
            unsafe { dict_set_in_place(_py, dict_ptr, name_bits, func_bits) };
            dec_ref_bits(_py, name_bits);
        }
        inc_ref_bits(_py, cls_bits);
        cls_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_total_ordering_op(
    closure_bits: u64,
    self_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let closure_ptr = obj_from_bits(closure_bits).as_ptr();
        let Some(closure_ptr) = closure_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(closure_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            if crate::object::seq_access::len(closure_ptr) < 3 {
                return MoltObject::none().bits();
            }
            let mut op_bits = 0;
            let mut swap_bits = 0;
            let mut negate_bits = 0;
            if crate::object::seq_access::read_item_gil_borrowed(closure_ptr, 0, &mut op_bits) == 0
                || crate::object::seq_access::read_item_gil_borrowed(closure_ptr, 1, &mut swap_bits)
                    == 0
                || crate::object::seq_access::read_item_gil_borrowed(
                    closure_ptr,
                    2,
                    &mut negate_bits,
                ) == 0
            {
                return MoltObject::none().bits();
            }
            let op_code = to_i64(obj_from_bits(op_bits)).unwrap_or(0);
            let swap = to_i64(obj_from_bits(swap_bits)).unwrap_or(0) != 0;
            let negate = to_i64(obj_from_bits(negate_bits)).unwrap_or(0) != 0;
            let (lhs, rhs) = if swap {
                (other_bits, self_bits)
            } else {
                (self_bits, other_bits)
            };
            let res_bits = match op_code {
                0 => crate::molt_lt(lhs, rhs),
                1 => crate::molt_le(lhs, rhs),
                2 => crate::molt_gt(lhs, rhs),
                _ => crate::molt_ge(lhs, rhs),
            };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let truth = is_truthy(_py, obj_from_bits(res_bits));
            let out = if negate { !truth } else { truth };
            MoltObject::from_bool(out).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_cache(maxsize_bits: u64, typed_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let typed = is_truthy(_py, obj_from_bits(typed_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let callable = is_truthy(_py, obj_from_bits(molt_is_callable(maxsize_bits)));
        if callable {
            let wrapper_bits = build_lru_wrapper(
                _py,
                maxsize_bits,
                MoltObject::from_int(128).bits(),
                MoltObject::from_bool(typed).bits(),
            );
            if obj_from_bits(wrapper_bits).is_none() {
                return MoltObject::none().bits();
            }
            return molt_functools_update_wrapper(
                wrapper_bits,
                maxsize_bits,
                default_wrapper_assignments(_py),
                default_wrapper_updates(_py),
            );
        }
        let maxsize_bits = if obj_from_bits(maxsize_bits).is_none() {
            maxsize_bits
        } else {
            let mut maxsize = index_i64_from_obj(_py, maxsize_bits, "maxsize must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if maxsize < 0 {
                maxsize = 0;
            }
            MoltObject::from_int(maxsize).bits()
        };
        let class_bits = lru_factory_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            lru_factory_set_maxsize_bits(inst_ptr, maxsize_bits);
            lru_factory_set_typed_bits(inst_ptr, MoltObject::from_bool(typed).bits());
        }
        inc_ref_bits(_py, maxsize_bits);
        inst_bits
    })
}

fn build_lru_wrapper(_py: &PyToken<'_>, func_bits: u64, maxsize_bits: u64, typed_bits: u64) -> u64 {
    let class_bits = lru_wrapper_class(_py);
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
    if obj_from_bits(inst_bits).is_none() {
        return MoltObject::none().bits();
    }
    let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
    let dict_ptr = crate::alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        dec_ref_bits(_py, inst_bits);
        return MoltObject::none().bits();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let order = Box::new(LruOrderState::default());
    let order_ptr = Box::into_raw(order);
    unsafe {
        lru_set_func_bits(inst_ptr, func_bits);
        lru_set_maxsize_bits(inst_ptr, maxsize_bits);
        lru_set_typed_bits(inst_ptr, typed_bits);
        lru_set_cache_bits(inst_ptr, dict_bits);
        lru_set_order_ptr(inst_ptr, order_ptr);
        lru_set_hits(inst_ptr, 0);
        lru_set_misses(inst_ptr, 0);
    }
    inc_ref_bits(_py, func_bits);
    inc_ref_bits(_py, maxsize_bits);
    inc_ref_bits(_py, typed_bits);
    inst_bits
}

fn default_wrapper_assignments(_py: &PyToken<'_>) -> u64 {
    // tuple of __module__, __name__, __qualname__, __doc__, __annotations__
    let names = [
        "__module__",
        "__name__",
        "__qualname__",
        "__doc__",
        "__annotations__",
    ];
    let mut elems: Vec<u64> = Vec::with_capacity(names.len());
    for name in names.iter() {
        let ptr = alloc_string(_py, name.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        elems.push(MoltObject::from_ptr(ptr).bits());
    }
    let tuple_ptr = alloc_tuple(_py, elems.as_slice());
    for bits in elems.iter() {
        dec_ref_bits(_py, *bits);
    }
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn default_wrapper_updates(_py: &PyToken<'_>) -> u64 {
    let ptr = alloc_string(_py, b"__dict__");
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    let tuple_ptr = alloc_tuple(_py, &[bits]);
    dec_ref_bits(_py, bits);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn make_lru_key(_py: &PyToken<'_>, args_bits: u64, kwargs_bits: u64, typed: bool) -> u64 {
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let mut parts: Vec<u64> = Vec::new();
    extend_owned_lru_key_parts_from_call_arg(_py, args_bits, &mut parts);
    if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
        let missing = kwd_mark_bits(_py);
        if missing == 0 || exception_pending(_py) {
            release_owned_lru_key_parts(_py, &parts);
            return MoltObject::none().bits();
        }
        push_owned_lru_key_part(_py, &mut parts, missing);
        if let Some(dict_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
            unsafe {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let Some(order) = (unsafe {
                        crate::object::ops_dict::dict_snapshot(
                            _py,
                            dict_ptr,
                            crate::object::ops_dict::DictSnapshotKind::Entries,
                        )
                    }) else {
                        release_owned_lru_key_parts(_py, &parts);
                        return MoltObject::none().bits();
                    };
                    let mut idx = 0;
                    while idx + 1 < order.len() {
                        let pair_ptr = alloc_tuple(_py, &[order[idx], order[idx + 1]]);
                        if pair_ptr.is_null() {
                            release_owned_lru_key_parts(_py, &parts);
                            return MoltObject::none().bits();
                        }
                        parts.push(MoltObject::from_ptr(pair_ptr).bits());
                        idx += 2;
                    }
                }
            }
        }
    }
    if typed {
        let mut typed_args: Vec<u64> = Vec::new();
        extend_positional_from_call_arg(args_bits, &mut typed_args);
        for val_bits in typed_args {
            let type_bits = crate::type_of_bits(_py, val_bits);
            push_owned_lru_key_part(_py, &mut parts, type_bits);
        }
        if kwargs_bits != 0
            && !obj_from_bits(kwargs_bits).is_none()
            && let Some(dict_ptr) = obj_from_bits(kwargs_bits).as_ptr()
        {
            unsafe {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let Some(order) = (unsafe {
                        crate::object::ops_dict::dict_snapshot(
                            _py,
                            dict_ptr,
                            crate::object::ops_dict::DictSnapshotKind::Entries,
                        )
                    }) else {
                        release_owned_lru_key_parts(_py, &parts);
                        return MoltObject::none().bits();
                    };
                    let mut idx = 0;
                    while idx + 1 < order.len() {
                        let val_bits = order[idx + 1];
                        let type_bits = crate::type_of_bits(_py, val_bits);
                        push_owned_lru_key_part(_py, &mut parts, type_bits);
                        idx += 2;
                    }
                }
            }
        }
    }
    let tuple_ptr = if parts.is_empty() {
        alloc_tuple(_py, &[])
    } else {
        crate::object::builders::alloc_tuple_owned(_py, parts.as_slice())
    };
    if tuple_ptr.is_null() {
        release_owned_lru_key_parts(_py, &parts);
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_call(self_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let func_bits = unsafe { lru_func_bits(self_ptr) };
        let maxsize_bits = unsafe { lru_maxsize_bits(self_ptr) };
        let typed_bits = unsafe { lru_typed_bits(self_ptr) };
        let typed = is_truthy(_py, obj_from_bits(typed_bits));
        let maxsize = if obj_from_bits(maxsize_bits).is_none() {
            None
        } else {
            let mut val = index_i64_from_obj(_py, maxsize_bits, "maxsize must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if val < 0 {
                val = 0;
            }
            Some(val)
        };
        if maxsize == Some(0) {
            let misses = unsafe { lru_misses(self_ptr) } + 1;
            unsafe { lru_set_misses(self_ptr, misses) };
            let mut call_pos = Vec::new();
            extend_positional_from_call_arg(args_bits, &mut call_pos);
            let mapping = if kwargs_bits == 0 {
                MoltObject::none().bits()
            } else {
                kwargs_bits
            };
            return unsafe {
                crate::call::bind::call_bind_capi(_py, func_bits, None, &call_pos, mapping)
            };
        }
        let key_bits = make_lru_key(_py, args_bits, kwargs_bits, typed);
        if obj_from_bits(key_bits).is_none() {
            return MoltObject::none().bits();
        }
        let cache_bits = unsafe { lru_cache_bits(self_ptr) };
        let cache_ptr = obj_from_bits(cache_bits).as_ptr();
        let Some(cache_ptr) = cache_ptr else {
            dec_ref_bits(_py, key_bits);
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(cache_ptr) != TYPE_ID_DICT {
                dec_ref_bits(_py, key_bits);
                return MoltObject::none().bits();
            }
            if let Some((cache_key_bits, val_bits)) =
                dict_find_entry_kv_in_place(_py, cache_ptr, key_bits)
            {
                let hits = lru_hits(self_ptr) + 1;
                lru_set_hits(self_ptr, hits);
                let order_ptr = lru_order_ptr(self_ptr);
                if !order_ptr.is_null() {
                    let order = &mut *order_ptr;
                    order.touch(cache_key_bits);
                }
                dec_ref_bits(_py, key_bits);
                inc_ref_bits(_py, val_bits);
                return val_bits;
            }
        }
        let misses = unsafe { lru_misses(self_ptr) } + 1;
        unsafe { lru_set_misses(self_ptr, misses) };
        let mut call_pos = Vec::new();
        extend_positional_from_call_arg(args_bits, &mut call_pos);
        let mapping = if kwargs_bits == 0 {
            MoltObject::none().bits()
        } else {
            kwargs_bits
        };
        let result_bits =
            unsafe { crate::call::bind::call_bind_capi(_py, func_bits, None, &call_pos, mapping) };

        if exception_pending(_py) {
            dec_ref_bits(_py, key_bits);
            return MoltObject::none().bits();
        }
        unsafe {
            dict_set_in_place(_py, cache_ptr, key_bits, result_bits);
        }
        let order_ptr = unsafe { lru_order_ptr(self_ptr) };
        if !order_ptr.is_null() {
            let order = unsafe { &mut *order_ptr };
            order.touch(key_bits);
            if let Some(maxsize) = maxsize {
                order.evict_over_limit(_py, cache_ptr, maxsize.max(0) as usize);
            }
        }
        dec_ref_bits(_py, key_bits);
        result_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_descriptor_get(
    self_bits: u64,
    instance_bits: u64,
    _owner_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(instance_bits).is_none() {
            inc_ref_bits(_py, self_bits);
            return self_bits;
        }
        crate::molt_bound_method_new(self_bits, instance_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_cache_info(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let hits = unsafe { lru_hits(self_ptr) };
        let misses = unsafe { lru_misses(self_ptr) };
        let maxsize_bits = unsafe { lru_maxsize_bits(self_ptr) };
        let cache_bits = unsafe { lru_cache_bits(self_ptr) };
        let currsize = if let Some(cache_ptr) = obj_from_bits(cache_bits).as_ptr() {
            unsafe {
                if object_type_id(cache_ptr) == TYPE_ID_DICT {
                    crate::dict_len(cache_ptr)
                } else {
                    0
                }
            }
        } else {
            0
        };
        let class_bits = cacheinfo_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            cacheinfo_set_hits(inst_ptr, hits);
            cacheinfo_set_misses(inst_ptr, misses);
            cacheinfo_set_maxsize_bits(inst_ptr, maxsize_bits);
            cacheinfo_set_currsize(inst_ptr, currsize as i64);
        }
        inc_ref_bits(_py, maxsize_bits);
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cacheinfo_iter(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let hits = MoltObject::from_int(unsafe { cacheinfo_hits(self_ptr) }).bits();
        let misses = MoltObject::from_int(unsafe { cacheinfo_misses(self_ptr) }).bits();
        let maxsize_bits = unsafe { cacheinfo_maxsize_bits(self_ptr) };
        let currsize = MoltObject::from_int(unsafe { cacheinfo_currsize(self_ptr) }).bits();
        let tuple_ptr = alloc_tuple(_py, &[hits, misses, maxsize_bits, currsize]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let iter_bits = crate::molt_iter(tuple_bits);
        dec_ref_bits(_py, tuple_bits);
        iter_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cacheinfo_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let hits = unsafe { cacheinfo_hits(self_ptr) };
        let misses = unsafe { cacheinfo_misses(self_ptr) };
        let maxsize_bits = unsafe { cacheinfo_maxsize_bits(self_ptr) };
        let maxsize_repr_bits = molt_repr_from_obj(maxsize_bits);
        let maxsize_repr =
            string_obj_to_owned(obj_from_bits(maxsize_repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, maxsize_repr_bits);
        let currsize = unsafe { cacheinfo_currsize(self_ptr) };
        let out = format!(
            "CacheInfo(hits={hits}, misses={misses}, maxsize={maxsize_repr}, currsize={currsize})"
        );
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cacheinfo_getattr(self_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "attribute name must be string");
        };
        match name.as_str() {
            "hits" => MoltObject::from_int(unsafe { cacheinfo_hits(self_ptr) }).bits(),
            "misses" => MoltObject::from_int(unsafe { cacheinfo_misses(self_ptr) }).bits(),
            "maxsize" => {
                let value_bits = unsafe { cacheinfo_maxsize_bits(self_ptr) };
                inc_ref_bits(_py, value_bits);
                value_bits
            }
            "currsize" => MoltObject::from_int(unsafe { cacheinfo_currsize(self_ptr) }).bits(),
            _ => {
                let msg = format!("'CacheInfo' object has no attribute '{name}'");
                raise_exception::<u64>(_py, "AttributeError", &msg)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_cache_clear(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let cache_bits = unsafe { lru_cache_bits(self_ptr) };
        if let Some(cache_ptr) = obj_from_bits(cache_bits).as_ptr() {
            unsafe {
                if object_type_id(cache_ptr) == TYPE_ID_DICT {
                    crate::dict_clear_in_place(_py, cache_ptr);
                }
            }
        }
        let order_ptr = unsafe { lru_order_ptr(self_ptr) };
        if !order_ptr.is_null() {
            unsafe {
                let order = &mut *order_ptr;
                order.clear();
            }
        }
        unsafe {
            lru_set_hits(self_ptr, 0);
            lru_set_misses(self_ptr, 0);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_cache_params(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let maxsize_bits = unsafe { lru_maxsize_bits(self_ptr) };
        let typed_bits = unsafe { lru_typed_bits(self_ptr) };
        let key1_ptr = alloc_string(_py, b"maxsize");
        let key2_ptr = alloc_string(_py, b"typed");
        if key1_ptr.is_null() || key2_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let key1_bits = MoltObject::from_ptr(key1_ptr).bits();
        let key2_bits = MoltObject::from_ptr(key2_ptr).bits();
        let dict_ptr =
            crate::alloc_dict_with_pairs(_py, &[key1_bits, maxsize_bits, key2_bits, typed_bits]);
        dec_ref_bits(_py, key1_bits);
        dec_ref_bits(_py, key2_bits);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_factory_call(self_bits: u64, func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let maxsize_bits = unsafe { lru_factory_maxsize_bits(self_ptr) };
        let typed_bits = unsafe { lru_factory_typed_bits(self_ptr) };
        let wrapper_bits = build_lru_wrapper(_py, func_bits, maxsize_bits, typed_bits);
        if obj_from_bits(wrapper_bits).is_none() {
            return MoltObject::none().bits();
        }
        molt_functools_update_wrapper(
            wrapper_bits,
            func_bits,
            default_wrapper_assignments(_py),
            default_wrapper_updates(_py),
        )
    })
}

pub(crate) unsafe fn functools_visit_owned_edges(
    shape: crate::object::ObjectShapeId,
    ptr: *mut u8,
    mut visit: impl FnMut(u64),
) {
    use crate::object::ObjectShapeId;
    unsafe {
        match shape {
            ObjectShapeId::FunctoolsPartial => {
                visit(partial_func_bits(ptr));
                visit(partial_args_bits(ptr));
                visit(partial_kwargs_bits(ptr));
            }
            ObjectShapeId::FunctoolsCmpKey => {
                visit(cmpkey_obj_bits(ptr));
                visit(cmpkey_cmp_bits(ptr));
            }
            ObjectShapeId::FunctoolsLruWrapper => {
                visit(lru_func_bits(ptr));
                visit(lru_maxsize_bits(ptr));
                visit(lru_typed_bits(ptr));
                visit(lru_cache_bits(ptr));
            }
            ObjectShapeId::FunctoolsLruFactory => {
                visit(lru_factory_maxsize_bits(ptr));
                visit(lru_factory_typed_bits(ptr));
            }
            ObjectShapeId::FunctoolsCacheInfo => visit(cacheinfo_maxsize_bits(ptr)),
            _ => unreachable!("non-functools object shape"),
        }
    }
}

pub(crate) unsafe fn functools_detach_owned_edges(
    shape: crate::object::ObjectShapeId,
    ptr: *mut u8,
    mut detach: impl FnMut(u64),
) {
    use crate::object::ObjectShapeId;
    let none = MoltObject::none().bits();
    let mut detached = [none; 5];
    let detached_len;
    unsafe {
        match shape {
            ObjectShapeId::FunctoolsPartial => {
                detached[..3].copy_from_slice(&[
                    partial_func_bits(ptr),
                    partial_args_bits(ptr),
                    partial_kwargs_bits(ptr),
                ]);
                detached_len = 3;
                partial_set_func_bits(ptr, none);
                partial_set_args_bits(ptr, none);
                partial_set_kwargs_bits(ptr, none);
            }
            ObjectShapeId::FunctoolsCmpKey => {
                detached[..2].copy_from_slice(&[cmpkey_obj_bits(ptr), cmpkey_cmp_bits(ptr)]);
                detached_len = 2;
                cmpkey_set_obj_bits(ptr, none);
                cmpkey_set_cmp_bits(ptr, none);
            }
            ObjectShapeId::FunctoolsLruWrapper => {
                detached[..4].copy_from_slice(&[
                    lru_func_bits(ptr),
                    lru_maxsize_bits(ptr),
                    lru_typed_bits(ptr),
                    lru_cache_bits(ptr),
                ]);
                detached_len = 4;
                lru_set_func_bits(ptr, none);
                lru_set_maxsize_bits(ptr, none);
                lru_set_typed_bits(ptr, none);
                lru_set_cache_bits(ptr, none);
                let order = lru_order_ptr(ptr);
                if !order.is_null() {
                    (*order).clear();
                }
            }
            ObjectShapeId::FunctoolsLruFactory => {
                detached[..2]
                    .copy_from_slice(&[lru_factory_maxsize_bits(ptr), lru_factory_typed_bits(ptr)]);
                detached_len = 2;
                lru_factory_set_maxsize_bits(ptr, none);
                lru_factory_set_typed_bits(ptr, none);
            }
            ObjectShapeId::FunctoolsCacheInfo => {
                detached[0] = cacheinfo_maxsize_bits(ptr);
                detached_len = 1;
                cacheinfo_set_maxsize_bits(ptr, none);
            }
            _ => unreachable!("non-functools object shape"),
        }
    }
    for bits in detached.into_iter().take(detached_len) {
        detach(bits);
    }
}

pub(crate) struct DetachedFunctoolsResource(*mut LruOrderState);

pub(crate) unsafe fn functools_detach_typed_resources(
    shape: crate::object::ObjectShapeId,
    ptr: *mut u8,
) -> DetachedFunctoolsResource {
    if shape == crate::object::ObjectShapeId::FunctoolsLruWrapper {
        let order = unsafe { lru_order_ptr(ptr) };
        unsafe { lru_set_order_ptr(ptr, std::ptr::null_mut()) };
        DetachedFunctoolsResource(order)
    } else {
        DetachedFunctoolsResource(std::ptr::null_mut())
    }
}

pub(crate) fn functools_release_typed_resources(resource: DetachedFunctoolsResource) {
    if !resource.0.is_null() {
        unsafe { drop(Box::from_raw(resource.0)) };
    }
}

// ─── singledispatch state ──────────────────────────────────────────────────

struct SingleDispatchState {
    default_func: u64,
    /// Registry: (type_bits, func_bits) pairs.
    /// type_bits is the NaN-boxed class/type object.
    registry: Vec<(u64, u64)>,
    /// Exact registrations for fast direct dispatch.
    exact_registry: HashMap<u64, u64>,
    /// Memoized dispatch results by concrete type bits.
    dispatch_cache: HashMap<u64, u64>,
    /// Runtime ABC invalidation token used to keep dispatch cache coherent.
    abc_cache_token: u64,
}

impl SingleDispatchState {
    fn release(self, _py: &PyToken<'_>) {
        dec_ref_bits(_py, self.default_func);
        for (type_bits, func_bits) in self.registry {
            dec_ref_bits(_py, type_bits);
            dec_ref_bits(_py, func_bits);
        }
    }
}

fn next_singledispatch_handle(_py: &PyToken<'_>) -> i64 {
    crate::runtime_state(_py)
        .functools
        .next_singledispatch_handle
        .fetch_add(1, Ordering::Relaxed)
}

fn singledispatch_registry(_py: &PyToken<'_>) -> &'static Mutex<HashMap<i64, SingleDispatchState>> {
    &crate::runtime_state(_py).functools.singledispatch_registry
}

fn sd_handle_from_bits(_py: &PyToken<'_>, handle_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "singledispatch handle must be an int");
        return None;
    };
    Some(id)
}

fn singledispatch_abc_cache_token(_py: &PyToken<'_>) -> u64 {
    crate::runtime_state(_py)
        .abc_invalidation_counter
        .load(Ordering::Acquire)
}

fn singledispatch_resolve_for_type(
    _py: &PyToken<'_>,
    state: &mut SingleDispatchState,
    type_bits: u64,
) -> u64 {
    let token = singledispatch_abc_cache_token(_py);
    if state.abc_cache_token != token {
        state.dispatch_cache.clear();
        state.abc_cache_token = token;
    }
    if let Some(&reg_func) = state.exact_registry.get(&type_bits) {
        return reg_func;
    }
    if let Some(&cached_func) = state.dispatch_cache.get(&type_bits) {
        return cached_func;
    }
    let mut resolved = state.default_func;
    for &(reg_type, reg_func) in &state.registry {
        if issubclass_runtime(_py, type_bits, reg_type) {
            resolved = reg_func;
            break;
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
    }
    state.dispatch_cache.insert(type_bits, resolved);
    resolved
}

// ─── singledispatch intrinsics ─────────────────────────────────────────────

/// Create a new singledispatch handle, storing the default function.
/// Returns an integer handle (NaN-boxed).
#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_singledispatch_new(default_func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = next_singledispatch_handle(_py);
        inc_ref_bits(_py, default_func_bits);
        let state = SingleDispatchState {
            default_func: default_func_bits,
            registry: Vec::new(),
            exact_registry: HashMap::new(),
            dispatch_cache: HashMap::new(),
            abc_cache_token: singledispatch_abc_cache_token(_py),
        };
        singledispatch_registry(_py)
            .lock()
            .unwrap()
            .insert(id, state);
        MoltObject::from_int(id).bits()
    })
}

/// Register a function for a given type. Adds (type_bits, func_bits) to the
/// registry, incrementing refcounts for both.  Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_singledispatch_register(
    handle_bits: u64,
    type_bits: u64,
    func_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = sd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = singledispatch_registry(_py).lock().unwrap();
            if let Some(state) = map.get_mut(&id) {
                inc_ref_bits(_py, type_bits);
                inc_ref_bits(_py, func_bits);
                // If a registration already exists for this type, replace it.
                if let Some(entry) = state.registry.iter_mut().find(|(t, _)| *t == type_bits) {
                    // Dec-ref the old func, dec-ref the extra type inc we did above.
                    dec_ref_bits(_py, entry.1);
                    dec_ref_bits(_py, type_bits);
                    entry.1 = func_bits;
                } else {
                    state.registry.push((type_bits, func_bits));
                }
                state.exact_registry.insert(type_bits, func_bits);
                state.dispatch_cache.clear();
                state.abc_cache_token = singledispatch_abc_cache_token(_py);
            }
        }
        MoltObject::none().bits()
    })
}

/// Dispatch: given an argument's type, look up the registered function.
/// Checks for an exact type match first, then walks registered classes with
/// subclass checks. Returns the function bits
/// (registered function or default).
#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_singledispatch_dispatch(handle_bits: u64, type_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = sd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let mut map = singledispatch_registry(_py).lock().unwrap();
            let Some(state) = map.get_mut(&id) else {
                return MoltObject::none().bits();
            };
            let resolved = singledispatch_resolve_for_type(_py, state, type_bits);
            inc_ref_bits(_py, resolved);
            resolved
        }
    })
}

/// Call the singledispatch wrapper: extract first positional arg, get its
/// type, dispatch, and return the matched function bits.  The Python shim
/// performs the actual function call.
#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_singledispatch_call(
    handle_bits: u64,
    args_bits: u64,
    _kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = sd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        // Extract first positional arg from the args tuple.
        let first_arg_bits = if let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() {
            unsafe {
                if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                    if crate::object::seq_access::len(args_ptr) == 0 {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "singledispatch requires at least one positional argument",
                        );
                    }
                    let mut first_bits = 0;
                    if crate::object::seq_access::read_item_gil_borrowed(
                        args_ptr,
                        0,
                        &mut first_bits,
                    ) == 0
                    {
                        return MoltObject::none().bits();
                    }
                    first_bits
                } else {
                    args_bits
                }
            }
        } else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "singledispatch requires at least one positional argument",
            );
        };
        // Get the type of the first argument.
        let arg_type_bits = type_of_bits(_py, first_arg_bits);
        // Look up the function for this type.
        {
            let mut map = singledispatch_registry(_py).lock().unwrap();
            let Some(state) = map.get_mut(&id) else {
                return MoltObject::none().bits();
            };
            let resolved = singledispatch_resolve_for_type(_py, state, arg_type_bits);
            inc_ref_bits(_py, resolved);
            resolved
        }
    })
}

/// Return a dict mapping {type: func} for all registered implementations,
/// including the `object` -> default mapping.
#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_singledispatch_registry(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = sd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        {
            let map = singledispatch_registry(_py).lock().unwrap();
            let Some(state) = map.get(&id) else {
                return MoltObject::none().bits();
            };
            // Build pairs: [type0, func0, type1, func1, ...]
            // Include the `object` -> default_func mapping first.
            let builtins = builtin_classes(_py);
            let mut pairs: Vec<u64> = Vec::with_capacity(2 + state.registry.len() * 2);
            pairs.push(builtins.object);
            pairs.push(state.default_func);
            for &(type_bits, func_bits) in &state.registry {
                pairs.push(type_bits);
                pairs.push(func_bits);
            }
            let dict_ptr = crate::alloc_dict_with_pairs(_py, &pairs);
            if dict_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

/// Drop a singledispatch handle, dec-refing all stored bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_singledispatch_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = sd_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let state = singledispatch_registry(_py).lock().unwrap().remove(&id);
        if let Some(state) = state {
            state.release(_py);
        }
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        cacheinfo_class, cmpkey_class, functools_clear_runtime_state, kwd_mark_bits,
        lru_factory_class, lru_wrapper_class, molt_functools_kwd_mark,
        molt_functools_singledispatch_drop, molt_functools_singledispatch_new, partial_class,
    };
    use crate::{MoltObject, dec_ref_bits, inc_ref_bits, obj_from_bits, runtime_state, to_i64};
    use std::sync::atomic::Ordering;

    #[test]
    fn cached_native_class_families_survive_callback_drain_and_cold_restart() {
        use crate::builtins::operator::{
            molt_operator_attrgetter_type, molt_operator_itemgetter_type,
            molt_operator_methodcaller_type, operator_clear_runtime_callbacks,
            operator_runtime_class_roots,
        };
        use crate::builtins::types::{
            capsule_class, cell_class, frame_locals_proxy_class, mappingproxy_class, method_class,
            simplenamespace_class, types_clear_runtime_callbacks, types_runtime_class_roots,
        };

        for _ in 0..2 {
            crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
                assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
                crate::with_gil_entry_nopanic!(py, {
                    let state = runtime_state(py);
                    let native_types = [
                        mappingproxy_class(py),
                        frame_locals_proxy_class(py),
                        method_class(py),
                        simplenamespace_class(py),
                        capsule_class(py),
                        cell_class(py),
                    ];
                    let native_functools =
                        [partial_class(py), cmpkey_class(py), lru_wrapper_class(py)];
                    let native_operator = [
                        molt_operator_itemgetter_type(),
                        molt_operator_attrgetter_type(),
                        molt_operator_methodcaller_type(),
                    ];
                    let mutable = [lru_factory_class(py), cacheinfo_class(py)];
                    assert!(!crate::exception_pending(py));
                    assert!(
                        native_types
                            .iter()
                            .chain(native_functools.iter())
                            .chain(native_operator.iter())
                            .chain(mutable.iter())
                            .all(|&class| class != 0 && obj_from_bits(class).as_ptr().is_some())
                    );

                    // Published payloads and direct results each release only
                    // their own edges, leaving the borrowed cache anchors live.
                    let payload = crate::builtins::types::molt_types_bootstrap();
                    assert!(obj_from_bits(payload).as_ptr().is_some());
                    assert!(!crate::exception_pending(py));
                    dec_ref_bits(py, payload);
                    for class in native_operator {
                        dec_ref_bits(py, class);
                    }

                    // Exercise the actual pre-retirement drain and full GC
                    // after every external result/payload owner is released.
                    types_clear_runtime_callbacks(py, state);
                    super::functools_clear_runtime_callbacks(py, state);
                    operator_clear_runtime_callbacks(py, state);
                    assert_eq!(
                        unsafe { crate::object::gc::collect_cycles(py) }.status,
                        crate::object::gc::GcCollectStatus::Completed
                    );
                    let type_roots = types_runtime_class_roots(py, state);
                    let functools_roots = super::functools_runtime_class_roots(py, state);
                    let operator_roots = operator_runtime_class_roots(py, state);
                    for class in native_types {
                        assert!(type_roots.contains(&class));
                    }
                    for class in native_functools {
                        assert!(functools_roots.contains(&class));
                    }
                    for class in native_operator {
                        assert!(operator_roots.contains(&class));
                    }
                    for class in mutable {
                        assert!(!functools_roots.contains(&class));
                    }
                    assert_eq!(state.functools.cacheinfo_class.load(Ordering::Acquire), 0);
                    assert_eq!(state.functools.lru_factory_class.load(Ordering::Acquire), 0);
                    assert_eq!(partial_class(py), native_functools[0]);
                    assert_eq!(cmpkey_class(py), native_functools[1]);
                    assert_eq!(lru_wrapper_class(py), native_functools[2]);
                    for (owned, cached) in [
                        molt_operator_itemgetter_type(),
                        molt_operator_attrgetter_type(),
                        molt_operator_methodcaller_type(),
                    ]
                    .into_iter()
                    .zip(native_operator)
                    {
                        assert_eq!(owned, cached);
                        dec_ref_bits(py, owned);
                    }
                    assert!(!crate::exception_pending(py));
                });
                // The lifecycle owner now runs the production retirement path,
                // including static PyDictProxy_Type, before the next cold boot.
            });
        }
    }

    #[test]
    fn functools_runtime_state_is_owned_and_clearable() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            functools_clear_runtime_state(_py, state);

            let kwd_mark = kwd_mark_bits(_py);
            let partial = partial_class(_py);
            let cmpkey = cmpkey_class(_py);
            let lru_wrapper = lru_wrapper_class(_py);
            let lru_factory = lru_factory_class(_py);
            let cacheinfo = cacheinfo_class(_py);
            for bits in [
                kwd_mark,
                partial,
                cmpkey,
                lru_wrapper,
                lru_factory,
                cacheinfo,
            ] {
                assert!(!obj_from_bits(bits).is_none());
            }
            for slot in state.functools.object_slots() {
                assert_ne!(slot.load(Ordering::Acquire), 0);
            }

            let first = molt_functools_singledispatch_new(MoltObject::none().bits());
            let second = molt_functools_singledispatch_new(MoltObject::none().bits());
            assert_eq!(to_i64(obj_from_bits(first)), Some(1));
            assert_eq!(to_i64(obj_from_bits(second)), Some(2));
            assert_eq!(
                state
                    .functools
                    .singledispatch_registry
                    .lock()
                    .unwrap()
                    .len(),
                2
            );

            functools_clear_runtime_state(_py, state);
            for slot in state.functools.object_slots() {
                assert_eq!(slot.load(Ordering::Acquire), 0);
            }
            assert_eq!(
                state
                    .functools
                    .next_singledispatch_handle
                    .load(Ordering::Acquire),
                1
            );
            assert!(
                state
                    .functools
                    .singledispatch_registry
                    .lock()
                    .unwrap()
                    .is_empty()
            );

            let reset = molt_functools_singledispatch_new(MoltObject::none().bits());
            assert_eq!(to_i64(obj_from_bits(reset)), Some(1));
            let _ = molt_functools_singledispatch_drop(reset);
        });
    }

    #[test]
    fn functools_kwd_mark_public_return_is_owned_without_releasing_runtime_root() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            functools_clear_runtime_state(_py, state);

            let root_bits = kwd_mark_bits(_py);
            let owned_bits = molt_functools_kwd_mark();
            assert_eq!(root_bits, owned_bits);
            dec_ref_bits(_py, owned_bits);

            inc_ref_bits(_py, root_bits);
            dec_ref_bits(_py, root_bits);

            functools_clear_runtime_state(_py, state);
        });
    }

    struct DenyKwdMarkAllocations;

    impl DenyKwdMarkAllocations {
        fn enter() -> Self {
            use crate::resource::{LimitedTracker, ResourceLimits, set_tracker};
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                max_allocations: Some(0),
                ..Default::default()
            })));
            Self
        }
    }

    impl Drop for DenyKwdMarkAllocations {
        fn drop(&mut self) {
            crate::resource::set_tracker(Box::new(crate::resource::UnlimitedTracker));
        }
    }

    fn kwd_mark_error_identity(
        py: &crate::PyToken<'_>,
    ) -> (molt_cpython_abi::hooks::PendingExceptionClass, Option<u64>) {
        (
            crate::builtins::exceptions::pending_exception_class(py),
            crate::exception_last_bits_noinc(py),
        )
    }

    fn assert_kwd_mark_memory_error(py: &crate::PyToken<'_>) {
        use crate::builtins::exceptions::{
            exception_class_is_subtype, exception_type_bits_from_name,
        };
        use molt_cpython_abi::hooks::PendingExceptionClass;

        assert!(crate::exception_pending(py));
        let pending = kwd_mark_error_identity(py);
        // Emergency MemoryError deliberately has no heap exception to fetch.
        // Class metadata borrows the raised state and preserves its owners.
        let is_memory_error = match pending.0 {
            PendingExceptionClass::EmergencyMemoryError => true,
            PendingExceptionClass::Class(class) => {
                let memory_error = exception_type_bits_from_name(py, "MemoryError");
                exception_class_is_subtype(py, class, memory_error)
            }
            PendingExceptionClass::NativeClass(class) => unsafe {
                molt_cpython_abi::api::typeobj::PyType_IsSubtype(
                    class,
                    &raw mut molt_cpython_abi::abi_types::PyExc_MemoryError,
                ) != 0
            },
            PendingExceptionClass::None => false,
        };
        assert!(
            is_memory_error,
            "allocation failure must remain a MemoryError"
        );
        assert!(crate::exception_pending(py));
        assert_eq!(kwd_mark_error_identity(py), pending);
    }

    #[test]
    fn kwd_mark_allocation_failure_is_retryable_across_builtin_profiles() {
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
            crate::with_gil_entry_nopanic!(py, {
                let none = MoltObject::none().bits();
                for get in [
                    molt_functools_kwd_mark as extern "C" fn() -> u64,
                    crate::molt_itertools_kwd_mark,
                ] {
                    let failed = {
                        let _budget = DenyKwdMarkAllocations::enter();
                        get()
                    };
                    assert_eq!(failed, none);
                    assert_kwd_mark_memory_error(py);
                    let pending = kwd_mark_error_identity(py);
                    assert_eq!(get(), none, "pending allocation failure must be preserved");
                    assert!(crate::exception_pending(py));
                    assert_eq!(kwd_mark_error_identity(py), pending);
                    crate::clear_exception(py);

                    // Retry is the public discriminator for an empty slot in
                    // both the in-tree and satellite itertools profiles.
                    let first = get();
                    assert!(!crate::exception_pending(py));
                    let ptr = obj_from_bits(first)
                        .as_ptr()
                        .expect("retried keyword marker");
                    assert_eq!(unsafe { crate::object_type_id(ptr) }, crate::TYPE_ID_OBJECT);
                    let refs =
                        || unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() };
                    assert_eq!(refs(), 2, "cache and caller own separate references");
                    let repeated = get();
                    assert_eq!(repeated, first);
                    assert_eq!(refs(), 3);
                    dec_ref_bits(py, repeated);
                    dec_ref_bits(py, first);
                    assert_eq!(refs(), 1, "only the cache anchor remains");
                }

                let empty = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
                let result =
                    super::molt_functools_reduce(crate::builtin_classes(py).int, empty, none);
                assert!(!crate::exception_pending(py));
                assert_eq!(result, none, "None is a real reduce initializer");
                dec_ref_bits(py, result);
                dec_ref_bits(py, empty);

                let key_name = MoltObject::from_ptr(crate::alloc_string(py, b"k")).bits();
                let one = MoltObject::from_int(1).bits();
                let two = MoltObject::from_int(2).bits();
                let pair = MoltObject::from_ptr(crate::alloc_tuple(py, &[key_name, two])).bits();
                let args = MoltObject::from_ptr(crate::alloc_tuple(py, &[one])).bits();
                let kwargs =
                    MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[key_name, two])).bits();
                let positional =
                    MoltObject::from_ptr(crate::alloc_tuple(py, &[one, none, pair])).bits();
                let keyword_key = super::make_lru_key(py, args, kwargs, false);
                let positional_key = super::make_lru_key(py, positional, none, false);
                let equal = crate::molt_eq(keyword_key, positional_key);
                assert!(!crate::exception_pending(py));
                assert_eq!(obj_from_bits(equal).as_bool(), Some(false));
                for owned in [
                    equal,
                    keyword_key,
                    positional_key,
                    positional,
                    kwargs,
                    args,
                    pair,
                    key_name,
                ] {
                    dec_ref_bits(py, owned);
                }

                let values = [0, 1, 2, 3, 4].map(|value| MoltObject::from_int(value).bits());
                let sequence = MoltObject::from_ptr(crate::alloc_tuple(py, &values)).bits();
                let sliced = crate::molt_itertools_islice(sequence, two, none, none);
                assert!(!crate::exception_pending(py));
                {
                    let mut iterator =
                        crate::object::iterable::OwnedIterator::new(py, sliced).unwrap();
                    for expected in [2, 3, 4] {
                        let value = iterator.next().unwrap().expect("slice item");
                        assert_eq!(to_i64(obj_from_bits(value)), Some(expected));
                        dec_ref_bits(py, value);
                    }
                    assert!(iterator.next().unwrap().is_none());
                }
                dec_ref_bits(py, sliced);
                dec_ref_bits(py, sequence);
            });
        });
    }

    #[test]
    fn kwd_mark_consumers_preserve_failure_and_release_temporary_key_owners() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let value = MoltObject::from_ptr(crate::alloc_string(py, b"owned key argument")).bits();
            let args = MoltObject::from_ptr(crate::alloc_tuple(py, &[value])).bits();
            let kwargs =
                MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[value, none])).bits();
            let value_ptr = obj_from_bits(value).as_ptr().unwrap();
            let refs = || unsafe { (*crate::header_from_obj_ptr(value_ptr)).ref_count_snapshot() };
            let before = refs();
            let slot = &runtime_state(py).functools.kwd_mark_bits;
            crate::state::cache::clear_atomic_bits(py, slot);
            let key = {
                let _budget = DenyKwdMarkAllocations::enter();
                super::make_lru_key(py, args, kwargs, false)
            };
            assert_eq!(key, none);
            assert_eq!(slot.load(Ordering::Acquire), 0);
            assert_eq!(
                refs(),
                before,
                "failed keyword marker releases copied key parts"
            );
            assert_kwd_mark_memory_error(py);
            crate::clear_exception(py);

            let values = [1, 2].map(|value| MoltObject::from_int(value).bits());
            let sequence = MoltObject::from_ptr(crate::alloc_tuple(py, &values)).bits();
            let iterator = crate::molt_iter(sequence);
            let accumulated =
                crate::molt_itertools_accumulate(sequence, none, MoltObject::from_int(10).bits());
            let marker = molt_functools_kwd_mark();
            dec_ref_bits(py, marker);
            assert!(!crate::exception_pending(py));
            {
                let _budget = DenyKwdMarkAllocations::enter();
                assert_eq!(crate::state::cache::alloc_kwd_mark(py), 0);
            }
            assert_kwd_mark_memory_error(py);
            let pending = kwd_mark_error_identity(py);
            // Sentinel consumers and accumulate advancement must preserve the
            // original error, including warm slots and an unstarted object.
            for operation in 0..5 {
                let result = match operation {
                    0 => {
                        super::molt_functools_reduce(crate::builtin_classes(py).int, iterator, none)
                    }
                    1 => super::make_lru_key(py, args, kwargs, false),
                    2 => crate::molt_itertools_islice(iterator, values[0], none, none),
                    3 => crate::molt_itertools_accumulate(iterator, none, none),
                    _ => crate::molt_itertools_accumulate_next(accumulated),
                };
                assert_eq!(result, none);
                assert!(crate::exception_pending(py));
                assert_eq!(kwd_mark_error_identity(py), pending);
                assert_eq!(refs(), before);
            }
            crate::clear_exception(py);
            {
                let mut input = crate::object::iterable::OwnedIterator::new(py, iterator).unwrap();
                let first = input.next().unwrap().expect("unconsumed input");
                assert_eq!(to_i64(obj_from_bits(first)), Some(1));
                dec_ref_bits(py, first);
            }
            for expected in [10, 11, 13] {
                let next = crate::molt_itertools_accumulate_next(accumulated);
                assert!(!crate::exception_pending(py));
                assert_eq!(to_i64(obj_from_bits(next)), Some(expected));
                dec_ref_bits(py, next);
            }
            for owned in [accumulated, iterator, sequence, kwargs, args, value] {
                dec_ref_bits(py, owned);
            }
        });
    }

    fn iterator_owned_refs(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("heap fixture");
        unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    fn iterator_owned_function(py: &crate::PyToken<'_>, target: *const (), arity: u64) -> u64 {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::provenance::abi::expose_function_address(target),
            arity,
        );
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn iterator_owned_drain(py: &crate::PyToken<'_>, bits: u64, count: usize, exhaust: bool) {
        let mut iter = crate::object::iterable::OwnedIterator::new(py, bits).unwrap();
        for _ in 0..count {
            let value = iter.next().unwrap().expect("expected item");
            dec_ref_bits(py, value);
        }
        if exhaust {
            assert!(iter.next().unwrap().is_none());
        }
    }

    fn iterator_owned_list(py: &crate::PyToken<'_>, values: &[i64]) -> u64 {
        let values: Vec<_> = values
            .iter()
            .map(|&value| MoltObject::from_int(value).bits())
            .collect();
        let ptr = crate::alloc_list(py, &values);
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn iterator_owned_assert_list(bits: u64, expected: &[i64]) {
        let ptr = obj_from_bits(bits).as_ptr().unwrap();
        assert_eq!(
            unsafe { crate::object::seq_access::len(ptr) },
            expected.len()
        );
        for (index, &expected) in expected.iter().enumerate() {
            let item = unsafe { crate::object::seq_access::item(ptr, index) }.unwrap();
            assert_eq!(to_i64(obj_from_bits(item)), Some(expected));
        }
    }

    extern "C" fn iterator_owned_right(_left: u64, right: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            inc_ref_bits(py, right);
            right
        })
    }

    extern "C" fn iterator_owned_true(_value: u64) -> u64 {
        MoltObject::from_bool(true).bits()
    }

    extern "C" fn iterator_owned_false(_value: u64) -> u64 {
        MoltObject::from_bool(false).bits()
    }

    extern "C" fn iterator_owned_fail(_value: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            crate::raise_exception::<u64>(py, "ValueError", "owned iterator callback")
        })
    }

    extern "C" fn iterator_owned_fail_pair(_left: u64, right: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            // A native callback may return an owned handle and set an error.
            // The consumer must retire both the operand and that return value.
            inc_ref_bits(py, right);
            crate::raise_exception::<()>(py, "ValueError", "owned iterator callback");
            right
        })
    }

    #[test]
    fn accumulate_initial_edges_preserve_marker_heap_values_none_and_signed_zero() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let marker = crate::molt_itertools_kwd_mark();
            let marker_refs = iterator_owned_refs(marker);
            let ints = [1, 2].map(|value| MoltObject::from_int(value).bits());
            let sequence = MoltObject::from_ptr(crate::alloc_tuple(py, &ints)).bits();
            let empty = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
            for advance in [false, true] {
                for _ in 0..marker_refs.saturating_add(8) {
                    let accumulated = crate::molt_itertools_accumulate(sequence, none, marker);
                    assert_eq!(iterator_owned_refs(marker), marker_refs);
                    if advance {
                        for expected in [1, 3] {
                            let value = crate::molt_itertools_accumulate_next(accumulated);
                            assert!(!crate::exception_pending(py));
                            assert_eq!(to_i64(obj_from_bits(value)), Some(expected));
                            dec_ref_bits(py, value);
                        }
                    }
                    dec_ref_bits(py, accumulated);
                    assert_eq!(iterator_owned_refs(marker), marker_refs);
                }
            }
            for initial in [marker, none] {
                let accumulated = crate::molt_itertools_accumulate(sequence, none, initial);
                for expected in [1, 3] {
                    let value = crate::molt_itertools_accumulate_next(accumulated);
                    assert_eq!(to_i64(obj_from_bits(value)), Some(expected));
                    dec_ref_bits(py, value);
                }
                iterator_owned_drain(py, accumulated, 0, true);
                dec_ref_bits(py, accumulated);
                let accumulated = crate::molt_itertools_accumulate(empty, none, initial);
                iterator_owned_drain(py, accumulated, 0, true);
                dec_ref_bits(py, accumulated);
            }
            for initial in [0.0_f64, -0.0_f64] {
                for input in [sequence, empty] {
                    let initial_bits = MoltObject::from_float(initial).bits();
                    let accumulated = crate::molt_itertools_accumulate(input, none, initial_bits);
                    let value = crate::molt_itertools_accumulate_next(accumulated);
                    assert!(!crate::exception_pending(py));
                    assert_eq!(
                        obj_from_bits(value).as_float().unwrap().to_bits(),
                        initial.to_bits()
                    );
                    dec_ref_bits(py, value);
                    iterator_owned_drain(
                        py,
                        accumulated,
                        if input == sequence { 2 } else { 0 },
                        true,
                    );
                    dec_ref_bits(py, accumulated);
                }
            }
            let initial = iterator_owned_list(py, &[0]);
            let left = iterator_owned_list(py, &[1]);
            let right = iterator_owned_list(py, &[2]);
            let lists = MoltObject::from_ptr(crate::alloc_tuple(py, &[left, right])).bits();
            let baseline = iterator_owned_refs(initial);
            let left_baseline = iterator_owned_refs(left);
            for count in 0..=3 {
                let accumulated = crate::molt_itertools_accumulate(lists, none, initial);
                assert_eq!(iterator_owned_refs(initial), baseline + 1);
                for index in 0..count {
                    let value = crate::molt_itertools_accumulate_next(accumulated);
                    assert!(!crate::exception_pending(py));
                    iterator_owned_assert_list(value, &[0, 1, 2][..index + 1]);
                    assert_eq!(
                        iterator_owned_refs(initial),
                        baseline + if index == 0 { 2 } else { 0 }
                    );
                    dec_ref_bits(py, value);
                    assert_eq!(
                        iterator_owned_refs(initial),
                        baseline + if index == 0 { 1 } else { 0 }
                    );
                    assert_eq!(iterator_owned_refs(left), left_baseline);
                }
                dec_ref_bits(py, accumulated);
                assert_eq!(iterator_owned_refs(initial), baseline);
            }
            for bits in [lists, initial, left, right, empty, sequence, marker] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn owned_iterator_family_releases_items_containers_and_partial_results() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let int = |value| MoltObject::from_int(value).bits();
            let values = [0, 1, 2].map(|value| iterator_owned_list(py, &[value]));
            let sequence = MoltObject::from_ptr(crate::alloc_tuple(py, &values)).bits();
            let outer = MoltObject::from_ptr(crate::alloc_tuple(py, &[sequence])).bits();
            let empty = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
            let zipped = MoltObject::from_ptr(crate::alloc_tuple(py, &[sequence, empty])).bits();
            let selectors =
                MoltObject::from_ptr(crate::alloc_tuple(py, &[int(0), int(1), int(0)])).bits();
            let short_selectors = MoltObject::from_ptr(crate::alloc_tuple(py, &[int(1)])).bits();
            let pair = MoltObject::from_ptr(crate::alloc_tuple(py, &[values[0], values[1]])).bits();
            let args = MoltObject::from_ptr(crate::alloc_tuple(py, &[pair, pair])).bits();
            let truth = iterator_owned_function(py, iterator_owned_true as *const (), 1);
            let falsity = iterator_owned_function(py, iterator_owned_false as *const (), 1);
            let right = iterator_owned_function(py, iterator_owned_right as *const (), 2);
            let heap_predicate =
                iterator_owned_function(py, iterator_owned_heap_predicate as *const (), 1);
            let predicate_result = iterator_owned_list(py, &[9]);
            ITERATOR_OWNED_PREDICATE_RESULT.store(predicate_result, Ordering::SeqCst);
            let marker_itertools = crate::molt_itertools_kwd_mark();
            let baseline = values.map(iterator_owned_refs);
            let sequence_refs = iterator_owned_refs(sequence);
            for mode in 0..18 {
                let (iter, count, exhaust) = match mode {
                    0 => (crate::molt_itertools_chain(outer), 3, true),
                    1 => (
                        crate::molt_itertools_islice(sequence, int(1), int(3), int(1)),
                        2,
                        true,
                    ),
                    2 => (crate::molt_itertools_cycle(sequence), 5, false),
                    3 => (
                        crate::molt_itertools_batched(
                            sequence,
                            int(2),
                            MoltObject::from_bool(false).bits(),
                        ),
                        2,
                        true,
                    ),
                    4 => (crate::molt_itertools_compress(sequence, selectors), 1, true),
                    5 => (
                        crate::molt_itertools_compress(sequence, short_selectors),
                        1,
                        true,
                    ),
                    6 => (crate::molt_itertools_dropwhile(truth, sequence), 0, true),
                    7 => (crate::molt_itertools_dropwhile(falsity, sequence), 3, true),
                    8 => (crate::molt_itertools_filterfalse(none, sequence), 0, true),
                    9 => (
                        crate::molt_itertools_filterfalse(falsity, sequence),
                        3,
                        true,
                    ),
                    10 => (crate::molt_itertools_pairwise(sequence), 2, true),
                    11 => (crate::molt_itertools_starmap(right, args), 2, true),
                    12 => (crate::molt_itertools_takewhile(truth, sequence), 3, true),
                    13 => (
                        crate::molt_itertools_zip_longest(zipped, values[0]),
                        3,
                        true,
                    ),
                    14 => (
                        crate::molt_itertools_dropwhile(heap_predicate, sequence),
                        0,
                        true,
                    ),
                    15 => (
                        crate::molt_itertools_filterfalse(heap_predicate, sequence),
                        0,
                        true,
                    ),
                    16 => (
                        crate::molt_itertools_takewhile(heap_predicate, sequence),
                        3,
                        true,
                    ),
                    _ => (
                        crate::molt_itertools_accumulate(sequence, none, marker_itertools),
                        3,
                        true,
                    ),
                };
                assert!(!crate::exception_pending(py), "constructor {mode}");
                iterator_owned_drain(py, iter, count, exhaust);
                dec_ref_bits(py, iter);
                assert_eq!(
                    iterator_owned_refs(predicate_result),
                    1,
                    "predicate output {mode}"
                );
                assert_eq!(values.map(iterator_owned_refs), baseline, "consumer {mode}");
                assert_eq!(
                    iterator_owned_refs(sequence),
                    sequence_refs,
                    "iterator owner {mode}"
                );
            }
            let stopped = crate::molt_itertools_takewhile(falsity, sequence);
            iterator_owned_drain(py, stopped, 0, true);
            dec_ref_bits(py, stopped);
            let strict =
                crate::molt_itertools_batched(sequence, int(2), MoltObject::from_bool(true).bits());
            {
                let mut iter = crate::object::iterable::OwnedIterator::new(py, strict).unwrap();
                dec_ref_bits(py, iter.next().unwrap().unwrap());
                assert!(iter.next().is_err());
                assert!(crate::exception_pending(py));
                crate::clear_exception(py);
            }
            dec_ref_bits(py, strict);
            let tee = crate::molt_itertools_tee(sequence, int(2));
            let tee_ptr = obj_from_bits(tee).as_ptr().unwrap();
            for index in 0..2 {
                let child = unsafe { crate::object::seq_access::item(tee_ptr, index) }.unwrap();
                iterator_owned_drain(py, child, 3, true);
            }
            dec_ref_bits(py, tee);
            let marker = molt_functools_kwd_mark();
            for initial in [marker, values[0]] {
                let result = super::molt_functools_reduce(right, sequence, initial);
                assert_eq!(result, values[2]);
                assert!(!crate::exception_pending(py));
                dec_ref_bits(py, result);
                assert_eq!(values.map(iterator_owned_refs), baseline);
            }
            assert_eq!(values.map(iterator_owned_refs), baseline);
            assert_eq!(iterator_owned_refs(sequence), sequence_refs);
            ITERATOR_OWNED_PREDICATE_RESULT.store(0, Ordering::SeqCst);
            for bits in [
                heap_predicate,
                predicate_result,
                marker_itertools,
                marker,
                args,
                pair,
                selectors,
                short_selectors,
                zipped,
                empty,
                outer,
                sequence,
                truth,
                falsity,
                right,
            ] {
                dec_ref_bits(py, bits);
            }
            for bits in values {
                assert_eq!(iterator_owned_refs(bits), 1);
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn owned_iterator_callback_failures_release_transients_and_preserve_state() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let values = [0, 1].map(|value| iterator_owned_list(py, &[value]));
            let sequence = MoltObject::from_ptr(crate::alloc_tuple(py, &values)).bits();
            let pair = MoltObject::from_ptr(crate::alloc_tuple(py, &values)).bits();
            let args = MoltObject::from_ptr(crate::alloc_tuple(py, &[pair])).bits();
            let failure = iterator_owned_function(py, iterator_owned_fail as *const (), 1);
            let failure2 = iterator_owned_function(py, iterator_owned_fail_pair as *const (), 2);
            let baseline = values.map(iterator_owned_refs);
            let sequence_refs = iterator_owned_refs(sequence);
            for mode in 0..6 {
                let iter = match mode {
                    0 => crate::molt_itertools_dropwhile(failure, sequence),
                    1 => crate::molt_itertools_filterfalse(failure, sequence),
                    2 => crate::molt_itertools_takewhile(failure, sequence),
                    3 => crate::molt_itertools_groupby(sequence, failure),
                    4 => crate::molt_itertools_starmap(failure2, args),
                    _ => crate::molt_itertools_accumulate(sequence, failure2, values[0]),
                };
                assert!(!crate::exception_pending(py));
                if mode == 5 {
                    let first = crate::molt_itertools_accumulate_next(iter);
                    assert_eq!(first, values[0]);
                    dec_ref_bits(py, first);
                }
                {
                    let mut owned = crate::object::iterable::OwnedIterator::new(py, iter).unwrap();
                    assert!(owned.next().is_err(), "callback {mode}");
                }
                assert!(crate::exception_pending(py));
                let error = kwd_mark_error_identity(py);
                // Dropping the remaining state under the error exercises all
                // field owners, including accumulate's unchanged prior total.
                unsafe { molt_runtime_core::ffi::__molt_runtime_release_owned_value(iter) };
                assert!(crate::exception_pending(py));
                assert_eq!(kwd_mark_error_identity(py), error);
                crate::clear_exception(py);
                assert_eq!(values.map(iterator_owned_refs), baseline, "callback {mode}");
                assert_eq!(iterator_owned_refs(sequence), sequence_refs);
            }
            let marker = molt_functools_kwd_mark();
            let result = super::molt_functools_reduce(failure2, sequence, marker);
            assert_eq!(result, none);
            assert!(crate::exception_pending(py));
            crate::clear_exception(py);
            assert_eq!(values.map(iterator_owned_refs), baseline);
            assert_eq!(iterator_owned_refs(sequence), sequence_refs);
            for bits in [
                marker, args, pair, sequence, failure, failure2, values[0], values[1],
            ] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn groupby_owned_keys_groups_and_values_survive_parent_advancement() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let a = iterator_owned_list(py, &[0]);
            let b = iterator_owned_list(py, &[1]);
            let sequence = MoltObject::from_ptr(crate::alloc_tuple(py, &[a, a, b, a])).bits();
            let baseline = [a, b].map(iterator_owned_refs);
            let parent = crate::molt_itertools_groupby(sequence, none);
            let first = crate::molt_itertools_groupby_next(parent);
            let first_ptr = obj_from_bits(first).as_ptr().unwrap();
            let first_group = unsafe { crate::object::seq_access::item(first_ptr, 1) }.unwrap();
            let value = crate::molt_itertools_groupby_iter_next(first_group);
            assert_eq!(value, a);
            dec_ref_bits(py, value);
            // The next parent skips the remaining first group. A stale grouper
            // must stay exhausted even when a later key equals its old key.
            let second = crate::molt_itertools_groupby_next(parent);
            iterator_owned_drain(py, first_group, 0, true);
            let third = crate::molt_itertools_groupby_next(parent);
            iterator_owned_drain(py, first_group, 0, true);
            let third_ptr = obj_from_bits(third).as_ptr().unwrap();
            let third_group = unsafe { crate::object::seq_access::item(third_ptr, 1) }.unwrap();
            dec_ref_bits(py, parent);
            iterator_owned_drain(py, third_group, 1, true);
            for bits in [first, second, third] {
                dec_ref_bits(py, bits);
            }
            assert_eq!([a, b].map(iterator_owned_refs), baseline);
            for bits in [sequence, a, b] {
                dec_ref_bits(py, bits);
            }
            // Presence facts must not consume the +0.0 value encoding.
            let zero = MoltObject::from_float(0.0).bits();
            let input = MoltObject::from_ptr(crate::alloc_tuple(py, &[zero, zero, none])).bits();
            let parent = crate::molt_itertools_groupby(input, none);
            for (expected, count) in [(zero, 2), (none, 1)] {
                let pair = crate::molt_itertools_groupby_next(parent);
                assert!(!crate::exception_pending(py));
                let ptr = obj_from_bits(pair).as_ptr().unwrap();
                assert_eq!(
                    unsafe { crate::object::seq_access::item(ptr, 0) },
                    Some(expected)
                );
                let group = unsafe { crate::object::seq_access::item(ptr, 1) }.unwrap();
                iterator_owned_drain(py, group, count, true);
                dec_ref_bits(py, pair);
            }
            iterator_owned_drain(py, parent, 0, true);
            dec_ref_bits(py, parent);
            dec_ref_bits(py, input);
        });
    }

    static BATCHED_RETIRE_TARGET: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static BATCHED_RETIRE_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static BATCHED_RETIRE_FINALIZERS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    extern "C" fn batched_retire_iter(this: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            inc_ref_bits(py, this);
            this
        })
    }

    extern "C" fn batched_retire_next(_this: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            assert_eq!(BATCHED_RETIRE_FINALIZERS.load(Ordering::SeqCst), 0);
            if BATCHED_RETIRE_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
                let batch = BATCHED_RETIRE_TARGET.load(Ordering::SeqCst);
                let result = crate::molt_itertools_batched_next(batch);
                assert!(crate::exception_pending(py));
                crate::clear_exception(py);
                dec_ref_bits(py, result);
                assert_eq!(BATCHED_RETIRE_FINALIZERS.load(Ordering::SeqCst), 0);
                MoltObject::from_ptr(crate::object::builders::alloc_string_nointern(py, b"x"))
                    .bits()
            } else {
                crate::raise_exception::<u64>(py, "StopIteration", "")
            }
        })
    }

    extern "C" fn batched_retire_finalizer(_this: u64) -> u64 {
        BATCHED_RETIRE_FINALIZERS.fetch_add(1, Ordering::SeqCst);
        MoltObject::none().bits()
    }

    #[test]
    fn batched_keeps_input_owned_across_reentrant_field_retirement() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            BATCHED_RETIRE_CALLS.store(0, Ordering::SeqCst);
            BATCHED_RETIRE_FINALIZERS.store(0, Ordering::SeqCst);
            let name =
                MoltObject::from_ptr(crate::alloc_string(py, b"ReentrantBatchSource")).bits();
            let class = crate::molt_class_new(name);
            let iter = iterator_owned_function(py, batched_retire_iter as *const (), 1);
            let next = iterator_owned_function(py, batched_retire_next as *const (), 1);
            let finalizer = iterator_owned_function(py, batched_retire_finalizer as *const (), 1);
            let iter_name = MoltObject::from_ptr(crate::alloc_string(py, b"__iter__")).bits();
            let next_name = MoltObject::from_ptr(crate::alloc_string(py, b"__next__")).bits();
            let del_name = MoltObject::from_ptr(crate::alloc_string(py, b"__del__")).bits();
            for (name, method) in [(iter_name, iter), (next_name, next), (del_name, finalizer)] {
                crate::molt_set_attr_name(class, name, method);
            }
            let source = unsafe {
                crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap())
            };
            let batch = crate::molt_itertools_batched(
                source,
                MoltObject::from_int(3).bits(),
                MoltObject::from_bool(false).bits(),
            );
            assert!(!crate::exception_pending(py));
            BATCHED_RETIRE_TARGET.store(batch, Ordering::SeqCst);
            dec_ref_bits(py, source); // The batch now owns the only persistent input edge.
            let result = crate::molt_itertools_batched_next(batch);
            assert!(!crate::exception_pending(py));
            assert_eq!(BATCHED_RETIRE_CALLS.load(Ordering::SeqCst), 3);
            assert_eq!(BATCHED_RETIRE_FINALIZERS.load(Ordering::SeqCst), 1);
            let ptr = obj_from_bits(result).as_ptr().unwrap();
            assert_eq!(unsafe { crate::object::seq_access::len(ptr) }, 1);
            let item = unsafe { crate::object::seq_access::item(ptr, 0) }.unwrap();
            let expected = MoltObject::from_ptr(crate::alloc_string(py, b"x")).bits();
            let equal = crate::molt_eq(item, expected);
            assert_eq!(obj_from_bits(equal).as_bool(), Some(true));
            assert_eq!(
                iterator_owned_refs(item),
                1,
                "the result tuple owns its item"
            );
            BATCHED_RETIRE_TARGET.store(0, Ordering::SeqCst);
            iterator_owned_drain(py, batch, 0, true);
            for bits in [
                equal, expected, result, batch, iter_name, next_name, del_name, iter, next,
                finalizer, class, name,
            ] {
                dec_ref_bits(py, bits);
            }
            assert_eq!(BATCHED_RETIRE_FINALIZERS.load(Ordering::SeqCst), 1);
        });
    }

    static GROUPBY_DRAIN_HOOK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static GROUPBY_DRAIN_RESULT: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static GROUPBY_DRAIN_TRUTH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static GROUPBY_DRAIN_AT_TRUTH: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    static GROUPBY_DRAIN_PARENT: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    static GROUPBY_DRAIN_SAME: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn groupby_drain_hook(py: &crate::PyToken<'_>) {
        let hook = GROUPBY_DRAIN_HOOK.swap(0, Ordering::SeqCst);
        if hook == 0 {
            return;
        }
        let value = if GROUPBY_DRAIN_PARENT.load(Ordering::SeqCst) {
            let pair = crate::molt_itertools_groupby_next(hook);
            assert!(!crate::exception_pending(py));
            let ptr = obj_from_bits(pair).as_ptr().unwrap();
            let grouper = unsafe { crate::object::seq_access::item(ptr, 1) }.unwrap();
            let value = crate::molt_itertools_groupby_iter_next(grouper);
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, pair);
            value
        } else {
            crate::molt_itertools_groupby_iter_next(hook)
        };
        assert!(!crate::exception_pending(py));
        GROUPBY_DRAIN_RESULT.store(value, Ordering::SeqCst);
    }

    extern "C" fn groupby_drain_eq(left: u64, right: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let same = !GROUPBY_DRAIN_PARENT.load(Ordering::SeqCst) || left == right;
            if GROUPBY_DRAIN_AT_TRUTH.load(Ordering::SeqCst) {
                GROUPBY_DRAIN_SAME.store(same, Ordering::SeqCst);
                let truth = GROUPBY_DRAIN_TRUTH.load(Ordering::SeqCst);
                inc_ref_bits(py, truth);
                truth
            } else {
                groupby_drain_hook(py);
                MoltObject::from_bool(same).bits()
            }
        })
    }

    extern "C" fn groupby_drain_bool(_this: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let same = GROUPBY_DRAIN_SAME.load(Ordering::SeqCst);
            groupby_drain_hook(py);
            MoltObject::from_bool(same).bits()
        })
    }

    fn groupby_reentrant_drain_case(parent_reentry: bool) {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let name = MoltObject::from_ptr(crate::alloc_string(py, b"ReentrantGroupKey")).bits();
            let class = crate::molt_class_new(name);
            let eq = iterator_owned_function(py, groupby_drain_eq as *const (), 2);
            let eq_name = MoltObject::from_ptr(crate::alloc_string(py, b"__eq__")).bits();
            crate::molt_set_attr_name(class, eq_name, eq);
            let truth_name =
                MoltObject::from_ptr(crate::alloc_string(py, b"ReentrantGroupTruth")).bits();
            let truth_class = crate::molt_class_new(truth_name);
            let bool_fn = iterator_owned_function(py, groupby_drain_bool as *const (), 1);
            let bool_name = MoltObject::from_ptr(crate::alloc_string(py, b"__bool__")).bits();
            crate::molt_set_attr_name(truth_class, bool_name, bool_fn);
            let truth = unsafe {
                crate::alloc_instance_for_class(py, obj_from_bits(truth_class).as_ptr().unwrap())
            };
            GROUPBY_DRAIN_TRUTH.store(truth, Ordering::SeqCst);
            GROUPBY_DRAIN_PARENT.store(parent_reentry, Ordering::SeqCst);
            let values = [0, 1, 2].map(|_| unsafe {
                crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap())
            });
            let sequence = MoltObject::from_ptr(crate::alloc_tuple(py, &values)).bits();
            let baseline = values.map(iterator_owned_refs);
            for at_truth in [false, true] {
                GROUPBY_DRAIN_AT_TRUTH.store(at_truth, Ordering::SeqCst);
                GROUPBY_DRAIN_HOOK.store(0, Ordering::SeqCst);
                let parent = crate::molt_itertools_groupby(sequence, MoltObject::none().bits());
                let first = crate::molt_itertools_groupby_next(parent);
                assert!(!crate::exception_pending(py));
                let ptr = obj_from_bits(first).as_ptr().unwrap();
                let grouper = unsafe { crate::object::seq_access::item(ptr, 1) }.unwrap();
                let value = crate::molt_itertools_groupby_iter_next(grouper);
                assert_eq!(value, values[0]);
                dec_ref_bits(py, value);
                GROUPBY_DRAIN_HOOK.store(
                    if parent_reentry { parent } else { grouper },
                    Ordering::SeqCst,
                );
                let outer = if parent_reentry {
                    crate::molt_itertools_groupby_next(parent)
                } else {
                    crate::molt_itertools_groupby_iter_next(grouper)
                };
                if parent_reentry {
                    assert!(!crate::exception_pending(py));
                    let ptr = obj_from_bits(outer).as_ptr().unwrap();
                    assert_eq!(
                        unsafe { crate::object::seq_access::item(ptr, 0) },
                        Some(values[2])
                    );
                    let group = unsafe { crate::object::seq_access::item(ptr, 1) }.unwrap();
                    let value = crate::molt_itertools_groupby_iter_next(group);
                    assert_eq!(value, values[2]);
                    dec_ref_bits(py, value);
                } else {
                    assert!(
                        crate::exception_pending(py),
                        "reentrant drain exhausts this advance"
                    );
                    crate::clear_exception(py);
                    assert!(
                        obj_from_bits(outer).is_none(),
                        "missing current is not +0.0"
                    );
                }
                let nested = GROUPBY_DRAIN_RESULT.swap(0, Ordering::SeqCst);
                assert_eq!(
                    nested, values[1],
                    "comparison callback consumed the current item"
                );
                assert_eq!(GROUPBY_DRAIN_HOOK.load(Ordering::SeqCst), 0);
                for bits in [nested, outer, first, parent] {
                    dec_ref_bits(py, bits);
                }
                assert_eq!(values.map(iterator_owned_refs), baseline);
            }
            GROUPBY_DRAIN_TRUTH.store(0, Ordering::SeqCst);
            for bits in [
                sequence,
                values[0],
                values[1],
                values[2],
                truth,
                bool_name,
                bool_fn,
                truth_class,
                truth_name,
                eq_name,
                eq,
                class,
                name,
            ] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn grouper_revalidates_current_after_equality_and_truth_reentry() {
        groupby_reentrant_drain_case(false);
    }

    #[test]
    fn groupby_revalidates_current_after_equality_and_truth_reentry() {
        groupby_reentrant_drain_case(true);
    }

    static ACCUMULATE_REENTRY_TARGET: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static ACCUMULATE_REENTRY_RESULT: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static ACCUMULATE_REENTRY_MODE: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static ACCUMULATE_FINALIZERS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    extern "C" fn accumulate_reenter_finalizer(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            ACCUMULATE_FINALIZERS.fetch_add(1, Ordering::SeqCst);
            if ACCUMULATE_REENTRY_MODE.load(Ordering::SeqCst) == 1 {
                let result = crate::molt_itertools_accumulate_next(
                    ACCUMULATE_REENTRY_TARGET.load(Ordering::SeqCst),
                );
                assert!(!crate::exception_pending(py));
                ACCUMULATE_REENTRY_RESULT.store(result, Ordering::SeqCst);
            }
            MoltObject::none().bits()
        })
    }

    extern "C" fn accumulate_reenter_callback(left: u64, right: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            if ACCUMULATE_REENTRY_MODE
                .compare_exchange(2, 3, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                let nested = crate::molt_itertools_accumulate_next(
                    ACCUMULATE_REENTRY_TARGET.load(Ordering::SeqCst),
                );
                assert!(!crate::exception_pending(py));
                ACCUMULATE_REENTRY_RESULT.store(nested, Ordering::SeqCst);
                assert_eq!(
                    ACCUMULATE_FINALIZERS.load(Ordering::SeqCst),
                    0,
                    "outer operand remains owned across the callback"
                );
                assert!(obj_from_bits(left).as_ptr().is_some());
            }
            inc_ref_bits(py, right);
            right
        })
    }

    #[test]
    fn accumulate_publishes_owned_total_before_callback_and_finalizer_reentry() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let name = MoltObject::from_ptr(crate::alloc_string(py, b"AccumulateInitial")).bits();
            let class = crate::molt_class_new(name);
            let finalizer =
                iterator_owned_function(py, accumulate_reenter_finalizer as *const (), 1);
            let del_name = MoltObject::from_ptr(crate::alloc_string(py, b"__del__")).bits();
            crate::molt_set_attr_name(class, del_name, finalizer);
            let callback = iterator_owned_function(py, accumulate_reenter_callback as *const (), 2);
            let left = iterator_owned_list(py, &[1]);
            let right = iterator_owned_list(py, &[2]);
            let sequence = MoltObject::from_ptr(crate::alloc_tuple(py, &[left, right])).bits();
            let baseline = [left, right].map(iterator_owned_refs);
            for mode in [1, 2] {
                ACCUMULATE_FINALIZERS.store(0, Ordering::SeqCst);
                ACCUMULATE_REENTRY_MODE.store(mode, Ordering::SeqCst);
                ACCUMULATE_REENTRY_RESULT.store(0, Ordering::SeqCst);
                let initial = unsafe {
                    crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap())
                };
                let accumulated = crate::molt_itertools_accumulate(sequence, callback, initial);
                let yielded = crate::molt_itertools_accumulate_next(accumulated);
                assert_eq!(yielded, initial);
                dec_ref_bits(py, yielded);
                dec_ref_bits(py, initial); // Total is now the only persistent owner.
                ACCUMULATE_REENTRY_TARGET.store(accumulated, Ordering::SeqCst);
                let outer = crate::molt_itertools_accumulate_next(accumulated);
                assert!(!crate::exception_pending(py));
                assert_eq!(outer, left);
                assert_eq!(ACCUMULATE_FINALIZERS.load(Ordering::SeqCst), 1);
                let nested = ACCUMULATE_REENTRY_RESULT.swap(0, Ordering::SeqCst);
                assert_eq!(nested, right);
                iterator_owned_assert_list(outer, &[1]);
                iterator_owned_assert_list(nested, &[2]);
                ACCUMULATE_REENTRY_TARGET.store(0, Ordering::SeqCst);
                ACCUMULATE_REENTRY_MODE.store(0, Ordering::SeqCst);
                for bits in [outer, nested, accumulated] {
                    dec_ref_bits(py, bits);
                }
                assert_eq!([left, right].map(iterator_owned_refs), baseline);
            }
            for bits in [
                sequence, left, right, callback, del_name, finalizer, class, name,
            ] {
                dec_ref_bits(py, bits);
            }
        });
    }

    static ITERATOR_OWNED_CALLBACK_COUNT: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static ITERATOR_OWNED_PREDICATE_RESULT: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    extern "C" fn iterator_owned_heap_predicate(_value: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let bits = ITERATOR_OWNED_PREDICATE_RESULT.load(Ordering::SeqCst);
            inc_ref_bits(py, bits);
            bits
        })
    }

    extern "C" fn iterator_owned_one_then_fail(_left: u64, right: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            if ITERATOR_OWNED_CALLBACK_COUNT.fetch_add(1, Ordering::SeqCst) == 0 {
                inc_ref_bits(py, right);
                right
            } else {
                crate::raise_exception::<u64>(py, "ValueError", "input failed after one item")
            }
        })
    }

    #[test]
    fn owned_iterator_input_and_allocation_failures_release_partial_collections() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let int = |value| MoltObject::from_int(value).bits();
            let none = MoltObject::none().bits();
            let value = iterator_owned_list(py, &[1]);
            let sequence = MoltObject::from_ptr(crate::alloc_tuple(py, &[value, value])).bits();
            let pair = MoltObject::from_ptr(crate::alloc_tuple(py, &[value, value])).bits();
            let args = MoltObject::from_ptr(crate::alloc_tuple(py, &[pair, pair])).bits();
            let callback =
                iterator_owned_function(py, iterator_owned_one_then_fail as *const (), 2);
            let right = iterator_owned_function(py, iterator_owned_right as *const (), 2);
            let marker = molt_functools_kwd_mark();
            let baseline = iterator_owned_refs(value);
            for mode in 0..5 {
                ITERATOR_OWNED_CALLBACK_COUNT.store(0, Ordering::SeqCst);
                let source = crate::molt_itertools_starmap(callback, args);
                let mut consumer = none;
                let mut outer = none;
                match mode {
                    0 => assert_eq!(crate::molt_itertools_cycle(source), none),
                    1 => {
                        consumer = crate::molt_itertools_batched(
                            source,
                            int(3),
                            MoltObject::from_bool(false).bits(),
                        )
                    }
                    2 => {
                        outer =
                            MoltObject::from_ptr(crate::alloc_tuple(py, &[source, source])).bits();
                        consumer = crate::molt_itertools_zip_longest(outer, none);
                    }
                    3 => assert_eq!(super::molt_functools_reduce(right, source, marker), none),
                    _ => consumer = crate::molt_itertools_islice(source, int(1), int(2), int(1)),
                }
                if consumer != none {
                    let mut iter =
                        crate::object::iterable::OwnedIterator::new(py, consumer).unwrap();
                    assert!(iter.next().is_err(), "partial input {mode}");
                }
                assert!(crate::exception_pending(py), "partial input {mode}");
                let error = kwd_mark_error_identity(py);
                for bits in [consumer, outer, source] {
                    unsafe { molt_runtime_core::ffi::__molt_runtime_release_owned_value(bits) };
                }
                assert_eq!(kwd_mark_error_identity(py), error);
                crate::clear_exception(py);
                assert_eq!(iterator_owned_refs(value), baseline, "partial input {mode}");
            }
            let outer = MoltObject::from_ptr(crate::alloc_tuple(py, &[sequence, sequence])).bits();
            for mode in 0..3 {
                let consumer = match mode {
                    0 => crate::molt_itertools_batched(
                        sequence,
                        int(2),
                        MoltObject::from_bool(false).bits(),
                    ),
                    1 => crate::molt_itertools_zip_longest(outer, none),
                    _ => crate::molt_itertools_pairwise(sequence),
                };
                assert!(!crate::exception_pending(py));
                {
                    let _budget = DenyKwdMarkAllocations::enter();
                    let result = match mode {
                        0 => crate::molt_itertools_batched_next(consumer),
                        1 => crate::molt_itertools_zip_longest_next(consumer),
                        _ => crate::molt_itertools_pairwise_next(consumer),
                    };
                    assert_eq!(result, none);
                }
                assert_kwd_mark_memory_error(py);
                let error = kwd_mark_error_identity(py);
                unsafe { molt_runtime_core::ffi::__molt_runtime_release_owned_value(consumer) };
                assert_eq!(kwd_mark_error_identity(py), error);
                crate::clear_exception(py);
                assert_eq!(
                    iterator_owned_refs(value),
                    baseline,
                    "tuple allocation {mode}"
                );
            }
            for bits in [outer, marker, callback, right, args, pair, sequence, value] {
                dec_ref_bits(py, bits);
            }
        });
    }
}
