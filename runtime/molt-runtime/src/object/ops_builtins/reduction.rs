//! Default object reduction: CPython 3.12 Objects/typeobject.c,
//! object___reduce_ex___impl, _common_reduce, reduce_newobj and
//! _PyObject_GetNewArguments. Copy and serialization consume this authority.
use crate::*;
use molt_runtime_core::ProtectedRuntimeValue as Value;

fn owned<'a>(py: &'a PyToken<'_>, bits: u64) -> Result<Value<'a>, u64> {
    let value = unsafe { Value::from_owned_bits(py.core_token(), bits) };
    if exception_pending(py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(value)
    }
}

fn tuple<'a>(py: &'a PyToken<'_>, fields: &[u64]) -> Result<Value<'a>, u64> {
    let ptr = alloc_tuple(py, fields);
    if ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    owned(py, MoltObject::from_ptr(ptr).bits())
}

fn attr<'a>(py: &'a PyToken<'_>, obj: u64, name: crate::NameId) -> Result<Option<Value<'a>>, u64> {
    let name = name
        .owned_bits(py)
        .ok_or_else(|| MoltObject::none().bits())?;
    let name = owned(py, name)?;
    match lookup_attribute(py, obj, name.bits()) {
        AttributeLookup::Found(bits) => owned(py, bits).map(Some),
        AttributeLookup::Missing => Ok(None),
        AttributeLookup::Error => Err(MoltObject::none().bits()),
    }
}

fn required<'a>(py: &'a PyToken<'_>, obj: u64, name: crate::NameId) -> Result<Value<'a>, u64> {
    let name = name
        .owned_bits(py)
        .ok_or_else(|| MoltObject::none().bits())?;
    let name = owned(py, name)?;
    owned(
        py,
        molt_getattr_builtin_attached_body(py, obj, name.bits(), missing_bits(py)),
    )
}

fn special<'a>(
    py: &'a PyToken<'_>,
    obj: u64,
    name: crate::NameId,
) -> Result<Option<Value<'a>>, u64> {
    unsafe { crate::builtins::attr::lookup_special_method(py, obj, name).into_result() }
        .map_err(|()| MoltObject::none().bits())?
        .map(|bits| owned(py, bits))
        .transpose()
}

fn copyreg<'a>(py: &'a PyToken<'_>, name: crate::NameId) -> Result<Value<'a>, u64> {
    use crate::builtins::module_table::{CachedModuleOutcome, ModuleImportPolicy};
    // typeobject.c import_copyreg first uses PyImport_GetModule. Reuse the
    // interpreter's public module authority, including initialization waits
    // and target-version identity revalidation; do not keep a private cache.
    if let Some(observed) = crate::builtins::module_table::module_lookup(py, "copyreg")? {
        let observed = owned(py, observed)?;
        let dictionary = crate::builtins::module_table::module_dictionary(py)
            .ok_or_else(|| MoltObject::none().bits())?;
        match crate::builtins::module_table::resolve_cached_module(
            py,
            "copyreg",
            None,
            dictionary,
            observed.into_bits(),
            ModuleImportPolicy::CGetModule,
        )? {
            CachedModuleOutcome::Return(bits) => {
                let module = owned(py, bits)?;
                return required(py, module.bits(), name);
            }
            CachedModuleOutcome::Missing => {}
            CachedModuleOutcome::Reload => unreachable!("cache-only import never reloads"),
        }
    }
    let module_name = alloc_string(py, b"copyreg");
    if module_name.is_null() {
        return Err(MoltObject::none().bits());
    }
    let module_name = owned(py, MoltObject::from_ptr(module_name).bits())?;
    let module = owned(py, molt_module_import_attached_body(py, module_name.bits()))?;
    required(py, module.bits(), name)
}

fn require_kind(py: &PyToken<'_>, bits: u64, kind: u32, message: &str) -> Result<(), u64> {
    if obj_from_bits(bits).as_ptr().is_some_and(|ptr| unsafe {
        object_type_id(ptr) == kind
            || (kind == TYPE_ID_DICT
                && crate::object::layout::dict_subclass_storage_slot(ptr).is_some())
    }) {
        Ok(())
    } else {
        let name = type_name(py, obj_from_bits(bits));
        Err(raise_exception::<u64>(
            py,
            BuiltinExc::TypeError,
            &format!("{message}, not '{name}'"),
        ))
    }
}

fn new_arguments<'a>(
    py: &'a PyToken<'_>,
    obj: u64,
) -> Result<(Option<Value<'a>>, Option<Value<'a>>), u64> {
    if let Some(method) = special(py, obj, crate::NameId::DunderGetnewargsEx)? {
        let result = owned(py, unsafe { call_callable0(py, method.bits()) })?;
        require_kind(
            py,
            result.bits(),
            TYPE_ID_TUPLE,
            "__getnewargs_ex__ should return a tuple",
        )?;
        let ptr = obj_from_bits(result.bits()).as_ptr().unwrap();
        let fields = unsafe {
            crate::object::seq_access::snapshot(py, ptr, "reduction arguments allocation failed")
        }
        .ok_or_else(|| MoltObject::none().bits())?;
        if fields.len() != 2 {
            return Err(raise_exception::<u64>(
                py,
                BuiltinExc::ValueError,
                &format!(
                    "__getnewargs_ex__ should return a tuple of length 2, not {}",
                    fields.len()
                ),
            ));
        }
        require_kind(
            py,
            fields[0],
            TYPE_ID_TUPLE,
            "first item of the tuple returned by __getnewargs_ex__ must be a tuple",
        )?;
        require_kind(
            py,
            fields[1],
            TYPE_ID_DICT,
            "second item of the tuple returned by __getnewargs_ex__ must be a dict",
        )?;
        return Ok((
            Some(Value::retain(py.core_token(), fields[0])),
            Some(Value::retain(py.core_token(), fields[1])),
        ));
    }
    if let Some(method) = special(py, obj, crate::NameId::DunderGetnewargs)? {
        let args = owned(py, unsafe { call_callable0(py, method.bits()) })?;
        require_kind(
            py,
            args.bits(),
            TYPE_ID_TUPLE,
            "__getnewargs__ should return a tuple",
        )?;
        return Ok((Some(args), None));
    }
    Ok((None, None))
}

fn reduce_newobj(py: &PyToken<'_>, obj: u64) -> Result<u64, u64> {
    // Construction is class metadata, like CPython's tp_new check. Reduction
    // must not observe a metaclass's __getattribute__ or bind __new__ here.
    let initial_class = Value::retain(py.core_token(), type_of_bits(py, obj));
    let name = crate::NameId::DunderNew
        .owned_bits(py)
        .ok_or_else(|| MoltObject::none().bits())?;
    let name = owned(py, name)?;
    let constructor_present = unsafe {
        crate::builtins::attr::class_attr_lookup_raw_mro(
            py,
            obj_from_bits(initial_class.bits()).as_ptr().unwrap(),
            name.bits(),
        )
    }
    .map_err(|()| MoltObject::none().bits())?
    .is_some();
    if !constructor_present {
        return Err(raise_exception::<u64>(
            py,
            BuiltinExc::TypeError,
            &format!(
                "cannot pickle '{}' object",
                type_name(py, obj_from_bits(obj))
            ),
        ));
    }
    let (args, kwargs) = new_arguments(py, obj)?;
    let has_kwargs = if let Some(kw) = &kwargs {
        let storage = crate::object::ops::dict_backing_bits(py, kw.bits())
            .map_err(|()| MoltObject::none().bits())?
            .unwrap();
        unsafe { dict_len(obj_from_bits(storage).as_ptr().unwrap()) != 0 }
    } else {
        false
    };
    let constructor = copyreg(
        py,
        if has_kwargs {
            crate::NameId::DunderNewobjEx
        } else {
            crate::NameId::DunderNewobj
        },
    )?;
    // __getnewargs__ may have changed the receiver's class. CPython samples
    // Py_TYPE for the constructor args after that callback and copyreg lookup.
    let cls = Value::retain(py.core_token(), type_of_bits(py, obj));
    let newargs = if has_kwargs {
        let args = args.as_ref().unwrap();
        tuple(
            py,
            &[cls.bits(), args.bits(), kwargs.as_ref().unwrap().bits()],
        )?
    } else {
        let mut fields = vec![cls.bits()];
        if let Some(args) = &args {
            let ptr = obj_from_bits(args.bits()).as_ptr().unwrap();
            let snapshot = unsafe {
                crate::object::seq_access::snapshot(
                    py,
                    ptr,
                    "reduction arguments allocation failed",
                )
            }
            .ok_or_else(|| MoltObject::none().bits())?;
            fields.extend_from_slice(&snapshot);
        }
        tuple(py, &fields)?
    };
    let type_id = obj_from_bits(obj)
        .as_ptr()
        .map(|ptr| unsafe { object_type_id(ptr) });
    let is_list = matches!(
        type_id,
        Some(TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL)
    );
    let is_dict = type_id == Some(TYPE_ID_DICT)
        || obj_from_bits(obj).as_ptr().is_some_and(|ptr| unsafe {
            crate::object::layout::dict_subclass_storage_slot(ptr).is_some()
        });
    let getstate = required(py, obj, crate::NameId::DunderGetstate)?;
    // CPython object_getstate identifies the native bound descriptor before
    // calling its default implementation with `required`. Overrides retain
    // ordinary call semantics, including a user-provided opaque-state recipe.
    let default_getstate = obj_from_bits(getstate.bits())
        .as_ptr()
        .is_some_and(|ptr| unsafe {
            object_type_id(ptr) == TYPE_ID_BOUND_METHOD
                && bound_method_self_bits(ptr) == obj
                && obj_from_bits(bound_method_func_bits(ptr))
                    .as_ptr()
                    .is_some_and(|func| {
                        object_type_id(func) == TYPE_ID_FUNCTION
                            && function_fn_ptr(func) == fn_addr!(crate::molt_object_getstate)
                    })
        });
    let state = owned(
        py,
        if default_getstate {
            super::object_getstate_default(py, obj, args.is_none() && !is_list && !is_dict)
        } else {
            unsafe { call_callable0(py, getstate.bits()) }
        },
    )?;
    let listitems = if is_list {
        owned(py, molt_iter_attached_body(py, obj))?
    } else {
        Value::retain(py.core_token(), MoltObject::none().bits())
    };
    let dictitems = if is_dict {
        let items = required(py, obj, crate::NameId::Items)?;
        let items = owned(py, unsafe { call_callable0(py, items.bits()) })?;
        owned(py, molt_iter_attached_body(py, items.bits()))?
    } else {
        Value::retain(py.core_token(), MoltObject::none().bits())
    };
    Ok(tuple(
        py,
        &[
            constructor.bits(),
            newargs.bits(),
            state.bits(),
            listitems.bits(),
            dictitems.bits(),
        ],
    )?
    .into_bits())
}

fn common_reduce(py: &PyToken<'_>, obj: u64, protocol: i32) -> Result<u64, u64> {
    if protocol >= 2 {
        reduce_newobj(py, obj)
    } else {
        let reducer = copyreg(py, crate::NameId::PrivateReduceEx)?;
        Ok(owned(py, unsafe {
            call_callable2(
                py,
                reducer.bits(),
                obj,
                MoltObject::from_int(i64::from(protocol)).bits(),
            )
        })?
        .into_bits())
    }
}

pub(crate) fn molt_object_reduce_attached_body(_py: &crate::PyToken<'_>, obj: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        common_reduce(py, obj, 0).unwrap_or_else(|error| error)
    })
}

pub(crate) fn molt_object_reduce_ex_attached_body(
    _py: &crate::PyToken<'_>,
    obj: u64,
    protocol: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(protocol) = crate::builtins::numbers::index_c_int_from_obj(py, protocol) else {
            return MoltObject::none().bits();
        };
        let result = (|| {
            if let Some(reduce) = attr(py, obj, crate::NameId::DunderReduce)? {
                let cls = Value::retain(py.core_token(), type_of_bits(py, obj));
                let class_reduce = required(py, cls.bits(), crate::NameId::DunderReduce)?;
                // The immutable object's descriptor is metadata, as in
                // CPython's interpreter-cached objreduce. Only the receiver's
                // class lookup above observes metaclass attribute semantics.
                let name = crate::NameId::DunderReduce
                    .owned_bits(py)
                    .ok_or_else(|| MoltObject::none().bits())?;
                let name = owned(py, name)?;
                let base_reduce = unsafe {
                    crate::builtins::attr::class_attr_lookup_raw_mro(
                        py,
                        obj_from_bits(builtin_classes(py).object).as_ptr().unwrap(),
                        name.bits(),
                    )
                }
                .map_err(|()| MoltObject::none().bits())?
                .expect("object owns __reduce__");
                let base_reduce = Value::retain(py.core_token(), base_reduce);
                if class_reduce.bits() != base_reduce.bits() {
                    return Ok(owned(py, unsafe { call_callable0(py, reduce.bits()) })?.into_bits());
                }
            }
            common_reduce(py, obj, protocol)
        })();
        result.unwrap_or_else(|error| error)
    })
}

// Builtin-specific producers supply only their physical constructor arguments
// or the reduction specified by their CPython object implementation. The
// default object reduce protocol above remains shared by every consumer.
fn one_newarg(py: &PyToken<'_>, value: u64) -> Result<u64, u64> {
    let value = owned(py, value)?;
    Ok(tuple(py, &[value.bits()])?.into_bits())
}

pub(crate) fn molt_int_getnewargs_attached_body(_py: &crate::PyToken<'_>, obj: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        // CPython longobject.c int___getnewargs___impl: copy storage, never
        // invoke a subtype's __int__ override.
        one_newarg(py, molt_int_int_attached_body(py, obj)).unwrap_or_else(|error| error)
    })
}

pub(crate) fn molt_float_getnewargs_attached_body(_py: &crate::PyToken<'_>, obj: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        // CPython floatobject.c float___getnewargs___impl.
        one_newarg(py, molt_float_float_attached_body(py, obj)).unwrap_or_else(|error| error)
    })
}

pub(crate) fn molt_str_getnewargs_attached_body(_py: &crate::PyToken<'_>, obj: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        one_newarg(
            py,
            crate::object::ops_format::string_str_slot(_py.attached_context(), obj),
        )
        .unwrap_or_else(|error| error)
    })
}

pub(crate) fn molt_bytes_getnewargs_attached_body(_py: &crate::PyToken<'_>, obj: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        one_newarg(
            py,
            crate::object::ops_bytes::bytes_bytes(_py.attached_context(), obj),
        )
        .unwrap_or_else(|error| error)
    })
}

pub(crate) fn molt_tuple_getnewargs_attached_body(_py: &crate::PyToken<'_>, obj: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let result = (|| {
            require_kind(
                py,
                obj,
                TYPE_ID_TUPLE,
                "tuple.__getnewargs__ requires a tuple",
            )?;
            let base = if type_of_bits(py, obj) == builtin_classes(py).tuple {
                Value::retain(py.core_token(), obj)
            } else {
                let items = unsafe {
                    crate::object::seq_access::snapshot(
                        py,
                        obj_from_bits(obj).as_ptr().unwrap(),
                        "tuple newargs allocation failed",
                    )
                }
                .ok_or_else(|| MoltObject::none().bits())?;
                tuple(py, &items)?
            };
            Ok(tuple(py, &[base.bits()])?.into_bits())
        })();
        result.unwrap_or_else(|error| error)
    })
}

pub(crate) fn molt_complex_getnewargs_attached_body(_py: &crate::PyToken<'_>, obj: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(ptr) = complex_ptr_from_bits(obj) else {
            return raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "complex.__getnewargs__ requires a complex object",
            );
        };
        let parts = unsafe { *complex_ref(ptr) };
        let result = (|| {
            let real = owned(py, crate::object::ops::float_result_bits(py, parts.re))?;
            let imag = owned(py, crate::object::ops::float_result_bits(py, parts.im))?;
            Ok(tuple(py, &[real.bits(), imag.bits()])?.into_bits())
        })();
        result.unwrap_or_else(|error| error)
    })
}

pub(crate) fn molt_set_reduce_attached_body(_py: &crate::PyToken<'_>, obj: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        // CPython Objects/setobject.c set_reduce, shared by set/frozenset:
        // iteration is observable before __getstate__, including on subtypes.
        let result = (|| {
            let keys = owned(py, molt_list_builtin_attached_body(py, obj))?;
            let args = tuple(py, &[keys.bits()])?;
            let getter = required(py, obj, crate::NameId::DunderGetstate)?;
            let state = owned(py, unsafe { call_callable0(py, getter.bits()) })?;
            let cls = Value::retain(py.core_token(), type_of_bits(py, obj));
            Ok(tuple(py, &[cls.bits(), args.bits(), state.bits()])?.into_bits())
        })();
        result.unwrap_or_else(|error| error)
    })
}

fn bytearray_reduce(py: &PyToken<'_>, obj: u64, protocol: i32) -> Result<u64, u64> {
    // CPython Objects/bytearrayobject.c _common_reduce. This is a physical
    // descriptor: a subtype's __bytes__/__buffer__ override is not consulted.
    require_kind(
        py,
        obj,
        TYPE_ID_BYTEARRAY,
        "bytearray reduction requires a bytearray",
    )?;
    let getter = required(py, obj, crate::NameId::DunderGetstate)?;
    let state = owned(py, unsafe { call_callable0(py, getter.bits()) })?;
    let cls = Value::retain(py.core_token(), type_of_bits(py, obj));
    let ptr = obj_from_bits(obj).as_ptr().unwrap();
    let args = if unsafe { bytearray_len(ptr) } == 0 {
        tuple(py, &[])?
    } else {
        let pin = crate::object::buffer_exports::ScopedBufferExport::new(py, obj)
            .map_err(|()| MoltObject::none().bits())?;
        let length = unsafe { bytearray_len(ptr) };
        let bytes = crate::object::builders::alloc_inline_bytes_with_len(
            py,
            length,
            crate::object::builders::InlineBytesKind::Bytes,
        );
        if bytes.is_null() {
            return Err(MoltObject::none().bits());
        }
        // Allocate before borrowing mutable source storage. The counted export
        // pins its extent across allocation/GC; the canonical backing lock then
        // covers the callback-free copy, including in-place buffer writers.
        unsafe {
            let backing = bytearray_vec_ptr(ptr);
            let _lock = crate::object::backing::tracked_vec_mutation_lock(backing);
            std::ptr::copy_nonoverlapping(
                bytearray_data(ptr),
                crate::object::layout::InlineBytesStorage::data(bytes),
                length,
            );
        }
        let bytes = owned(py, MoltObject::from_ptr(bytes).bits())?;
        drop(pin);
        if protocol < 3 {
            let encoding = alloc_string(py, b"latin-1");
            if encoding.is_null() {
                return Err(MoltObject::none().bits());
            }
            let encoding = owned(py, MoltObject::from_ptr(encoding).bits())?;
            let text = owned(
                py,
                molt_bytes_decode_attached_body(
                    py,
                    bytes.bits(),
                    encoding.bits(),
                    missing_bits(py),
                ),
            )?;
            tuple(py, &[text.bits(), encoding.bits()])?
        } else {
            tuple(py, &[bytes.bits()])?
        }
    };
    Ok(tuple(py, &[cls.bits(), args.bits(), state.bits()])?.into_bits())
}

pub(crate) fn molt_bytearray_reduce_attached_body(_py: &crate::PyToken<'_>, obj: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        bytearray_reduce(py, obj, 2).unwrap_or_else(|error| error)
    })
}

pub(crate) fn molt_bytearray_reduce_ex_attached_body(
    _py: &crate::PyToken<'_>,
    obj: u64,
    protocol: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(protocol) = crate::builtins::numbers::index_c_int_from_obj(py, protocol) else {
            return MoltObject::none().bits();
        };
        bytearray_reduce(py, obj, protocol).unwrap_or_else(|error| error)
    })
}

include!("../../attached_exports_generated/object/ops_builtins/reduction.rs");
