use crate::*;

pub(crate) unsafe fn class_mro_ref(class_ptr: *mut u8) -> Option<&'static Vec<u64>> {
    let mro_bits = class_mro_bits(class_ptr);
    let mro_obj = obj_from_bits(mro_bits);
    let mro_ptr = mro_obj.as_ptr()?;
    if object_type_id(mro_ptr) != TYPE_ID_TUPLE {
        return None;
    }
    Some(seq_vec_ref(mro_ptr))
}

pub(crate) fn class_mro_vec(class_bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return vec![class_bits];
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return vec![class_bits];
        }
        if let Some(mro) = class_mro_ref(ptr) {
            return mro.clone();
        }
        let mut out = vec![class_bits];
        let bases_bits = class_bases_bits(ptr);
        let bases = class_bases_vec(bases_bits);
        for base in bases {
            out.extend(class_mro_vec(base));
        }
        out
    }
}

pub(crate) fn class_bases_vec(bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() || bits == 0 {
        return Vec::new();
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_TYPE => return vec![bits],
                TYPE_ID_TUPLE => return seq_vec_ref(ptr).clone(),
                _ => {}
            }
        }
    }
    Vec::new()
}

pub(crate) fn type_of_bits(_py: &PyToken<'_>, val_bits: u64) -> u64 {
    let builtins = builtin_classes(_py);
    let obj = obj_from_bits(val_bits);
    if obj.is_none() {
        return builtins.none_type;
    }
    if obj.is_bool() {
        return builtins.bool;
    }
    if obj.is_int() {
        return builtins.int;
    }
    if obj.is_float() {
        return builtins.float;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            return match object_type_id(ptr) {
                TYPE_ID_STRING => builtins.str,
                TYPE_ID_BYTES => builtins.bytes,
                TYPE_ID_BYTEARRAY => builtins.bytearray,
                TYPE_ID_LIST => builtins.list,
                TYPE_ID_TUPLE => builtins.tuple,
                TYPE_ID_DICT => builtins.dict,
                TYPE_ID_SET => builtins.set,
                TYPE_ID_FROZENSET => builtins.frozenset,
                TYPE_ID_BIGINT => builtins.int,
                TYPE_ID_RANGE => builtins.range,
                TYPE_ID_SLICE => builtins.slice,
                TYPE_ID_MEMORYVIEW => builtins.memoryview,
                TYPE_ID_FILE_HANDLE => {
                    let handle_ptr = file_handle_ptr(ptr);
                    if !handle_ptr.is_null() {
                        let handle = &*handle_ptr;
                        if handle.class_bits != 0 {
                            return handle.class_bits;
                        }
                    }
                    builtins.file
                }
                TYPE_ID_NOT_IMPLEMENTED => builtins.not_implemented_type,
                TYPE_ID_ELLIPSIS => builtins.ellipsis_type,
                TYPE_ID_EXCEPTION => {
                    let class_bits = exception_class_bits(ptr);
                    if !obj_from_bits(class_bits).is_none() && class_bits != 0 {
                        class_bits
                    } else {
                        exception_type_bits(_py, exception_kind_bits(ptr))
                    }
                }
                TYPE_ID_FUNCTION => builtins.function,
                TYPE_ID_CODE => builtins.code,
                TYPE_ID_MODULE => builtins.module,
                TYPE_ID_TYPE => builtins.type_obj,
                TYPE_ID_GENERIC_ALIAS => builtins.generic_alias,
                TYPE_ID_SUPER => builtins.super_type,
                TYPE_ID_DATACLASS => {
                    let desc_ptr = dataclass_desc_ptr(ptr);
                    if !desc_ptr.is_null() {
                        let class_bits = (*desc_ptr).class_bits;
                        if class_bits != 0 {
                            return class_bits;
                        }
                    }
                    builtins.object
                }
                TYPE_ID_OBJECT => {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_bits
                    } else {
                        builtins.object
                    }
                }
                _ => builtins.object,
            };
        }
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let class_bits = object_class_bits(ptr);
            if class_bits != 0 {
                return class_bits;
            }
        }
    }
    builtins.object
}

fn collect_classinfo_isinstance(_py: &PyToken<'_>, class_bits: u64, out: &mut Vec<u64>) {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>(_py,
            "TypeError",
            "isinstance() arg 2 must be a type or tuple of types",
        );
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_TYPE => out.push(class_bits),
            TYPE_ID_TUPLE => {
                let items = seq_vec_ref(ptr);
                for item in items.iter() {
                    collect_classinfo_isinstance(_py, *item, out);
                }
            }
            _ => {
                return raise_exception::<_>(_py,
                    "TypeError",
                    "isinstance() arg 2 must be a type or tuple of types",
                )
            }
        }
    }
}

pub(crate) fn issubclass_bits(sub_bits: u64, class_bits: u64) -> bool {
    if sub_bits == class_bits {
        return true;
    }
    let obj = obj_from_bits(sub_bits);
    let Some(ptr) = obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return false;
        }
        if let Some(mro) = class_mro_ref(ptr) {
            return mro.contains(&class_bits);
        }
    }
    class_mro_vec(sub_bits).contains(&class_bits)
}

pub(crate) fn isinstance_bits(_py: &PyToken<'_>, val_bits: u64, class_bits: u64) -> bool {
    let mut classes = Vec::new();
    collect_classinfo_isinstance(_py, class_bits, &mut classes);
    let val_type = type_of_bits(_py, val_bits);
    for class_bits in classes {
        if issubclass_bits(val_type, class_bits) {
            return true;
        }
    }
    false
}
