//! Runtime-local exception descriptors and managed storage for the shared renderer.
//! Neither descriptor publication nor managed rendering acquires process C state.

use super::*;
use molt_obj_model::{
    ExceptionStrSlot,
    exception_render::{
        ExceptionRenderContext, ExceptionRenderSnapshot, render_exception_repr,
        render_exception_str,
    },
};

/// Resolve the immutable factory identity, never a mutable __name__ or module
/// binding. The existing class cache owns non-root identities; no second table.
fn declaring_spec(py: &PyToken<'_>, class: u64) -> Option<&'static BuiltinExceptionSpec> {
    let builtins = builtin_classes(py);
    for (bits, name) in [
        (builtins.base_exception, "BaseException"),
        (builtins.exception, "Exception"),
        (builtins.base_exception_group, "BaseExceptionGroup"),
        (builtins.exception_group, "ExceptionGroup"),
    ] {
        if class == bits {
            return builtin_exception_spec(name);
        }
    }
    if !is_builtin_exception_class_bits(py, class) {
        return None;
    }
    exception_type_cache(py)
        .lock()
        .unwrap()
        .iter()
        .find_map(|(name, &bits)| {
            (class == bits)
                .then(|| builtin_exception_spec(name))
                .flatten()
        })
}

pub(crate) fn exception_render_method_bits(
    py: &PyToken<'_>,
    owner: u64,
    name: &str,
) -> Result<Option<u64>, ()> {
    crate::builtins::methods::method_dispatch(py, || {
        let Some(declaration) = declaring_spec(py, owner) else {
            return Ok(None);
        };
        let (name, address) = match name {
            "__str__" => (
                "__str__",
                string_slot_address(match declaration.declared_str_slot() {
                    Some(slot) => slot,
                    None => return Ok(None),
                }),
            ),
            "__repr__" if declaration.declares_repr() => ("__repr__", fn_addr!(exception_repr)),
            _ => return Ok(None),
        };
        Ok(Some(builtin_func_bits(
            py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, owner, name)
                .with_text_signature("($self, /)"),
            address,
            1,
        )))
    })
}

pub(crate) fn publish_exception_render_methods(py: &PyToken<'_>, owner: u64) -> bool {
    // Declarations come from the same schema path used by direct lookup. A
    // subclass inherits a descriptor; it does not acquire its own dictionary row.
    for name in ["__str__", "__repr__"] {
        if exception_render_method_bits(py, owner, name).is_err() {
            return false;
        }
    }
    true
}

macro_rules! managed_string_slots {
    ($($name:ident => $slot:ident),* $(,)?) => {
        fn string_slot_address(slot: ExceptionStrSlot) -> u64 {
            match slot { $(ExceptionStrSlot::$slot => fn_addr!($name),)* }
        }
        $(extern "C" fn $name(object: u64) -> u64 {
            crate::with_gil_entry_nopanic!(py, {
                invoke(py, object, Some(ExceptionStrSlot::$slot))
            })
        })*
    };
}

managed_string_slots! {
    exception_str => Base,
    group_str => Group,
    key_error_str => KeyError,
    syntax_str => Syntax,
    import_str => Import,
    os_str => OSError,
    unicode_decode_str => UnicodeDecode,
    unicode_encode_str => UnicodeEncode,
    unicode_translate_str => UnicodeTranslate,
}

extern "C" fn exception_repr(object: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { invoke(py, object, None) })
}

fn invoke(py: &PyToken<'_>, object: u64, slot: Option<ExceptionStrSlot>) -> u64 {
    let Some(storage) = ExceptionStorage::for_exception(py, object) else {
        if !exception_pending(py) {
            raise_exception::<()>(
                py,
                "TypeError",
                "exception rendering slot requires a compatible exception instance",
            );
        }
        return MoltObject::none().bits();
    };
    let _receiver = ExceptionValue::pin(py, object);
    let result = match storage {
        ExceptionStorage::Managed(_) => {
            let context = ManagedRender(py);
            match slot {
                Some(slot) => render_exception_str(&context, object, slot),
                None => render_exception_repr(&context, object),
            }
        }
        ExceptionStorage::Native(pointer) => unsafe {
            // Physical C storage already owns a complete adapter. This is a
            // borrowed native pointer, never projection of a managed isolate.
            let value = match slot {
                Some(slot) => {
                    molt_cpython_abi::api::errors::render_native_exception_str(pointer, slot)
                }
                None => molt_cpython_abi::api::errors::molt_native_exception_repr(pointer),
            };
            if value.is_null() {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "native exception rendering",
                );
                None
            } else {
                storage::native_owned_value(py, value)
            }
        },
    };
    result.map_or(MoltObject::none().bits(), ExceptionValue::into_bits)
}

struct ManagedRender<'a, 'py>(&'a PyToken<'py>);

impl ManagedRender<'_, '_> {
    /// Typed fields can retain genuine C values after exception-view commit.
    /// The snapshot owns their wrappers; reading their physical pointer never
    /// projects a managed value or acquires process-static C ownership.
    fn native(value: u64) -> Option<*mut molt_cpython_abi::abi_types::PyObject> {
        let pointer = obj_from_bits(value).as_ptr()?;
        (unsafe { object_type_id(pointer) } == crate::TYPE_ID_FOREIGN).then(|| {
            std::ptr::with_exposed_provenance_mut(unsafe {
                crate::object::foreign::foreign_ptr_from_obj(pointer)
            })
        })
    }

    fn invalid<T>(&self) -> Option<T> {
        if !exception_pending(self.0) {
            raise_exception::<()>(self.0, "SystemError", "bad argument to internal function");
        }
        None
    }
}

impl<'a, 'py> ExceptionRenderContext for ManagedRender<'a, 'py> {
    type Object = u64;
    type Owned = ExceptionValue<'a, 'py>;

    fn borrow(value: &Self::Owned) -> u64 {
        value.bits()
    }
    fn none(&self) -> u64 {
        MoltObject::none().bits()
    }
    fn retain(&self, value: u64) -> Self::Owned {
        ExceptionValue::pin(self.0, value)
    }

    fn capture(
        &self,
        object: u64,
        required: Option<ExceptionLayoutKind>,
        capture_args: bool,
        fields: &[ExceptionTypedField],
    ) -> Option<ExceptionRenderSnapshot<Self::Owned>> {
        let pointer = obj_from_bits(object).as_ptr()?;
        if unsafe { object_type_id(pointer) } != TYPE_ID_EXCEPTION {
            return self.invalid();
        }
        let layout = unsafe { exception_layout_kind(pointer) };
        if required.is_some_and(|required| required != layout) {
            raise_exception::<()>(
                self.0,
                "TypeError",
                "exception rendering slot requires a compatible exception instance",
            );
            return None;
        }
        let args = if capture_args {
            Some(self.retain(exception_materialized_args_bits(self.0, pointer)?))
        } else {
            None
        };
        let mut snapshot = ExceptionRenderSnapshot {
            args,
            fields: std::array::from_fn(|_| None),
            class: self.retain(unsafe { object_class_bits(pointer) }),
            layout,
            start: 0,
            end: 0,
            unicode_object_present: false,
        };
        for (index, policy) in layout.field_policies().iter().enumerate() {
            if policy.storage != ExceptionFieldStorage::PySsize && fields.contains(&policy.field) {
                let raw = unsafe { exception_typed_field_raw_bits(pointer, policy.field) }
                    .expect("schema object field must have managed storage");
                if !exception_field_is_missing(raw) {
                    snapshot.fields[index] = Some(self.retain(raw));
                }
            }
        }
        if layout == ExceptionLayoutKind::Unicode {
            snapshot.start = unsafe {
                exception_typed_field_raw_bits(pointer, ExceptionTypedField::UnicodeStart)
            }? as isize;
            snapshot.end = unsafe {
                exception_typed_field_raw_bits(pointer, ExceptionTypedField::UnicodeEnd)
            }? as isize;
            snapshot.unicode_object_present = !exception_field_is_missing(unsafe {
                exception_typed_field_raw_bits(pointer, ExceptionTypedField::UnicodeObject)
            }?);
        }
        Some(snapshot)
    }

    fn string_from_bytes(&self, bytes: &[u8]) -> Option<Self::Owned> {
        let pointer = alloc_string(self.0, bytes);
        if pointer.is_null() {
            return None;
        }
        Some(ExceptionValue::adopt(
            self.0,
            MoltObject::from_ptr(pointer).bits(),
        ))
    }

    fn render(&self, value: Option<u64>, repr: bool) -> Option<Self::Owned> {
        let Some(value) = value else {
            return self.string_from_bytes(b"<NULL>");
        };
        let result = ExceptionValue::adopt(
            self.0,
            if repr {
                molt_repr_from_obj(value)
            } else {
                molt_str_from_obj(value)
            },
        );
        if exception_pending(self.0) {
            None
        } else {
            Some(result)
        }
    }

    fn string_bytes(&self, value: u64) -> Option<Vec<u8>> {
        crate::object::ops_format::string_obj_bytes(obj_from_bits(value)).or_else(|| self.invalid())
    }

    fn is_string(&self, value: u64, exact: bool) -> bool {
        if let Some(native) = Self::native(value) {
            return unsafe {
                if exact {
                    molt_cpython_abi::api::strings::PyUnicode_CheckExact(native) != 0
                } else {
                    molt_cpython_abi::api::strings::PyUnicode_Check(native) != 0
                }
            };
        }
        let class = type_of_bits(self.0, value);
        let string = builtin_classes(self.0).str;
        if exact {
            class == string
        } else {
            issubclass_bits(class, string)
        }
    }

    fn type_short_name(&self, class: u64) -> Option<Vec<u8>> {
        let bytes = crate::object::ops_format::format_class_name_bytes(class);
        let start = bytes
            .iter()
            .rposition(|&byte| byte == b'.')
            .map_or(0, |index| index + 1);
        Some(bytes[start..].to_vec())
    }

    fn tuple_len(&self, value: Option<u64>) -> Option<usize> {
        if let Some(value) = value
            && Self::native(value).is_some()
        {
            // GroupExceptions admits native tuples. Read the physical root
            // length through its existing owner, bypassing subtype __len__.
            return crate::object::tuple_storage::TupleStorage::from_bits(self.0, value)
                .and_then(|tuple| tuple.len())
                .or_else(|| self.invalid());
        }
        value
            .and_then(|value| obj_from_bits(value).as_ptr())
            .and_then(|pointer| unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(pointer, |items| items.len())
            })
            .or_else(|| self.invalid())
    }

    fn tuple_item(&self, value: u64, index: usize) -> Option<u64> {
        // Only base args use this borrowed-item read. Their publication and C
        // snapshot commit require managed tuple storage; typed group children
        // use tuple_len and never expose a borrowed native item as runtime bits.
        obj_from_bits(value)
            .as_ptr()
            .and_then(|pointer| unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(pointer, |items| {
                    items.get(index).copied()
                })
            })
            .flatten()
            .or_else(|| self.invalid())
    }

    #[allow(clippy::useless_conversion, clippy::unnecessary_cast)] // C long differs on LLP64 and wasm32.
    fn syntax_line(&self, value: u64) -> Option<i64> {
        if let Some(native) = Self::native(value) {
            use molt_cpython_abi::api::{errors, numbers};
            if unsafe { numbers::PyLong_CheckExact(native) } == 0 {
                return None;
            }
            let mut overflow = 0;
            let line = unsafe { numbers::PyLong_AsLongAndOverflow(native, &raw mut overflow) };
            if !unsafe { errors::PyErr_Occurred() }.is_null() {
                crate::cpython_abi_hooks::propagate_native_failure(self.0, "exception line number");
                return None;
            }
            return Some(line as i64);
        }
        if type_of_bits(self.0, value) != builtin_classes(self.0).int {
            return None;
        }
        // Match PyLong_AsLongAndOverflow, including LLP64's 32-bit C long.
        Some(
            to_i64(obj_from_bits(value))
                .and_then(|number| libc::c_long::try_from(number).ok())
                .map_or(-1, |number| number as i64),
        )
    }

    fn unicode_unit(
        &self,
        value: Option<u64>,
        index: Option<usize>,
        decode: bool,
    ) -> Option<Option<u32>> {
        if decode {
            if let Some(native) = value.and_then(Self::native) {
                let mut data = std::ptr::null_mut();
                let mut len = 0;
                if unsafe {
                    molt_cpython_abi::api::strings::PyBytes_AsStringAndSize(
                        native,
                        &raw mut data,
                        &raw mut len,
                    )
                } != 0
                {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        self.0,
                        "exception byte input",
                    );
                    return None;
                }
                return Some(
                    index
                        .filter(|&index| index < len as usize)
                        .map(|index| u32::from(unsafe { *data.cast::<u8>().add(index) })),
                );
            }
            let bytes = value
                .and_then(|value| obj_from_bits(value).as_ptr())
                .filter(|&pointer| unsafe { object_type_id(pointer) } == crate::TYPE_ID_BYTES)
                .and_then(|pointer| unsafe { bytes_like_slice(pointer) });
            let Some(bytes) = bytes else {
                let name = value.map_or_else(
                    || "object".to_owned(),
                    |value| type_name(self.0, obj_from_bits(value)).into_owned(),
                );
                raise_exception::<()>(
                    self.0,
                    "TypeError",
                    &format!("expected bytes, {name:.200} found"),
                );
                return None;
            };
            Some(
                index
                    .and_then(|index| bytes.get(index).copied())
                    .map(u32::from),
            )
        } else {
            let Some(value) = value else {
                return self.invalid();
            };
            unsafe {
                crate::object::ops_format::with_string_bytes(obj_from_bits(value), |bytes| {
                    index.and_then(|index| {
                        molt_obj_model::text_storage::code_points(bytes).nth(index)
                    })
                })
            }
            .or_else(|| self.invalid())
        }
    }
}

#[cfg(test)]
#[path = "rendering_tests.rs"]
pub(crate) mod tests;
