//! Class construction and cooperative subclass initialization.
//!
//! State stays with its existing owner; calls preserve transaction and drop order.

use super::*;

pub(crate) unsafe fn dispatch_init_subclass_hooks(
    _py: &PyToken<'_>,
    class_bits: u64,
    kw_names: &[u64],
    kw_values: &[u64],
) -> bool {
    unsafe {
        // Own keyword values before descriptor resolution can run user code.
        // The class keywords arrive as one validated mapping's entries.
        let arguments = match CallArguments::retained(_py, None, &[], kw_names, kw_values) {
            Ok(arguments) => arguments,
            Err(_) => return false,
        };
        let init_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.init_subclass_name,
            b"__init_subclass__",
        );
        if exception_pending(_py) {
            return false;
        }
        // The constructor has proved both arguments are the same live type.
        // Use the existing super/descriptor authority, not a second MRO walk:
        // one inherited hook owns cooperative dispatch to the remaining bases.
        let super_ptr =
            crate::object::builders::alloc_super_obj(_py, class_bits, class_bits, class_bits);
        if super_ptr.is_null() {
            return false;
        }
        let _super_owner = PtrDropGuard::new(super_ptr);
        let init_bits =
            crate::molt_get_attr_name(MoltObject::from_ptr(super_ptr).bits(), init_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, init_bits);
            return false;
        }
        // Descriptor binding already supplied the new class receiver.
        let result = call_bind_with_arguments(_py, init_bits, arguments);
        crate::call::discard_owned_call_result(_py, result);
        dec_ref_bits(_py, init_bits);
        !exception_pending(_py)
    }
}

fn trace_call_type_builder_enabled_raw(raw: Option<&str>) -> bool {
    raw == Some("1")
}

fn trace_call_type_builder_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        trace_call_type_builder_enabled_raw(
            std::env::var("MOLT_TRACE_CALL_TYPE_BUILDER")
                .ok()
                .as_deref(),
        )
    })
}

pub(super) unsafe fn is_default_type_call(_py: &PyToken<'_>, call_bits: u64) -> bool {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return false;
        };
        match object_type_id(call_ptr) {
            TYPE_ID_BOUND_METHOD => {
                let func_bits = bound_method_func_bits(call_ptr);
                is_default_type_call(_py, func_bits)
            }
            TYPE_ID_FUNCTION => crate::builtins::functions::runtime_callable_represents_symbol(
                function_fn_ptr(call_ptr),
                function_trampoline_ptr(call_ptr),
                fn_key!(molt_type_call),
            ),
            _ => false,
        }
    }
}

/// Class construction lends the call's argument vector to `__new__` and
/// `__init__`, as `type.__call__` does: each phase retains its own vector.
/// A class never inlines a frame, so the call instruction releases the
/// construction arguments once construction ends, in its own order.
pub(super) unsafe fn call_type_with_arguments(
    _py: &PyToken<'_>,
    call_ptr: *mut u8,
    mut arguments: CallArguments<'_, '_>,
) -> u64 {
    unsafe {
        let class_bits = MoltObject::from_ptr(call_ptr).bits();
        let builtins = builtin_classes(_py);
        let args = match arguments.unpacked_view() {
            Ok(args) => args,
            Err(err) => return err,
        };
        let pos_args = args.pos;
        let kw_names = args.kw_names;
        let kw_values = args.kw_values;
        if class_bits == builtins.type_obj && pos_args.len() == 3 {
            return build_class_from_args(
                _py,
                class_bits,
                pos_args[0],
                pos_args[1],
                pos_args[2],
                kw_names,
                kw_values,
            );
        }
        // Custom metaclass (subclass of type) with 3 args:
        // Meta(name, bases, namespace).  CPython's `type.__call__` dispatches
        // to `Meta.__new__(Meta, name, bases, namespace, **kwds)` and then
        // `Meta.__init__(cls, name, bases, namespace, **kwds)`.  Honor user
        // overrides of either method.
        if pos_args.len() == 3 && issubclass_bits(class_bits, builtins.type_obj) {
            // Build the kwargs dict once; reused for the fast path
            // (`molt_type_new`) and to dec-ref at exit.
            let kwargs_bits = if kw_names.is_empty() {
                MoltObject::none().bits()
            } else {
                let mut pairs = Vec::with_capacity(kw_names.len() * 2);
                for (k, v) in kw_names.iter().zip(kw_values.iter()) {
                    pairs.push(*k);
                    pairs.push(*v);
                }
                let ptr = alloc_dict_with_pairs(_py, &pairs);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(ptr).bits()
            };

            // Look up `__new__` on the metaclass.  If the user did not
            // override it, the lookup resolves to the inherited
            // `type.__new__` (intrinsic `molt_type_new`); use the fast
            // path that also runs `__init_subclass__` and class slot
            // setup inline.  Otherwise dispatch to the user's override.
            let new_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
            let new_lookup = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits);
            let new_is_default = new_lookup
                .map(|bits| {
                    let obj = obj_from_bits(bits);
                    let Some(p) = obj.as_ptr() else { return true };
                    if object_type_id(p) != TYPE_ID_FUNCTION {
                        return false;
                    }
                    function_fn_ptr(p) == fn_key!(molt_type_new)
                })
                .unwrap_or(true);

            // `class_attr_lookup_raw_mro` returns borrowed bits.  Match
            // the OLD code path's lifetime contract: never dec-ref the
            // looked-up function bits.
            let new_class_bits = if new_is_default {
                molt_type_new(
                    class_bits,
                    pos_args[0],
                    pos_args[1],
                    pos_args[2],
                    kwargs_bits,
                )
            } else {
                let new_bits = new_lookup.expect("non-default __new__ must resolve");
                // `type.__call__` lends its arguments to each constructor
                // phase; the phase retains its own argument vector.
                match CallArguments::retained(_py, Some(class_bits), pos_args, kw_names, kw_values)
                {
                    Ok(new_arguments) => call_bind_with_arguments(_py, new_bits, new_arguments),
                    Err(err) => {
                        if !kw_names.is_empty() {
                            dec_ref_bits(_py, kwargs_bits);
                        }
                        return err;
                    }
                }
            };

            if exception_pending(_py) {
                if !kw_names.is_empty() {
                    dec_ref_bits(_py, kwargs_bits);
                }
                return MoltObject::none().bits();
            }

            // CPython: only invoke `__init__` when `__new__` returned an
            // instance of `cls` (here, of the metaclass).  This matches
            // `type.__call__` semantics.
            let new_class_obj = obj_from_bits(new_class_bits);
            let returned_instance = if let Some(p) = new_class_obj.as_ptr() {
                let inst_class_bits = object_class_bits(p);
                inst_class_bits != 0 && issubclass_bits(inst_class_bits, class_bits)
            } else {
                false
            };

            if returned_instance {
                // Call Meta.__init__(new_class, name, bases, namespace, **kwds).
                // `class_attr_lookup_raw_mro` returns borrowed bits — do
                // not dec-ref.
                let init_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
                if let Some(init_bits) = class_attr_lookup_raw_mro(_py, call_ptr, init_name_bits) {
                    let init_result = match CallArguments::retained(
                        _py,
                        Some(new_class_bits),
                        pos_args,
                        kw_names,
                        kw_values,
                    ) {
                        Ok(init_arguments) => {
                            call_bind_with_arguments(_py, init_bits, init_arguments)
                        }
                        Err(err) => err,
                    };
                    // A failed allocation or `__init__` leaves the error pending;
                    // consuming the result reports both the same way.
                    if !crate::call::class_init::consume_init_result(_py, init_result) {
                        dec_ref_bits(_py, new_class_bits);
                        if !kw_names.is_empty() {
                            dec_ref_bits(_py, kwargs_bits);
                        }
                        return MoltObject::none().bits();
                    }
                }
            }

            if !kw_names.is_empty() {
                dec_ref_bits(_py, kwargs_bits);
            }
            return new_class_bits;
        }
        if class_bits == builtins.type_obj && pos_args.len() == 1 && kw_names.is_empty() {
            let bits = type_of_bits(_py, pos_args[0]);
            inc_ref_bits(_py, bits);
            return bits;
        }
        if is_builtin_class_bits(_py, class_bits)
            && crate::object::class_is_immutable(_py, call_ptr)
            && class_bits != builtins.module
            && !crate::builtins::types::native_constructors::owns_constructor_descriptors(
                _py, class_bits,
            )
        {
            if let Some(result) = crate::builtins::types::wrappers::try_construct_exact_wrapper(
                _py, class_bits, pos_args, kw_names, kw_values,
            ) {
                return result;
            }

            if class_bits == builtins.super_type {
                return crate::builtins::types::descriptor_objects::super_call(
                    _py,
                    pos_args,
                    !kw_names.is_empty(),
                );
            }

            if class_bits == builtins.enumerate {
                if pos_args.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "enumerate() missing required argument 'iterable' (pos 1)",
                    );
                }
                if pos_args.len() > 2 {
                    let msg = format!(
                        "enumerate expected at most 2 arguments, got {}",
                        pos_args.len()
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let iterable_bits = pos_args[0];
                let mut start_opt = if pos_args.len() == 2 {
                    Some(pos_args[1])
                } else {
                    None
                };
                for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
                    let name = string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<name>".to_string());
                    if name != "start" {
                        let msg =
                            format!("enumerate() got an unexpected keyword argument '{name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if start_opt.is_some() {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "enumerate() got multiple values for argument 'start'",
                        );
                    }
                    start_opt = Some(val_bits);
                }
                return crate::object::ops::enumerate_new_impl(_py, iterable_bits, start_opt);
            }

            if class_bits == builtins.bool {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "bool() takes no keyword arguments",
                    );
                }
                if pos_args.len() > 1 {
                    let msg = format!("bool expected at most 1 argument, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if pos_args.is_empty() {
                    return MoltObject::from_bool(false).bits();
                }
                let result = is_truthy(_py, obj_from_bits(pos_args[0]));
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_bool(result).bits();
            }

            if class_bits == builtins.reversed {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "reversed() takes no keyword arguments",
                    );
                }
                if pos_args.len() != 1 {
                    let msg = format!("reversed expected 1 argument, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                return crate::object::ops::reversed_new_impl(_py, pos_args[0]);
            }

            if class_bits == builtins.map {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "map() takes no keyword arguments",
                    );
                }
                if pos_args.len() < 2 {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "map() must have at least two arguments",
                    );
                }
                return crate::object::ops::map_new_impl(_py, pos_args[0], &pos_args[1..]);
            }

            if class_bits == builtins.filter {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "filter() takes no keyword arguments",
                    );
                }
                if pos_args.len() != 2 {
                    let msg = format!("filter expected 2 arguments, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                return crate::object::ops::filter_new_impl(_py, pos_args[0], pos_args[1]);
            }

            if class_bits == builtins.zip {
                let mut strict = false;
                for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
                    let name = string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<name>".to_string());
                    if name != "strict" {
                        let msg = format!("zip() got an unexpected keyword argument '{name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    strict = is_truthy(_py, obj_from_bits(val_bits));
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                return crate::object::ops::zip_new_impl(_py, pos_args, strict);
            }

            if class_bits == builtins.text_io_wrapper && !kw_names.is_empty() {
                if let Some(bound_args) =
                    builtin_args::bind_builtin_class_text_io_wrapper(_py, &args)
                {
                    return call_class_init_with_args(_py, call_ptr, &bound_args);
                }
                return MoltObject::none().bits();
            }
            if class_bits == builtins.string_io && !kw_names.is_empty() {
                if let Some(bound_args) = builtin_args::bind_builtin_class_string_io(_py, &args) {
                    return call_class_init_with_args(_py, call_ptr, &bound_args);
                }
                return MoltObject::none().bits();
            }
            if !kw_names.is_empty() {
                let class_name = class_name_for_error(class_bits);
                let msg = format!("{class_name}() takes no keyword arguments");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            return call_class_init_with_args(_py, call_ptr, pos_args);
        }
        let is_exc_subclass = issubclass_bits(class_bits, builtins.base_exception);
        if trace_call_type_builder_enabled() {
            let class_name = class_name_for_error(class_bits);
            eprintln!(
                "[DEBUG] call_type_with_arguments: class={} bits={:#x} is_exc_subclass={}",
                class_name, class_bits, is_exc_subclass
            );
        }
        if is_exc_subclass {
            return crate::call::class_init::construct_exception_from_args(
                _py, call_ptr, pos_args, kw_names, kw_values,
            );
        }
        crate::call::class_init::construct_regular_class(
            _py, call_ptr, pos_args, kw_names, kw_values,
        )
    }
}

unsafe fn build_class_from_args(
    _py: &PyToken<'_>,
    metaclass_bits: u64,
    name_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    kw_names: &[u64],
    kw_values: &[u64],
) -> u64 {
    unsafe {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        };
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        }

        let mut bases_vec: Vec<u64> = Vec::new();
        let mut bases_tuple_bits = bases_bits;
        let mut bases_owned = false;
        if obj_from_bits(bases_bits).is_none() || bases_bits == 0 {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            bases_owned = true;
        } else if let Some(bases_ptr) = obj_from_bits(bases_bits).as_ptr() {
            match object_type_id(bases_ptr) {
                TYPE_ID_TUPLE => {
                    bases_vec =
                        crate::object::seq_access::with_borrowed(bases_ptr, |bases| bases.to_vec());
                }
                TYPE_ID_TYPE => {
                    let tuple_ptr = alloc_tuple(_py, &[bases_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                    bases_owned = true;
                    bases_vec.push(bases_bits);
                }
                _ => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "bases must be a tuple of types",
                    );
                }
            }
        }

        if bases_vec.is_empty() {
            let builtins = builtin_classes(_py);
            let tuple_ptr = alloc_tuple(_py, &[builtins.object]);
            if tuple_ptr.is_null() {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            bases_owned = true;
            bases_vec.push(builtins.object);
        }

        let mut winner_bits = metaclass_bits;
        for base_bits in bases_vec.iter().copied() {
            let base_meta_bits = type_of_bits(_py, base_bits);
            if issubclass_bits(winner_bits, base_meta_bits) {
                continue;
            }
            if issubclass_bits(base_meta_bits, winner_bits) {
                winner_bits = base_meta_bits;
                continue;
            }
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the metaclasses of all its bases",
            );
        }

        if winner_bits != metaclass_bits {
            // The winning metaclass receives its own retained argument vector;
            // this adapter keeps its borrowed operands.
            let class_bits = match CallArguments::retained(
                _py,
                None,
                &[name_bits, bases_tuple_bits, namespace_bits],
                kw_names,
                kw_values,
            ) {
                Ok(arguments) => call_bind_with_arguments(_py, winner_bits, arguments),
                Err(err) => err,
            };
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return class_bits;
        }

        // Metaclass selection is the adapter's only construction policy.
        // The canonical type constructor owns namespace copying, metadata cells,
        // unpublished-class cleanup, slots, and the ordered callback phases.
        let kwargs_bits = if kw_names.is_empty() {
            MoltObject::none().bits()
        } else {
            let pairs: Vec<u64> = kw_names
                .iter()
                .zip(kw_values.iter())
                .flat_map(|(&name, &value)| [name, value])
                .collect();
            let kwargs = alloc_dict_with_pairs(_py, &pairs);
            if kwargs.is_null() {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(kwargs).bits()
        };
        let result = molt_type_new(
            metaclass_bits,
            name_bits,
            bases_tuple_bits,
            namespace_bits,
            kwargs_bits,
        );
        if !kw_names.is_empty() {
            dec_ref_bits(_py, kwargs_bits);
        }
        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        result
    }
}

#[cfg(test)]
#[path = "class_constructor_tests.rs"]
mod class_constructor_tests;

#[cfg(test)]
#[path = "constructors_binding_tests.rs"]
mod tests;
