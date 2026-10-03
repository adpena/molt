//! Physical exception storage behind the shared Python exception protocol.
//! The foreign wrapper remains the identity owner; field projections are
//! explicitly owned and never masquerade as borrows from native storage.

use super::*;
use molt_cpython_abi::{
    abi_types as cabi,
    api::{errors as cerrors, refcount as cref},
    bridge::GLOBAL_BRIDGE,
};

fn fail<T: ExceptionSentinel>(py: &PyToken<'_>, kind: &str, message: &str) -> T {
    if exception_pending(py) {
        T::exception_sentinel()
    } else {
        raise_exception(py, kind, message)
    }
}

/// One owned reference held across Python callbacks. Release keeps a pending
/// exception exact: finalizers it triggers cannot replace the raised error.
pub(crate) struct ExceptionValue<'a, 'py> {
    pub(crate) py: &'a PyToken<'py>,
    bits: u64,
}

impl<'a, 'py> ExceptionValue<'a, 'py> {
    /// Take custody of an already-owned reference.
    pub(crate) fn adopt(py: &'a PyToken<'py>, bits: u64) -> Self {
        Self { py, bits }
    }

    /// Pin a borrowed reference for the guard's lifetime.
    pub(crate) fn pin(py: &'a PyToken<'py>, bits: u64) -> Self {
        inc_ref_bits(py, bits);
        Self { py, bits }
    }

    pub(crate) fn bits(&self) -> u64 {
        self.bits
    }

    /// Transfer the owned reference to the caller.
    pub(crate) fn into_bits(self) -> u64 {
        let bits = self.bits;
        std::mem::forget(self);
        bits
    }
}

impl Drop for ExceptionValue<'_, '_> {
    fn drop(&mut self) {
        cerrors::with_preserved_error(|| dec_ref_bits(self.py, self.bits));
    }
}

/// A checked exception's physical representation. Field policy and offsets
/// remain owned by the shared exception schema and the existing C-API writers.
#[derive(Clone, Copy)]
pub(crate) enum ExceptionStorage {
    Managed(*mut u8),
    Native(*mut cabi::PyObject),
}

impl ExceptionStorage {
    pub(crate) fn for_exception(py: &PyToken<'_>, bits: u64) -> Option<Self> {
        if !exception_is_instance(py, bits) {
            return None;
        }
        let object = obj_from_bits(bits).as_ptr()?;
        Some(if unsafe { object_type_id(object) } == TYPE_ID_EXCEPTION {
            Self::Managed(object)
        } else {
            Self::Native(std::ptr::with_exposed_provenance_mut(unsafe {
                crate::object::foreign::foreign_ptr_from_obj(object)
            }))
        })
    }

    pub(crate) fn typed_field<'a, 'py>(
        self,
        py: &'a PyToken<'py>,
        field: ExceptionTypedField,
    ) -> Option<ExceptionValue<'a, 'py>> {
        match self {
            Self::Managed(ptr) => match exception_typed_field_get(py, ptr, field) {
                Some(Ok(bits)) => Some(ExceptionValue::adopt(py, bits)),
                Some(Err(name)) => fail(py, "AttributeError", name),
                None => fail(
                    py,
                    "SystemError",
                    "exception field is absent from its layout",
                ),
            },
            Self::Native(ptr) => unsafe {
                let Some(layout) = cabi::exception_layout_for_type((*ptr).ob_type) else {
                    return fail(py, "SystemError", "exception has no native layout");
                };
                let Some(policy) = layout.field_policy(field) else {
                    return fail(
                        py,
                        "SystemError",
                        "exception field is absent from its layout",
                    );
                };
                if policy.storage == ExceptionFieldStorage::PySsize {
                    let Some(offset) = cabi::exception_typed_field_offset(field) else {
                        return fail(py, "SystemError", "exception field has no native offset");
                    };
                    let raw = *ptr.cast::<u8>().add(offset as usize).cast::<isize>();
                    if policy.missing_read == ExceptionMissingRead::AttributeError && raw == -1 {
                        return fail(py, "AttributeError", policy.python_name);
                    }
                    return Some(ExceptionValue::adopt(py, int_bits_from_i64(py, raw as i64)));
                }
                let Some(slot) = cabi::exception_typed_object_slot(ptr.cast(), layout, field)
                else {
                    return fail(
                        py,
                        "SystemError",
                        "exception field is absent from its layout",
                    );
                };
                let value = *slot;
                if value.is_null() {
                    return match policy.missing_read {
                        ExceptionMissingRead::None => {
                            Some(ExceptionValue::pin(py, MoltObject::none().bits()))
                        }
                        ExceptionMissingRead::AttributeError => {
                            fail(py, "AttributeError", policy.python_name)
                        }
                    };
                }
                // Retain before conversion can publish a wrapper or run cleanup.
                cref::Py_INCREF(value);
                native_owned_value(py, value)
            },
        }
    }

    pub(crate) fn metadata<'a, 'py>(
        self,
        py: &'a PyToken<'py>,
        field: ExceptionFieldSlot,
    ) -> Option<ExceptionValue<'a, 'py>> {
        if matches!(field, ExceptionFieldSlot::Traceback)
            && let Self::Managed(ptr) = self
        {
            let mut result = None;
            with_saved_raised_exception(py, || {
                let bits = crate::exception_materialize_traceback_bits(py, ptr);
                if exception_pending(py) {
                    return false;
                }
                result = Some(ExceptionValue::pin(py, bits));
                true
            });
            return result;
        }
        self.read(py, field)
    }

    /// Retain a field before any callback may replace its native or managed
    /// owner. Traceback payloads stay lazy until a Python/C observer needs one.
    pub(crate) fn read<'a, 'py>(
        self,
        py: &'a PyToken<'py>,
        field: ExceptionFieldSlot,
    ) -> Option<ExceptionValue<'a, 'py>> {
        match self {
            Self::Managed(ptr) => {
                let bits = unsafe {
                    match field {
                        ExceptionFieldSlot::Args => exception_materialized_args_bits(py, ptr)?,
                        _ => *(ptr.add(field.offset() * std::mem::size_of::<u64>()) as *const u64),
                    }
                };
                Some(ExceptionValue::pin(py, bits))
            }
            Self::Native(ptr) => unsafe {
                // for_exception admitted the physical BaseException prefix.
                // Read storage without attribute dispatch or a pending-error
                // probe: callers may be observing this very raised exception.
                let base = &*ptr.cast::<cabi::PyBaseExceptionObject>();
                let value = match field {
                    ExceptionFieldSlot::Traceback => base.traceback,
                    ExceptionFieldSlot::Context => base.context,
                    ExceptionFieldSlot::Cause => base.cause,
                    ExceptionFieldSlot::Args => base.args,
                    ExceptionFieldSlot::Dict => base.dict,
                    ExceptionFieldSlot::Notes => base.notes,
                };
                if value.is_null() {
                    Some(ExceptionValue::pin(py, MoltObject::none().bits()))
                } else {
                    cref::Py_INCREF(value);
                    native_owned_value(py, value)
                }
            },
        }
    }

    pub(crate) fn suppress_context(self) -> bool {
        unsafe {
            match self {
                Self::Managed(ptr) => obj_from_bits(exception_suppress_bits(ptr))
                    .as_bool()
                    .unwrap_or(false),
                Self::Native(ptr) => {
                    (*ptr.cast::<cabi::PyBaseExceptionObject>()).suppress_context != 0
                }
            }
        }
    }

    /// Diagnostic names do not invoke Python or materialize a runtime class.
    pub(crate) fn class_name(self) -> String {
        unsafe {
            match self {
                Self::Managed(ptr) => string_obj_to_owned(obj_from_bits(exception_kind_bits(ptr)))
                    .unwrap_or_else(|| "<unknown>".into()),
                Self::Native(ptr) => {
                    let name = (*(*ptr).ob_type).tp_name;
                    if name.is_null() {
                        "<unknown>".into()
                    } else {
                        std::ffi::CStr::from_ptr(name)
                            .to_string_lossy()
                            .into_owned()
                    }
                }
            }
        }
    }

    pub(crate) fn publish(
        self,
        py: &PyToken<'_>,
        field: ExceptionFieldSlot,
        value: u64,
    ) -> Option<()> {
        if matches!(self, Self::Native(_)) {
            return with_saved_raised_exception(py, || {
                self.publish_inner(py, field, value).is_some()
            })
            .then_some(());
        }
        self.publish_inner(py, field, value)
    }

    fn publish_inner(self, py: &PyToken<'_>, field: ExceptionFieldSlot, value: u64) -> Option<()> {
        match self {
            Self::Managed(ptr) => {
                if matches!(field, ExceptionFieldSlot::Traceback) {
                    if !unsafe { exception_publish_field_slot(py, ptr, field, value) } {
                        return fail(py, "SystemError", "exception traceback publication failed");
                    }
                } else if let Err(message) =
                    exception_replace_field_bits(py, MoltObject::from_ptr(ptr).bits(), field, value)
                {
                    return fail(py, "SystemError", message);
                }
                Some(())
            }
            Self::Native(ptr) => unsafe {
                // A native C exception exposes a real traceback pointer, so
                // convert the runtime's private lazy payload at this boundary.
                let materialized = if matches!(field, ExceptionFieldSlot::Traceback)
                    && traceback_payload_is_lazy(value)
                {
                    let bits =
                        crate::builtins::frames::traceback_payload_to_traceback_bits(py, value);
                    if obj_from_bits(bits).is_none() {
                        return fail(py, "MemoryError", "traceback materialization failed");
                    }
                    Some(ExceptionValue::adopt(py, bits))
                } else {
                    None
                };
                let value = materialized.as_ref().map_or(value, ExceptionValue::bits);
                let c_value = if obj_from_bits(value).is_none() {
                    std::ptr::null_mut()
                } else {
                    let projected = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(value);
                    if projected.is_null() {
                        crate::cpython_abi_hooks::propagate_native_failure(
                            py,
                            "exception metadata projection",
                        );
                        return None;
                    }
                    projected
                };
                let status = match field {
                    ExceptionFieldSlot::Traceback => {
                        let status = cerrors::PyException_SetTraceback(ptr, c_value);
                        cerrors::with_preserved_error(|| cref::Py_XDECREF(c_value));
                        status
                    }
                    ExceptionFieldSlot::Context => {
                        cerrors::PyException_SetContext(ptr, c_value);
                        0
                    }
                    ExceptionFieldSlot::Cause => {
                        cerrors::PyException_SetCause(ptr, c_value);
                        0
                    }
                    ExceptionFieldSlot::Args => {
                        cerrors::PyException_SetArgs(ptr, c_value);
                        cerrors::with_preserved_error(|| cref::Py_XDECREF(c_value));
                        0
                    }
                    ExceptionFieldSlot::Dict | ExceptionFieldSlot::Notes => {
                        let base = &mut *ptr.cast::<cabi::PyBaseExceptionObject>();
                        let slot = if matches!(field, ExceptionFieldSlot::Dict) {
                            &mut base.dict
                        } else {
                            &mut base.notes
                        };
                        let previous = std::mem::replace(slot, c_value);
                        cerrors::with_preserved_error(|| cref::Py_XDECREF(previous));
                        0
                    }
                };
                if status != 0 || !cerrors::PyErr_Occurred().is_null() {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "exception metadata publication",
                    );
                    None
                } else {
                    Some(())
                }
            },
        }
    }
}

pub(crate) fn exception_field<'a, 'py>(
    py: &'a PyToken<'py>,
    bits: u64,
    field: ExceptionFieldSlot,
) -> Option<ExceptionValue<'a, 'py>> {
    ExceptionStorage::for_exception(py, bits)?.read(py, field)
}

/// Native projection of the same bounded, atomic typed-field transaction used
/// for managed storage. Convert every input before publishing any slot, then
/// release the replaced graph only after the whole new snapshot is visible.
pub(super) unsafe fn replace_native_typed_fields(
    py: &PyToken<'_>,
    ptr: *mut cabi::PyObject,
    fields: &[(ExceptionTypedField, u64)],
) -> Result<(), &'static str> {
    enum Value {
        Object(cref::OwnedPyObject),
        Scalar(isize),
    }
    let mut result = Err("native typed exception update failed");
    with_saved_raised_exception(py, || {
        result = (|| unsafe {
            let layout = cabi::exception_layout_for_type((*ptr).ob_type)
                .ok_or("exception has no native layout")?;
            if fields.len() > MAX_EXCEPTION_TYPED_FIELDS {
                return Err("typed exception update exceeds schema field bound");
            }
            let mut updates: [Option<(usize, Value)>; MAX_EXCEPTION_TYPED_FIELDS] =
                std::array::from_fn(|_| None);
            for (index, &(field, bits)) in fields.iter().enumerate() {
                let policy = layout
                    .field_policy(field)
                    .ok_or("typed field does not belong to exception layout")?;
                let offset = cabi::exception_typed_field_offset(field)
                    .ok_or("typed field has no native storage slot")?
                    as usize;
                if updates[..index]
                    .iter()
                    .flatten()
                    .any(|(previous, _)| *previous == offset)
                {
                    return Err("duplicate typed exception field update");
                }
                let value = if policy.storage == ExceptionFieldStorage::PySsize {
                    Value::Scalar(typed_py_ssize_raw_from_bits(py, field, bits)? as isize)
                } else {
                    let object = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
                    if object.is_null() {
                        crate::cpython_abi_hooks::propagate_native_failure(
                            py,
                            "typed exception field projection",
                        );
                        return Err("typed exception field projection failed");
                    }
                    Value::Object(cref::OwnedPyObject::from_owned(object))
                };
                updates[index] = Some((offset, value));
            }
            let mut retired: [Option<cref::OwnedPyObject>; MAX_EXCEPTION_TYPED_FIELDS] =
                std::array::from_fn(|_| None);
            for (index, (offset, value)) in updates.into_iter().flatten().enumerate() {
                let address = ptr.cast::<u8>().add(offset);
                match value {
                    Value::Object(value) => {
                        let old = std::mem::replace(
                            &mut *address.cast::<*mut cabi::PyObject>(),
                            value.into_ptr(),
                        );
                        retired[index] = Some(cref::OwnedPyObject::from_owned(old));
                    }
                    Value::Scalar(value) => *address.cast::<isize>() = value,
                }
            }
            drop(retired);
            Ok(())
        })();
        result.is_ok()
    });
    result
}

pub(crate) fn exception_traceback<'a, 'py>(
    py: &'a PyToken<'py>,
    bits: u64,
) -> Option<ExceptionValue<'a, 'py>> {
    ExceptionStorage::for_exception(py, bits)?.metadata(py, ExceptionFieldSlot::Traceback)
}

pub(crate) fn exception_class<'a, 'py>(
    py: &'a PyToken<'py>,
    bits: u64,
) -> Option<ExceptionValue<'a, 'py>> {
    ExceptionStorage::for_exception(py, bits)?;
    match unsafe { crate::object::class_layout::real_type_bits(py, bits) } {
        Ok(class) => Some(ExceptionValue::adopt(py, class)),
        Err(()) => {
            crate::cpython_abi_hooks::propagate_native_failure(py, "exception class projection");
            None
        }
    }
}

/// Consume a new C reference and retain its canonical runtime identity.
pub(crate) unsafe fn native_owned_value<'a, 'py>(
    py: &'a PyToken<'py>,
    value: *mut cabi::PyObject,
) -> Option<ExceptionValue<'a, 'py>> {
    let bits = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(value) };
    cerrors::with_preserved_error(|| unsafe { cref::Py_DECREF(value) });
    match bits {
        Some(bits) => Some(ExceptionValue::adopt(py, bits)),
        None => {
            crate::cpython_abi_hooks::propagate_native_failure(py, "exception field conversion");
            None
        }
    }
}
