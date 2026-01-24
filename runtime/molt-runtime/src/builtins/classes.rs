use crate::{GilGuard, PyToken};
use std::sync::atomic::Ordering as AtomicOrdering;

use molt_obj_model::MoltObject;

use crate::state::runtime_state::runtime_state_lock;
use crate::{
    alloc_class_obj, alloc_string, class_break_cycles, class_name_bits, dec_ref_bits,
    molt_class_set_base, obj_from_bits, object_type_id, runtime_state, string_obj_to_owned,
    RuntimeState, BUILTIN_TAG_BASE_EXCEPTION, BUILTIN_TAG_EXCEPTION, BUILTIN_TAG_OBJECT,
    BUILTIN_TAG_TYPE, TYPE_ID_TYPE, TYPE_TAG_BOOL, TYPE_TAG_BYTEARRAY, TYPE_TAG_BYTES,
    TYPE_TAG_DICT, TYPE_TAG_FLOAT, TYPE_TAG_FROZENSET, TYPE_TAG_INT, TYPE_TAG_LIST,
    TYPE_TAG_MEMORYVIEW, TYPE_TAG_NONE, TYPE_TAG_RANGE, TYPE_TAG_SET, TYPE_TAG_SLICE, TYPE_TAG_STR,
    TYPE_TAG_TUPLE,
};

pub(crate) struct BuiltinClasses {
    pub(crate) object: u64,
    pub(crate) type_obj: u64,
    pub(crate) none_type: u64,
    pub(crate) not_implemented_type: u64,
    pub(crate) ellipsis_type: u64,
    pub(crate) base_exception: u64,
    pub(crate) exception: u64,
    pub(crate) int: u64,
    pub(crate) float: u64,
    pub(crate) bool: u64,
    pub(crate) str: u64,
    pub(crate) bytes: u64,
    pub(crate) bytearray: u64,
    pub(crate) list: u64,
    pub(crate) tuple: u64,
    pub(crate) dict: u64,
    pub(crate) set: u64,
    pub(crate) frozenset: u64,
    pub(crate) range: u64,
    pub(crate) slice: u64,
    pub(crate) memoryview: u64,
    pub(crate) file: u64,
    pub(crate) file_io: u64,
    pub(crate) buffered_reader: u64,
    pub(crate) buffered_writer: u64,
    pub(crate) buffered_random: u64,
    pub(crate) text_io_wrapper: u64,
    pub(crate) function: u64,
    pub(crate) code: u64,
    pub(crate) frame: u64,
    pub(crate) traceback: u64,
    pub(crate) module: u64,
    pub(crate) super_type: u64,
    pub(crate) generic_alias: u64,
}

impl BuiltinClasses {
    fn dec_ref_all(&self, _py: &PyToken<'_>) {
        crate::gil_assert();
        for bits in [
            self.object,
            self.type_obj,
            self.none_type,
            self.not_implemented_type,
            self.ellipsis_type,
            self.base_exception,
            self.exception,
            self.int,
            self.float,
            self.bool,
            self.str,
            self.bytes,
            self.bytearray,
            self.list,
            self.tuple,
            self.dict,
            self.set,
            self.frozenset,
            self.range,
            self.slice,
            self.memoryview,
            self.file,
            self.file_io,
            self.buffered_reader,
            self.buffered_writer,
            self.buffered_random,
            self.text_io_wrapper,
            self.function,
            self.code,
            self.frame,
            self.traceback,
            self.module,
            self.super_type,
            self.generic_alias,
        ] {
            dec_ref_bits(_py, bits);
        }
    }
}

impl Drop for BuiltinClasses {
    fn drop(&mut self) {
        crate::with_gil_entry!(_py, {
            self.dec_ref_all(_py);
        });
    }
}

fn make_builtin_class(_py: &PyToken<'_>, name: &str) -> u64 {
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let class_ptr = alloc_class_obj(_py, name_bits);
    dec_ref_bits(_py, name_bits);
    if class_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(class_ptr).bits()
}

fn build_builtin_classes(_py: &PyToken<'_>) -> BuiltinClasses {
    let object = make_builtin_class(_py, "object");
    let type_obj = make_builtin_class(_py, "type");
    let none_type = make_builtin_class(_py, "NoneType");
    let not_implemented_type = make_builtin_class(_py, "NotImplementedType");
    let ellipsis_type = make_builtin_class(_py, "ellipsis");
    let base_exception = make_builtin_class(_py, "BaseException");
    let exception = make_builtin_class(_py, "Exception");
    let int = make_builtin_class(_py, "int");
    let float = make_builtin_class(_py, "float");
    let bool = make_builtin_class(_py, "bool");
    let str = make_builtin_class(_py, "str");
    let bytes = make_builtin_class(_py, "bytes");
    let bytearray = make_builtin_class(_py, "bytearray");
    let list = make_builtin_class(_py, "list");
    let tuple = make_builtin_class(_py, "tuple");
    let dict = make_builtin_class(_py, "dict");
    let set = make_builtin_class(_py, "set");
    let frozenset = make_builtin_class(_py, "frozenset");
    let range = make_builtin_class(_py, "range");
    let slice = make_builtin_class(_py, "slice");
    let memoryview = make_builtin_class(_py, "memoryview");
    let file = make_builtin_class(_py, "file");
    let file_io = make_builtin_class(_py, "FileIO");
    let buffered_reader = make_builtin_class(_py, "BufferedReader");
    let buffered_writer = make_builtin_class(_py, "BufferedWriter");
    let buffered_random = make_builtin_class(_py, "BufferedRandom");
    let text_io_wrapper = make_builtin_class(_py, "TextIOWrapper");
    let function = make_builtin_class(_py, "function");
    let code = make_builtin_class(_py, "code");
    let frame = make_builtin_class(_py, "frame");
    let traceback = make_builtin_class(_py, "traceback");
    let module = make_builtin_class(_py, "module");
    let super_type = make_builtin_class(_py, "super");
    let generic_alias = make_builtin_class(_py, "GenericAlias");

    let _ = molt_class_set_base(object, MoltObject::none().bits());
    let _ = molt_class_set_base(type_obj, object);
    let _ = molt_class_set_base(none_type, object);
    let _ = molt_class_set_base(not_implemented_type, object);
    let _ = molt_class_set_base(ellipsis_type, object);
    let _ = molt_class_set_base(base_exception, object);
    let _ = molt_class_set_base(exception, base_exception);
    let _ = molt_class_set_base(int, object);
    let _ = molt_class_set_base(float, object);
    let _ = molt_class_set_base(bool, int);
    let _ = molt_class_set_base(str, object);
    let _ = molt_class_set_base(bytes, object);
    let _ = molt_class_set_base(bytearray, object);
    let _ = molt_class_set_base(list, object);
    let _ = molt_class_set_base(tuple, object);
    let _ = molt_class_set_base(dict, object);
    let _ = molt_class_set_base(set, object);
    let _ = molt_class_set_base(frozenset, object);
    let _ = molt_class_set_base(range, object);
    let _ = molt_class_set_base(slice, object);
    let _ = molt_class_set_base(memoryview, object);
    let _ = molt_class_set_base(file, object);
    let _ = molt_class_set_base(file_io, file);
    let _ = molt_class_set_base(buffered_reader, file);
    let _ = molt_class_set_base(buffered_writer, file);
    let _ = molt_class_set_base(buffered_random, file);
    let _ = molt_class_set_base(text_io_wrapper, file);
    let _ = molt_class_set_base(function, object);
    let _ = molt_class_set_base(code, object);
    let _ = molt_class_set_base(frame, object);
    let _ = molt_class_set_base(traceback, object);
    let _ = molt_class_set_base(module, object);
    let _ = molt_class_set_base(super_type, object);
    let _ = molt_class_set_base(generic_alias, object);

    BuiltinClasses {
        object,
        type_obj,
        none_type,
        not_implemented_type,
        ellipsis_type,
        base_exception,
        exception,
        int,
        float,
        bool,
        str,
        bytes,
        bytearray,
        list,
        tuple,
        dict,
        set,
        frozenset,
        range,
        slice,
        memoryview,
        file,
        file_io,
        buffered_reader,
        buffered_writer,
        buffered_random,
        text_io_wrapper,
        function,
        code,
        frame,
        traceback,
        module,
        super_type,
        generic_alias,
    }
}

pub(crate) fn builtin_classes(_py: &PyToken<'_>) -> &'static BuiltinClasses {
    let state = runtime_state(_py);
    let ptr = state.builtin_classes.load(AtomicOrdering::Acquire);
    if !ptr.is_null() {
        return unsafe { &*ptr };
    }
    init_builtin_classes()
}

fn init_builtin_classes() -> &'static BuiltinClasses {
    let gil = GilGuard::new();
    let py = gil.token();
    let _guard = runtime_state_lock().lock().unwrap();
    let state = runtime_state(&py);
    let ptr = state.builtin_classes.load(AtomicOrdering::Acquire);
    if !ptr.is_null() {
        return unsafe { &*ptr };
    }
    let builtins = build_builtin_classes(&py);
    let boxed = Box::new(builtins);
    let ptr = Box::into_raw(boxed);
    state.builtin_classes.store(ptr, AtomicOrdering::Release);
    unsafe { &*ptr }
}

pub(crate) fn builtin_classes_shutdown(py: &PyToken<'_>, state: &RuntimeState) {
    let ptr = state
        .builtin_classes
        .swap(std::ptr::null_mut(), AtomicOrdering::AcqRel);
    if ptr.is_null() {
        return;
    }
    unsafe {
        let builtins = &*ptr;
        for bits in [
            builtins.object,
            builtins.type_obj,
            builtins.none_type,
            builtins.not_implemented_type,
            builtins.ellipsis_type,
            builtins.base_exception,
            builtins.exception,
            builtins.int,
            builtins.float,
            builtins.bool,
            builtins.str,
            builtins.bytes,
            builtins.bytearray,
            builtins.list,
            builtins.tuple,
            builtins.dict,
            builtins.set,
            builtins.frozenset,
            builtins.range,
            builtins.slice,
            builtins.memoryview,
            builtins.file,
            builtins.file_io,
            builtins.buffered_reader,
            builtins.buffered_writer,
            builtins.buffered_random,
            builtins.text_io_wrapper,
            builtins.function,
            builtins.code,
            builtins.frame,
            builtins.traceback,
            builtins.module,
            builtins.super_type,
            builtins.generic_alias,
        ] {
            class_break_cycles(&py, bits);
        }
    }
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

pub(crate) fn is_builtin_class_bits(_py: &PyToken<'_>, bits: u64) -> bool {
    let builtins = builtin_classes(_py);
    bits == builtins.object
        || bits == builtins.type_obj
        || bits == builtins.none_type
        || bits == builtins.not_implemented_type
        || bits == builtins.ellipsis_type
        || bits == builtins.base_exception
        || bits == builtins.exception
        || bits == builtins.int
        || bits == builtins.float
        || bits == builtins.bool
        || bits == builtins.str
        || bits == builtins.bytes
        || bits == builtins.bytearray
        || bits == builtins.list
        || bits == builtins.tuple
        || bits == builtins.dict
        || bits == builtins.set
        || bits == builtins.frozenset
        || bits == builtins.range
        || bits == builtins.slice
        || bits == builtins.memoryview
        || bits == builtins.file
        || bits == builtins.file_io
        || bits == builtins.buffered_reader
        || bits == builtins.buffered_writer
        || bits == builtins.buffered_random
        || bits == builtins.text_io_wrapper
        || bits == builtins.function
        || bits == builtins.code
        || bits == builtins.frame
        || bits == builtins.traceback
        || bits == builtins.module
        || bits == builtins.super_type
        || bits == builtins.generic_alias
}

pub(crate) fn builtin_type_bits(_py: &PyToken<'_>, tag: i64) -> Option<u64> {
    let builtins = builtin_classes(_py);
    match tag {
        TYPE_TAG_INT => Some(builtins.int),
        TYPE_TAG_FLOAT => Some(builtins.float),
        TYPE_TAG_BOOL => Some(builtins.bool),
        TYPE_TAG_NONE => Some(builtins.none_type),
        TYPE_TAG_STR => Some(builtins.str),
        TYPE_TAG_BYTES => Some(builtins.bytes),
        TYPE_TAG_BYTEARRAY => Some(builtins.bytearray),
        TYPE_TAG_LIST => Some(builtins.list),
        TYPE_TAG_TUPLE => Some(builtins.tuple),
        TYPE_TAG_DICT => Some(builtins.dict),
        TYPE_TAG_SET => Some(builtins.set),
        TYPE_TAG_FROZENSET => Some(builtins.frozenset),
        TYPE_TAG_RANGE => Some(builtins.range),
        TYPE_TAG_SLICE => Some(builtins.slice),
        TYPE_TAG_MEMORYVIEW => Some(builtins.memoryview),
        BUILTIN_TAG_OBJECT => Some(builtins.object),
        BUILTIN_TAG_TYPE => Some(builtins.type_obj),
        BUILTIN_TAG_BASE_EXCEPTION => Some(builtins.base_exception),
        BUILTIN_TAG_EXCEPTION => Some(builtins.exception),
        _ => None,
    }
}

pub(crate) fn class_name_for_error(class_bits: u64) -> String {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return "<class>".to_string();
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return "<class>".to_string();
        }
        string_obj_to_owned(obj_from_bits(class_name_bits(ptr)))
            .unwrap_or_else(|| "<class>".to_string())
    }
}
