//! Physical C exception storage adapter for the shared rendering authority.
//! Managed isolates use the same algorithms without publishing process C views.

use super::{PyErr_BadInternalCall, PyErr_SetString};
use crate::abi_types::{PyBaseExceptionObject, PyObject};
use crate::api::{numbers, refcount, sequences, strings, typeobj};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::{
    ExceptionFieldStorage, ExceptionLayoutKind, ExceptionStrSlot, ExceptionTypedField,
    exception_render::{
        ExceptionRenderContext, ExceptionRenderOwned, ExceptionRenderSnapshot,
        render_exception_repr, render_exception_str,
    },
};
use std::{mem::ManuallyDrop, ptr};

type StringSlot = unsafe extern "C" fn(*mut PyObject) -> *mut PyObject;

pub(crate) fn exception_str_slot(slot: ExceptionStrSlot) -> StringSlot {
    match slot {
        ExceptionStrSlot::Base => molt_native_exception_str,
        ExceptionStrSlot::Group => group_str,
        ExceptionStrSlot::KeyError => key_error_str,
        ExceptionStrSlot::Syntax => syntax_error_str,
        ExceptionStrSlot::Import => import_error_str,
        ExceptionStrSlot::OSError => os_error_str,
        ExceptionStrSlot::UnicodeDecode => unicode_decode_error_str,
        ExceptionStrSlot::UnicodeEncode => unicode_encode_error_str,
        ExceptionStrSlot::UnicodeTranslate => unicode_translate_error_str,
    }
}

struct RenderValue(std::ptr::NonNull<PyObject>);

impl RenderValue {
    fn into_ptr(self) -> *mut PyObject {
        let pointer = self.0.as_ptr();
        std::mem::forget(self);
        pointer
    }
}

impl Drop for RenderValue {
    fn drop(&mut self) {
        // Snapshot rebinding may make this the final reference. Preserve both
        // error channels through callbacks during destruction.
        super::with_preserved_error(|| unsafe { refcount::Py_DECREF(self.0.as_ptr()) });
    }
}

impl ExceptionRenderOwned for RenderValue {
    fn release_all(values: impl Iterator<Item = Self>) {
        // One snapshot has one custody boundary, including when releasing its
        // last references invokes callbacks. Individual owners still use Drop.
        super::with_preserved_error(|| {
            for value in values {
                unsafe { refcount::Py_DECREF(value.into_ptr()) };
            }
        });
    }
}

struct PhysicalRender;

impl ExceptionRenderContext for PhysicalRender {
    type Object = *mut PyObject;
    type Owned = RenderValue;

    fn borrow(value: &RenderValue) -> *mut PyObject {
        value.0.as_ptr()
    }
    fn none(&self) -> *mut PyObject {
        &raw mut crate::abi_types::Py_None
    }
    fn retain(&self, value: *mut PyObject) -> RenderValue {
        unsafe { refcount::Py_INCREF(value) };
        RenderValue(std::ptr::NonNull::new(value).expect("retained render value"))
    }

    fn capture(
        &self,
        op: *mut PyObject,
        required: Option<ExceptionLayoutKind>,
        capture_args: bool,
        fields: &[ExceptionTypedField],
    ) -> Option<ExceptionRenderSnapshot<RenderValue>> {
        unsafe {
            if op.is_null() {
                PyErr_BadInternalCall();
                return None;
            }
            if let Some(value) = GLOBAL_BRIDGE.observed_handle_for_pyobj(op) {
                // Refresh before taking any borrowed fields. Runtime projection
                // still owns args materialization and typed-field custody.
                if !GLOBAL_BRIDGE.refresh_exception_view(value.bits()) {
                    return None;
                }
            } else if super::foreign_exception_layout(op).is_none() {
                bad_receiver();
                return None;
            }
            let class = (*op).ob_type;
            let Some(layout) = crate::abi_types::exception_layout_for_type(class) else {
                bad_receiver();
                return None;
            };
            if required.is_some_and(|required| required != layout) {
                bad_receiver();
                return None;
            }
            let base = op.cast::<PyBaseExceptionObject>();
            let mut snapshot = ExceptionRenderSnapshot {
                args: if capture_args && !(*base).args.is_null() {
                    Some(self.retain((*base).args))
                } else {
                    None
                },
                fields: std::array::from_fn(|_| None),
                class: ManuallyDrop::new(self.retain(class.cast())),
                layout,
                start: 0,
                end: 0,
                unicode_object_present: false,
            };
            for (index, policy) in layout.field_policies().iter().enumerate() {
                if policy.storage != ExceptionFieldStorage::PySsize
                    && fields.contains(&policy.field)
                {
                    let slot =
                        crate::abi_types::exception_typed_object_slot(base, layout, policy.field)
                            .expect("schema object field must have physical storage");
                    if !(*slot).is_null() {
                        snapshot.fields[index] = Some(self.retain(*slot));
                    }
                }
            }
            if layout == ExceptionLayoutKind::Unicode {
                let unicode = op.cast::<crate::abi_types::PyUnicodeErrorObject>();
                snapshot.start = (*unicode).start;
                snapshot.end = (*unicode).end;
                snapshot.unicode_object_present = !(*unicode).object.is_null();
            }
            Some(snapshot)
        }
    }

    fn string_from_bytes(&self, bytes: &[u8]) -> Option<RenderValue> {
        let value = unsafe { strings::unicode_from_python_bytes(bytes) };
        std::ptr::NonNull::new(value).map(RenderValue)
    }

    fn render(&self, value: Option<*mut PyObject>, repr: bool) -> Option<RenderValue> {
        let value = value.unwrap_or(ptr::null_mut());
        let result = unsafe {
            if repr {
                typeobj::PyObject_Repr(value)
            } else {
                typeobj::PyObject_Str(value)
            }
        };
        std::ptr::NonNull::new(result).map(RenderValue)
    }

    fn string_bytes(&self, value: *mut PyObject) -> Option<Vec<u8>> {
        let bytes = unsafe { strings::unicode_bytes(value) }.map(<[u8]>::to_vec);
        if bytes.is_none() {
            unsafe { PyErr_BadInternalCall() };
        }
        bytes
    }

    fn is_string(&self, value: *mut PyObject, exact: bool) -> bool {
        unsafe {
            if exact {
                strings::PyUnicode_CheckExact(value) != 0
            } else {
                strings::PyUnicode_Check(value) != 0
            }
        }
    }

    fn type_short_name(&self, class: *mut PyObject) -> Option<Vec<u8>> {
        unsafe { typeobj::type_short_name_bytes(class.cast()) }
    }

    fn tuple_len(&self, value: Option<*mut PyObject>) -> Option<usize> {
        usize::try_from(unsafe { sequences::PyTuple_Size(value.unwrap_or(ptr::null_mut())) }).ok()
    }

    fn tuple_item(&self, value: *mut PyObject, index: usize) -> Option<*mut PyObject> {
        let item = unsafe { sequences::PyTuple_GetItem(value, index as isize) };
        (!item.is_null()).then_some(item)
    }

    fn syntax_line(&self, value: *mut PyObject) -> Option<i64> {
        if unsafe { numbers::PyLong_CheckExact(value) } == 0 {
            return None;
        }
        let mut overflow = 0;
        Some(unsafe { numbers::PyLong_AsLongAndOverflow(value, &raw mut overflow) } as i64)
    }

    fn unicode_unit(
        &self,
        value: Option<*mut PyObject>,
        index: Option<usize>,
        decode: bool,
    ) -> Option<Option<u32>> {
        let value = value.unwrap_or(ptr::null_mut());
        if decode {
            let mut data = ptr::null_mut();
            let mut length = 0;
            if unsafe { strings::PyBytes_AsStringAndSize(value, &raw mut data, &raw mut length) }
                != 0
            {
                return None;
            }
            Some(
                index
                    .filter(|&index| index < length as usize)
                    .map(|index| u32::from(unsafe { *data.cast::<u8>().add(index) })),
            )
        } else {
            let Some(bytes) = (unsafe { strings::unicode_bytes(value) }) else {
                unsafe { PyErr_BadInternalCall() };
                return None;
            };
            let Some(text) = strings::PythonStringBytes::from_bytes(bytes) else {
                unsafe { PyErr_BadInternalCall() };
                return None;
            };
            Some(index.and_then(|index| text.code_points().nth(index)))
        }
    }
}

unsafe fn bad_receiver() {
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast(),
            c"exception rendering slot requires a compatible exception instance".as_ptr(),
        )
    };
}

pub unsafe extern "C" fn molt_native_exception_repr(op: *mut PyObject) -> *mut PyObject {
    render_exception_repr(&PhysicalRender, op).map_or(ptr::null_mut(), RenderValue::into_ptr)
}

macro_rules! physical_string_slots {
    ($($name:ident => $slot:ident),* $(,)?) => {
        $(pub unsafe extern "C" fn $name(op: *mut PyObject) -> *mut PyObject {
            render_exception_str(&PhysicalRender, op, ExceptionStrSlot::$slot)
                .map_or(ptr::null_mut(), RenderValue::into_ptr)
        })*
    };
}

physical_string_slots! {
    molt_native_exception_str => Base,
    group_str => Group,
    key_error_str => KeyError,
    syntax_error_str => Syntax,
    import_error_str => Import,
    os_error_str => OSError,
    unicode_decode_error_str => UnicodeDecode,
    unicode_encode_error_str => UnicodeEncode,
    unicode_translate_error_str => UnicodeTranslate,
}

/// Render a declaring Rust slot against an already admitted native pointer.
pub unsafe fn render_native_exception_str(
    op: *mut PyObject,
    slot: ExceptionStrSlot,
) -> *mut PyObject {
    render_exception_str(&PhysicalRender, op, slot).map_or(ptr::null_mut(), RenderValue::into_ptr)
}
