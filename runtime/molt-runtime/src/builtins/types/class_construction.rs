use super::*;
use crate::object::class_layout::{try_is_real_instance, try_is_real_subtype};
use crate::object::seq_access::with_immutable_tuple_slice;

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
    unsafe { with_immutable_tuple_slice(args_ptr, |args| args.to_vec()) }
}

/// Keyword names and their retained source values share one callback lifetime.
pub(crate) struct CallKeywords<'a, 'py> {
    names: Vec<String>,
    entries: crate::object::seq_access::PinnedSequenceSnapshot<'a, 'py>,
}

impl CallKeywords<'_, '_> {
    pub(crate) fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &u64)> {
        self.names
            .iter()
            .zip(self.entries.as_chunks::<2>().0.iter().map(|row| &row[1]))
    }
}

pub(crate) fn call_vararg_kwargs<'a, 'py>(
    _py: &'a PyToken<'py>,
    func_name: &str,
    kwargs_bits: u64,
) -> Option<CallKeywords<'a, 'py>> {
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
    let order = unsafe {
        crate::object::ops_dict::dict_snapshot(
            _py,
            kwargs_ptr,
            crate::object::ops_dict::DictSnapshotKind::Entries,
        )
    }?;
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
        entries.push(key_name);
    }
    Some(CallKeywords {
        names: entries,
        entries: order,
    })
}

fn copy_kwds_mapping(_py: &PyToken<'_>, kwds_bits: u64) -> Option<u64> {
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    if !obj_from_bits(kwds_bits).is_none()
        && !unsafe { dict_update_apply(_py, dict_bits, dict_update_set_in_place, kwds_bits) }
    {
        dec_ref_bits(_py, dict_bits);
        return None;
    }
    Some(dict_bits)
}

pub(crate) fn call_with_kwargs(
    _py: &PyToken<'_>,
    callable_bits: u64,
    positional: &[u64],
    kwargs_bits: u64,
) -> u64 {
    let mapping = if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
        if unsafe { object_type_id(kwargs_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "keyword arguments must be a dict");
        }
        kwargs_bits
    } else {
        MoltObject::none().bits()
    };
    unsafe { crate::call::bind::call_bind_capi(_py, callable_bits, None, positional, mapping) }
}

fn resolve_bases_impl(_py: &PyToken<'_>, bases_bits: u64) -> u64 {
    let Some(bases_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, bases_bits) }) else {
        return MoltObject::none().bits();
    };
    let Some(bases_tuple_ptr) = obj_from_bits(bases_tuple_bits).as_ptr() else {
        dec_ref_bits(_py, bases_tuple_bits);
        return MoltObject::none().bits();
    };
    let Some(bases) =
        (unsafe { with_immutable_tuple_slice(bases_tuple_ptr, |bases| bases.to_vec()) })
    else {
        dec_ref_bits(_py, bases_tuple_bits);
        return MoltObject::none().bits();
    };
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
        // Type inquiry is semantic. A native class has foreign storage, but
        // must never enter the non-type __mro_entries__ protocol.
        let is_type =
            match unsafe { try_is_real_instance(_py, base_bits, builtin_classes(_py).type_obj) } {
                Ok(value) => value,
                Err(()) => {
                    crate::cpython_abi_hooks::propagate_native_failure(_py, "base type");
                    for bits in keepalive_tuples.drain(..) {
                        dec_ref_bits(_py, bits);
                    }
                    dec_ref_bits(_py, mro_entries_name_bits);
                    dec_ref_bits(_py, bases_tuple_bits);
                    return MoltObject::none().bits();
                }
            };
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
            let copied = with_immutable_tuple_slice(resolved_ptr, |resolved| {
                out.extend_from_slice(resolved);
            });
            if copied.is_none() {
                dec_ref_bits(_py, resolved_bits);
                for bits in keepalive_tuples.drain(..) {
                    dec_ref_bits(_py, bits);
                }
                dec_ref_bits(_py, mro_entries_name_bits);
                dec_ref_bits(_py, bases_tuple_bits);
                return MoltObject::none().bits();
            }
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
    let metaclass_bits = match unsafe { dict_get_in_place(_py, kwds_copy_ptr, metaclass_name_bits) }
    {
        Ok(value) => value,
        Err(()) => {
            dec_ref_bits(_py, metaclass_name_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
    };
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
        if unsafe { dict_del_in_place(_py, kwds_copy_ptr, metaclass_name_bits) }.is_err() {
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
    // The dict branch is already owned (incref'd above); actual type
    // projection returns an owned reference too. Only the builtin default
    // below is borrowed from the registry.
    let mut winner_bits = if let Some(bits) = metaclass_bits {
        bits
    } else if is_truthy(_py, obj_from_bits(bases_bits)) {
        let index_zero = MoltObject::from_int(0).bits();
        let first_base_bits = molt_index(bases_bits, index_zero);
        if exception_pending(_py) {
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        let bits = class_model::type_of_owned(_py, first_base_bits);
        dec_ref_bits(_py, first_base_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        bits
    } else {
        let bits = builtin_classes(_py).type_obj;
        inc_ref_bits(_py, bits);
        bits
    };

    let winner_is_type =
        match unsafe { try_is_real_instance(_py, winner_bits, builtin_classes(_py).type_obj) } {
            Ok(value) => value,
            Err(()) => {
                crate::cpython_abi_hooks::propagate_native_failure(_py, "metaclass type");
                dec_ref_bits(_py, winner_bits);
                dec_ref_bits(_py, kwds_copy_bits);
                return None;
            }
        };
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
        let Some(bases) =
            (unsafe { with_immutable_tuple_slice(bases_tuple_ptr, |bases| bases.to_vec()) })
        else {
            dec_ref_bits(_py, bases_tuple_bits);
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        };
        for base_bits in bases.iter().copied() {
            let base_meta_bits = class_model::type_of_owned(_py, base_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, base_meta_bits);
                dec_ref_bits(_py, bases_tuple_bits);
                dec_ref_bits(_py, winner_bits);
                dec_ref_bits(_py, kwds_copy_bits);
                return None;
            }
            let _base_meta_owner = crate::PtrDropGuard::new(
                obj_from_bits(base_meta_bits)
                    .as_ptr()
                    .expect("owned actual type"),
            );
            let relation = unsafe {
                try_is_real_subtype(_py, winner_bits, base_meta_bits).and_then(|wins| {
                    if wins {
                        Ok((true, false))
                    } else {
                        try_is_real_subtype(_py, base_meta_bits, winner_bits)
                            .map(|promote| (false, promote))
                    }
                })
            };
            let (wins, promote) = match relation {
                Ok(value) => value,
                Err(()) => {
                    crate::cpython_abi_hooks::propagate_native_failure(_py, "metaclass relation");
                    dec_ref_bits(_py, bases_tuple_bits);
                    dec_ref_bits(_py, winner_bits);
                    dec_ref_bits(_py, kwds_copy_bits);
                    return None;
                }
            };
            if wins {
                continue;
            }
            if promote {
                // Both identities are owned; promotion acquires the reference
                // that survives this iteration's base-metaclass owner.
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
        let mapping = match crate::object::ops::value_supports_mp_subscript(_py, val_bits) {
            Ok(mapping) => mapping,
            Err(()) => {
                dec_ref_bits(_py, val_bits);
                dec_ref_bits(_py, winner_bits);
                dec_ref_bits(_py, kwds_copy_bits);
                return None;
            }
        };
        if !mapping {
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
        use crate::builtins::exceptions::{
            ExceptionFieldSlot, ExceptionStackScope, ExceptionValue, exception_context_set,
            exception_replace_field_bits, molt_exception_last_pending,
        };
        let _input = ExceptionValue::pin(_py, cls_bits);
        // Lib/types.py uses the normal namespace protocol, not an inherited
        // __orig_bases__ lookup or a storage-tag type test. This also permits
        // metaclass overrides and objects implementing that exact protocol.
        let read = |object, name: &[u8]| {
            let name = ExceptionValue::adopt(_py, attr_name_bits_from_bytes(_py, name)?);
            crate::builtins::attributes::get_attr_name(_py, object, name.bits())
                .ok()
                .map(|value| ExceptionValue::adopt(_py, value))
        };
        let result = (|| {
            let namespace = read(cls_bits, b"__dict__")?;
            let get = read(namespace.bits(), b"get");
            // Attribute lookup has consumed the namespace receiver. The bound
            // method, if any, owns it across the following eager default lookup.
            drop(namespace);
            let get = get?;
            let name =
                ExceptionValue::adopt(_py, attr_name_bits_from_bytes(_py, b"__orig_bases__")?);
            let bases = read(cls_bits, b"__bases__")?;
            let result = ExceptionValue::adopt(_py, unsafe {
                call_callable2(_py, get.bits(), name.bits(), bases.bits())
            });
            if exception_pending(_py) {
                None
            } else {
                Some(result.into_bits())
            }
        })();
        if let Some(result) = result {
            return result;
        }
        let original = ExceptionValue::adopt(_py, molt_exception_last_pending());
        if !crate::builtins::attr::exception_is_attribute_error(_py, original.bits()) {
            return MoltObject::none().bits();
        }
        // Only AttributeError is normalized, with the original handled context
        // retained and explicit `from None` on the resulting TypeError.
        let _handler = ExceptionStackScope::push(_py);
        exception_context_set(_py, original.bits());
        clear_exception(_py);
        let class = ExceptionValue::adopt(_py, class_model::type_of_owned(_py, cls_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(name) = read(class.bits(), b"__name__") else {
            return MoltObject::none().bits();
        };
        let rendered = crate::object::ops_format::format_obj_bytes(_py, obj_from_bits(name.bits()));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let mut message = b"Expected an instance of type, not ".to_vec();
        message.extend_from_slice(&rendered);
        let raised = crate::builtins::exceptions::alloc_exception_bytes(_py, "TypeError", &message);
        if raised.is_null() {
            if !exception_pending(_py) {
                crate::builtins::exceptions::record_memory_error_without_allocation(_py);
            }
            return MoltObject::none().bits();
        }
        let raised_owner = ExceptionValue::adopt(_py, MoltObject::from_ptr(raised).bits());
        if let Err(message) = exception_replace_field_bits(
            _py,
            raised_owner.bits(),
            ExceptionFieldSlot::Cause,
            MoltObject::none().bits(),
        ) {
            if !exception_pending(_py) {
                return raise_exception::<u64>(_py, "SystemError", message);
            }
            return MoltObject::none().bits();
        }
        let _ = raised_owner.into_bits();
        crate::builtins::exceptions::record_exception_owned(_py, raised);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_prepare_class(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(positional) = call_vararg_args(_py, "prepare_class", args_bits) else {
            return MoltObject::none().bits();
        };
        let Some(keywords) = call_vararg_kwargs(_py, "prepare_class", kwargs_bits) else {
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
        let Some(keywords) = call_vararg_kwargs(_py, "resolve_bases", kwargs_bits) else {
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
        let Some(keywords) = call_vararg_kwargs(_py, "new_class", kwargs_bits) else {
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

#[cfg(test)]
mod keyword_snapshot_tests {
    use super::*;
    use crate::header_from_obj_ptr;

    #[test]
    fn keyword_selection_keeps_values_owned_after_source_mapping_is_destroyed() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let name = attr_name_bits_from_bytes(py, b"selected").expect("keyword name");
            let value_ptr = alloc_list(py, &[]);
            assert!(!value_ptr.is_null());
            let value = MoltObject::from_ptr(value_ptr).bits();
            let dict_ptr = alloc_dict_with_pairs(py, &[name, value]);
            assert!(!dict_ptr.is_null());
            let dict = MoltObject::from_ptr(dict_ptr).bits();
            let keywords = call_vararg_kwargs(py, "probe", dict).expect("keyword selection");
            dec_ref_bits(py, dict);
            // A raw copied handle would now have only the test's reference.
            assert_eq!(
                unsafe { (*header_from_obj_ptr(value_ptr)).ref_count_snapshot() },
                2
            );
            let pairs: Vec<_> = keywords
                .iter()
                .map(|(name, value)| (name.as_str(), *value))
                .collect();
            assert_eq!(pairs, vec![("selected", value)]);
            drop(keywords);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(value_ptr)).ref_count_snapshot() },
                1
            );
            dec_ref_bits(py, name);
            dec_ref_bits(py, value);
            assert!(!exception_pending(py));
        });
    }
}

#[cfg(test)]
mod native_base_tests {
    use super::*;
    use molt_cpython_abi::abi_types::*;
    use molt_cpython_abi::api::{errors, refcount::OwnedPyObject, strings, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

    #[test]
    fn native_base_resolution_and_prepare_preserve_identity_until_layout_admission() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                for (name, flags) in [
                    (c"native.NotABase", Py_TPFLAGS_DEFAULT),
                    (
                        c"native.AllowedBase",
                        Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE,
                    ),
                ] {
                    let mut slots = [PyType_Slot {
                        slot: 0,
                        pfunc: std::ptr::null_mut(),
                    }];
                    let mut spec = PyType_Spec {
                        name: name.as_ptr(),
                        basicsize: std::mem::size_of::<PyObject>() as i32,
                        itemsize: 0,
                        flags: flags as u32,
                        slots: slots.as_mut_ptr(),
                    };
                    let native = OwnedPyObject::from_owned(typeobj::PyType_FromSpec(&mut spec));
                    assert!(!native.as_ptr().is_null());
                    let class = GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()).unwrap();
                    assert_eq!(
                        object_type_id(obj_from_bits(class).as_ptr().unwrap()),
                        crate::TYPE_ID_FOREIGN
                    );
                    let bases = MoltObject::from_ptr(alloc_tuple(py, &[class])).bits();
                    dec_ref_bits(py, class);
                    let child_name = attr_name_bits_from_bytes(py, b"NativeChild").unwrap();
                    let count = (*native.as_ptr()).ob_refcnt;
                    for _ in 0..2 {
                        let original_bases = molt_types_get_original_bases(class);
                        assert!(!exception_pending(py));
                        assert_eq!(
                            with_immutable_tuple_slice(
                                obj_from_bits(original_bases).as_ptr().unwrap(),
                                |items| items.to_vec()
                            ),
                            Some(vec![builtin_classes(py).object]),
                        );
                        dec_ref_bits(py, original_bases);
                        let resolved = resolve_bases_impl(py, bases);
                        assert!(!exception_pending(py));
                        assert_eq!(resolved, bases, "a real class bypasses __mro_entries__");
                        dec_ref_bits(py, resolved);
                        let state =
                            prepare_class_impl(py, child_name, bases, MoltObject::none().bits())
                                .expect("native bases participate in metaclass selection");
                        assert_eq!(state.metaclass_bits, builtin_classes(py).type_obj);
                        assert!(!exception_pending(py));
                        if flags & Py_TPFLAGS_BASETYPE == 0 {
                            let result = molt_type_new(
                                state.metaclass_bits,
                                child_name,
                                bases,
                                state.namespace_bits,
                                state.kwds_bits,
                            );
                            dec_ref_bits(py, result);
                            assert_ne!(
                                errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                                0
                            );
                            let error =
                                OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                            assert!(!error.as_ptr().is_null());
                            let message =
                                OwnedPyObject::from_owned(typeobj::PyObject_Str(error.as_ptr()));
                            assert!(!message.as_ptr().is_null());
                            let text = strings::PyUnicode_AsUTF8(message.as_ptr());
                            assert!(!text.is_null());
                            assert_eq!(
                                std::ffi::CStr::from_ptr(text).to_bytes(),
                                b"type 'native.NotABase' is not an acceptable base type"
                            );
                            drop(message);
                            drop(error);
                        }
                        for bits in [state.metaclass_bits, state.namespace_bits, state.kwds_bits] {
                            dec_ref_bits(py, bits);
                        }
                        assert_eq!(
                            (*native.as_ptr()).ob_refcnt,
                            count,
                            "temporary class inquiries must retire"
                        );
                        assert!(errors::PyErr_Occurred().is_null());
                        assert!(!exception_pending(py));
                    }
                    dec_ref_bits(py, child_name);
                    dec_ref_bits(py, bases);
                }
            }
        });
    }
}
