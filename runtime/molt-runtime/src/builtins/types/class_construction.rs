use super::*;

struct PreparedClassState {
    metaclass_bits: u64,
    namespace_bits: u64,
    kwds_bits: u64,
}

pub(crate) fn call_vararg_args(
    _py: &PyToken<'_>,
    func_name: &str,
    args_bits: u64,
) -> Option<Vec<u64>> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        let msg = format!("{func_name}() expects positional arguments tuple");
        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
        return None;
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            let msg = format!("{func_name}() expects positional arguments tuple");
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return None;
        }
    }
    Some(unsafe { seq_vec_ref(args_ptr) }.clone())
}

pub(crate) fn call_vararg_kwargs(
    _py: &PyToken<'_>,
    func_name: &str,
    kwargs_bits: u64,
) -> Option<(*mut u8, Vec<(String, u64)>)> {
    let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() else {
        let msg = format!("{func_name}() expects keyword arguments dict");
        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
        return None;
    };
    unsafe {
        if object_type_id(kwargs_ptr) != TYPE_ID_DICT {
            let msg = format!("{func_name}() expects keyword arguments dict");
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return None;
        }
    }
    let order = unsafe { dict_order(kwargs_ptr) }.clone();
    let mut entries = Vec::with_capacity(order.len() / 2);
    for pair in order.chunks(2) {
        if pair.len() != 2 {
            continue;
        }
        let key_name = match string_obj_to_owned(obj_from_bits(pair[0])) {
            Some(name) => name,
            None => {
                let _ = raise_exception::<u64>(_py, "TypeError", "keywords must be strings");
                return None;
            }
        };
        entries.push((key_name, pair[1]));
    }
    Some((kwargs_ptr, entries))
}

fn copy_kwds_mapping(_py: &PyToken<'_>, kwds_bits: u64) -> Option<u64> {
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    if !obj_from_bits(kwds_bits).is_none() {
        unsafe {
            let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, kwds_bits);
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, dict_bits);
            return None;
        }
    }
    Some(dict_bits)
}

pub(crate) fn call_with_kwargs(
    _py: &PyToken<'_>,
    callable_bits: u64,
    positional: &[u64],
    kwargs_bits: u64,
) -> u64 {
    let mut kw_pairs: Vec<(u64, u64)> = Vec::new();
    if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
        unsafe {
            if object_type_id(kwargs_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "keyword arguments must be a dict");
            }
            let order = dict_order(kwargs_ptr).clone();
            for pair in order.chunks(2) {
                if pair.len() == 2 {
                    kw_pairs.push((pair[0], pair[1]));
                }
            }
        }
    }
    let builder_bits = molt_callargs_new(
        (positional.len() + kw_pairs.len()) as u64,
        kw_pairs.len() as u64,
    );
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    for val_bits in positional.iter().copied() {
        unsafe {
            let _ = molt_callargs_push_pos(builder_bits, val_bits);
        }
    }
    for (name_bits, val_bits) in kw_pairs.iter().copied() {
        unsafe {
            let _ = molt_callargs_push_kw(builder_bits, name_bits, val_bits);
        }
    }
    molt_call_bind(callable_bits, builder_bits)
}

fn resolve_bases_impl(_py: &PyToken<'_>, bases_bits: u64) -> u64 {
    let Some(bases_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, bases_bits) }) else {
        return MoltObject::none().bits();
    };
    let Some(bases_tuple_ptr) = obj_from_bits(bases_tuple_bits).as_ptr() else {
        dec_ref_bits(_py, bases_tuple_bits);
        return MoltObject::none().bits();
    };
    let bases = unsafe { seq_vec_ref(bases_tuple_ptr) }.clone();
    let Some(mro_entries_name_bits) = attr_name_bits_from_bytes(_py, b"__mro_entries__") else {
        dec_ref_bits(_py, bases_tuple_bits);
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let mut out: Vec<u64> = Vec::new();
    let mut updated = false;
    // Keep any temporary tuples (including bases_tuple_bits and __mro_entries__ results) alive
    // until we materialize the final resolved bases tuple. Otherwise, elements that are only
    // referenced by those temporary tuples may be freed, leaving dangling bits in `out`.
    let mut keepalive_tuples: Vec<u64> = Vec::new();

    for (idx, base_bits) in bases.iter().copied().enumerate() {
        let is_type = obj_from_bits(base_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE });
        if is_type {
            if updated {
                out.push(base_bits);
            }
            continue;
        }
        let mro_entries_bits = molt_getattr_builtin(base_bits, mro_entries_name_bits, missing);
        if exception_pending(_py) {
            for bits in keepalive_tuples.drain(..) {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, mro_entries_name_bits);
            dec_ref_bits(_py, bases_tuple_bits);
            return MoltObject::none().bits();
        }
        if mro_entries_bits == missing {
            if updated {
                out.push(base_bits);
            }
            continue;
        }
        if !updated {
            out.extend_from_slice(&bases[..idx]);
            updated = true;
        }
        let resolved_bits = unsafe { call_callable1(_py, mro_entries_bits, bases_bits) };
        dec_ref_bits(_py, mro_entries_bits);
        if exception_pending(_py) {
            for bits in keepalive_tuples.drain(..) {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, mro_entries_name_bits);
            dec_ref_bits(_py, bases_tuple_bits);
            return MoltObject::none().bits();
        }
        let Some(resolved_ptr) = obj_from_bits(resolved_bits).as_ptr() else {
            for bits in keepalive_tuples.drain(..) {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, mro_entries_name_bits);
            dec_ref_bits(_py, bases_tuple_bits);
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(resolved_ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, resolved_bits);
                for bits in keepalive_tuples.drain(..) {
                    dec_ref_bits(_py, bits);
                }
                dec_ref_bits(_py, mro_entries_name_bits);
                dec_ref_bits(_py, bases_tuple_bits);
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "__mro_entries__ must return a tuple",
                );
            }
            out.extend_from_slice(seq_vec_ref(resolved_ptr));
        }
        // Keep the returned tuple alive until after we allocate the final output tuple.
        keepalive_tuples.push(resolved_bits);
    }

    dec_ref_bits(_py, mro_entries_name_bits);
    if !updated {
        dec_ref_bits(_py, bases_tuple_bits);
        inc_ref_bits(_py, bases_bits);
        return bases_bits;
    }
    let out_ptr = alloc_tuple(_py, out.as_slice());
    // Now that `out_ptr` has taken ownership (via inc-refs) of the elements, we can drop the
    // temporary tuples that were keeping the elements alive.
    for bits in keepalive_tuples.drain(..) {
        dec_ref_bits(_py, bits);
    }
    dec_ref_bits(_py, bases_tuple_bits);
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

fn prepare_class_impl(
    _py: &PyToken<'_>,
    name_bits: u64,
    bases_bits: u64,
    kwds_bits: u64,
) -> Option<PreparedClassState> {
    let Some(name_ptr) = obj_from_bits(name_bits).as_ptr() else {
        let _ = raise_exception::<u64>(_py, "TypeError", "class name must be str");
        return None;
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            let _ = raise_exception::<u64>(_py, "TypeError", "class name must be str");
            return None;
        }
    }

    let kwds_copy_bits = copy_kwds_mapping(_py, kwds_bits)?;
    let Some(kwds_copy_ptr) = obj_from_bits(kwds_copy_bits).as_ptr() else {
        dec_ref_bits(_py, kwds_copy_bits);
        return None;
    };
    unsafe {
        if object_type_id(kwds_copy_ptr) != TYPE_ID_DICT {
            dec_ref_bits(_py, kwds_copy_bits);
            let _ = raise_exception::<u64>(_py, "TypeError", "kwds must be a mapping");
            return None;
        }
    }

    let Some(metaclass_name_bits) = attr_name_bits_from_bytes(_py, b"metaclass") else {
        dec_ref_bits(_py, kwds_copy_bits);
        return None;
    };
    let metaclass_bits = unsafe { dict_get_in_place(_py, kwds_copy_ptr, metaclass_name_bits) };
    if let Some(bits) = metaclass_bits {
        // `dict_get_in_place` returns a borrowed reference into `kwds_copy`.
        // The following `dict_del_in_place` drops the dict's strong reference to
        // this value, which frees the object outright when the dict held the
        // only reference (the common `prepare_class(name, (), {'metaclass':
        // Factory()})` literal case). Acquire an owned reference *before* the
        // delete so the metaclass survives — without this, `winner_bits` below
        // would dangle and `__prepare__` would be read from freed memory (a
        // use-after-free that release-mode codegen happened to mask while
        // dev-mode codegen surfaced as a missing namespace key).
        inc_ref_bits(_py, bits);
        unsafe {
            dict_del_in_place(_py, kwds_copy_ptr, metaclass_name_bits);
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, bits);
            dec_ref_bits(_py, metaclass_name_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
    }
    dec_ref_bits(_py, metaclass_name_bits);

    // `winner_bits` is held as an OWNED reference for the whole function and is
    // returned as an owned reference in `PreparedClassState.metaclass_bits`
    // (both callers decref it, symmetric with `namespace_bits`/`kwds_bits`).
    // The dict branch is already owned (incref'd above); the registry-backed
    // branches return borrowed references, so incref them to make ownership
    // uniform.
    let mut winner_bits = if let Some(bits) = metaclass_bits {
        bits
    } else if is_truthy(_py, obj_from_bits(bases_bits)) {
        let index_zero = MoltObject::from_int(0).bits();
        let first_base_bits = molt_index(bases_bits, index_zero);
        if exception_pending(_py) {
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        let bits = type_of_bits(_py, first_base_bits);
        inc_ref_bits(_py, bits);
        dec_ref_bits(_py, first_base_bits);
        bits
    } else {
        let bits = builtin_classes(_py).type_obj;
        inc_ref_bits(_py, bits);
        bits
    };

    let winner_is_type = obj_from_bits(winner_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE });
    if winner_is_type {
        let Some(bases_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, bases_bits) }) else {
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        };
        let Some(bases_tuple_ptr) = obj_from_bits(bases_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, bases_tuple_bits);
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        };
        let bases = unsafe { seq_vec_ref(bases_tuple_ptr) }.clone();
        for base_bits in bases.iter().copied() {
            let base_meta_bits = type_of_bits(_py, base_bits);
            if issubclass_bits(winner_bits, base_meta_bits) {
                continue;
            }
            if issubclass_bits(base_meta_bits, winner_bits) {
                // Promote to the more-derived base metaclass. `winner_bits` is
                // owned, so release the prior winner and acquire the new one to
                // keep ownership balanced (the registry-backed `base_meta_bits`
                // is borrowed).
                inc_ref_bits(_py, base_meta_bits);
                dec_ref_bits(_py, winner_bits);
                winner_bits = base_meta_bits;
                continue;
            }
            dec_ref_bits(_py, bases_tuple_bits);
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the metaclasses of all its bases",
            );
            return None;
        }
        dec_ref_bits(_py, bases_tuple_bits);
    }

    let missing = missing_bits(_py);
    let prepare_bits = if winner_bits == builtin_classes(_py).type_obj {
        missing
    } else {
        let Some(prepare_name_bits) = attr_name_bits_from_bytes(_py, b"__prepare__") else {
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        };
        let mut lookup_bits = crate::builtins::attributes::molt_get_attr_name_default(
            winner_bits,
            prepare_name_bits,
            missing,
        );
        dec_ref_bits(_py, prepare_name_bits);
        if exception_pending(_py) {
            if crate::builtins::attr::clear_attribute_error_if_pending(_py) {
                lookup_bits = missing;
            } else {
                dec_ref_bits(_py, winner_bits);
                dec_ref_bits(_py, kwds_copy_bits);
                return None;
            }
        }
        lookup_bits
    };
    let namespace_bits = if crate::is_missing_bits(_py, prepare_bits) {
        let ptr = alloc_dict_with_pairs(_py, &[]);
        if ptr.is_null() {
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        MoltObject::from_ptr(ptr).bits()
    } else {
        let val_bits =
            call_with_kwargs(_py, prepare_bits, &[name_bits, bases_bits], kwds_copy_bits);
        dec_ref_bits(_py, prepare_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        // CPython parity (3.12+): `__build_class__` rejects a `__prepare__`
        // result that is not a mapping with
        //   TypeError: <metaclass>.__prepare__() must return a mapping, not <type>
        // The runtime check is `PyMapping_Check` (`tp_as_mapping->mp_subscript`).
        // `value_supports_mp_subscript` is the single source of truth for that
        // contract: an exact dict, a dict subclass / custom mapping (class with
        // `__getitem__`), and even list/tuple/str/bytes/bytearray/range/
        // memoryview all pass — the latter then fail downstream on the first
        // string-keyed store with their own "indices must be integers" message,
        // exactly as CPython does.  int/object/set/slice/dict-views (no
        // `mp_subscript`) are rejected here.
        if !crate::object::ops::value_supports_mp_subscript(_py, val_bits) {
            let meta_name = class_name_for_error(winner_bits);
            let ns_type = type_name(_py, obj_from_bits(val_bits)).into_owned();
            dec_ref_bits(_py, val_bits);
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            let msg = format!("{meta_name}.__prepare__() must return a mapping, not {ns_type}");
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return None;
        }
        val_bits
    };

    Some(PreparedClassState {
        metaclass_bits: winner_bits,
        namespace_bits,
        kwds_bits: kwds_copy_bits,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_get_original_bases(cls_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(cls_ptr) = obj_from_bits(cls_bits).as_ptr() else {
            let owner = type_name(_py, obj_from_bits(cls_bits));
            let msg = format!("Expected an instance of type, not '{owner}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                let owner = type_name(_py, obj_from_bits(cls_bits));
                let msg = format!("Expected an instance of type, not '{owner}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let missing = missing_bits(_py);
        let Some(orig_name_bits) = attr_name_bits_from_bytes(_py, b"__orig_bases__") else {
            return MoltObject::none().bits();
        };
        let orig_bits = molt_getattr_builtin(cls_bits, orig_name_bits, missing);
        dec_ref_bits(_py, orig_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if orig_bits != missing {
            return orig_bits;
        }
        let Some(bases_name_bits) = attr_name_bits_from_bytes(_py, b"__bases__") else {
            return MoltObject::none().bits();
        };
        let bases_bits = molt_getattr_builtin(cls_bits, bases_name_bits, missing);
        dec_ref_bits(_py, bases_name_bits);
        if exception_pending(_py) || bases_bits == missing {
            return MoltObject::none().bits();
        }
        bases_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_prepare_class(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(positional) = call_vararg_args(_py, "prepare_class", args_bits) else {
            return MoltObject::none().bits();
        };
        let Some((_, keywords)) = call_vararg_kwargs(_py, "prepare_class", kwargs_bits) else {
            return MoltObject::none().bits();
        };
        if positional.len() > 3 {
            let msg = format!(
                "prepare_class() takes at most 3 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let mut name_bits = positional.first().copied().unwrap_or(0);
        let mut bases_bits = positional.get(1).copied().unwrap_or(0);
        let mut kwds_arg_bits = positional
            .get(2)
            .copied()
            .unwrap_or(MoltObject::none().bits());
        let mut has_name = !positional.is_empty();
        let mut has_bases = positional.len() >= 2;
        let mut has_kwds = positional.len() >= 3;
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "name" => {
                    if has_name {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "prepare_class() got multiple values for argument 'name'",
                        );
                    }
                    name_bits = *val_bits;
                    has_name = true;
                }
                "bases" => {
                    if has_bases {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "prepare_class() got multiple values for argument 'bases'",
                        );
                    }
                    bases_bits = *val_bits;
                    has_bases = true;
                }
                "kwds" => {
                    if has_kwds {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "prepare_class() got multiple values for argument 'kwds'",
                        );
                    }
                    kwds_arg_bits = *val_bits;
                    has_kwds = true;
                }
                _ => {
                    let msg = format!(
                        "prepare_class() got an unexpected keyword argument '{}'",
                        key
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if !has_name {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "prepare_class() missing 1 required positional argument: 'name'",
            );
        }
        let mut owned_bases = false;
        if !has_bases {
            let ptr = alloc_tuple(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_bits = MoltObject::from_ptr(ptr).bits();
            owned_bases = true;
        }
        let Some(state) = prepare_class_impl(_py, name_bits, bases_bits, kwds_arg_bits) else {
            if owned_bases {
                dec_ref_bits(_py, bases_bits);
            }
            return MoltObject::none().bits();
        };
        let out_ptr = alloc_tuple(
            _py,
            &[state.metaclass_bits, state.namespace_bits, state.kwds_bits],
        );
        if owned_bases {
            dec_ref_bits(_py, bases_bits);
        }
        // `prepare_class_impl` returns all three state fields as owned
        // references; `alloc_tuple` took its own reference on each element, so
        // release ours.
        dec_ref_bits(_py, state.metaclass_bits);
        dec_ref_bits(_py, state.namespace_bits);
        dec_ref_bits(_py, state.kwds_bits);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_resolve_bases(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(positional) = call_vararg_args(_py, "resolve_bases", args_bits) else {
            return MoltObject::none().bits();
        };
        let Some((_, keywords)) = call_vararg_kwargs(_py, "resolve_bases", kwargs_bits) else {
            return MoltObject::none().bits();
        };
        if positional.len() > 1 {
            let msg = format!(
                "resolve_bases() takes 1 positional argument but {} were given",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let mut bases_bits = positional.first().copied().unwrap_or(0);
        let mut has_bases = !positional.is_empty();
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "bases" => {
                    if has_bases {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "resolve_bases() got multiple values for argument 'bases'",
                        );
                    }
                    bases_bits = *val_bits;
                    has_bases = true;
                }
                _ => {
                    let msg = format!(
                        "resolve_bases() got an unexpected keyword argument '{}'",
                        key
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if !has_bases {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "resolve_bases() missing 1 required positional argument: 'bases'",
            );
        }
        resolve_bases_impl(_py, bases_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_new_class(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(positional) = call_vararg_args(_py, "new_class", args_bits) else {
            return MoltObject::none().bits();
        };
        let Some((_, keywords)) = call_vararg_kwargs(_py, "new_class", kwargs_bits) else {
            return MoltObject::none().bits();
        };
        if positional.len() > 4 {
            let msg = format!(
                "new_class() takes at most 4 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let mut name_bits = positional.first().copied().unwrap_or(0);
        let mut bases_bits = positional.get(1).copied().unwrap_or(0);
        let mut kwds_arg_bits = positional
            .get(2)
            .copied()
            .unwrap_or(MoltObject::none().bits());
        let mut exec_body_bits = positional
            .get(3)
            .copied()
            .unwrap_or(MoltObject::none().bits());
        let mut has_name = !positional.is_empty();
        let mut has_bases = positional.len() >= 2;
        let mut has_kwds = positional.len() >= 3;
        let mut has_exec = positional.len() >= 4;
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "name" => {
                    if has_name {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "new_class() got multiple values for argument 'name'",
                        );
                    }
                    name_bits = *val_bits;
                    has_name = true;
                }
                "bases" => {
                    if has_bases {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "new_class() got multiple values for argument 'bases'",
                        );
                    }
                    bases_bits = *val_bits;
                    has_bases = true;
                }
                "kwds" => {
                    if has_kwds {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "new_class() got multiple values for argument 'kwds'",
                        );
                    }
                    kwds_arg_bits = *val_bits;
                    has_kwds = true;
                }
                "exec_body" => {
                    if has_exec {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "new_class() got multiple values for argument 'exec_body'",
                        );
                    }
                    exec_body_bits = *val_bits;
                    has_exec = true;
                }
                _ => {
                    let msg = format!("new_class() got an unexpected keyword argument '{}'", key);
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if !has_name {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "new_class() missing 1 required positional argument: 'name'",
            );
        }
        let mut owned_bases = false;
        if !has_bases {
            let ptr = alloc_tuple(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_bits = MoltObject::from_ptr(ptr).bits();
            owned_bases = true;
        }
        let resolved_bases_bits = resolve_bases_impl(_py, bases_bits);
        if obj_from_bits(resolved_bases_bits).is_none() {
            if owned_bases {
                dec_ref_bits(_py, bases_bits);
            }
            return MoltObject::none().bits();
        }
        let Some(state) = prepare_class_impl(_py, name_bits, resolved_bases_bits, kwds_arg_bits)
        else {
            dec_ref_bits(_py, resolved_bases_bits);
            if owned_bases {
                dec_ref_bits(_py, bases_bits);
            }
            return MoltObject::none().bits();
        };
        if !obj_from_bits(exec_body_bits).is_none() {
            let _ = unsafe { call_callable1(_py, exec_body_bits, state.namespace_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, state.metaclass_bits);
                dec_ref_bits(_py, state.namespace_bits);
                dec_ref_bits(_py, state.kwds_bits);
                dec_ref_bits(_py, resolved_bases_bits);
                if owned_bases {
                    dec_ref_bits(_py, bases_bits);
                }
                return MoltObject::none().bits();
            }
        }
        if resolved_bases_bits != bases_bits {
            let Some(orig_bases_name_bits) = attr_name_bits_from_bytes(_py, b"__orig_bases__")
            else {
                dec_ref_bits(_py, state.metaclass_bits);
                dec_ref_bits(_py, state.namespace_bits);
                dec_ref_bits(_py, state.kwds_bits);
                dec_ref_bits(_py, resolved_bases_bits);
                if owned_bases {
                    dec_ref_bits(_py, bases_bits);
                }
                return MoltObject::none().bits();
            };
            let _ = molt_setitem_method(state.namespace_bits, orig_bases_name_bits, bases_bits);
            dec_ref_bits(_py, orig_bases_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, state.metaclass_bits);
                dec_ref_bits(_py, state.namespace_bits);
                dec_ref_bits(_py, state.kwds_bits);
                dec_ref_bits(_py, resolved_bases_bits);
                if owned_bases {
                    dec_ref_bits(_py, bases_bits);
                }
                return MoltObject::none().bits();
            }
        }
        let class_bits = call_with_kwargs(
            _py,
            state.metaclass_bits,
            &[name_bits, resolved_bases_bits, state.namespace_bits],
            state.kwds_bits,
        );
        // `prepare_class_impl` returns `metaclass_bits` as an owned reference;
        // release it now that the metaclass has been called.
        dec_ref_bits(_py, state.metaclass_bits);
        dec_ref_bits(_py, state.namespace_bits);
        dec_ref_bits(_py, state.kwds_bits);
        dec_ref_bits(_py, resolved_bases_bits);
        if owned_bases {
            dec_ref_bits(_py, bases_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        class_bits
    })
}
