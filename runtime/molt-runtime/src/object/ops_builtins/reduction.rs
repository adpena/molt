//! Default object reduction: CPython 3.12 Objects/typeobject.c,
//! object___reduce_ex___impl, _common_reduce, reduce_newobj and
//! _PyObject_GetNewArguments. Copy and serialization consume this authority.
use crate::builtins::attr::clear_attribute_error_if_pending;
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

fn attr<'a>(py: &'a PyToken<'_>, obj: u64, name: &[u8]) -> Result<Option<Value<'a>>, u64> {
    let name = attr_name_bits_from_bytes(py, name).ok_or_else(|| MoltObject::none().bits())?;
    let name = owned(py, name)?;
    let bits = molt_getattr_builtin(obj, name.bits(), missing_bits(py));
    if exception_pending(py) && clear_attribute_error_if_pending(py) {
        return Ok(None);
    }
    let value = owned(py, bits)?;
    if is_missing_bits(py, value.bits()) {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn required<'a>(py: &'a PyToken<'_>, obj: u64, name: &[u8]) -> Result<Value<'a>, u64> {
    let name = attr_name_bits_from_bytes(py, name).ok_or_else(|| MoltObject::none().bits())?;
    let name = owned(py, name)?;
    owned(py, molt_getattr_builtin(obj, name.bits(), missing_bits(py)))
}

fn special<'a>(py: &'a PyToken<'_>, obj: u64, name: &[u8]) -> Result<Option<Value<'a>>, u64> {
    unsafe { crate::builtins::attr::lookup_special_method(py, obj, name) }
        .map_err(|()| MoltObject::none().bits())?
        .map(|bits| owned(py, bits))
        .transpose()
}

fn copyreg<'a>(py: &'a PyToken<'_>, name: &[u8]) -> Result<Value<'a>, u64> {
    let module_name = alloc_string(py, b"copyreg");
    if module_name.is_null() {
        return Err(MoltObject::none().bits());
    }
    let module_name = owned(py, MoltObject::from_ptr(module_name).bits())?;
    let module = owned(py, molt_module_import(module_name.bits()))?;
    required(py, module.bits(), name)
}

fn require_kind(py: &PyToken<'_>, bits: u64, kind: u32, message: &str) -> Result<(), u64> {
    if obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == kind })
    {
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
    if let Some(method) = special(py, obj, b"__getnewargs_ex__")? {
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
    if let Some(method) = special(py, obj, b"__getnewargs__")? {
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
    let cls = Value::retain(py.core_token(), type_of_bits(py, obj));
    // A class must supply construction. Preserve lookup errors and descriptors.
    let _new = required(py, cls.bits(), b"__new__")?;
    let (args, kwargs) = new_arguments(py, obj)?;
    let (constructor, newargs) = if kwargs
        .as_ref()
        .is_some_and(|kw| unsafe { dict_len(obj_from_bits(kw.bits()).as_ptr().unwrap()) != 0 })
    {
        let args = args.as_ref().unwrap();
        (
            copyreg(py, b"__newobj_ex__")?,
            tuple(
                py,
                &[cls.bits(), args.bits(), kwargs.as_ref().unwrap().bits()],
            )?,
        )
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
        (copyreg(py, b"__newobj__")?, tuple(py, &fields)?)
    };
    let getstate = required(py, obj, b"__getstate__")?;
    let state = owned(py, unsafe { call_callable0(py, getstate.bits()) })?;
    let type_id = obj_from_bits(obj)
        .as_ptr()
        .map(|ptr| unsafe { object_type_id(ptr) });
    let listitems = if matches!(
        type_id,
        Some(TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL)
    ) {
        owned(py, molt_iter(obj))?
    } else {
        Value::retain(py.core_token(), MoltObject::none().bits())
    };
    let is_dict = type_id == Some(TYPE_ID_DICT)
        || obj_from_bits(obj).as_ptr().is_some_and(|ptr| unsafe {
            crate::object::layout::dict_subclass_storage_slot(ptr).is_some()
        });
    let dictitems = if is_dict {
        let items = required(py, obj, b"items")?;
        let items = owned(py, unsafe { call_callable0(py, items.bits()) })?;
        owned(py, molt_iter(items.bits()))?
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
        let reducer = copyreg(py, b"_reduce_ex")?;
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_reduce(obj: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        common_reduce(py, obj, 0).unwrap_or_else(|error| error)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_reduce_ex(obj: u64, protocol: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(protocol) = crate::builtins::numbers::index_c_int_from_obj(py, protocol) else {
            return MoltObject::none().bits();
        };
        let result = (|| {
            if let Some(reduce) = attr(py, obj, b"__reduce__")? {
                let cls = Value::retain(py.core_token(), type_of_bits(py, obj));
                let class_reduce = required(py, cls.bits(), b"__reduce__")?;
                let base_reduce = required(py, builtin_classes(py).object, b"__reduce__")?;
                if class_reduce.bits() != base_reduce.bits() {
                    return Ok(owned(py, unsafe { call_callable0(py, reduce.bits()) })?.into_bits());
                }
            }
            common_reduce(py, obj, protocol)
        })();
        result.unwrap_or_else(|error| error)
    })
}
