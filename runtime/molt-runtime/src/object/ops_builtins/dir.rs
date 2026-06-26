// dir() and object.__dir__ introspection authority.
// Kept separate from call dispatch and object slot wrappers so builtin method-surface facts have one home.

use crate::*;
use molt_obj_model::MoltObject;
use std::collections::HashSet;

fn dir_runtime_python_at_least(_py: &PyToken<'_>, major: i64, minor: i64) -> bool {
    let state = runtime_state(_py);
    let guard = state.sys_version_info.lock().unwrap();
    let (runtime_major, runtime_minor) = guard
        .as_ref()
        .map(|info| (info.major, info.minor))
        .unwrap_or((3, 12));
    runtime_major > major || (runtime_major == major && runtime_minor >= minor)
}

fn dir_add_builtin_method_surface(
    _py: &PyToken<'_>,
    target_class_bits: u64,
    add_name: &mut dyn FnMut(&[u8]) -> bool,
) -> bool {
    let builtins = builtin_classes(_py);
    if target_class_bits == builtins.str {
        for name in [
            &b"capitalize"[..],
            &b"casefold"[..],
            &b"center"[..],
            &b"count"[..],
            &b"encode"[..],
            &b"endswith"[..],
            &b"expandtabs"[..],
            &b"find"[..],
            &b"format"[..],
            &b"format_map"[..],
            &b"index"[..],
            &b"isalnum"[..],
            &b"isalpha"[..],
            &b"isascii"[..],
            &b"isdecimal"[..],
            &b"isdigit"[..],
            &b"isidentifier"[..],
            &b"islower"[..],
            &b"isnumeric"[..],
            &b"isprintable"[..],
            &b"isspace"[..],
            &b"istitle"[..],
            &b"isupper"[..],
            &b"join"[..],
            &b"ljust"[..],
            &b"lower"[..],
            &b"lstrip"[..],
            &b"maketrans"[..],
            &b"partition"[..],
            &b"removeprefix"[..],
            &b"removesuffix"[..],
            &b"replace"[..],
            &b"rfind"[..],
            &b"rindex"[..],
            &b"rjust"[..],
            &b"rpartition"[..],
            &b"rsplit"[..],
            &b"rstrip"[..],
            &b"split"[..],
            &b"splitlines"[..],
            &b"startswith"[..],
            &b"strip"[..],
            &b"swapcase"[..],
            &b"title"[..],
            &b"translate"[..],
            &b"upper"[..],
            &b"zfill"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.bytes {
        for name in [
            &b"capitalize"[..],
            &b"center"[..],
            &b"count"[..],
            &b"decode"[..],
            &b"endswith"[..],
            &b"expandtabs"[..],
            &b"find"[..],
            &b"fromhex"[..],
            &b"hex"[..],
            &b"index"[..],
            &b"isalnum"[..],
            &b"isalpha"[..],
            &b"isascii"[..],
            &b"isdigit"[..],
            &b"islower"[..],
            &b"isspace"[..],
            &b"istitle"[..],
            &b"isupper"[..],
            &b"join"[..],
            &b"ljust"[..],
            &b"lower"[..],
            &b"lstrip"[..],
            &b"maketrans"[..],
            &b"partition"[..],
            &b"removeprefix"[..],
            &b"removesuffix"[..],
            &b"replace"[..],
            &b"rfind"[..],
            &b"rindex"[..],
            &b"rjust"[..],
            &b"rpartition"[..],
            &b"rsplit"[..],
            &b"rstrip"[..],
            &b"split"[..],
            &b"splitlines"[..],
            &b"startswith"[..],
            &b"strip"[..],
            &b"swapcase"[..],
            &b"title"[..],
            &b"translate"[..],
            &b"upper"[..],
            &b"zfill"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.bytearray {
        for name in [
            &b"append"[..],
            &b"capitalize"[..],
            &b"center"[..],
            &b"clear"[..],
            &b"copy"[..],
            &b"count"[..],
            &b"decode"[..],
            &b"endswith"[..],
            &b"expandtabs"[..],
            &b"extend"[..],
            &b"find"[..],
            &b"fromhex"[..],
            &b"hex"[..],
            &b"index"[..],
            &b"insert"[..],
            &b"isalnum"[..],
            &b"isalpha"[..],
            &b"isascii"[..],
            &b"isdigit"[..],
            &b"islower"[..],
            &b"isspace"[..],
            &b"istitle"[..],
            &b"isupper"[..],
            &b"join"[..],
            &b"ljust"[..],
            &b"lower"[..],
            &b"lstrip"[..],
            &b"maketrans"[..],
            &b"partition"[..],
            &b"pop"[..],
            &b"remove"[..],
            &b"removeprefix"[..],
            &b"removesuffix"[..],
            &b"replace"[..],
            &b"reverse"[..],
            &b"rfind"[..],
            &b"rindex"[..],
            &b"rjust"[..],
            &b"rpartition"[..],
            &b"rsplit"[..],
            &b"rstrip"[..],
            &b"split"[..],
            &b"splitlines"[..],
            &b"startswith"[..],
            &b"strip"[..],
            &b"swapcase"[..],
            &b"title"[..],
            &b"translate"[..],
            &b"upper"[..],
            &b"zfill"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        if dir_runtime_python_at_least(_py, 3, 14) && !add_name(&b"resize"[..]) {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.int || target_class_bits == builtins.bool {
        for name in [
            &b"as_integer_ratio"[..],
            &b"bit_count"[..],
            &b"bit_length"[..],
            &b"conjugate"[..],
            &b"from_bytes"[..],
            &b"is_integer"[..],
            &b"to_bytes"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.float {
        for name in [
            &b"as_integer_ratio"[..],
            &b"conjugate"[..],
            &b"fromhex"[..],
            &b"hex"[..],
            &b"is_integer"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        if dir_runtime_python_at_least(_py, 3, 14) && !add_name(&b"from_number"[..]) {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.complex {
        if !add_name(&b"conjugate"[..]) {
            return false;
        }
        if dir_runtime_python_at_least(_py, 3, 14) && !add_name(&b"from_number"[..]) {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.list {
        for name in [
            &b"append"[..],
            &b"clear"[..],
            &b"copy"[..],
            &b"count"[..],
            &b"extend"[..],
            &b"index"[..],
            &b"insert"[..],
            &b"pop"[..],
            &b"remove"[..],
            &b"reverse"[..],
            &b"sort"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.tuple {
        return add_name(&b"count"[..]) && add_name(&b"index"[..]);
    }
    if target_class_bits == builtins.range {
        return add_name(&b"count"[..]) && add_name(&b"index"[..]);
    }
    if target_class_bits == builtins.dict {
        for name in [
            &b"clear"[..],
            &b"copy"[..],
            &b"fromkeys"[..],
            &b"get"[..],
            &b"items"[..],
            &b"keys"[..],
            &b"pop"[..],
            &b"popitem"[..],
            &b"setdefault"[..],
            &b"update"[..],
            &b"values"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.set {
        for name in [
            &b"add"[..],
            &b"clear"[..],
            &b"copy"[..],
            &b"difference"[..],
            &b"difference_update"[..],
            &b"discard"[..],
            &b"intersection"[..],
            &b"intersection_update"[..],
            &b"isdisjoint"[..],
            &b"issubset"[..],
            &b"issuperset"[..],
            &b"pop"[..],
            &b"remove"[..],
            &b"symmetric_difference"[..],
            &b"symmetric_difference_update"[..],
            &b"union"[..],
            &b"update"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.frozenset {
        for name in [
            &b"copy"[..],
            &b"difference"[..],
            &b"intersection"[..],
            &b"isdisjoint"[..],
            &b"issubset"[..],
            &b"issuperset"[..],
            &b"symmetric_difference"[..],
            &b"union"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.memoryview {
        for name in [
            &b"_from_flags"[..],
            &b"cast"[..],
            &b"hex"[..],
            &b"release"[..],
            &b"tobytes"[..],
            &b"tolist"[..],
            &b"toreadonly"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        if dir_runtime_python_at_least(_py, 3, 14)
            && (!add_name(&b"count"[..]) || !add_name(&b"index"[..]))
        {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.property {
        return add_name(&b"getter"[..]) && add_name(&b"setter"[..]) && add_name(&b"deleter"[..]);
    }
    if target_class_bits == builtins.base_exception_group
        || issubclass_bits(target_class_bits, builtins.base_exception_group)
    {
        return add_name(&b"add_note"[..])
            && add_name(&b"with_traceback"[..])
            && add_name(&b"derive"[..])
            && add_name(&b"split"[..])
            && add_name(&b"subgroup"[..]);
    }
    if target_class_bits == builtins.base_exception
        || issubclass_bits(target_class_bits, builtins.base_exception)
    {
        return add_name(&b"add_note"[..]) && add_name(&b"with_traceback"[..]);
    }
    if target_class_bits == builtins.slice {
        return add_name(&b"indices"[..]);
    }
    if target_class_bits == builtins.type_obj {
        return add_name(&b"mro"[..]);
    }
    true
}

unsafe fn dir_default_collect(_py: &PyToken<'_>, obj_bits: u64) -> u64 {
    unsafe {
        crate::gil_assert();

        let mut names: Vec<u64> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut extra_owned: Vec<u64> = Vec::new();

        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            let type_id = object_type_id(obj_ptr);
            if type_id == TYPE_ID_TYPE {
                dir_collect_from_class_bits(obj_bits, &mut seen, &mut names);
            } else {
                dir_collect_from_instance(_py, obj_ptr, &mut seen, &mut names);
                dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
            }
        } else {
            dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
        }

        // Our runtime keeps many builtin methods in fast method caches rather than in
        // `type.__dict__`. CPython's dir() includes those names, so ensure they're visible.
        let mut add_name = |name: &[u8]| -> bool {
            let Ok(name_str) = std::str::from_utf8(name) else {
                return true;
            };
            if !seen.insert(name_str.to_string()) {
                return true;
            }
            let Some(bits) = attr_name_bits_from_bytes(_py, name) else {
                return false;
            };
            extra_owned.push(bits);
            names.push(bits);
            true
        };

        // Object surface (ordering-critical names appear early in CPython's sorted dir()).
        for name in [
            &b"__class__"[..],
            &b"__delattr__"[..],
            &b"__dir__"[..],
            &b"__doc__"[..],
            &b"__eq__"[..],
            &b"__format__"[..],
            &b"__ge__"[..],
            &b"__getattribute__"[..],
            &b"__getstate__"[..],
            &b"__gt__"[..],
            &b"__hash__"[..],
            &b"__init__"[..],
            &b"__init_subclass__"[..],
            &b"__le__"[..],
            &b"__lt__"[..],
            &b"__ne__"[..],
            &b"__new__"[..],
            &b"__repr__"[..],
            &b"__setattr__"[..],
            &b"__str__"[..],
        ] {
            if !add_name(name) {
                for owned in extra_owned {
                    dec_ref_bits(_py, owned);
                }
                return MoltObject::none().bits();
            }
        }

        if maybe_ptr_from_bits(obj_bits).is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_TYPE) {
            for name in [
                &b"__bases__"[..],
                &b"__dict__"[..],
                &b"__module__"[..],
                &b"__mro__"[..],
                &b"__name__"[..],
                &b"__qualname__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        }

        let builtins = builtin_classes(_py);
        let target_class_bits = if maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_TYPE)
        {
            obj_bits
        } else {
            type_of_bits(_py, obj_bits)
        };

        if target_class_bits == builtins.int || target_class_bits == builtins.bool {
            for name in [
                &b"__abs__"[..],
                &b"__add__"[..],
                &b"__and__"[..],
                &b"__bool__"[..],
                &b"__ceil__"[..],
                &b"__divmod__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.str {
            for name in [&b"__add__"[..], &b"__contains__"[..], &b"__getitem__"[..]] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.list {
            for name in [
                &b"__add__"[..],
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.dict {
            for name in [
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
                &b"__getitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.none_type && !add_name(&b"__bool__"[..]) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }
        if !dir_add_builtin_method_surface(_py, target_class_bits, &mut add_name) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }

        // Hide names that CPython deliberately excludes from dir() output (even though the
        // attributes exist).
        let hide_module = is_builtin_class_bits(_py, target_class_bits);
        names.retain(|&bits| {
            let Some(name) = string_obj_to_owned(obj_from_bits(bits)) else {
                return true;
            };
            if name == "__mro__" || name == "__bases__" || name == "__text_signature__" {
                return false;
            }
            if name.starts_with("__molt_") {
                return false;
            }
            if hide_module && name == "__module__" {
                return false;
            }
            true
        });

        let list_ptr = alloc_list(_py, &names);
        for owned in extra_owned {
            dec_ref_bits(_py, owned);
        }
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let none_bits = MoltObject::none().bits();
        let reverse_bits = MoltObject::from_int(0).bits();
        let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        list_bits
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_dir_method(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { unsafe { dir_default_collect(_py, self_bits) } })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dir_builtin(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if obj_bits == missing {
            // CPython: dir() (no args) lists the caller's local scope.
            unsafe {
                // Note: `molt_locals_builtin` is safe to call here; `with_gil_entry` is
                // re-entrant and many runtime helpers rely on nested calls.
                let locals_bits = crate::molt_locals_builtin();
                if exception_pending(_py) {
                    if !obj_from_bits(locals_bits).is_none() {
                        dec_ref_bits(_py, locals_bits);
                    }
                    return MoltObject::none().bits();
                }
                let list_bits = list_from_iter_bits(_py, locals_bits)
                    .unwrap_or_else(|| MoltObject::none().bits());
                if !obj_from_bits(locals_bits).is_none() {
                    dec_ref_bits(_py, locals_bits);
                }
                if obj_from_bits(list_bits).is_none() || exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let none_bits = MoltObject::none().bits();
                let reverse_bits = MoltObject::from_int(0).bits();
                let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return list_bits;
            }
        }

        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            unsafe {
                // CPython's dir() respects user-defined `__dir__`, but it must not
                // dispatch to our internal fast-path method-cache implementation.
                static DIR_NAME: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                let dir_name_bits = intern_static_name(_py, &DIR_NAME, b"__dir__");
                let mut override_bits: u64 = 0;

                // PEP 562: a module's own `__dir__` in its namespace overrides
                // dir(module). Module objects do not use the object instance dict slot,
                // so select the module dict explicitly.
                let dict_bits = if object_type_id(obj_ptr) == TYPE_ID_MODULE {
                    module_dict_bits(obj_ptr)
                } else {
                    instance_dict_bits(obj_ptr)
                };
                if dict_bits != 0
                    && !obj_from_bits(dict_bits).is_none()
                    && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                    && let Some(val_bits) = dict_get_in_place(_py, dict_ptr, dir_name_bits)
                {
                    inc_ref_bits(_py, val_bits);
                    override_bits = val_bits;
                }

                if override_bits == 0 {
                    let class_bits = type_of_bits(_py, obj_bits);
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                        && let Some(attr_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, dir_name_bits)
                    {
                        let bound_opt = descriptor_bind(_py, attr_bits, class_ptr, Some(obj_ptr));
                        dec_ref_bits(_py, attr_bits);

                        if exception_pending(_py) {
                            if let Some(bound_bits) = bound_opt
                                && !obj_from_bits(bound_bits).is_none()
                            {
                                dec_ref_bits(_py, bound_bits);
                            }
                            return MoltObject::none().bits();
                        }

                        if let Some(bound_bits) = bound_opt {
                            override_bits = bound_bits;
                        }
                    }
                }

                if override_bits != 0 && !obj_from_bits(override_bits).is_none() {
                    let res_bits = call_callable0(_py, override_bits);
                    dec_ref_bits(_py, override_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    // CPython materializes and sorts a user `__dir__` result.
                    let Some(list_bits) = list_from_iter_bits(_py, res_bits) else {
                        dec_ref_bits(_py, res_bits);
                        return MoltObject::none().bits();
                    };
                    dec_ref_bits(_py, res_bits);
                    let none_bits = MoltObject::none().bits();
                    let reverse_bits = MoltObject::from_int(0).bits();
                    let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, list_bits);
                        return MoltObject::none().bits();
                    }
                    return list_bits;
                }
            }
        }

        unsafe { dir_default_collect(_py, obj_bits) }
    })
}
