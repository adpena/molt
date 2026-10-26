//! Canonical itertools class construction shared by reduced and full profiles.

use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;
use molt_runtime_core::ObjectShapeId;

/// Release a constructor-owned class that has not reached its runtime slot.
fn discard_itertools_class(py: &crate::PyToken<'_>, class: u64) -> u64 {
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        let ptr = obj_from_bits(class)
            .as_ptr()
            .expect("unpublished itertools class");
        unsafe { crate::object::class_storage::clear_class_runtime_contents(py, ptr) };
        class_break_cycles(py, class);
        dec_ref_bits(py, class);
    });
    MoltObject::none().bits()
}

/// Allocate a complete itertools declaration before publishing its class cache.
///
/// Iterator callbacks have the positional `(self)` ABI. An optional constructor
/// supplies its full positional arity, including an explicit first `cls` operand,
/// and retained trailing defaults. Attribute lookup never binds `cls` implicitly:
/// type.__call__ prepends it, while explicit __new__ callers supply it themselves.
/// The namespace owns the callables; there is no parallel method cache.
/// The `__next__` callback is also the exact class native next slot.
/// Its immutable namespace keeps the method and slot coherent; tee and
/// groupby iterator classes remain final.
pub(crate) fn alloc_itertools_class(
    py: &crate::PyToken<'_>,
    name: &str,
    layout_size: i64,
    shape: ObjectShapeId,
    iter_fn: u64,
    next_fn: u64,
    constructor: Option<(u64, u64, &[u64])>,
) -> u64 {
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let is_tee_block = shape == ObjectShapeId::ItertoolsTeeBlock;
    let declared_extent = match shape {
        ObjectShapeId::ItertoolsTee => Some(std::mem::size_of::<
            molt_runtime_core::itertools_tee::TeeCursor,
        >()),
        ObjectShapeId::ItertoolsTeeBlock => Some(std::mem::size_of::<
            molt_runtime_core::itertools_tee::TeeBlock,
        >()),
        _ => None,
    };
    if declared_extent.is_some_and(|extent| layout_size != extent as i64)
        || (is_tee_block && (iter_fn != 0 || next_fn != 0 || constructor.is_some()))
        || (!is_tee_block && (iter_fn == 0 || next_fn == 0))
        || constructor.is_some_and(|(callback, arity, defaults)| {
            callback == 0 || arity == 0 || defaults.len() as u64 >= arity
        })
    {
        return raise_exception::<u64>(py, "SystemError", "invalid itertools callable declaration");
    }
    let name_str_ptr = alloc_string(py, name.as_bytes());
    if name_str_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_str_ptr).bits();
    let class_ptr = alloc_class_obj(py, name_bits);
    dec_ref_bits(py, name_bits);
    if class_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    unsafe {
        crate::object::class_storage::class_declare_native_slots(
            class_ptr,
            crate::object::class_storage::ClassSlotPolicy {
                allows_dict: false,
                allows_weakref: shape == ObjectShapeId::ItertoolsTee,
                variable_sized: false,
            },
        );
    }
    if !unsafe { crate::object::class_set_instance_shape_id(class_ptr, shape) } {
        if !exception_pending(py) {
            raise_exception::<u64>(py, "SystemError", "invalid itertools instance shape");
        }
        return discard_itertools_class(py, class_bits);
    }
    let builtins = builtin_classes(py);
    if !unsafe {
        crate::object::object_init_class_edge_unpublished(
            py,
            class_ptr,
            builtins.type_obj,
            ClassEdgeOwnership::Owned,
        )
    } {
        if !exception_pending(py) {
            raise_exception::<u64>(py, "SystemError", "invalid itertools metaclass edge");
        }
        return discard_itertools_class(py, class_bits);
    }
    let _ = molt_class_set_base(class_bits, builtins.object);
    if exception_pending(py) {
        return discard_itertools_class(py, class_bits);
    }
    let namespace = obj_from_bits(unsafe { class_dict_bits(class_ptr) }).as_ptr();
    let Some(dict_ptr) = namespace.filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_DICT)
    else {
        raise_exception::<u64>(py, "SystemError", "itertools class has no valid namespace");
        return discard_itertools_class(py, class_bits);
    };
    let layout_name = intern_static_name(
        py,
        &crate::runtime_state(py).interned.molt_layout_size,
        b"__molt_layout_size__",
    );
    if obj_from_bits(layout_name).as_ptr().is_none()
        || !unsafe {
            dict_set_in_place(
                py,
                dict_ptr,
                layout_name,
                MoltObject::from_int(layout_size).bits(),
            )
        }
    {
        return discard_itertools_class(py, class_bits);
    }
    for (name, callback) in [("__iter__", iter_fn), ("__next__", next_fn)]
        .into_iter()
        .filter(|_| !is_tee_block)
    {
        if crate::builtins::methods::builtin_func_bits(
            py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, class_bits, name),
            callback,
            1,
        ) == 0
        {
            return discard_itertools_class(py, class_bits);
        }
    }
    if shape == ObjectShapeId::ItertoolsTee
        && crate::builtins::methods::builtin_func_bits(
            py,
            NativeCallableSpec::declared(
                NativeCallableKind::MethodDescriptor,
                class_bits,
                "__copy__",
            ),
            crate::molt_itertools_tee_copy as *const () as usize as u64,
            1,
        ) == 0
    {
        return discard_itertools_class(py, class_bits);
    }
    if !is_tee_block {
        // SAFETY: `next_fn` is the address of the `(self)` callback just declared
        // as `__next__`; the class is unpublished.
        unsafe {
            crate::object::layout::class_declare_native_next(
                class_ptr,
                std::mem::transmute::<usize, crate::object::layout::NativeNextFn>(next_fn as usize),
            );
        }
    }
    if let Some((callback, arity, defaults)) = constructor
        && crate::builtins::methods::builtin_func_bits_with_defaults_tuple(
            py,
            NativeCallableSpec::constructor(class_bits),
            callback,
            arity,
            defaults,
        ) == 0
    {
        return discard_itertools_class(py, class_bits);
    }
    let module_key = attr_name_bits_from_bytes(py, b"__module__");
    let module_value = attr_name_bits_from_bytes(py, b"itertools");
    let (Some(module_key), Some(module_value)) = (module_key, module_value) else {
        for bits in module_key.into_iter().chain(module_value) {
            dec_ref_bits(py, bits);
        }
        return discard_itertools_class(py, class_bits);
    };
    let module_set = unsafe { dict_set_in_place(py, dict_ptr, module_key, module_value) };
    dec_ref_bits(py, module_key);
    dec_ref_bits(py, module_value);
    let basetype = !matches!(
        shape,
        ObjectShapeId::ItertoolsTee | ObjectShapeId::ItertoolsGroupbyIter
    );
    if !module_set
        || exception_pending(py)
        || unsafe {
            crate::object::class_storage::class_capture_native_module(py, class_ptr, false)
        }
        .is_err()
        || unsafe { crate::object::class_finish_definition(py, class_ptr) }.is_err()
        || !unsafe {
            crate::object::class_storage::ClassSemanticPolicy::heap(true, basetype)
                .apply(py, class_ptr)
        }
    {
        return discard_itertools_class(py, class_bits);
    }
    class_bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tee_pair(py: &crate::PyToken<'_>, source: u64) -> [u64; 2] {
        let result = crate::molt_itertools_tee(source, MoltObject::from_int(2).bits());
        assert!(!exception_pending(py));
        let tuple = obj_from_bits(result).as_ptr().unwrap();
        let readers = std::array::from_fn(|index| unsafe {
            let bits = crate::object::seq_access::item(tuple, index).unwrap();
            inc_ref_bits(py, bits);
            bits
        });
        dec_ref_bits(py, result);
        readers
    }

    #[test]
    fn tee_shared_block_outlives_first_reader_and_releases_every_owned_value() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(py, {
            let value = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let sequence = MoltObject::from_ptr(alloc_tuple(py, &[value])).bits();
            let [first, second] = tee_pair(py, sequence);
            let yielded = crate::molt_itertools_tee_next(first);
            assert_eq!(yielded, value);
            dec_ref_bits(py, yielded);
            dec_ref_bits(py, first);
            unsafe { crate::object::gc::collect_cycles(py) };
            let yielded = crate::molt_itertools_tee_next(second);
            assert!(!exception_pending(py));
            assert_eq!(
                yielded, value,
                "deleting first reader must preserve the shared item"
            );
            dec_ref_bits(py, yielded);
            dec_ref_bits(py, second);
            let sequence_ptr = obj_from_bits(sequence).as_ptr().unwrap();
            assert_eq!(
                unsafe { (*header_from_obj_ptr(sequence_ptr)).owned_ref_count_snapshot() },
                1,
                "tee must release the source iterator exactly once"
            );
            dec_ref_bits(py, sequence);
            let value_ptr = obj_from_bits(value).as_ptr().unwrap();
            assert_eq!(
                unsafe { (*header_from_obj_ptr(value_ptr)).owned_ref_count_snapshot() },
                1,
                "all cache and source owners must be gone"
            );
            dec_ref_bits(py, value);
        });
    }

    #[test]
    fn tee_synchronized_readers_bound_live_value_owners() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::state::metrics::enable_profile_for_test();
        crate::with_gil_entry_nopanic!(py, {
            let value = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let value_ptr = obj_from_bits(value).as_ptr().unwrap();
            let source = crate::molt_itertools_repeat(value, MoltObject::from_int(900).bits());
            // Measure the same source protocol independently. Repeat currently
            // takes generic descriptor dispatch, whose temporary allocations
            // belong to source advancement rather than tee cache storage.
            let source_allocations_before =
                crate::ALLOC_COUNT.load(std::sync::atomic::Ordering::Relaxed);
            let source_bytes_before =
                crate::ALLOC_BYTES_TOTAL.load(std::sync::atomic::Ordering::Relaxed);
            {
                let iterator =
                    molt_runtime_core::OwnedRuntimeValue::retain(py.core_token(), source);
                for _ in 0..400 {
                    let yielded = molt_runtime_core::iter_next_owned(py.core_token(), &iterator)
                        .expect("repeat source must advance")
                        .expect("repeat source must not exhaust");
                    assert_eq!(yielded.bits(), value);
                }
            }
            let source_allocations = crate::ALLOC_COUNT.load(std::sync::atomic::Ordering::Relaxed)
                - source_allocations_before;
            let source_bytes = crate::ALLOC_BYTES_TOTAL.load(std::sync::atomic::Ordering::Relaxed)
                - source_bytes_before;
            let baseline = unsafe { (*header_from_obj_ptr(value_ptr)).owned_ref_count_snapshot() };
            let [first, second] = tee_pair(py, source);
            let allocations_before = crate::ALLOC_COUNT.load(std::sync::atomic::Ordering::Relaxed);
            let allocated_bytes_before =
                crate::ALLOC_BYTES_TOTAL.load(std::sync::atomic::Ordering::Relaxed);
            for _ in 0..400 {
                for reader in [first, second] {
                    let before = crate::ALLOC_COUNT.load(std::sync::atomic::Ordering::Relaxed);
                    let yielded = crate::molt_itertools_tee_next(reader);
                    assert!(!exception_pending(py));
                    assert_eq!(yielded, value);
                    dec_ref_bits(py, yielded);
                    if reader == second {
                        assert_eq!(
                            crate::ALLOC_COUNT.load(std::sync::atomic::Ordering::Relaxed),
                            before,
                            "cached reader advancement must not allocate"
                        );
                    }
                }
                let owners =
                    unsafe { (*header_from_obj_ptr(value_ptr)).owned_ref_count_snapshot() };
                assert!(
                    owners <= baseline + 57,
                    "cache retained already-consumed blocks"
                );
            }
            let allocations =
                crate::ALLOC_COUNT.load(std::sync::atomic::Ordering::Relaxed) - allocations_before;
            let allocated_bytes = crate::ALLOC_BYTES_TOTAL
                .load(std::sync::atomic::Ordering::Relaxed)
                - allocated_bytes_before;
            // The first block exists already. 400 successful source advances
            // cross seven CPython block boundaries; cached reads allocate none.
            // Keep generic source dispatch costs visible instead of attributing
            // every allocation to tee or assuming source advancement is free.
            assert_eq!(
                allocations,
                source_allocations + 7,
                "tee must add one cache block per 57 source yields"
            );
            let block_bytes = allocated_bytes
                .checked_sub(source_bytes)
                .expect("tee adds cache storage to the same source cost");
            eprintln!(
                "tee lockstep: source_yields=400 reader_yields=800 total_allocations={allocations} source_allocations={source_allocations} block_allocations=7 total_bytes={allocated_bytes} source_bytes={source_bytes} block_bytes={block_bytes} live_cached_value_bound=57"
            );
            dec_ref_bits(py, first);
            dec_ref_bits(py, second);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(value_ptr)).owned_ref_count_snapshot() },
                baseline
            );
            dec_ref_bits(py, source);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(value_ptr)).owned_ref_count_snapshot() },
                1
            );
            dec_ref_bits(py, value);
        });
    }

    #[test]
    fn tee_terminal_links_retire_iteratively_through_native_and_abi_ownership() {
        use molt_runtime_core::itertools_tee::{TeeBlock, TeeCursor};
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(py, {
            // Get the block class from the public producer, then construct a
            // long ownership chain directly to isolate terminal retirement from
            // iterator throughput. Registry absence is the independent oracle.
            let empty = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            let [first, second] = tee_pair(py, empty);
            let cursor = obj_from_bits(first).as_ptr().unwrap().cast::<TeeCursor>();
            let block = obj_from_bits(unsafe { (*cursor).block_bits })
                .as_ptr()
                .unwrap();
            let class = unsafe { object_class_bits(block) };
            assert!(
                unsafe {
                    crate::object::layout::class_native_next(obj_from_bits(class).as_ptr().unwrap())
                }
                .is_none(),
                "tee storage blocks have no iterator callback or native next slot"
            );
            inc_ref_bits(py, class);
            dec_ref_bits(py, first);
            dec_ref_bits(py, second);
            dec_ref_bits(py, empty);
            for (viewed, c_rooted) in [(false, false), (true, false), (true, true)] {
                let mut head = 0;
                let mut nodes = Vec::new();
                let mut c_root = std::ptr::null_mut();
                for index in 0..4096 {
                    let bits = unsafe {
                        crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap())
                    };
                    let ptr = obj_from_bits(bits).as_ptr().unwrap();
                    let mut payload = TeeBlock::new(0);
                    payload.next_bits = head; // Transfer the previous stack owner.
                    unsafe { ptr.cast::<TeeBlock>().write(payload) };
                    head = bits;
                    nodes.push(ptr);
                    if viewed {
                        let view = unsafe {
                            molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits)
                        };
                        assert!(!view.is_null());
                        if c_rooted && index == 2048 {
                            unsafe { molt_cpython_abi::api::refcount::Py_INCREF(view) };
                            c_root = view;
                        }
                    }
                }
                dec_ref_bits(py, head);
                for (index, ptr) in nodes.iter().copied().enumerate() {
                    assert_eq!(
                        unsafe { crate::object::gc::gc_is_tracked(ptr) },
                        c_rooted && index <= 2048,
                        "retirement must stop exactly at the externally C-owned suffix"
                    );
                }
                if c_rooted {
                    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(c_root) };
                    assert!(
                        nodes
                            .into_iter()
                            .all(|ptr| !unsafe { crate::object::gc::gc_is_tracked(ptr) })
                    );
                }
                assert!(!exception_pending(py));
            }
            dec_ref_bits(py, class);
        });
    }

    #[test]
    fn cached_public_results_own_references_independent_of_runtime_slots() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // Both profiles call these same exports: reduced builds include the
            // source in-tree, while edge/full builds use the satellite bridge.
            for (get, minimum_owners) in [
                (crate::molt_itertools_kwd_mark as extern "C" fn() -> u64, 2),
                (
                    crate::molt_itertools_repeat_type as extern "C" fn() -> u64,
                    3,
                ),
            ] {
                let first = get();
                assert!(!exception_pending(py));
                let ptr = obj_from_bits(first).as_ptr().expect("cached heap result");
                let owners = unsafe { (*header_from_obj_ptr(ptr)).owned_ref_count_snapshot() };
                // Cache + caller; the repeat class also has its MRO self-edge.
                assert!(
                    owners >= minimum_owners,
                    "public result consumed a cache owner"
                );
                for _ in 0..4 {
                    let next = get();
                    assert_eq!(first, next);
                    assert_eq!(
                        unsafe { (*header_from_obj_ptr(ptr)).owned_ref_count_snapshot() },
                        owners + 1,
                        "every public return must supply a fresh reference"
                    );
                    dec_ref_bits(py, next);
                    assert_eq!(
                        unsafe { (*header_from_obj_ptr(ptr)).owned_ref_count_snapshot() },
                        owners
                    );
                }
                dec_ref_bits(py, first);
            }
        });
    }

    #[test]
    fn class_construction_publishes_generated_instance_shape() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let shape = ObjectShapeId::ItertoolsCount;
            let class_bits = alloc_itertools_class(
                py,
                "count",
                24,
                shape,
                crate::molt_itertools_iter_self as *const () as usize as u64,
                crate::molt_itertools_count_next as *const () as usize as u64,
                None,
            );
            let class_ptr = obj_from_bits(class_bits)
                .as_ptr()
                .expect("itertools class allocation must succeed");
            assert_eq!(
                unsafe { crate::object::class_instance_shape_id(class_ptr) },
                shape
            );
            discard_itertools_class(py, class_bits);
        });
    }

    #[test]
    fn iterator_attributes_bind_declaring_owner_and_constructor_stays_explicit() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            fn attr(py: &crate::PyToken<'_>, object: u64, name: &[u8]) -> u64 {
                let key = attr_name_bits_from_bytes(py, name).expect("attribute name");
                let value = crate::molt_get_attr_name(object, key);
                dec_ref_bits(py, key);
                assert!(!exception_pending(py), "attribute lookup failed");
                value
            }
            let value = MoltObject::from_int(17).bits();
            let repeat = crate::molt_itertools_repeat(value, MoltObject::from_int(2).bits());
            let count = crate::molt_itertools_count(value, MoltObject::from_int(3).bits());
            assert!(!exception_pending(py));
            let repeat_class =
                unsafe { object_class_bits(obj_from_bits(repeat).as_ptr().unwrap()) };
            let count_class = unsafe { object_class_bits(obj_from_bits(count).as_ptr().unwrap()) };
            let mut iter_descriptors = Vec::new();
            for (instance, owner) in [(repeat, repeat_class), (count, count_class)] {
                for name in [b"__iter__".as_slice(), b"__next__".as_slice()] {
                    let descriptor = attr(py, owner, name);
                    assert_eq!(
                        unsafe { object_class_bits(obj_from_bits(descriptor).as_ptr().unwrap()) },
                        builtin_classes(py).method_descriptor
                    );
                    let declaring = attr(py, descriptor, b"__objclass__");
                    assert_eq!(declaring, owner);
                    dec_ref_bits(py, declaring);
                    let bound = attr(py, instance, name);
                    let result = unsafe { call_callable0(py, bound) };
                    assert!(!exception_pending(py));
                    assert_eq!(result, if name == b"__iter__" { instance } else { value });
                    dec_ref_bits(py, result);
                    dec_ref_bits(py, bound);
                    if name == b"__iter__" {
                        iter_descriptors.push(descriptor);
                    } else {
                        dec_ref_bits(py, descriptor);
                    }
                }
            }
            assert_ne!(
                iter_descriptors[0], iter_descriptors[1],
                "each class must own its declaration"
            );
            let rejected = unsafe { call_callable1(py, iter_descriptors[0], count) };
            assert!(
                exception_pending(py),
                "a foreign receiver must not reach the raw callback"
            );
            clear_exception(py);
            dec_ref_bits(py, rejected);
            for descriptor in iter_descriptors {
                dec_ref_bits(py, descriptor);
            }
            let constructor = attr(py, repeat_class, b"__new__");
            let instance_constructor = attr(py, repeat, b"__new__");
            assert_eq!(
                instance_constructor, constructor,
                "__new__ must not bind the instance"
            );
            let declared_owner = attr(py, constructor, b"__self__");
            assert_eq!(declared_owner, repeat_class);
            dec_ref_bits(py, declared_owner);
            let builder = crate::molt_callargs_new(
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(0).bits(),
            );
            unsafe { crate::molt_callargs_push_pos(builder, repeat_class) };
            unsafe { crate::molt_callargs_push_pos(builder, value) };
            let defaulted = crate::molt_call_bind(constructor, builder);
            assert!(
                !exception_pending(py),
                "constructor must retain its omitted count default"
            );
            let class_defaulted = unsafe { call_callable1(py, repeat_class, value) };
            assert!(
                !exception_pending(py),
                "repeat class must supply explicit cls"
            );
            for reader in [defaulted, class_defaulted] {
                for _ in 0..3 {
                    let next = attr(py, reader, b"__next__");
                    let yielded = unsafe { call_callable0(py, next) };
                    assert!(!exception_pending(py));
                    assert_eq!(yielded, value);
                    dec_ref_bits(py, yielded);
                    dec_ref_bits(py, next);
                }
            }
            let finite =
                unsafe { call_callable2(py, repeat_class, value, MoltObject::from_int(2).bits()) };
            assert!(
                !exception_pending(py),
                "repeat class must bind its full arity"
            );
            for _ in 0..2 {
                let yielded = crate::molt_itertools_repeat_next(finite);
                assert!(!exception_pending(py));
                assert_eq!(yielded, value);
                dec_ref_bits(py, yielded);
            }
            for owned in [
                finite,
                class_defaulted,
                defaulted,
                instance_constructor,
                constructor,
                count,
                repeat,
            ] {
                dec_ref_bits(py, owned);
            }
        });
    }

    #[test]
    fn exact_itertools_class_owns_its_native_next_and_seals_its_namespace() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let int = |value| MoltObject::from_int(value).bits();
            let count = crate::molt_itertools_count(int(5), int(2));
            let source = MoltObject::from_ptr(alloc_tuple(py, &[int(1)])).bits();
            let tees = crate::molt_itertools_tee(source, int(1));
            assert!(!exception_pending(py));
            let tee = unsafe {
                crate::object::seq_access::item(obj_from_bits(tees).as_ptr().unwrap(), 0)
            }
            .expect("one tee");
            for (instance, callback, basetype) in [
                (
                    count,
                    crate::molt_itertools_count_next as *const () as usize,
                    true,
                ),
                (
                    tee,
                    crate::molt_itertools_tee_next as *const () as usize,
                    false,
                ),
            ] {
                let class_bits =
                    unsafe { object_class_bits(obj_from_bits(instance).as_ptr().unwrap()) };
                let class = obj_from_bits(class_bits).as_ptr().unwrap();
                let next = unsafe { crate::object::layout::class_native_next(class) }
                    .expect("the exact class declares its native next");
                // One callback is both the slot and the `__next__` descriptor.
                assert_eq!(next as usize, callback);
                assert!(unsafe { crate::object::class_is_immutable(py, class) });
                assert_eq!(
                    unsafe { crate::object::class_is_not_base(py, class) },
                    !basetype
                );
                let key = attr_name_bits_from_bytes(py, b"__module__").unwrap();
                let module = crate::molt_get_attr_name(class_bits, key);
                dec_ref_bits(py, key);
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(module)).as_deref(),
                    Some("itertools")
                );
                dec_ref_bits(py, module);
            }
            // CPython itertools types are immutable heap types.
            let key = attr_name_bits_from_bytes(py, b"__next__").unwrap();
            let count_class = unsafe { object_class_bits(obj_from_bits(count).as_ptr().unwrap()) };
            let _ = crate::molt_set_attr_name(count_class, key, int(0));
            assert!(
                exception_pending(py),
                "an immutable type rejects __next__ rebinding"
            );
            let error = crate::molt_exception_last();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "TypeError"
            ));
            crate::clear_exception(py);
            dec_ref_bits(py, error);
            dec_ref_bits(py, key);
            // The slot is the iteration step: no lookup, bound method or binding.
            for expected in [5, 7, 9] {
                match crate::object::ops_iter::iteration_step(
                    py,
                    count,
                    crate::object::iterable::IterationExhaustion::Normalize,
                ) {
                    Ok(crate::object::iterable::SpecialIterationStep::Item(value)) => {
                        assert_eq!(obj_from_bits(value).as_int(), Some(expected));
                    }
                    _ => panic!("count must advance through its native next"),
                }
            }
            // A heap subclass declares no slot; its own or inherited __next__
            // is reached through the special-method protocol.
            let name = attr_name_bits_from_bytes(py, b"CountChild").unwrap();
            let child = crate::molt_class_new(name);
            dec_ref_bits(py, name);
            let base = crate::molt_class_set_base(child, count_class);
            assert!(!exception_pending(py));
            dec_ref_bits(py, base);
            let child_ptr = obj_from_bits(child).as_ptr().unwrap();
            assert!(unsafe { crate::object::layout::class_native_next(child_ptr) }.is_none());
            for owned in [child, tees, source, count] {
                dec_ref_bits(py, owned);
            }
        });
    }
}
