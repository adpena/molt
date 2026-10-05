//! Executable declarations for every published physical type slot.
//! CPython 3.12.13 Objects/typeobject.c and Objects/descrobject.c are the oracle.

use super::descriptors::{self, Pins, new_reference, system_error, type_error};
use super::{
    AsyncSlot, BufferSlot, DirectSlot, MappingSlot, NumberSlot, SequenceSlot, SlotWrapper,
};
use crate::abi_types::*;
use crate::api::callback::CallbackOperands;
use crate::api::{errors, mapping, memory, numbers, object, refcount, sequences, strings};
use std::ffi::{c_int, c_void};
use std::ptr;

mod adapters;

/// CPython's explicit __setattr__/__delattr__ receiver check (typeobject.c
/// hackcheck). This is shared by physical wrappers and Python's explicit
/// object/type defaults across the bridge. C PyObject_GenericSetAttr is unchecked.
///
/// Runtime-slot type views and declared process shells do not define a C
/// setter and are skipped like CPython's slot_tp_setattro. Native builtin and
/// foreign types retain their actual C tp_setattro (including NULL), even when
/// their namespace is bound to a runtime class.
pub(crate) unsafe fn setter_admitted(
    receiver: *mut PyObject,
    wrapped: *mut c_void,
    delete: bool,
) -> bool {
    unsafe {
        let ty = crate::bridge::semantic_type(receiver);
        if ty.is_null() {
            return false;
        }
        setter_type_admitted(ty, wrapped, delete)
    }
}

/// Admission depends only on the receiver's actual type and C MRO. Runtime
/// callers with a foreign lineage project this type, never the receiver value.
pub(crate) unsafe fn setter_type_admitted(
    ty: *mut PyTypeObject,
    wrapped: *mut c_void,
    delete: bool,
) -> bool {
    unsafe {
        if ty.is_null() {
            return false;
        }
        let _type_owner = refcount::OwnedPyObject::from_borrowed(ty.cast());
        let mro = (*ty).tp_mro;
        // CPython deliberately permits a call when no MRO is available. A
        // present MRO with a NULL setter is a different case and is checked.
        if mro.is_null() {
            return true;
        }
        let _mro_owner = refcount::OwnedPyObject::from_borrowed(mro);
        let current = (*ty).tp_setattro.map(|f| f as *const () as *mut c_void);
        let is_python_dispatch = |class: *mut PyTypeObject| {
            crate::bridge::GLOBAL_BRIDGE.type_uses_runtime_slots(class)
                || (*class).tp_setattro.map(|f| f as *const () as *mut c_void)
                    == Some(super::native_slot_dispatch::dispatcher(
                        SlotWrapper::Direct(DirectSlot::SetAttr),
                    ))
        };
        let current_is_python = is_python_dispatch(ty);
        let mut defining = ty;
        let count = sequences::PyTuple_Size(mro);
        if count < 0 {
            return false;
        }
        for index in (0..count).rev() {
            let base = sequences::PyTuple_GetItem(mro, index).cast::<PyTypeObject>();
            if base.is_null() {
                return false;
            }
            if is_python_dispatch(base) {
                continue;
            }
            if !current_is_python
                && (*base).tp_setattro.map(|f| f as *const () as *mut c_void) == current
            {
                defining = base;
                break;
            }
        }
        while !defining.is_null() {
            if is_python_dispatch(defining) {
                defining = (*defining).tp_base;
                continue;
            }
            if (*defining)
                .tp_setattro
                .map(|f| f as *const () as *mut c_void)
                == Some(wrapped)
            {
                return true;
            }
            let operation = if delete { "__delattr__" } else { "__setattr__" };
            type_error(&format!(
                "can't apply this {operation} to {} object",
                if (*ty).tp_name.is_null() {
                    "object".into()
                } else {
                    std::ffi::CStr::from_ptr((*ty).tp_name).to_string_lossy()
                },
            ));
            return false;
        }
        true
    }
}

pub(super) struct SlotWrapperDef {
    pub(super) slot: SlotWrapper,
    pub(super) base: PyWrapperBase,
}

macro_rules! definition {
    ($name:literal, $slot:expr, $offset:expr, $adapter:expr, $flags:expr, $signature:literal, $doc:literal) => {
        SlotWrapperDef {
            slot: $slot,
            base: PyWrapperBase {
                name: concat!($name, "\0").as_ptr().cast(),
                offset: $offset as c_int,
                // Declaration and mutation publication share one typed slot.
                function: super::native_slot_dispatch::dispatcher($slot),
                wrapper: Some($adapter),
                doc: concat!($name, $signature, "\n--\n\n", $doc, "\0")
                    .as_ptr()
                    .cast(),
                flags: $flags,
                name_strobj: ptr::null_mut(),
            },
        }
    };
}
macro_rules! direct {
    ($name:literal, $slot:ident, $field:ident, $adapter:expr, $signature:literal, $doc:literal) => {
        definition!(
            $name,
            SlotWrapper::Direct(DirectSlot::$slot),
            std::mem::offset_of!(PyTypeObject, $field),
            $adapter,
            0,
            $signature,
            $doc
        )
    };
}
macro_rules! keyword {
    ($name:literal, $slot:ident, $field:ident, $adapter:expr, $signature:literal, $doc:literal) => {
        definition!(
            $name,
            SlotWrapper::Direct(DirectSlot::$slot),
            std::mem::offset_of!(PyTypeObject, $field),
            unsafe { std::mem::transmute::<PyWrapperFuncKeywords, PyWrapperFunc>($adapter) },
            PyWrapperFlag_KEYWORDS,
            $signature,
            $doc
        )
    };
}
macro_rules! table {
    ($name:literal, $family:ident, $slot:expr, $table:ident, $type:ty, $field:ident, $adapter:expr, $signature:literal, $doc:literal) => {
        definition!(
            $name,
            SlotWrapper::$family($slot),
            std::mem::offset_of!(PyHeapTypeObject, $table) + std::mem::offset_of!($type, $field),
            $adapter,
            0,
            $signature,
            $doc
        )
    };
}
macro_rules! number {
    ($name:literal, $slot:ident, $field:ident, $adapter:expr, $signature:literal, $doc:literal) => {
        table!(
            $name,
            Number,
            NumberSlot::$slot,
            as_number,
            PyNumberMethods,
            $field,
            $adapter,
            $signature,
            $doc
        )
    };
}
macro_rules! binary {
    ($name:literal, $slot:ident, $field:ident, $reverse:literal) => {
        number!(
            $name,
            $slot,
            $field,
            adapters::binary::<$reverse>,
            "($self, value, /)",
            "Apply this numeric operation to self and value."
        )
    };
}
macro_rules! sequence {
    ($name:literal, $slot:ident, $field:ident, $adapter:expr, $signature:literal) => {
        table!(
            $name,
            Sequence,
            SequenceSlot::$slot,
            as_sequence,
            PySequenceMethods,
            $field,
            $adapter,
            $signature,
            "Apply the declaring sequence operation."
        )
    };
}
macro_rules! mapping {
    ($name:literal, $slot:ident, $field:ident, $adapter:expr, $signature:literal) => {
        table!(
            $name,
            Mapping,
            MappingSlot::$slot,
            as_mapping,
            PyMappingMethods,
            $field,
            $adapter,
            $signature,
            "Apply the declaring mapping operation."
        )
    };
}

pub(super) static SLOT_WRAPPER_DEFS: &[SlotWrapperDef] = &[
    direct!(
        "__repr__",
        Repr,
        tp_repr,
        adapters::unary,
        "($self, /)",
        "Return repr(self)."
    ),
    direct!(
        "__hash__",
        Hash,
        tp_hash,
        adapters::size,
        "($self, /)",
        "Return hash(self)."
    ),
    keyword!(
        "__call__",
        Call,
        tp_call,
        adapters::call,
        "($self, /, *args, **kwargs)",
        "Call self as a function."
    ),
    direct!(
        "__str__",
        Str,
        tp_str,
        adapters::unary,
        "($self, /)",
        "Return str(self)."
    ),
    direct!(
        "__getattribute__",
        GetAttr,
        tp_getattro,
        adapters::binary::<false>,
        "($self, name, /)",
        "Return getattr(self, name)."
    ),
    SlotWrapperDef {
        slot: SlotWrapper::Direct(DirectSlot::GetAttr),
        base: PyWrapperBase {
            name: c"__getattr__".as_ptr(),
            offset: std::mem::offset_of!(PyTypeObject, tp_getattro) as c_int,
            function: super::native_slot_dispatch::dispatcher(SlotWrapper::Direct(
                DirectSlot::GetAttr,
            )),
            wrapper: None,
            doc: ptr::null(),
            flags: 0,
            name_strobj: ptr::null_mut(),
        },
    },
    SlotWrapperDef {
        slot: SlotWrapper::Direct(DirectSlot::New),
        base: PyWrapperBase {
            name: c"__new__".as_ptr(),
            offset: std::mem::offset_of!(PyTypeObject, tp_new) as c_int,
            function: super::native_slot_dispatch::dispatcher(SlotWrapper::Direct(DirectSlot::New)),
            wrapper: None,
            doc: ptr::null(),
            flags: 0,
            name_strobj: ptr::null_mut(),
        },
    },
    direct!(
        "__setattr__",
        SetAttr,
        tp_setattro,
        adapters::attribute_set::<false>,
        "($self, name, value, /)",
        "Implement setattr(self, name, value)."
    ),
    direct!(
        "__delattr__",
        SetAttr,
        tp_setattro,
        adapters::attribute_set::<true>,
        "($self, name, /)",
        "Implement delattr(self, name)."
    ),
    direct!(
        "__lt__",
        RichCompare,
        tp_richcompare,
        adapters::compare::<0>,
        "($self, value, /)",
        "Return self<value."
    ),
    direct!(
        "__le__",
        RichCompare,
        tp_richcompare,
        adapters::compare::<1>,
        "($self, value, /)",
        "Return self<=value."
    ),
    direct!(
        "__eq__",
        RichCompare,
        tp_richcompare,
        adapters::compare::<2>,
        "($self, value, /)",
        "Return self==value."
    ),
    direct!(
        "__ne__",
        RichCompare,
        tp_richcompare,
        adapters::compare::<3>,
        "($self, value, /)",
        "Return self!=value."
    ),
    direct!(
        "__gt__",
        RichCompare,
        tp_richcompare,
        adapters::compare::<4>,
        "($self, value, /)",
        "Return self>value."
    ),
    direct!(
        "__ge__",
        RichCompare,
        tp_richcompare,
        adapters::compare::<5>,
        "($self, value, /)",
        "Return self>=value."
    ),
    direct!(
        "__iter__",
        Iter,
        tp_iter,
        adapters::unary,
        "($self, /)",
        "Implement iter(self)."
    ),
    direct!(
        "__next__",
        IterNext,
        tp_iternext,
        adapters::next,
        "($self, /)",
        "Implement next(self)."
    ),
    direct!(
        "__get__",
        DescrGet,
        tp_descr_get,
        adapters::descr_get,
        "($self, instance, owner=None, /)",
        "Return an attribute of instance, which is of type owner."
    ),
    direct!(
        "__set__",
        DescrSet,
        tp_descr_set,
        adapters::descr_set::<false>,
        "($self, instance, value, /)",
        "Set an attribute of instance to value."
    ),
    direct!(
        "__delete__",
        DescrSet,
        tp_descr_set,
        adapters::descr_set::<true>,
        "($self, instance, /)",
        "Delete an attribute of instance."
    ),
    keyword!(
        "__init__",
        Init,
        tp_init,
        adapters::init,
        "($self, /, *args, **kwargs)",
        "Initialize self. See help(type(self)) for accurate signature."
    ),
    direct!(
        "__del__",
        Finalize,
        tp_finalize,
        adapters::finalize,
        "($self, /)",
        "Called when the instance is about to be destroyed."
    ),
    table!(
        "__buffer__",
        Buffer,
        BufferSlot::Get,
        as_buffer,
        PyBufferProcs,
        bf_getbuffer,
        adapters::buffer,
        "($self, flags, /)",
        "Return a buffer object that exposes the underlying memory of the object."
    ),
    table!(
        "__release_buffer__",
        Buffer,
        BufferSlot::Release,
        as_buffer,
        PyBufferProcs,
        bf_releasebuffer,
        adapters::release_buffer,
        "($self, buffer, /)",
        "Release the buffer object that exposes the underlying memory of the object."
    ),
    table!(
        "__await__",
        Async,
        AsyncSlot::Await,
        as_async,
        PyAsyncMethods,
        am_await,
        adapters::unary,
        "($self, /)",
        "Return an iterator to be used in await expression."
    ),
    table!(
        "__aiter__",
        Async,
        AsyncSlot::Iter,
        as_async,
        PyAsyncMethods,
        am_aiter,
        adapters::unary,
        "($self, /)",
        "Return an asynchronous iterator."
    ),
    table!(
        "__anext__",
        Async,
        AsyncSlot::Next,
        as_async,
        PyAsyncMethods,
        am_anext,
        adapters::unary,
        "($self, /)",
        "Return a value or raise StopAsyncIteration."
    ),
    binary!("__add__", Add, nb_add, false),
    binary!("__radd__", Add, nb_add, true),
    binary!("__sub__", Subtract, nb_subtract, false),
    binary!("__rsub__", Subtract, nb_subtract, true),
    binary!("__mul__", Multiply, nb_multiply, false),
    binary!("__rmul__", Multiply, nb_multiply, true),
    binary!("__mod__", Remainder, nb_remainder, false),
    binary!("__rmod__", Remainder, nb_remainder, true),
    binary!("__divmod__", Divmod, nb_divmod, false),
    binary!("__rdivmod__", Divmod, nb_divmod, true),
    number!(
        "__pow__",
        Power,
        nb_power,
        adapters::ternary::<false>,
        "($self, value, mod=None, /)",
        "Return pow(self, value, mod)."
    ),
    number!(
        "__rpow__",
        Power,
        nb_power,
        adapters::ternary::<true>,
        "($self, value, mod=None, /)",
        "Return pow(value, self, mod)."
    ),
    number!(
        "__neg__",
        Negative,
        nb_negative,
        adapters::unary,
        "($self, /)",
        "-self"
    ),
    number!(
        "__pos__",
        Positive,
        nb_positive,
        adapters::unary,
        "($self, /)",
        "+self"
    ),
    number!(
        "__abs__",
        Absolute,
        nb_absolute,
        adapters::unary,
        "($self, /)",
        "abs(self)"
    ),
    number!(
        "__bool__",
        Bool,
        nb_bool,
        adapters::inquiry,
        "($self, /)",
        "True if self else False"
    ),
    number!(
        "__invert__",
        Invert,
        nb_invert,
        adapters::unary,
        "($self, /)",
        "~self"
    ),
    binary!("__lshift__", LShift, nb_lshift, false),
    binary!("__rlshift__", LShift, nb_lshift, true),
    binary!("__rshift__", RShift, nb_rshift, false),
    binary!("__rrshift__", RShift, nb_rshift, true),
    binary!("__and__", And, nb_and, false),
    binary!("__rand__", And, nb_and, true),
    binary!("__xor__", Xor, nb_xor, false),
    binary!("__rxor__", Xor, nb_xor, true),
    binary!("__or__", Or, nb_or, false),
    binary!("__ror__", Or, nb_or, true),
    number!(
        "__int__",
        Int,
        nb_int,
        adapters::unary,
        "($self, /)",
        "int(self)"
    ),
    number!(
        "__float__",
        Float,
        nb_float,
        adapters::unary,
        "($self, /)",
        "float(self)"
    ),
    binary!("__iadd__", InPlaceAdd, nb_inplace_add, false),
    binary!("__isub__", InPlaceSubtract, nb_inplace_subtract, false),
    binary!("__imul__", InPlaceMultiply, nb_inplace_multiply, false),
    binary!("__imod__", InPlaceRemainder, nb_inplace_remainder, false),
    number!(
        "__ipow__",
        InPlacePower,
        nb_inplace_power,
        adapters::ternary::<false>,
        "($self, value, mod=None, /)",
        "Return self**value."
    ),
    binary!("__ilshift__", InPlaceLShift, nb_inplace_lshift, false),
    binary!("__irshift__", InPlaceRShift, nb_inplace_rshift, false),
    binary!("__iand__", InPlaceAnd, nb_inplace_and, false),
    binary!("__ixor__", InPlaceXor, nb_inplace_xor, false),
    binary!("__ior__", InPlaceOr, nb_inplace_or, false),
    binary!("__floordiv__", FloorDivide, nb_floor_divide, false),
    binary!("__rfloordiv__", FloorDivide, nb_floor_divide, true),
    binary!("__truediv__", TrueDivide, nb_true_divide, false),
    binary!("__rtruediv__", TrueDivide, nb_true_divide, true),
    binary!(
        "__ifloordiv__",
        InPlaceFloorDivide,
        nb_inplace_floor_divide,
        false
    ),
    binary!(
        "__itruediv__",
        InPlaceTrueDivide,
        nb_inplace_true_divide,
        false
    ),
    number!(
        "__index__",
        Index,
        nb_index,
        adapters::unary,
        "($self, /)",
        "Return self converted to an integer index."
    ),
    binary!("__matmul__", MatrixMultiply, nb_matrix_multiply, false),
    binary!("__rmatmul__", MatrixMultiply, nb_matrix_multiply, true),
    binary!(
        "__imatmul__",
        InPlaceMatrixMultiply,
        nb_inplace_matrix_multiply,
        false
    ),
    mapping!("__len__", Length, mp_length, adapters::size, "($self, /)"),
    mapping!(
        "__getitem__",
        Subscript,
        mp_subscript,
        adapters::binary::<false>,
        "($self, key, /)"
    ),
    mapping!(
        "__setitem__",
        AssSubscript,
        mp_ass_subscript,
        adapters::object_set::<false>,
        "($self, key, value, /)"
    ),
    mapping!(
        "__delitem__",
        AssSubscript,
        mp_ass_subscript,
        adapters::object_set::<true>,
        "($self, key, /)"
    ),
    sequence!("__len__", Length, sq_length, adapters::size, "($self, /)"),
    sequence!(
        "__add__",
        Concat,
        sq_concat,
        adapters::binary::<false>,
        "($self, value, /)"
    ),
    sequence!(
        "__mul__",
        Repeat,
        sq_repeat,
        adapters::index_arg::<false>,
        "($self, value, /)"
    ),
    sequence!(
        "__rmul__",
        Repeat,
        sq_repeat,
        adapters::index_arg::<false>,
        "($self, value, /)"
    ),
    sequence!(
        "__getitem__",
        Item,
        sq_item,
        adapters::index_arg::<true>,
        "($self, key, /)"
    ),
    sequence!(
        "__setitem__",
        AssItem,
        sq_ass_item,
        adapters::sequence_set::<false>,
        "($self, key, value, /)"
    ),
    sequence!(
        "__delitem__",
        AssItem,
        sq_ass_item,
        adapters::sequence_set::<true>,
        "($self, key, /)"
    ),
    sequence!(
        "__contains__",
        Contains,
        sq_contains,
        adapters::contains,
        "($self, key, /)"
    ),
    sequence!(
        "__iadd__",
        InPlaceConcat,
        sq_inplace_concat,
        adapters::binary::<false>,
        "($self, value, /)"
    ),
    sequence!(
        "__imul__",
        InPlaceRepeat,
        sq_inplace_repeat,
        adapters::index_arg::<false>,
        "($self, value, /)"
    ),
];

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_NewWrapper(
    owner: *mut PyTypeObject,
    base: *mut PyWrapperBase,
    wrapped: *mut c_void,
) -> *mut PyObject {
    if base.is_null() || wrapped.is_null() || unsafe { (*base).wrapper.is_none() } {
        unsafe { errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let descr = unsafe { descriptors::allocate(&raw mut PyWrapperDescr_Type, owner, (*base).name) }
        .cast::<PyWrapperDescrObject>();
    if descr.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*descr).d_base = base;
        (*descr).d_wrapped = wrapped;
        descriptors::publish(descr.cast())
    }
}

unsafe fn raw_call(
    descr: *mut PyWrapperDescrObject,
    self_: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    if descr.is_null() || self_.is_null() || args.is_null() {
        unsafe { errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let base = unsafe { (*descr).d_base };
    let wrapped = unsafe { (*descr).d_wrapped };
    if base.is_null() || wrapped.is_null() {
        unsafe { system_error(c"slot wrapper has no invocation metadata") };
        return ptr::null_mut();
    }
    let Some(adapter) = (unsafe { (*base).wrapper }) else {
        unsafe { system_error(c"slot wrapper has no adapter") };
        return ptr::null_mut();
    };
    let keywords = unsafe { (*base).flags } & PyWrapperFlag_KEYWORDS != 0;
    if !keywords {
        if !kwargs.is_null() {
            let is_dictionary = unsafe { mapping::PyDict_Check(kwargs) };
            if descriptors::pending() {
                return ptr::null_mut();
            }
            if is_dictionary == 0 {
                return unsafe { type_error("slot wrapper keywords must be a dictionary") };
            }
            let count = unsafe { mapping::PyDict_Size(kwargs) };
            if count < 0 || descriptors::pending() {
                return ptr::null_mut();
            }
            if count != 0 {
                return unsafe { type_error("slot wrapper takes no keyword arguments") };
            }
        }
    }
    let Some(operands) = (unsafe {
        CallbackOperands::from_tuple_dict([descr.cast(), self_, args, kwargs], args, kwargs)
    }) else {
        return ptr::null_mut();
    };
    let result = if keywords {
        let adapter: PyWrapperFuncKeywords = unsafe { std::mem::transmute(adapter) };
        unsafe { adapter(self_, args, wrapped, kwargs) }
    } else {
        unsafe { adapter(self_, args, wrapped) }
    };
    unsafe { operands.complete_result(result, "native slot wrapper") }
}

/// These thunks complete at raw_call, where the hidden bound receiver is known.
/// Generic tp_call dispatch can defer to that exact boundary without scanning
/// the same argument projections again. Match callbacks, never mutable types.
pub(crate) fn completes_call_operands(
    call: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject,
) -> bool {
    type Call = unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject;
    std::ptr::fn_addr_eq(call, descr_call as Call)
        || std::ptr::fn_addr_eq(call, method_call as Call)
}

unsafe extern "C" fn descr_call(
    object: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let _pins = unsafe { Pins::new([object, args, kwargs]) };
    let count = unsafe { sequences::PyTuple_Size(args) };
    if count < 0 {
        return ptr::null_mut();
    }
    if count == 0 {
        return unsafe { type_error("descriptor needs an argument") };
    }
    let self_ = unsafe { sequences::PyTuple_GetItem(args, 0) };
    if !unsafe { descriptors::receiver(object, self_) } {
        return ptr::null_mut();
    }
    let tail = unsafe { sequences::PyTuple_GetSlice(args, 1, count) };
    if tail.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { raw_call(object.cast(), self_, tail, kwargs) };
    unsafe { errors::release_preserving_error(&[tail]) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyWrapper_New(
    descr: *mut PyObject,
    self_: *mut PyObject,
) -> *mut PyObject {
    if descr.is_null() || unsafe { (*descr).ob_type } != &raw mut PyWrapperDescr_Type {
        unsafe { errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let _pins = unsafe { Pins::new([descr, self_]) };
    if !unsafe { descriptors::receiver(descr, self_) } {
        return ptr::null_mut();
    }
    let wrapper = unsafe { memory::_PyObject_GC_New(&raw mut _PyMethodWrapper_Type) }
        .cast::<PyMethodWrapperObject>();
    if wrapper.is_null() {
        unsafe { system_error(c"native method wrapper allocation failed without an exception") };
        return ptr::null_mut();
    }
    unsafe {
        refcount::Py_INCREF(descr);
        refcount::Py_INCREF(self_);
        (*wrapper).descr = descr.cast();
        (*wrapper).self_ = self_;
        descriptors::publish(wrapper.cast())
    }
}

unsafe extern "C" fn descr_get(
    descr: *mut PyObject,
    self_: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    if self_.is_null() {
        unsafe { new_reference(descr) }
    } else {
        unsafe { PyWrapper_New(descr, self_) }
    }
}

unsafe extern "C" fn method_call(
    object: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let _pin = unsafe { Pins::new([object]) };
    let wrapper = object.cast::<PyMethodWrapperObject>();
    unsafe { raw_call((*wrapper).descr, (*wrapper).self_, args, kwargs) }
}

unsafe extern "C" fn method_traverse(
    object: *mut PyObject,
    visit: *mut c_void,
    context: *mut c_void,
) -> c_int {
    if visit.is_null() {
        return 0;
    }
    let callback: unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int =
        unsafe { std::mem::transmute(visit) };
    let wrapper = object.cast::<PyMethodWrapperObject>();
    for edge in unsafe { [(*wrapper).descr.cast(), (*wrapper).self_] } {
        if !edge.is_null() {
            let status = unsafe { callback(edge, context) };
            if status != 0 {
                return status;
            }
        }
    }
    0
}

unsafe extern "C" fn method_clear(object: *mut PyObject) -> c_int {
    let wrapper = object.cast::<PyMethodWrapperObject>();
    let edges = unsafe {
        [
            std::mem::replace(&mut (*wrapper).descr, ptr::null_mut()).cast(),
            std::mem::replace(&mut (*wrapper).self_, ptr::null_mut()),
        ]
    };
    unsafe { errors::release_preserving_error(&edges) };
    0
}

unsafe extern "C" fn method_dealloc(object: *mut PyObject) {
    unsafe {
        memory::PyObject_GC_UnTrack(object.cast());
        method_clear(object);
        memory::PyObject_GC_Del(object.cast());
    }
}

unsafe extern "C" fn method_compare(
    left: *mut PyObject,
    right: *mut PyObject,
    operation: c_int,
) -> *mut PyObject {
    if !matches!(operation, 2 | 3)
        || right.is_null()
        || unsafe { (*right).ob_type } != &raw mut _PyMethodWrapper_Type
    {
        return unsafe { new_reference(&raw mut Py_NotImplementedSentinel) };
    }
    let left = left.cast::<PyMethodWrapperObject>();
    let right = right.cast::<PyMethodWrapperObject>();
    let equal = unsafe { (*left).descr == (*right).descr && (*left).self_ == (*right).self_ };
    unsafe { numbers::PyBool_FromLong((equal == (operation == 2)) as _) }
}

unsafe extern "C" fn method_hash(object: *mut PyObject) -> isize {
    let wrapper = object.cast::<PyMethodWrapperObject>();
    let first =
        molt_lang_obj_model::hash_policy::hash_pointer(unsafe { (*wrapper).self_ } as u64) as isize;
    let second =
        molt_lang_obj_model::hash_policy::hash_pointer(unsafe { (*wrapper).descr } as u64) as isize;
    let result = first ^ second;
    if result == -1 { -2 } else { result }
}

unsafe extern "C" fn method_repr(object: *mut PyObject) -> *mut PyObject {
    let wrapper = object.cast::<PyMethodWrapperObject>();
    let descr = unsafe { (*wrapper).descr };
    let self_ = unsafe { (*wrapper).self_ };
    if descr.is_null() || self_.is_null() {
        unsafe { system_error(c"method wrapper has been cleared") };
        return ptr::null_mut();
    }
    let _pins = unsafe { Pins::new([object, descr.cast(), self_]) };
    let name = unsafe { strings::unicode_bytes((*descr).d_common.d_name) }
        .unwrap_or(b"?")
        .to_vec();
    let ty = unsafe { crate::bridge::semantic_type(self_) };
    if ty.is_null() {
        return ptr::null_mut();
    }
    let type_name = unsafe { (*ty).tp_name };
    let type_name = if type_name.is_null() {
        &b"?"[..]
    } else {
        unsafe { std::ffi::CStr::from_ptr(type_name).to_bytes() }
    };
    unsafe {
        strings::unicode_from_python_bytes(
            &[
                b"<method-wrapper '".as_slice(),
                &name,
                b"' of ",
                type_name,
                format!(" object at {self_:p}>").as_bytes(),
            ]
            .concat(),
        )
    }
}

pub(super) unsafe fn init() {
    unsafe {
        PyWrapperDescr_Type.tp_descr_get = Some(descr_get);
        PyWrapperDescr_Type.tp_call = Some(descr_call);
        PyWrapperDescr_Type.tp_flags |= Py_TPFLAGS_METHOD_DESCRIPTOR;
        _PyMethodWrapper_Type.tp_basicsize =
            std::mem::size_of::<PyMethodWrapperObject>() as Py_ssize_t;
        _PyMethodWrapper_Type.tp_flags |= Py_TPFLAGS_HAVE_GC;
        _PyMethodWrapper_Type.tp_base = &raw mut PyBaseObject_Type;
        _PyMethodWrapper_Type.tp_getattro = Some(object::PyObject_GenericGetAttr);
        _PyMethodWrapper_Type.tp_call = Some(method_call);
        _PyMethodWrapper_Type.tp_repr = Some(method_repr);
        _PyMethodWrapper_Type.tp_hash = Some(method_hash);
        _PyMethodWrapper_Type.tp_richcompare = Some(method_compare);
        _PyMethodWrapper_Type.tp_traverse = Some(method_traverse);
        _PyMethodWrapper_Type.tp_clear = Some(method_clear);
        _PyMethodWrapper_Type.tp_dealloc = Some(method_dealloc);
        _PyMethodWrapper_Type.tp_free = Some(memory::PyObject_GC_Del);
    }
}

#[cfg(test)]
mod tests;
