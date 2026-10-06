use super::*;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use std::cell::RefCell;

#[derive(Default)]
struct State {
    events: Vec<u8>,
    result: u64,
    left: u64,
    right: u64,
    left_value: u64,
    right_value: u64,
    mutation: u64,
    raise: bool,
    raise_getter: bool,
    stop_callable: bool,
    deque: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn class(py: &PyToken<'_>, base: u64) -> u64 {
    let name = attr_name_bits_from_bytes(py, b"ComparisonConsumerProbe").unwrap();
    let bits = molt_class_new(name);
    dec_ref_bits(py, name);
    let result = molt_class_set_base(bits, base);
    dec_ref_bits(py, result);
    unsafe { crate::object::class_finish_definition(py, obj_from_bits(bits).as_ptr().unwrap()) }
        .expect("valid fixture class");
    assert!(!exception_pending(py));
    bits
}

fn instance(py: &PyToken<'_>, class: u64) -> u64 {
    let bits = unsafe { alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap()) };
    assert!(!obj_from_bits(bits).is_none());
    bits
}

fn attr(py: &PyToken<'_>, object: u64, name: &[u8], value: u64) {
    let key = attr_name_bits_from_bytes(py, name).unwrap();
    let result = molt_set_attr_name(object, key, value);
    dec_ref_bits(py, result);
    dec_ref_bits(py, key);
    assert!(!exception_pending(py));
}

fn function(py: &PyToken<'_>, name: &'static str, target: *const (), arity: u64) -> u64 {
    let ptr = alloc_runtime_function_obj(py, runtime_fn_addr(name, target), arity);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn install(py: &PyToken<'_>, class: u64, name: &[u8], target: *const (), arity: u64) {
    let callable = function(py, "comparison_consumer_eq", target, arity);
    attr(py, class, name, callable);
    dec_ref_bits(py, callable);
}

extern "C" fn equal(left: u64, _right: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let (result, mutation, raise) = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let event = if left == state.left_value { 30 } else { 31 };
            state.events.push(event);
            (state.result, state.mutation, state.raise)
        });
        if mutation != 0 {
            let ptr = obj_from_bits(mutation).as_ptr().unwrap();
            let result = if unsafe { object_type_id(ptr) } == TYPE_ID_DICT {
                crate::molt_dict_clear(mutation)
            } else {
                crate::molt_list_clear(mutation)
            };
            dec_ref_bits(py, result);
        }
        #[cfg(feature = "stdlib_collections")]
        {
            let deque = STATE.with(|state| state.borrow().deque);
            if deque != 0 {
                let changed = molt_runtime_collections::collections_ext::molt_deque_setitem(
                    deque,
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(1).bits(),
                );
                dec_ref_bits(py, changed);
            }
        }
        if raise {
            return raise_exception::<_>(py, "ValueError", "comparison consumer sentinel");
        }
        inc_ref_bits(py, result);
        result
    })
}

extern "C" fn getitem(_self: u64, index: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let index = to_i64(obj_from_bits(index)).unwrap();
        STATE.with(|state| state.borrow_mut().events.push(40 + index as u8));
        if index != 0 {
            return raise_exception::<_>(py, "IndexError", "end");
        }
        let value = STATE.with(|state| state.borrow().left_value);
        inc_ref_bits(py, value);
        value
    })
}

extern "C" fn produce() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let value = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.events.push(50);
            state.right_value
        });
        if STATE.with(|state| state.borrow().stop_callable) {
            return raise_exception::<_>(py, "StopIteration", "callable exhausted");
        }
        inc_ref_bits(py, value);
        value
    })
}

fn field_get(py: &PyToken<'_>, object: u64, field: u8) -> u64 {
    let (value, raise) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let left = object == state.left;
        state
            .events
            .push(if left { 10 + field } else { 20 + field });
        (
            if left {
                state.left_value
            } else {
                state.right_value
            },
            state.raise_getter,
        )
    });
    if raise {
        return raise_exception::<_>(py, "ValueError", "field getter sentinel");
    }
    inc_ref_bits(py, value);
    value
}

extern "C" fn field_a(object: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { field_get(py, object, 1) })
}

extern "C" fn field_b(object: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { field_get(py, object, 2) })
}

fn expect_value_error(py: &PyToken<'_>, result: u64) {
    assert!(obj_from_bits(result).is_none());
    assert!(exception_pending(py));
    let exception = molt_exception_last();
    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
        py,
        exception,
        "ValueError"
    ));
    clear_exception(py);
    dec_ref_bits(py, exception);
}

#[test]
fn sequence_and_generic_membership_run_rich_equality_and_stop_on_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        STATE.with(|state| *state.borrow_mut() = State::default());
        let probe_class = class(py, builtin_classes(py).object);
        install(py, probe_class, b"__eq__", equal as *const (), 2);
        let probe = instance(py, probe_class);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.left_value = probe;
            state.result = MoltObject::from_bool(true).bits();
        });
        let needle = MoltObject::from_int(1).bits();
        assert!(obj_from_bits(molt_contains(MoltObject::none().bits(), needle)).is_none());
        assert!(exception_pending(py));
        clear_exception(py);
        let list = MoltObject::from_ptr(alloc_list(py, &[probe])).bits();
        let tuple = MoltObject::from_ptr(alloc_tuple(py, &[probe])).bits();
        for container in [list, tuple] {
            assert_eq!(
                molt_contains(container, needle),
                MoltObject::from_bool(true).bits()
            );
        }
        assert_eq!(
            molt_list_contains(list, needle),
            MoltObject::from_bool(true).bits()
        );
        let iterable_class = class(py, builtin_classes(py).object);
        install(py, iterable_class, b"__getitem__", getitem as *const (), 2);
        let iterable = instance(py, iterable_class);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.raise = true;
            state.events.clear();
        });
        expect_value_error(py, molt_contains(iterable, needle));
        assert_eq!(STATE.with(|state| state.borrow().events.clone()), [40, 30]);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.raise = false;
            state.result = MoltObject::from_bool(false).bits();
            state.mutation = list;
        });
        assert_eq!(
            molt_contains(list, needle),
            MoltObject::from_bool(false).bits()
        );
        assert!(!exception_pending(py));
        STATE.with(|state| state.borrow_mut().mutation = 0);
        for bits in [iterable, iterable_class, tuple, list, probe, probe_class] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn enum_callable_sentinel_and_stdio_do_not_publish_after_equality_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        STATE.with(|state| *state.borrow_mut() = State::default());
        let probe_class = class(py, builtin_classes(py).object);
        install(py, probe_class, b"__eq__", equal as *const (), 2);
        let probe = instance(py, probe_class);
        let returned = instance(py, probe_class);
        let key = attr_name_bits_from_bytes(py, b"member").unwrap();
        let members = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[key, probe])).bits();
        let enum_class = class(py, builtin_classes(py).object);
        attr(py, enum_class, b"__members__", members);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.left_value = probe;
            state.right_value = returned;
            state.result = MoltObject::from_bool(true).bits();
            state.mutation = members;
        });
        let found = crate::builtins::enum_ext::molt_enum_member(enum_class, returned);
        assert_eq!(found, key);
        assert_eq!(
            string_obj_to_owned(obj_from_bits(found)).as_deref(),
            Some("member")
        );
        dec_ref_bits(py, found);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.mutation = 0;
            state.raise = true;
            state.events.clear();
        });
        let callable = function(py, "comparison_consumer_produce", produce as *const (), 0);
        let iter = crate::molt_iter_sentinel(callable, probe);
        expect_value_error(py, crate::molt_iter_next(iter));
        assert_eq!(STATE.with(|state| state.borrow().events.clone()), [50, 30]);
        STATE.with(|state| state.borrow_mut().events.clear());
        let integer = |value| MoltObject::from_int(value).bits();
        let stdio = crate::async_rt::process::molt_asyncio_subprocess_stdio_normalize(
            probe,
            MoltObject::from_bool(true).bits(),
            integer(-1),
            integer(-2),
            integer(-3),
            integer(0),
            integer(1),
            integer(2),
            integer(3),
            integer(4),
            integer(100),
        );
        expect_value_error(py, stdio);
        assert_eq!(STATE.with(|state| state.borrow().events.clone()), [30]);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.raise = false;
            state.events.clear();
        });
        let done = crate::molt_iter_next(iter);
        let again = crate::molt_iter_next(iter);
        assert_eq!(STATE.with(|state| state.borrow().events.clone()), [50, 30]);
        for result in [done, again] {
            let pair = unsafe {
                crate::object::seq_access::tuple_pair(obj_from_bits(result).as_ptr().unwrap())
            }
            .unwrap();
            assert_eq!(pair.1, MoltObject::from_bool(true).bits());
            dec_ref_bits(py, result);
        }
        let stop_iter = crate::molt_iter_sentinel(callable, probe);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.stop_callable = true;
            state.events.clear();
        });
        for _ in 0..2 {
            let done = crate::molt_iter_next(stop_iter);
            assert!(!exception_pending(py));
            let pair = unsafe {
                crate::object::seq_access::tuple_pair(obj_from_bits(done).as_ptr().unwrap())
            }
            .unwrap();
            assert_eq!(pair.1, MoltObject::from_bool(true).bits());
            dec_ref_bits(py, done);
        }
        assert_eq!(STATE.with(|state| state.borrow().events.clone()), [50]);
        for bits in [
            stop_iter,
            iter,
            callable,
            enum_class,
            members,
            key,
            returned,
            probe,
            probe_class,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn dataclass_equality_matches_versioned_operand_order_and_raw_result_contract() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        STATE.with(|state| *state.borrow_mut() = State::default());
        let version = runtime_state(py).sys_version_info.lock().unwrap().clone();
        let probe_class = class(py, builtin_classes(py).object);
        install(py, probe_class, b"__eq__", equal as *const (), 2);
        let left_value = instance(py, probe_class);
        let right_value = instance(py, probe_class);
        let result = MoltObject::from_ptr(alloc_list(py, &[])).bits();
        let record_class = class(py, builtin_classes(py).object);
        let field_class = class(py, builtin_classes(py).object);
        let tag = instance(py, field_class);
        let tag_name = attr_name_bits_from_bytes(py, b"_FIELD").unwrap();
        attr(py, tag, b"name", tag_name);
        dec_ref_bits(py, tag_name);
        let mut field_pairs = Vec::new();
        let mut owned_fields = Vec::new();
        for (name, callback, label) in [
            (
                b"a".as_slice(),
                field_a as *const (),
                "comparison_consumer_field_a",
            ),
            (
                b"b".as_slice(),
                field_b as *const (),
                "comparison_consumer_field_b",
            ),
        ] {
            let field = instance(py, field_class);
            let name_bits = attr_name_bits_from_bytes(py, name).unwrap();
            attr(py, field, b"name", name_bits);
            attr(py, field, b"_field_type", tag);
            attr(py, field, b"compare", MoltObject::from_bool(true).bits());
            field_pairs.extend([name_bits, field]);
            owned_fields.extend([name_bits, field]);
            let getter = function(py, label, callback, 1);
            let property = MoltObject::from_ptr(alloc_property_obj(
                py,
                getter,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            ))
            .bits();
            attr(py, record_class, name, property);
            dec_ref_bits(py, property);
            dec_ref_bits(py, getter);
        }
        let names: Vec<u64> = field_pairs.chunks_exact(2).map(|pair| pair[0]).collect();
        let compare_names = MoltObject::from_ptr(alloc_tuple(py, &names)).bits();
        let fields = MoltObject::from_ptr(alloc_dict_with_pairs(py, &field_pairs)).bits();
        attr(py, record_class, b"__dataclass_fields__", fields);
        // Decoration captured the names before public Field metadata changed.
        for pair in field_pairs.chunks_exact(2) {
            attr(py, pair[1], b"compare", MoltObject::from_bool(false).bits());
        }
        let cleared = crate::molt_dict_clear(fields);
        dec_ref_bits(py, cleared);
        let left = instance(py, record_class);
        let right = instance(py, record_class);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.left = left;
            state.right = right;
            state.left_value = left_value;
            state.right_value = right_value;
            state.result = result;
        });
        for minor in [12, 13, 14] {
            *runtime_state(py).sys_version_info.lock().unwrap() =
                Some(crate::state::runtime_state::PythonVersionInfo {
                    major: 3,
                    minor,
                    micro: 0,
                    releaselevel: "final".into(),
                    serial: 0,
                });
            STATE.with(|state| state.borrow_mut().events.clear());
            let compared = crate::builtins::types::molt_dataclasses_eq(left, right, compare_names);
            assert_eq!(
                compared,
                if minor == 12 {
                    MoltObject::from_bool(false).bits()
                } else {
                    result
                }
            );
            let expected: &[u8] = if minor == 12 {
                &[11, 12, 21, 22, 30]
            } else {
                &[11, 21, 30]
            };
            assert_eq!(STATE.with(|state| state.borrow().events.clone()), expected);
            dec_ref_bits(py, compared);
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.events.clear();
                state.raise_getter = true;
            });
            expect_value_error(
                py,
                crate::builtins::types::molt_dataclasses_eq(left, right, compare_names),
            );
            assert_eq!(STATE.with(|state| state.borrow().events.clone()), [11]);
            STATE.with(|state| state.borrow_mut().raise_getter = false);
        }
        let declined = crate::builtins::types::molt_dataclasses_eq(left, left_value, compare_names);
        assert!(crate::builtins::methods::is_not_implemented_bits(
            py, declined
        ));
        dec_ref_bits(py, declined);
        *runtime_state(py).sys_version_info.lock().unwrap() = version;
        for bits in owned_fields {
            dec_ref_bits(py, bits);
        }
        for bits in [
            right,
            left,
            compare_names,
            fields,
            tag,
            field_class,
            record_class,
            result,
            right_value,
            left_value,
            probe_class,
        ] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

#[cfg(feature = "stdlib_collections")]
#[test]
fn deque_consumers_preserve_comparison_errors_across_the_runtime_bridge() {
    use molt_runtime_collections::collections_ext::*;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        STATE.with(|state| *state.borrow_mut() = State::default());
        let probe_class = class(py, builtin_classes(py).object);
        install(py, probe_class, b"__eq__", equal as *const (), 2);
        let probe = instance(py, probe_class);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.left_value = probe;
            state.raise = true;
        });
        let deque = molt_deque_new(MoltObject::none().bits());
        let appended = molt_deque_append(deque, probe);
        dec_ref_bits(py, appended);
        for operation in [molt_deque_contains, molt_deque_count, molt_deque_remove] {
            expect_value_error(py, operation(deque, MoltObject::from_int(1).bits()));
        }
        expect_value_error(
            py,
            molt_deque_index(
                deque,
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(1).bits(),
            ),
        );
        let dropped = molt_deque_drop(deque);
        dec_ref_bits(py, dropped);
        // Replacing an element preserves the structural version. Every scan
        // must see the replacement at its next position, rather than a snapshot.
        for operation in 0..4 {
            let deque = molt_deque_new(MoltObject::none().bits());
            let _ = molt_deque_append(deque, probe);
            let _ = molt_deque_append(deque, MoltObject::from_int(0).bits());
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.raise = false;
                state.result = MoltObject::from_bool(false).bits();
                state.deque = deque;
            });
            let needle = MoltObject::from_int(1).bits();
            let result = match operation {
                0 => molt_deque_contains(deque, needle),
                1 => molt_deque_count(deque, needle),
                2 => molt_deque_index(
                    deque,
                    needle,
                    MoltObject::from_int(0).bits(),
                    MoltObject::from_int(2).bits(),
                ),
                _ => molt_deque_remove(deque, needle),
            };
            assert!(!exception_pending(py));
            assert_eq!(
                result,
                match operation {
                    0 => MoltObject::from_bool(true).bits(),
                    1 | 2 => MoltObject::from_int(1).bits(),
                    _ => MoltObject::none().bits(),
                }
            );
            dec_ref_bits(py, result);
            STATE.with(|state| state.borrow_mut().deque = 0);
            let dropped = molt_deque_drop(deque);
            dec_ref_bits(py, dropped);
        }
        dec_ref_bits(py, probe);
        dec_ref_bits(py, probe_class);
    });
}
