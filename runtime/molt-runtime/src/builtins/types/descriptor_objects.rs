use super::*;
use crate::object::seq_access::snapshot;
use crate::object::{ClassEdgeOwnership, object_init_class_edge_unpublished};

#[derive(Clone, Copy)]
enum SuperConstructionMode {
    Explicit,
    Implicit,
}

fn super_receiver_class(_py: &PyToken<'_>, type_bits: u64, obj_bits: u64) -> Option<u64> {
    let obj_is_type = obj_from_bits(obj_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE });
    let actual_class = type_of_bits(_py, obj_bits);
    if obj_is_type && issubclass_bits(obj_bits, type_bits) {
        inc_ref_bits(_py, obj_bits);
        return Some(obj_bits);
    }
    if issubclass_bits(actual_class, type_bits) {
        inc_ref_bits(_py, actual_class);
        return Some(actual_class);
    }
    // CPython's supercheck uses the real subtype relationship first, then the
    // receiver's __class__ attribute. Metaclass __instancecheck__ is not involved.
    let name_bits = attr_name_bits_from_bytes(_py, b"__class__")?;
    let claimed_class = molt_getattr_builtin(obj_bits, name_bits, MoltObject::none().bits());
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, claimed_class);
        return None;
    }
    let claimed_is_type = obj_from_bits(claimed_class)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE });
    // The getter can replace the receiver's class and mutate the old class's
    // bases. Compare against the current type, not the pre-callback identity.
    let current_class = type_of_bits(_py, obj_bits);
    if claimed_is_type
        && claimed_class != current_class
        && issubclass_bits(claimed_class, type_bits)
    {
        return Some(claimed_class);
    }
    // Releasing a rejected claim can itself run a finalizer and replace the
    // receiver's class again; diagnostics below observe that later boundary.
    dec_ref_bits(_py, claimed_class);
    let message = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 13) {
        let owner = class_name_for_error(type_bits);
        let received = class_name_for_error(if obj_is_type {
            obj_bits
        } else {
            // The user __class__ getter may have changed the actual type.
            type_of_bits(_py, obj_bits)
        });
        let kind = if obj_is_type { "type" } else { "instance of" };
        format!(
            "super(type, obj): obj ({kind} {received}) is not an instance or subtype of type ({owner})."
        )
    } else {
        "super(type, obj): obj must be an instance or subtype of type".to_owned()
    };
    let _ = raise_exception::<u64>(_py, "TypeError", &message);
    None
}

fn super_construct(
    _py: &PyToken<'_>,
    type_bits: u64,
    obj_bits: u64,
    mode: SuperConstructionMode,
) -> u64 {
    let type_obj = obj_from_bits(type_bits);
    if !type_obj
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
    {
        let got = type_name(_py, type_obj);
        let (exception, message) = match mode {
            SuperConstructionMode::Explicit => (
                "TypeError",
                format!("super() argument 1 must be a type, not {got}"),
            ),
            SuperConstructionMode::Implicit => (
                "RuntimeError",
                format!("super(): __class__ is not a type ({got})"),
            ),
        };
        return raise_exception::<_>(_py, exception, &message);
    }
    // None is the unbound form for explicit and implicit construction alike.
    let obj_bits = if obj_bits == 0 {
        MoltObject::none().bits()
    } else {
        obj_bits
    };
    let receiver_class = if obj_from_bits(obj_bits).is_none() {
        MoltObject::none().bits()
    } else {
        let Some(receiver_class) = super_receiver_class(_py, type_bits, obj_bits) else {
            return MoltObject::none().bits();
        };
        receiver_class
    };
    let ptr = alloc_super_obj(_py, type_bits, obj_bits, receiver_class);
    dec_ref_bits(_py, receiver_class);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_super_new(type_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        super_construct(_py, type_bits, obj_bits, SuperConstructionMode::Explicit)
    })
}

pub(crate) fn super_from_current_frame(_py: &PyToken<'_>) -> u64 {
    use crate::builtins::frames::{PythonArgumentZero, frame_python_context_snapshot};
    use crate::builtins::methods::is_missing_bits;
    let frame = frame_python_context_snapshot(_py);
    let argument_item = match frame.context.argument_zero {
        PythonArgumentZero::NoArgument => {
            return raise_exception::<_>(_py, "RuntimeError", "super(): no arguments");
        }
        PythonArgumentZero::Value(_) => None,
        PythonArgumentZero::Cell(bits) => Some(unsafe {
            crate::object::cells::pin_cell_value(
                _py,
                obj_from_bits(bits)
                    .as_ptr()
                    .expect("validated frame argument cell"),
            )
        }),
    };
    let argument_bits = match frame.context.argument_zero {
        PythonArgumentZero::Value(bits) => Some(bits),
        PythonArgumentZero::Cell(_) => argument_item.as_ref().map(|item| item.bits()),
        PythonArgumentZero::NoArgument => unreachable!(),
    };
    let Some(argument_bits) = argument_bits.filter(|bits| !is_missing_bits(_py, *bits)) else {
        return raise_exception::<_>(_py, "RuntimeError", "super(): arg[0] deleted");
    };
    let Some(cell_bits) = frame.context.class_cell_bits else {
        return raise_exception::<_>(_py, "RuntimeError", "super(): __class__ cell not found");
    };
    let class_item = unsafe {
        crate::object::cells::pin_cell_value(
            _py,
            obj_from_bits(cell_bits)
                .as_ptr()
                .expect("validated frame class cell"),
        )
    };
    if is_missing_bits(_py, class_item.bits()) {
        return raise_exception::<_>(_py, "RuntimeError", "super(): empty __class__ cell");
    }
    super_construct(
        _py,
        class_item.bits(),
        argument_bits,
        SuperConstructionMode::Implicit,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_super_from_frame() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { super_from_current_frame(_py) })
}

/// One arity/keyword and frame-observation authority for all builtin call transports.
pub(crate) fn super_call(_py: &PyToken<'_>, args: &[u64], has_keywords: bool) -> u64 {
    if has_keywords {
        return raise_exception::<_>(_py, "TypeError", "super() takes no keyword arguments");
    }
    match args {
        [] => super_from_current_frame(_py),
        [type_bits] => super_construct(
            _py,
            *type_bits,
            MoltObject::none().bits(),
            SuperConstructionMode::Explicit,
        ),
        [type_bits, obj_bits] => {
            super_construct(_py, *type_bits, *obj_bits, SuperConstructionMode::Explicit)
        }
        _ => raise_exception::<_>(
            _py,
            "TypeError",
            &format!("super() expected at most 2 arguments, got {}", args.len()),
        ),
    }
}

#[cfg(test)]
mod super_frame_tests {
    use super::*;
    use crate::builtins::frames::{
        PythonArgumentZero, PythonFrameContext, frame_stack_pop, frame_stack_push,
        frame_stack_set_python_context,
    };

    fn cell(py: &PyToken<'_>, bits: u64) -> u64 {
        let ptr = crate::object::cells::alloc_cell(py, bits);
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn assert_error(py: &PyToken<'_>, result: u64, kind: &str, expected: &str) {
        assert!(obj_from_bits(result).is_none());
        let error = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            py, error, kind
        ));
        let message = crate::format_exception_message(py, obj_from_bits(error).as_ptr().unwrap());
        assert!(message.contains(expected), "{message}");
        crate::molt_exception_clear();
        dec_ref_bits(py, error);
        dec_ref_bits(py, result);
    }

    #[test]
    fn executing_frame_super_uses_cpython_error_precedence() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            frame_stack_push(py, 0);
            let missing = missing_bits(py);
            let empty_class_cell = cell(py, missing);
            let invalid_class_cell = cell(py, MoltObject::from_int(42).bits());
            let missing_argument_cell = cell(py, missing);
            let value = MoltObject::none().bits();
            let cases = [
                (
                    PythonArgumentZero::NoArgument,
                    Some(invalid_class_cell),
                    "super(): no arguments",
                ),
                (
                    PythonArgumentZero::Value(missing),
                    None,
                    "super(): arg[0] deleted",
                ),
                (
                    PythonArgumentZero::Cell(missing_argument_cell),
                    Some(invalid_class_cell),
                    "super(): arg[0] deleted",
                ),
                (
                    PythonArgumentZero::Value(value),
                    None,
                    "super(): __class__ cell not found",
                ),
                (
                    PythonArgumentZero::Value(value),
                    Some(empty_class_cell),
                    "super(): empty __class__ cell",
                ),
                (
                    PythonArgumentZero::Value(value),
                    Some(invalid_class_cell),
                    "super(): __class__ is not a type (int)",
                ),
            ];
            for (argument_zero, class_cell_bits, message) in cases {
                assert!(frame_stack_set_python_context(
                    py,
                    PythonFrameContext {
                        argument_zero,
                        class_cell_bits
                    }
                ));
                assert_error(py, molt_super_from_frame(), "RuntimeError", message);
            }
            frame_stack_pop(py);
            for bits in [empty_class_cell, invalid_class_cell, missing_argument_cell] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn indirect_and_bound_super_calls_share_live_cell_context() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let classes = builtin_classes(py);
            let argument_cell = cell(py, MoltObject::from_int(1).bits());
            let class_cell = cell(py, classes.object);
            frame_stack_push(py, 0);
            assert!(frame_stack_set_python_context(
                py,
                PythonFrameContext {
                    argument_zero: PythonArgumentZero::Cell(argument_cell),
                    class_cell_bits: Some(class_cell),
                }
            ));
            unsafe {
                let direct = call_callable0(py, classes.super_type);
                assert!(!exception_pending(py));
                let direct_ptr = obj_from_bits(direct).as_ptr().unwrap();
                assert_eq!(
                    crate::object::layout::super_receiver_class_bits(direct_ptr),
                    classes.int
                );
                dec_ref_bits(py, direct);
                // Mutate the existing argument cell; no snapshot republishing is needed.
                let changed = crate::molt_cell_set(argument_cell, MoltObject::none().bits());
                assert!(!exception_pending(py));
                assert!(obj_from_bits(changed).is_none());
                let args = molt_callargs_new(0, 0);
                let bound = molt_call_bind(classes.super_type, args);
                assert!(!exception_pending(py));
                let bound_ptr = obj_from_bits(bound).as_ptr().unwrap();
                assert!(
                    obj_from_bits(crate::object::layout::super_receiver_class_bits(bound_ptr))
                        .is_none()
                );
                dec_ref_bits(py, bound);
                let changed = crate::molt_cell_set(class_cell, MoltObject::from_int(42).bits());
                assert!(!exception_pending(py));
                assert!(obj_from_bits(changed).is_none());
                assert_error(
                    py,
                    call_callable0(py, classes.super_type),
                    "RuntimeError",
                    "super(): __class__ is not a type (int)",
                );
                for bits in [argument_cell, class_cell] {
                    let ptr = obj_from_bits(bits).as_ptr().unwrap();
                    assert_eq!(
                        (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot(),
                        2
                    );
                }
            }
            frame_stack_pop(py);
            for bits in [argument_cell, class_cell] {
                let ptr = obj_from_bits(bits).as_ptr().unwrap();
                assert_eq!(
                    unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() },
                    1,
                );
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn super_receiver_class_is_retained_and_exposed() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let classes = builtin_classes(py);
            let receiver = MoltObject::from_int(3).bits();
            let result = molt_super_new(classes.object, receiver);
            assert!(!exception_pending(py));
            let repr = crate::molt_repr_from_obj(result);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(repr)).as_deref(),
                Some("<super: <class 'object'>, <int object>>"),
            );
            dec_ref_bits(py, repr);
            for (name, expected) in [
                (b"__thisclass__".as_slice(), classes.object),
                (b"__self__".as_slice(), receiver),
                (b"__self_class__".as_slice(), classes.int),
            ] {
                let name_bits = attr_name_bits_from_bytes(py, name).unwrap();
                let actual = crate::molt_get_attr_name(result, name_bits);
                assert!(!exception_pending(py));
                assert_eq!(actual, expected);
                dec_ref_bits(py, actual);
                dec_ref_bits(py, name_bits);
            }
            dec_ref_bits(py, result);
            assert_error(
                py,
                molt_super_new(MoltObject::from_int(42).bits(), receiver),
                "TypeError",
                "super() argument 1 must be a type, not int",
            );
        });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bootstrap_descriptor_types() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                builtins.classmethod,
                builtins.staticmethod,
                builtins.property,
            ],
        );
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generic_alias_new(origin_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args_obj = obj_from_bits(args_bits);
        // Always create a fresh heap-allocated args tuple.  This is
        // necessary because the incoming tuple may be stack-allocated
        // (from the Cranelift stack-tuple optimisation) and would become
        // a dangling pointer once the caller's stack frame is unwound.
        // Copying the elements into a new heap tuple is cheap and safe.
        let args_tuple_bits = if let Some(args_ptr) = args_obj.as_ptr() {
            unsafe {
                if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                    let Some(elems) = snapshot(
                        _py,
                        args_ptr,
                        "GenericAlias argument tuple allocation failed",
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let new_ptr = alloc_tuple(_py, &elems);
                    if new_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(new_ptr).bits()
                } else {
                    let tuple_ptr = alloc_tuple(_py, &[args_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            }
        } else {
            let tuple_ptr = alloc_tuple(_py, &[args_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        };
        let ptr = alloc_generic_alias(_py, origin_bits, args_tuple_bits);
        // The new tuple was created above; dec_ref since alloc_generic_alias
        // inc_refs it.
        dec_ref_bits(_py, args_tuple_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generic_alias_mro_entries(alias_bits: u64, _bases_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(alias_ptr) = obj_from_bits(alias_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "GenericAlias.__mro_entries__ expected GenericAlias",
            );
        };
        unsafe {
            if object_type_id(alias_ptr) != TYPE_ID_GENERIC_ALIAS {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "GenericAlias.__mro_entries__ expected GenericAlias",
                );
            }
            let origin_bits = generic_alias_origin_bits(alias_ptr);
            let tuple_ptr = alloc_tuple(_py, &[origin_bits]);
            if tuple_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generic_alias_type_new(
    cls_bits: u64,
    origin_bits: u64,
    args_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "GenericAlias.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "GenericAlias.__new__ expects type");
            }
        }
        let builtins = builtin_classes(_py);
        let is_generic_alias_subtype =
            cls_bits == builtins.generic_alias || issubclass_bits(cls_bits, builtins.generic_alias);
        if !is_generic_alias_subtype {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "GenericAlias.__new__ expected GenericAlias subtype",
            );
        }

        let out_bits = molt_generic_alias_new(origin_bits, args_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(out_ptr) = obj_from_bits(out_bits).as_ptr() else {
            return out_bits;
        };
        unsafe {
            if !object_init_class_edge_unpublished(
                _py,
                out_ptr,
                cls_bits,
                ClassEdgeOwnership::Owned,
            ) {
                dec_ref_bits(_py, out_bits);
                return MoltObject::none().bits();
            }
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_typing_type_param(typevar_ctor_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "type parameter name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "type parameter name must be str");
            }
        }
        let builder_bits = molt_callargs_new(1, 0);
        if builder_bits == 0 {
            return MoltObject::none().bits();
        }
        unsafe {
            let _ = molt_callargs_push_pos(builder_bits, name_bits);
        }
        let typevar_bits = molt_call_bind(typevar_ctor_bits, builder_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(flag_name_bits) = attr_name_bits_from_bytes(_py, b"_pep695") else {
            return MoltObject::none().bits();
        };
        let _ = molt_object_setattr(
            typevar_bits,
            flag_name_bits,
            MoltObject::from_bool(true).bits(),
        );
        dec_ref_bits(_py, flag_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        typevar_bits
    })
}
