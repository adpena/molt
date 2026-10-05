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
/// supplies its positional arity and retained defaults and never binds a receiver.
/// The namespace owns the callables; there is no parallel method cache.
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
    if iter_fn == 0
        || next_fn == 0
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
    if !exception_pending(py) {
        unsafe {
            dict_set_in_place(
                py,
                dict_ptr,
                layout_name,
                MoltObject::from_int(layout_size).bits(),
            )
        };
    }
    for (name, callback) in [("__iter__", iter_fn), ("__next__", next_fn)] {
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
    if let Some((callback, arity, defaults)) = constructor {
        if crate::builtins::methods::builtin_func_bits_with_defaults_tuple(
            py,
            NativeCallableSpec::constructor(class_bits),
            callback,
            arity,
            defaults,
        ) == 0
        {
            return discard_itertools_class(py, class_bits);
        }
    }
    if exception_pending(py)
        || unsafe { crate::object::class_finish_definition(py, class_ptr) }.is_err()
    {
        return discard_itertools_class(py, class_bits);
    }
    class_bits
}

#[cfg(test)]
mod tests {
    use super::*;

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
            for _ in 0..3 {
                let next = attr(py, defaulted, b"__next__");
                let yielded = unsafe { call_callable0(py, next) };
                assert!(!exception_pending(py));
                assert_eq!(yielded, value);
                dec_ref_bits(py, yielded);
                dec_ref_bits(py, next);
            }
            for owned in [defaulted, instance_constructor, constructor, count, repeat] {
                dec_ref_bits(py, owned);
            }
        });
    }
}
