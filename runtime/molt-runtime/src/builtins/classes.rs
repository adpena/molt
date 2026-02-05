use crate::{GilGuard, PyToken};
use std::sync::atomic::Ordering as AtomicOrdering;

use molt_obj_model::MoltObject;

use crate::state::runtime_state::runtime_state_lock;
use crate::{
    alloc_class_obj, alloc_dict_with_pairs, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    class_break_cycles, class_bump_layout_version, class_dict_bits, class_name_bits, dec_ref_bits,
    dict_set_in_place, inc_ref_bits, intern_static_name, molt_class_set_base, obj_from_bits,
    object_set_class_bits, object_type_id, runtime_state, runtime_state_for_gil,
    string_obj_to_owned, RuntimeState, BUILTIN_TAG_BASE_EXCEPTION, BUILTIN_TAG_EXCEPTION,
    BUILTIN_TAG_OBJECT, BUILTIN_TAG_TYPE, TYPE_ID_DICT, TYPE_ID_TYPE, TYPE_TAG_BOOL,
    TYPE_TAG_BYTEARRAY, TYPE_TAG_BYTES, TYPE_TAG_COMPLEX, TYPE_TAG_DICT, TYPE_TAG_FLOAT,
    TYPE_TAG_FROZENSET, TYPE_TAG_INT, TYPE_TAG_LIST, TYPE_TAG_MEMORYVIEW, TYPE_TAG_NONE,
    TYPE_TAG_RANGE, TYPE_TAG_SET, TYPE_TAG_SLICE, TYPE_TAG_STR, TYPE_TAG_TUPLE,
};

pub(crate) struct BuiltinClasses {
    pub(crate) object: u64,
    pub(crate) type_obj: u64,
    pub(crate) none_type: u64,
    pub(crate) not_implemented_type: u64,
    pub(crate) ellipsis_type: u64,
    pub(crate) base_exception: u64,
    pub(crate) exception: u64,
    pub(crate) base_exception_group: u64,
    pub(crate) exception_group: u64,
    pub(crate) int: u64,
    pub(crate) float: u64,
    pub(crate) complex: u64,
    pub(crate) bool: u64,
    pub(crate) str: u64,
    pub(crate) bytes: u64,
    pub(crate) bytearray: u64,
    pub(crate) list: u64,
    pub(crate) tuple: u64,
    pub(crate) dict: u64,
    pub(crate) dict_keys: u64,
    pub(crate) dict_items: u64,
    pub(crate) dict_values: u64,
    pub(crate) set: u64,
    pub(crate) frozenset: u64,
    pub(crate) range: u64,
    pub(crate) slice: u64,
    pub(crate) memoryview: u64,
    pub(crate) io_base: u64,
    pub(crate) raw_io_base: u64,
    pub(crate) buffered_io_base: u64,
    pub(crate) text_io_base: u64,
    pub(crate) file: u64,
    pub(crate) file_io: u64,
    pub(crate) buffered_reader: u64,
    pub(crate) buffered_writer: u64,
    pub(crate) buffered_random: u64,
    pub(crate) text_io_wrapper: u64,
    pub(crate) bytes_io: u64,
    pub(crate) string_io: u64,
    pub(crate) function: u64,
    pub(crate) coroutine: u64,
    pub(crate) generator: u64,
    pub(crate) async_generator: u64,
    pub(crate) iterator: u64,
    pub(crate) callable_iterator: u64,
    pub(crate) enumerate: u64,
    pub(crate) reversed: u64,
    pub(crate) zip: u64,
    pub(crate) map: u64,
    pub(crate) filter: u64,
    pub(crate) builtin_function_or_method: u64,
    pub(crate) code: u64,
    pub(crate) frame: u64,
    pub(crate) traceback: u64,
    pub(crate) module: u64,
    pub(crate) super_type: u64,
    pub(crate) generic_alias: u64,
    pub(crate) union_type: u64,
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
            self.base_exception_group,
            self.exception_group,
            self.int,
            self.float,
            self.complex,
            self.bool,
            self.str,
            self.bytes,
            self.bytearray,
            self.list,
            self.tuple,
            self.dict,
            self.dict_keys,
            self.dict_items,
            self.dict_values,
            self.set,
            self.frozenset,
            self.range,
            self.slice,
            self.memoryview,
            self.io_base,
            self.raw_io_base,
            self.buffered_io_base,
            self.text_io_base,
            self.file,
            self.file_io,
            self.buffered_reader,
            self.buffered_writer,
            self.buffered_random,
            self.text_io_wrapper,
            self.bytes_io,
            self.string_io,
            self.function,
            self.coroutine,
            self.generator,
            self.async_generator,
            self.iterator,
            self.callable_iterator,
            self.enumerate,
            self.reversed,
            self.zip,
            self.map,
            self.filter,
            self.builtin_function_or_method,
            self.code,
            self.frame,
            self.traceback,
            self.module,
            self.super_type,
            self.generic_alias,
            self.union_type,
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

fn init_int_subclass_layout(_py: &PyToken<'_>, int_bits: u64) {
    let Some(int_ptr) = obj_from_bits(int_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(int_ptr) != TYPE_ID_TYPE {
            return;
        }
    }
    let dict_bits = unsafe { class_dict_bits(int_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }
    }
    let offsets_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.field_offsets_name,
        b"__molt_field_offsets__",
    );
    let layout_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_layout_size,
        b"__molt_layout_size__",
    );
    let Some(slot_name_bits) = attr_name_bits_from_bytes(_py, b"__molt_int_value__") else {
        return;
    };
    let offset_bits = MoltObject::from_int(0).bits();
    let offsets_ptr = alloc_dict_with_pairs(_py, &[slot_name_bits, offset_bits]);
    if offsets_ptr.is_null() {
        return;
    }
    let offsets_bits = MoltObject::from_ptr(offsets_ptr).bits();
    unsafe {
        dict_set_in_place(_py, dict_ptr, offsets_name_bits, offsets_bits);
    }
    let layout_bits = MoltObject::from_int(16).bits();
    unsafe {
        dict_set_in_place(_py, dict_ptr, layout_name_bits, layout_bits);
        class_bump_layout_version(int_ptr);
    }
    dec_ref_bits(_py, offsets_bits);
    dec_ref_bits(_py, slot_name_bits);
}

fn init_dict_subclass_layout(_py: &PyToken<'_>, dict_bits: u64) {
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_TYPE {
            return;
        }
    }
    let dict_bits = unsafe { class_dict_bits(dict_ptr) };
    let Some(dict_dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(dict_dict_ptr) != TYPE_ID_DICT {
            return;
        }
    }
    let layout_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_layout_size,
        b"__molt_layout_size__",
    );
    let layout_bits = MoltObject::from_int(16).bits();
    unsafe {
        dict_set_in_place(_py, dict_dict_ptr, layout_name_bits, layout_bits);
        class_bump_layout_version(dict_ptr);
    }
}

fn union_type_class_name() -> &'static str {
    let minor = std::env::var("MOLT_SYS_VERSION_INFO")
        .ok()
        .and_then(|raw| {
            let mut parts = raw.split(',');
            let _major = parts.next()?.trim().parse::<i64>().ok()?;
            let minor = parts.next()?.trim().parse::<i64>().ok()?;
            Some(minor)
        })
        .unwrap_or(14);
    if minor >= 14 {
        "Union"
    } else {
        "UnionType"
    }
}

fn build_builtin_classes(_py: &PyToken<'_>) -> BuiltinClasses {
    let object = make_builtin_class(_py, "object");
    let type_obj = make_builtin_class(_py, "type");
    let none_type = make_builtin_class(_py, "NoneType");
    let not_implemented_type = make_builtin_class(_py, "NotImplementedType");
    let ellipsis_type = make_builtin_class(_py, "ellipsis");
    let base_exception = make_builtin_class(_py, "BaseException");
    let exception = make_builtin_class(_py, "Exception");
    let base_exception_group = make_builtin_class(_py, "BaseExceptionGroup");
    let exception_group = make_builtin_class(_py, "ExceptionGroup");
    let int = make_builtin_class(_py, "int");
    let float = make_builtin_class(_py, "float");
    let complex = make_builtin_class(_py, "complex");
    let bool = make_builtin_class(_py, "bool");
    let str = make_builtin_class(_py, "str");
    let bytes = make_builtin_class(_py, "bytes");
    let bytearray = make_builtin_class(_py, "bytearray");
    let list = make_builtin_class(_py, "list");
    let tuple = make_builtin_class(_py, "tuple");
    let dict = make_builtin_class(_py, "dict");
    let dict_keys = make_builtin_class(_py, "dict_keys");
    let dict_items = make_builtin_class(_py, "dict_items");
    let dict_values = make_builtin_class(_py, "dict_values");
    let set = make_builtin_class(_py, "set");
    let frozenset = make_builtin_class(_py, "frozenset");
    let range = make_builtin_class(_py, "range");
    let slice = make_builtin_class(_py, "slice");
    let memoryview = make_builtin_class(_py, "memoryview");
    let io_base = make_builtin_class(_py, "IOBase");
    let raw_io_base = make_builtin_class(_py, "RawIOBase");
    let buffered_io_base = make_builtin_class(_py, "BufferedIOBase");
    let text_io_base = make_builtin_class(_py, "TextIOBase");
    let file = make_builtin_class(_py, "file");
    let file_io = make_builtin_class(_py, "FileIO");
    let buffered_reader = make_builtin_class(_py, "BufferedReader");
    let buffered_writer = make_builtin_class(_py, "BufferedWriter");
    let buffered_random = make_builtin_class(_py, "BufferedRandom");
    let text_io_wrapper = make_builtin_class(_py, "TextIOWrapper");
    let bytes_io = make_builtin_class(_py, "BytesIO");
    let string_io = make_builtin_class(_py, "StringIO");
    let function = make_builtin_class(_py, "function");
    let coroutine = make_builtin_class(_py, "coroutine");
    let generator = make_builtin_class(_py, "generator");
    let async_generator = make_builtin_class(_py, "async_generator");
    let iterator = make_builtin_class(_py, "iterator");
    let callable_iterator = make_builtin_class(_py, "callable_iterator");
    let enumerate = make_builtin_class(_py, "enumerate");
    let reversed = make_builtin_class(_py, "reversed");
    let zip = make_builtin_class(_py, "zip");
    let map = make_builtin_class(_py, "map");
    let filter = make_builtin_class(_py, "filter");
    let builtin_function_or_method = make_builtin_class(_py, "builtin_function_or_method");
    let code = make_builtin_class(_py, "code");
    let frame = make_builtin_class(_py, "frame");
    let traceback = make_builtin_class(_py, "traceback");
    let module = make_builtin_class(_py, "module");
    let super_type = make_builtin_class(_py, "super");
    let generic_alias = make_builtin_class(_py, "GenericAlias");
    let union_type = make_builtin_class(_py, union_type_class_name());

    unsafe {
        for bits in [
            object,
            none_type,
            not_implemented_type,
            ellipsis_type,
            base_exception,
            exception,
            base_exception_group,
            exception_group,
            int,
            float,
            complex,
            bool,
            str,
            bytes,
            bytearray,
            list,
            tuple,
            dict,
            dict_keys,
            dict_items,
            dict_values,
            set,
            frozenset,
            range,
            slice,
            memoryview,
            io_base,
            raw_io_base,
            buffered_io_base,
            text_io_base,
            file,
            file_io,
            buffered_reader,
            buffered_writer,
            buffered_random,
            text_io_wrapper,
            bytes_io,
            string_io,
            function,
            coroutine,
            generator,
            async_generator,
            iterator,
            callable_iterator,
            enumerate,
            reversed,
            zip,
            map,
            filter,
            builtin_function_or_method,
            code,
            frame,
            traceback,
            module,
            super_type,
            generic_alias,
            union_type,
        ] {
            if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                if object_type_id(ptr) == TYPE_ID_TYPE {
                    object_set_class_bits(_py, ptr, type_obj);
                    inc_ref_bits(_py, type_obj);
                }
            }
        }
        if let Some(ptr) = obj_from_bits(type_obj).as_ptr() {
            if object_type_id(ptr) == TYPE_ID_TYPE {
                object_set_class_bits(_py, ptr, type_obj);
                inc_ref_bits(_py, type_obj);
            }
        }
    }

    let _ = molt_class_set_base(object, MoltObject::none().bits());
    let _ = molt_class_set_base(type_obj, object);
    let _ = molt_class_set_base(none_type, object);
    let _ = molt_class_set_base(not_implemented_type, object);
    let _ = molt_class_set_base(ellipsis_type, object);
    let _ = molt_class_set_base(base_exception, object);
    let _ = molt_class_set_base(exception, base_exception);
    let _ = molt_class_set_base(base_exception_group, base_exception);
    let exception_group_bases = alloc_tuple(_py, &[base_exception_group, exception]);
    if !exception_group_bases.is_null() {
        let bases_bits = MoltObject::from_ptr(exception_group_bases).bits();
        let _ = molt_class_set_base(exception_group, bases_bits);
        dec_ref_bits(_py, bases_bits);
    }
    let _ = molt_class_set_base(int, object);
    let _ = molt_class_set_base(float, object);
    let _ = molt_class_set_base(complex, object);
    let _ = molt_class_set_base(bool, int);
    init_int_subclass_layout(_py, int);
    let _ = molt_class_set_base(str, object);
    let _ = molt_class_set_base(bytes, object);
    let _ = molt_class_set_base(bytearray, object);
    let _ = molt_class_set_base(list, object);
    let _ = molt_class_set_base(tuple, object);
    let _ = molt_class_set_base(dict, object);
    init_dict_subclass_layout(_py, dict);
    let _ = molt_class_set_base(dict_keys, object);
    let _ = molt_class_set_base(dict_items, object);
    let _ = molt_class_set_base(dict_values, object);
    let _ = molt_class_set_base(set, object);
    let _ = molt_class_set_base(frozenset, object);
    let _ = molt_class_set_base(range, object);
    let _ = molt_class_set_base(slice, object);
    let _ = molt_class_set_base(memoryview, object);
    let _ = molt_class_set_base(io_base, object);
    let _ = molt_class_set_base(raw_io_base, io_base);
    let _ = molt_class_set_base(buffered_io_base, io_base);
    let _ = molt_class_set_base(text_io_base, io_base);
    let _ = molt_class_set_base(file, io_base);
    let _ = molt_class_set_base(file_io, raw_io_base);
    let _ = molt_class_set_base(buffered_reader, buffered_io_base);
    let _ = molt_class_set_base(buffered_writer, buffered_io_base);
    let _ = molt_class_set_base(buffered_random, buffered_io_base);
    let _ = molt_class_set_base(text_io_wrapper, text_io_base);
    let _ = molt_class_set_base(bytes_io, buffered_io_base);
    let _ = molt_class_set_base(string_io, text_io_base);
    let _ = molt_class_set_base(function, object);
    let _ = molt_class_set_base(coroutine, object);
    let _ = molt_class_set_base(generator, object);
    let _ = molt_class_set_base(async_generator, object);
    let _ = molt_class_set_base(iterator, object);
    let _ = molt_class_set_base(callable_iterator, object);
    let _ = molt_class_set_base(enumerate, object);
    let _ = molt_class_set_base(reversed, object);
    let _ = molt_class_set_base(zip, object);
    let _ = molt_class_set_base(map, object);
    let _ = molt_class_set_base(filter, object);
    let _ = molt_class_set_base(builtin_function_or_method, object);
    let _ = molt_class_set_base(code, object);
    let _ = molt_class_set_base(frame, object);
    let _ = molt_class_set_base(traceback, object);
    let _ = molt_class_set_base(module, object);
    let _ = molt_class_set_base(super_type, object);
    let _ = molt_class_set_base(generic_alias, object);
    let _ = molt_class_set_base(union_type, object);

    BuiltinClasses {
        object,
        type_obj,
        none_type,
        not_implemented_type,
        ellipsis_type,
        base_exception,
        exception,
        base_exception_group,
        exception_group,
        int,
        float,
        complex,
        bool,
        str,
        bytes,
        bytearray,
        list,
        tuple,
        dict,
        dict_keys,
        dict_items,
        dict_values,
        set,
        frozenset,
        range,
        slice,
        memoryview,
        io_base,
        raw_io_base,
        buffered_io_base,
        text_io_base,
        file,
        file_io,
        buffered_reader,
        buffered_writer,
        buffered_random,
        text_io_wrapper,
        bytes_io,
        string_io,
        function,
        coroutine,
        generator,
        async_generator,
        iterator,
        callable_iterator,
        enumerate,
        reversed,
        zip,
        map,
        filter,
        builtin_function_or_method,
        code,
        frame,
        traceback,
        module,
        super_type,
        generic_alias,
        union_type,
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

pub(crate) fn builtin_classes_if_initialized(_py: &PyToken<'_>) -> Option<&'static BuiltinClasses> {
    let state = runtime_state_for_gil()?;
    let ptr = state.builtin_classes.load(AtomicOrdering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
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
            builtins.complex,
            builtins.bool,
            builtins.str,
            builtins.bytes,
            builtins.bytearray,
            builtins.list,
            builtins.tuple,
            builtins.dict,
            builtins.dict_keys,
            builtins.dict_items,
            builtins.dict_values,
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
            builtins.bytes_io,
            builtins.string_io,
            builtins.function,
            builtins.coroutine,
            builtins.generator,
            builtins.async_generator,
            builtins.iterator,
            builtins.callable_iterator,
            builtins.enumerate,
            builtins.reversed,
            builtins.zip,
            builtins.map,
            builtins.filter,
            builtins.builtin_function_or_method,
            builtins.code,
            builtins.frame,
            builtins.traceback,
            builtins.module,
            builtins.super_type,
            builtins.generic_alias,
            builtins.union_type,
        ] {
            class_break_cycles(py, bits);
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
        || bits == builtins.base_exception_group
        || bits == builtins.exception_group
        || bits == builtins.int
        || bits == builtins.float
        || bits == builtins.complex
        || bits == builtins.bool
        || bits == builtins.str
        || bits == builtins.bytes
        || bits == builtins.bytearray
        || bits == builtins.list
        || bits == builtins.tuple
        || bits == builtins.dict
        || bits == builtins.dict_keys
        || bits == builtins.dict_items
        || bits == builtins.dict_values
        || bits == builtins.set
        || bits == builtins.frozenset
        || bits == builtins.range
        || bits == builtins.slice
        || bits == builtins.memoryview
        || bits == builtins.io_base
        || bits == builtins.raw_io_base
        || bits == builtins.buffered_io_base
        || bits == builtins.text_io_base
        || bits == builtins.file
        || bits == builtins.file_io
        || bits == builtins.buffered_reader
        || bits == builtins.buffered_writer
        || bits == builtins.buffered_random
        || bits == builtins.text_io_wrapper
        || bits == builtins.bytes_io
        || bits == builtins.string_io
        || bits == builtins.function
        || bits == builtins.coroutine
        || bits == builtins.generator
        || bits == builtins.async_generator
        || bits == builtins.iterator
        || bits == builtins.callable_iterator
        || bits == builtins.enumerate
        || bits == builtins.reversed
        || bits == builtins.zip
        || bits == builtins.map
        || bits == builtins.filter
        || bits == builtins.builtin_function_or_method
        || bits == builtins.code
        || bits == builtins.frame
        || bits == builtins.traceback
        || bits == builtins.module
        || bits == builtins.super_type
        || bits == builtins.generic_alias
        || bits == builtins.union_type
}

pub(crate) fn builtin_type_bits(_py: &PyToken<'_>, tag: i64) -> Option<u64> {
    let builtins = builtin_classes(_py);
    match tag {
        TYPE_TAG_INT => Some(builtins.int),
        TYPE_TAG_FLOAT => Some(builtins.float),
        TYPE_TAG_COMPLEX => Some(builtins.complex),
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
