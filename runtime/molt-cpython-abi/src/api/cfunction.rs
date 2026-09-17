//! The C method calling-convention authority shared by ABI objects and runtime closures.
//!
//! Inputs are a borrowed vectorcall span: positional values followed by keyword
//! values, with a tuple of keyword names. Only tuple-based conventions allocate
//! argument containers. Callers own the input views and validate/translate the
//! returned C result at their execution boundary.

use crate::abi_types::{
    METH_CLASS, METH_COEXIST, METH_FASTCALL, METH_KEYWORDS, METH_METHOD, METH_NOARGS, METH_O,
    METH_STATIC, METH_VARARGS, Py_ssize_t, PyCFunction, PyCFunctionFast,
    PyCFunctionFastWithKeywords, PyCFunctionWithKeywords, PyCMethod, PyObject, PyTypeObject,
};
use std::os::raw::c_int;
use std::ptr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CFunctionConvention {
    NoArgs,
    OneObject,
    VarArgs,
    VarArgsKeywords,
    FastCall,
    FastCallKeywords,
    Method,
}

impl CFunctionConvention {
    pub fn from_flags(flags: c_int) -> Option<Self> {
        if flags & (METH_CLASS | METH_STATIC) == (METH_CLASS | METH_STATIC) {
            return None;
        }
        let convention = flags & !(METH_CLASS | METH_STATIC | METH_COEXIST);
        match convention {
            METH_NOARGS => Some(Self::NoArgs),
            METH_O => Some(Self::OneObject),
            METH_VARARGS => Some(Self::VarArgs),
            METH_FASTCALL => Some(Self::FastCall),
            value if value == METH_VARARGS | METH_KEYWORDS => Some(Self::VarArgsKeywords),
            value if value == METH_FASTCALL | METH_KEYWORDS => Some(Self::FastCallKeywords),
            value if value == METH_METHOD | METH_FASTCALL | METH_KEYWORDS => Some(Self::Method),
            _ => None,
        }
    }

    pub fn arity(self) -> u64 {
        u64::from(self == Self::OneObject)
    }

    pub fn is_variadic(self) -> bool {
        !matches!(self, Self::NoArgs | Self::OneObject)
    }

    /// Invoke a validated C method target without duplicating convention policy.
    ///
    /// # Safety
    /// The target must have the signature selected by `self`. All objects in
    /// `args`, `self_obj`, `defining_class`, and `kwnames` must remain alive for
    /// this call, including reentry. `args` contains all positional and keyword
    /// values; `kwnames` is NULL or a tuple of unique string names.
    pub unsafe fn invoke(
        self,
        meth_target: *const (),
        self_obj: *mut PyObject,
        defining_class: *mut PyTypeObject,
        args: &[*mut PyObject],
        positional_count: usize,
        kwnames: *mut PyObject,
        name: impl Fn() -> String,
    ) -> *mut PyObject {
        let Some(keyword_count) = (unsafe { validate_arguments(args, positional_count, kwnames) })
        else {
            return ptr::null_mut();
        };
        if meth_target.is_null() || (self == Self::Method) != !defining_class.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return ptr::null_mut();
        }
        if keyword_count != 0
            && !matches!(
                self,
                Self::VarArgsKeywords | Self::FastCallKeywords | Self::Method
            )
        {
            return unsafe { type_error(format!("{}() takes no keyword arguments", name())) };
        }
        match self {
            Self::NoArgs if positional_count != 0 => {
                return unsafe {
                    type_error(format!(
                        "{}() takes no arguments ({positional_count} given)",
                        name()
                    ))
                };
            }
            Self::OneObject if positional_count != 1 => {
                return unsafe {
                    type_error(format!(
                        "{}() takes exactly one argument ({positional_count} given)",
                        name()
                    ))
                };
            }
            _ => {}
        }
        let args_ptr = if args.is_empty() {
            ptr::null_mut()
        } else {
            args.as_ptr().cast_mut()
        };
        // An empty names tuple is semantically no keywords, on every path.
        let names = if keyword_count == 0 {
            ptr::null_mut()
        } else {
            kwnames
        };
        unsafe {
            match self {
                Self::NoArgs => {
                    let call: PyCFunction = std::mem::transmute(meth_target);
                    call(self_obj, ptr::null_mut())
                }
                Self::OneObject => {
                    let call: PyCFunction = std::mem::transmute(meth_target);
                    call(self_obj, args[0])
                }
                Self::VarArgs | Self::VarArgsKeywords => {
                    let Some(packed) =
                        TupleDictArguments::from_vector(args, positional_count, names)
                    else {
                        return ptr::null_mut();
                    };
                    if self == Self::VarArgs {
                        let call: PyCFunction = std::mem::transmute(meth_target);
                        call(self_obj, packed.tuple)
                    } else {
                        let call: PyCFunctionWithKeywords = std::mem::transmute(meth_target);
                        call(self_obj, packed.tuple, packed.dict)
                    }
                }
                Self::FastCall => {
                    let call: PyCFunctionFast = std::mem::transmute(meth_target);
                    call(self_obj, args_ptr, positional_count as Py_ssize_t)
                }
                Self::FastCallKeywords => {
                    let call: PyCFunctionFastWithKeywords = std::mem::transmute(meth_target);
                    call(self_obj, args_ptr, positional_count as Py_ssize_t, names)
                }
                Self::Method => {
                    let call: PyCMethod = std::mem::transmute(meth_target);
                    call(
                        self_obj,
                        defining_class,
                        args_ptr,
                        positional_count as usize,
                        names,
                    )
                }
            }
        }
    }
}

unsafe fn type_error(message: String) -> *mut PyObject {
    if let Ok(message) = std::ffi::CString::new(message) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast(),
                message.as_ptr(),
            );
        }
    } else {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
    }
    ptr::null_mut()
}

pub(crate) unsafe fn keyword_count(kwnames: *mut PyObject) -> Option<usize> {
    if kwnames.is_null() {
        return Some(0);
    }
    let count = unsafe { crate::api::sequences::PyTuple_Size(kwnames) };
    usize::try_from(count).ok()
}

unsafe fn validate_arguments(
    args: &[*mut PyObject],
    positional_count: usize,
    kwnames: *mut PyObject,
) -> Option<usize> {
    let count = unsafe { keyword_count(kwnames) }?;
    if positional_count.checked_add(count) != Some(args.len())
        || positional_count > Py_ssize_t::MAX as usize
        || args.iter().any(|arg| arg.is_null())
    {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return None;
    }
    for index in 0..count {
        let key = unsafe { crate::api::sequences::PyTuple_GetItem(kwnames, index as Py_ssize_t) };
        if key.is_null() {
            return None;
        }
        if unsafe { crate::api::strings::PyUnicode_Check(key) } == 0 {
            unsafe { type_error("keywords must be strings".to_owned()) };
            return None;
        }
    }
    Some(count)
}

/// The sole vector-to-tuple/dict adapter, also used by generic tp_call fallback.
pub(crate) struct TupleDictArguments {
    pub tuple: *mut PyObject,
    pub dict: *mut PyObject,
}

impl TupleDictArguments {
    pub(crate) unsafe fn from_vector(
        args: &[*mut PyObject],
        positional_count: usize,
        kwnames: *mut PyObject,
    ) -> Option<Self> {
        let count = unsafe { validate_arguments(args, positional_count, kwnames) }?;
        let tuple = unsafe { crate::api::sequences::native_call_args(&args[..positional_count]) };
        if tuple.is_null() {
            return None;
        }
        let mut packed = Self {
            tuple,
            dict: ptr::null_mut(),
        };
        if count != 0 {
            packed.dict = unsafe { crate::api::mapping::PyDict_New() };
            if packed.dict.is_null() {
                return None;
            }
            for index in 0..count {
                let key =
                    unsafe { crate::api::sequences::PyTuple_GetItem(kwnames, index as Py_ssize_t) };
                if unsafe {
                    crate::api::mapping::PyDict_SetItem(
                        packed.dict,
                        key,
                        args[positional_count + index],
                    )
                } != 0
                {
                    return None;
                }
            }
        }
        Some(packed)
    }
}

impl Drop for TupleDictArguments {
    fn drop(&mut self) {
        unsafe { crate::api::errors::release_preserving_error(&[self.tuple, self.dict]) };
    }
}
