//! Formatting, repr, and string conversion — extracted from ops.rs.
#![allow(clippy::items_after_test_module)]

use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use std::borrow::Cow;

use super::ops::{range_components_bigint, unicode_printable_table};
use super::ops_string::wtf8_from_bytes;

#[unsafe(no_mangle)]
/// Print a bare newline to stdout (used by the `print_newline` op).
pub extern "C" fn molt_print_newline() {
    use std::io::Write;
    let _ = std::io::stdout().write_all(b"\n");
    let _ = std::io::stdout().flush();
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_print_obj(val: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let args_ptr = alloc_tuple(_py, &[val]);
        if args_ptr.is_null() {
            return;
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let none_bits = MoltObject::none().bits();
        let flush_bits = MoltObject::from_bool(true).bits();
        let res_bits = molt_print_builtin(args_bits, none_bits, none_bits, none_bits, flush_bits);
        dec_ref_bits(_py, res_bits);
        dec_ref_bits(_py, args_bits);
    })
}

#[unsafe(no_mangle)]
/// Print a string to stderr followed by newline.  Used by the compiler to
/// emit runtime warnings (DeprecationWarning, etc.) in CPython's format.
pub extern "C" fn molt_warn_stderr(msg_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(msg_bits);
        if let Some(s) = string_obj_to_owned(obj) {
            // Flush stdout first to ensure correct ordering when stdout and
            // stderr are merged (e.g. `./binary 2>&1`).  CPython's warnings
            // module does this implicitly through Python's I/O layer.
            use std::io::Write;
            let _ = std::io::stdout().flush();
            eprintln!("{}", s);
        }
    })
}

fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        if f.is_sign_negative() {
            return "-inf".to_string();
        }
        return "inf".to_string();
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        return format_float_scientific(f);
    }
    if f.fract() == 0.0 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

fn format_float_scientific(f: f64) -> String {
    let raw = f.to_string();
    if raw.contains('e') || raw.contains('E') {
        return normalize_scientific(&raw);
    }
    let mut digits = raw.as_str();
    if let Some(rest) = digits.strip_prefix('-') {
        digits = rest;
    }
    let digits_only: String = digits.chars().filter(|ch| *ch != '.').collect();
    let sig_digits = digits_only.trim_start_matches('0').len().max(1);
    let precision = sig_digits.saturating_sub(1).min(16);
    let formatted = format!("{:.*e}", precision, f);
    normalize_scientific(&formatted)
}

fn normalize_scientific(formatted: &str) -> String {
    let normalized = formatted.to_lowercase();
    let Some(exp_pos) = normalized.find('e') else {
        return normalized;
    };
    let (mantissa, exp) = normalized.split_at(exp_pos);
    let mut mant = mantissa.to_string();
    if mant.contains('.') {
        while mant.ends_with('0') {
            mant.pop();
        }
        if mant.ends_with('.') {
            mant.pop();
        }
    }
    let exp_val: i32 = exp[1..].parse().unwrap_or(0);
    let sign = if exp_val < 0 { "-" } else { "+" };
    let exp_abs = exp_val.unsigned_abs();
    let exp_text = format!("{exp_abs:02}");
    format!("{mant}e{sign}{exp_text}")
}

fn format_complex_float(f: f64) -> String {
    let text = format_float(f);
    if let Some(stripped) = text.strip_suffix(".0") {
        stripped.to_string()
    } else {
        text
    }
}

fn format_complex(re: f64, im: f64) -> String {
    let re_zero = re == 0.0 && !re.is_sign_negative();
    let re_text = format_complex_float(re);
    if re_zero {
        let im_text = format_complex_float(im);
        return format!("{im_text}j");
    }
    let sign = if im.is_sign_negative() { "-" } else { "+" };
    let im_text = format_complex_float(im.abs());
    format!("({re_text}{sign}{im_text}j)")
}

fn format_range(start: &BigInt, stop: &BigInt, step: &BigInt) -> String {
    if step == &BigInt::from(1) {
        format!("range({start}, {stop})")
    } else {
        format!("range({start}, {stop}, {step})")
    }
}

fn format_slice(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    unsafe {
        let start = format_obj(_py, obj_from_bits(slice_start_bits(ptr)));
        let stop = format_obj(_py, obj_from_bits(slice_stop_bits(ptr)));
        let step = format_obj(_py, obj_from_bits(slice_step_bits(ptr)));
        format!("slice({start}, {stop}, {step})")
    }
}

fn format_type_name_for_alias(_py: &PyToken<'_>, type_ptr: *mut u8) -> Option<String> {
    unsafe {
        let name =
            string_obj_to_owned(obj_from_bits(class_name_bits(type_ptr))).unwrap_or_default();
        if name.is_empty() {
            return None;
        }
        let mut qualname = name;
        let mut module_name: Option<String> = None;
        if !exception_pending(_py) {
            let dict_bits = class_dict_bits(type_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(module_key) = attr_name_bits_from_bytes(_py, b"__module__")
                    && let Some(bits) = dict_get_in_place(_py, dict_ptr, module_key)
                    && let Some(val) = string_obj_to_owned(obj_from_bits(bits))
                {
                    module_name = Some(val);
                }
                if let Some(qual_key) = attr_name_bits_from_bytes(_py, b"__qualname__")
                    && let Some(bits) = dict_get_in_place(_py, dict_ptr, qual_key)
                    && let Some(val) = string_obj_to_owned(obj_from_bits(bits))
                {
                    qualname = val;
                }
            }
        }
        if let Some(module) = module_name
            && !module.is_empty()
            && module != "builtins"
        {
            return Some(format!("{module}.{qualname}"));
        }
        Some(qualname)
    }
}

fn format_generic_alias(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    unsafe {
        let origin_bits = generic_alias_origin_bits(ptr);
        let args_bits = generic_alias_args_bits(ptr);
        let origin_obj = obj_from_bits(origin_bits);
        let render_arg = |arg_bits: u64| {
            let arg_obj = obj_from_bits(arg_bits);
            if let Some(arg_ptr) = arg_obj.as_ptr()
                && object_type_id(arg_ptr) == TYPE_ID_TYPE
                && let Some(name) = format_type_name_for_alias(_py, arg_ptr)
            {
                return name;
            }
            format_obj(_py, arg_obj)
        };
        let origin_repr = if let Some(origin_ptr) = origin_obj.as_ptr() {
            if object_type_id(origin_ptr) == TYPE_ID_TYPE {
                format_type_name_for_alias(_py, origin_ptr)
                    .unwrap_or_else(|| format_obj(_py, origin_obj))
            } else {
                format_obj(_py, origin_obj)
            }
        } else {
            format_obj(_py, origin_obj)
        };
        let mut out = String::new();
        out.push_str(&origin_repr);
        out.push('[');
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr() {
            if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(args_ptr);
                for (idx, elem_bits) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&render_arg(*elem_bits));
                }
            } else {
                out.push_str(&render_arg(args_bits));
            }
        } else {
            out.push_str(&render_arg(args_bits));
        }
        out.push(']');
        out
    }
}

fn format_union_type(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    unsafe {
        let args_bits = union_type_args_bits(ptr);
        let render_arg = |arg_bits: u64| {
            let arg_obj = obj_from_bits(arg_bits);
            if let Some(arg_ptr) = arg_obj.as_ptr()
                && object_type_id(arg_ptr) == TYPE_ID_TYPE
                && let Some(name) = format_type_name_for_alias(_py, arg_ptr)
            {
                return name;
            }
            format_obj(_py, arg_obj)
        };
        let mut out = String::new();
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr()
            && object_type_id(args_ptr) == TYPE_ID_TUPLE
        {
            let elems = seq_vec_ref(args_ptr);
            for (idx, elem_bits) in elems.iter().enumerate() {
                if idx > 0 {
                    out.push_str(" | ");
                }
                out.push_str(&render_arg(*elem_bits));
            }
            return out;
        }
        out.push_str(&render_arg(args_bits));
        out
    }
}

pub(crate) fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_STRING {
            return None;
        }
        let len = string_len(ptr);
        let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
        Some(String::from_utf8_lossy(bytes).to_string())
    }
}

pub(crate) fn decode_string_list(obj: MoltObject) -> Option<Vec<String>> {
    let ptr = obj.as_ptr()?;
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return None;
        }
        let elems = seq_vec_ref(ptr);
        let mut out = Vec::with_capacity(elems.len());
        for &elem_bits in elems.iter() {
            let elem_obj = obj_from_bits(elem_bits);
            let s = string_obj_to_owned(elem_obj)?;
            out.push(s);
        }
        Some(out)
    }
}

pub(crate) fn decode_value_list(obj: MoltObject) -> Option<Vec<u64>> {
    let ptr = obj.as_ptr()?;
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return None;
        }
        let elems = seq_vec_ref(ptr);
        Some(elems.to_vec())
    }
}

fn format_dataclass(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(ptr);
        if desc_ptr.is_null() {
            return "<dataclass>".to_string();
        }
        let desc = &*desc_ptr;
        let fields = dataclass_fields_ref(ptr);
        let mut out = String::new();
        out.push_str(&desc.name);
        out.push('(');
        let mut first = true;
        for (idx, name) in desc.field_names.iter().enumerate() {
            let flag = desc.field_flags.get(idx).copied().unwrap_or(0x7);
            if (flag & 0x1) == 0 {
                continue;
            }
            if !first {
                out.push_str(", ");
            }
            first = false;
            out.push_str(name);
            out.push('=');
            let val = fields
                .get(idx)
                .copied()
                .unwrap_or(MoltObject::none().bits());
            if is_missing_bits(_py, val) {
                let type_label = if desc.name.is_empty() {
                    "dataclass"
                } else {
                    desc.name.as_str()
                };
                let _ = attr_error(_py, type_label, name);
                return "<dataclass>".to_string();
            }
            out.push_str(&format_obj(_py, obj_from_bits(val)));
        }
        out.push(')');
        out
    }
}

struct ReprGuard {
    ptr: *mut u8,
    active: bool,
    depth_active: bool,
}

impl ReprGuard {
    fn new(_py: &PyToken<'_>, ptr: *mut u8) -> Self {
        if !repr_depth_enter() {
            let _ = raise_exception::<u64>(
                _py,
                "RecursionError",
                "maximum recursion depth exceeded while getting the repr of an object",
            );
            return Self {
                ptr,
                active: false,
                depth_active: false,
            };
        }
        let active = REPR_STACK.with(|stack| {
            REPR_SET.with(|set| {
                let mut set = set.borrow_mut();
                let slot = PtrSlot(ptr);
                if !set.insert(slot) {
                    return false;
                }
                stack.borrow_mut().push(slot);
                true
            })
        });
        if !active {
            repr_depth_exit();
        }
        Self {
            ptr,
            active,
            depth_active: active,
        }
    }

    fn active(&self) -> bool {
        self.active
    }
}

impl Drop for ReprGuard {
    fn drop(&mut self) {
        if self.active {
            REPR_SET.with(|set| {
                set.borrow_mut().remove(&PtrSlot(self.ptr));
            });
            REPR_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                if stack.last().is_some_and(|slot| slot.0 == self.ptr) {
                    stack.pop();
                } else if let Some(pos) = stack.iter().rposition(|slot| slot.0 == self.ptr) {
                    stack.remove(pos);
                }
            });
        }
        if self.depth_active {
            repr_depth_exit();
        }
    }
}

fn repr_depth_enter() -> bool {
    let limit = recursion_limit_get();
    REPR_DEPTH.with(|depth| {
        let current = depth.get();
        if current + 1 > limit {
            false
        } else {
            depth.set(current + 1);
            true
        }
    })
}

fn repr_depth_exit() {
    REPR_DEPTH.with(|depth| {
        let current = depth.get();
        if current > 0 {
            depth.set(current - 1);
        }
    });
}

fn format_default_object_repr(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let class_bits = unsafe {
        if object_type_id(ptr) == TYPE_ID_OBJECT || object_type_id(ptr) == TYPE_ID_DATACLASS {
            object_class_bits(ptr)
        } else {
            type_of_bits(_py, MoltObject::from_ptr(ptr).bits())
        }
    };
    let class_name = class_name_for_error(class_bits);
    // Look up __module__ on the class to produce CPython-style qualified repr.
    let class_obj = obj_from_bits(class_bits);
    if let Some(class_ptr) = class_obj.as_ptr() {
        unsafe {
            if object_type_id(class_ptr) == TYPE_ID_TYPE && !exception_pending(_py) {
                let dict_bits = class_dict_bits(class_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                    && let Some(module_key) = attr_name_bits_from_bytes(_py, b"__module__")
                    && let Some(bits) = dict_get_in_place(_py, dict_ptr, module_key)
                    && let Some(module) = string_obj_to_owned(obj_from_bits(bits))
                    && !module.is_empty()
                    && module != "builtins"
                {
                    let mut qualname = class_name.clone();
                    if let Some(qual_key) = attr_name_bits_from_bytes(_py, b"__qualname__")
                        && let Some(qbits) = dict_get_in_place(_py, dict_ptr, qual_key)
                        && let Some(val) = string_obj_to_owned(obj_from_bits(qbits))
                    {
                        qualname = val;
                    }
                    return format!("<{module}.{qualname} object at 0x{:x}>", ptr as usize);
                }
            }
        }
    }
    format!("<{class_name} object at 0x{:x}>", ptr as usize)
}

fn call_bits_is_default_object_repr(call_bits: u64) -> bool {
    let call_obj = obj_from_bits(call_bits);
    let Some(mut call_ptr) = call_obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(call_ptr) == TYPE_ID_BOUND_METHOD {
            let func_bits = bound_method_func_bits(call_ptr);
            let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
                return false;
            };
            call_ptr = func_ptr;
        }
        object_type_id(call_ptr) == TYPE_ID_FUNCTION
            && function_fn_ptr(call_ptr) == fn_addr!(molt_repr_from_obj)
    }
}

fn call_bits_is_default_object_str(call_bits: u64) -> bool {
    let call_obj = obj_from_bits(call_bits);
    let Some(mut call_ptr) = call_obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(call_ptr) == TYPE_ID_BOUND_METHOD {
            let func_bits = bound_method_func_bits(call_ptr);
            let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
                return false;
            };
            call_ptr = func_ptr;
        }
        object_type_id(call_ptr) == TYPE_ID_FUNCTION
            && function_fn_ptr(call_ptr) == fn_addr!(molt_str_from_obj)
    }
}

unsafe fn exception_class_can_use_cached_message_str(
    _py: &PyToken<'_>,
    class_bits: u64,
    class_ptr: *mut u8,
) -> bool {
    unsafe {
        let builtins = builtin_classes(_py);
        if !issubclass_bits(class_bits, builtins.base_exception) {
            return false;
        }
        if builtins.base_exception_group != 0
            && issubclass_bits(class_bits, builtins.base_exception_group)
        {
            return false;
        }
        let class_name =
            string_obj_to_owned(obj_from_bits(class_name_bits(class_ptr))).unwrap_or_default();
        if matches!(
            class_name.as_str(),
            "KeyError"
                | "UnicodeDecodeError"
                | "UnicodeEncodeError"
                | "HTTPError"
                | "URLError"
                | "ContentTooShortError"
        ) {
            return false;
        }
        let str_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.str_name, b"__str__");
        let raw_str = class_attr_lookup_raw_mro(_py, class_ptr, str_name_bits);
        match raw_str {
            Some(bits) => {
                call_bits_is_default_object_str(bits) || call_bits_is_default_object_repr(bits)
            }
            None => true,
        }
    }
}

pub(crate) unsafe fn exception_uses_cached_message_str(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    unsafe {
        let class_bits = exception_class_bits(ptr);
        exception_class_bits_uses_cached_message_str(_py, class_bits)
    }
}

pub(crate) unsafe fn exception_class_bits_uses_cached_message_str(
    _py: &PyToken<'_>,
    class_bits: u64,
) -> bool {
    unsafe {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return false;
        };
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return false;
        }
        let class_version = class_layout_version_bits(class_ptr);
        let cached = runtime_state(_py)
            .exception_str_cache
            .lock()
            .unwrap()
            .get(&class_bits)
            .copied();
        match cached {
            Some((version, value)) if version == class_version => value,
            _ => {
                let value = exception_class_can_use_cached_message_str(_py, class_bits, class_ptr);
                runtime_state(_py)
                    .exception_str_cache
                    .lock()
                    .unwrap()
                    .insert(class_bits, (class_version, value));
                value
            }
        }
    }
}

pub(crate) unsafe fn exception_cached_message_str_bits(
    _py: &PyToken<'_>,
    ptr: *mut u8,
) -> Option<u64> {
    unsafe {
        if !exception_uses_cached_message_str(_py, ptr) {
            return None;
        }
        let msg_bits = exception_materialized_message_bits(_py, ptr);
        let msg_ptr = obj_from_bits(msg_bits).as_ptr()?;
        if object_type_id(msg_ptr) != TYPE_ID_STRING {
            return None;
        }
        inc_ref_bits(_py, msg_bits);
        Some(msg_bits)
    }
}

pub(crate) fn format_obj_str(_py: &PyToken<'_>, obj: MoltObject) -> String {
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_TYPE {
                return format_obj(_py, obj);
            }
            if type_id == TYPE_ID_FLOAT {
                let f = crate::object::ops::heap_float_value(ptr);
                return format_float(f);
            }
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                return String::from_utf8_lossy(bytes).into_owned();
            }
            let subclass_str_override = match type_id {
                TYPE_ID_LIST => {
                    try_subclass_str_or_repr_override(_py, ptr, builtin_classes(_py).list)
                }
                TYPE_ID_TUPLE => {
                    try_subclass_str_or_repr_override(_py, ptr, builtin_classes(_py).tuple)
                }
                TYPE_ID_DICT => {
                    try_subclass_str_or_repr_override(_py, ptr, builtin_classes(_py).dict)
                }
                TYPE_ID_SET => {
                    try_subclass_str_or_repr_override(_py, ptr, builtin_classes(_py).set)
                }
                TYPE_ID_FROZENSET => {
                    try_subclass_str_or_repr_override(_py, ptr, builtin_classes(_py).frozenset)
                }
                _ => None,
            };
            if let Some(rendered) = subclass_str_override {
                return rendered;
            }
            if matches!(
                type_id,
                TYPE_ID_LIST | TYPE_ID_TUPLE | TYPE_ID_DICT | TYPE_ID_SET | TYPE_ID_FROZENSET
            ) {
                return format_obj(_py, obj);
            }
            if type_id == TYPE_ID_EXCEPTION {
                if let Some(bits) = exception_cached_message_str_bits(_py, ptr) {
                    let rendered = string_obj_to_owned(obj_from_bits(bits)).unwrap_or_default();
                    dec_ref_bits(_py, bits);
                    return rendered;
                }
                // Check for a custom __str__ on the exception class before
                // falling back to the default format_exception_message path.
                let str_name_exc =
                    intern_static_name(_py, &runtime_state(_py).interned.str_name, b"__str__");
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, str_name_exc) {
                    // If this is NOT a default object/__str__, call the custom one.
                    if !call_bits_is_default_object_str(call_bits)
                        && !call_bits_is_default_object_repr(call_bits)
                    {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(rendered) = string_obj_to_owned(res_obj) {
                            dec_ref_bits(_py, res_bits);
                            return rendered;
                        }
                        dec_ref_bits(_py, res_bits);
                    } else {
                        dec_ref_bits(_py, call_bits);
                    }
                }
                return format_exception_message(_py, ptr);
            }
            let str_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.str_name, b"__str__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, str_name_bits) {
                if call_bits_is_default_object_str(call_bits) {
                    dec_ref_bits(_py, call_bits);
                    // CPython's default object.__str__ delegates to __repr__;
                    // preserve that path so custom __repr__ methods render correctly.
                    return format_obj(_py, obj);
                }
                if call_bits_is_default_object_repr(call_bits) {
                    dec_ref_bits(_py, call_bits);
                    // object.__str__ delegates to repr; use format_obj so custom
                    // __repr__ overrides participate instead of forcing default
                    // pointer-style formatting.
                    return format_obj(_py, obj);
                }
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(rendered) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(_py, res_bits);
                    return rendered;
                }
                dec_ref_bits(_py, res_bits);
            }
            if exception_pending(_py) {
                return "<object>".to_string();
            }
        }
    }
    format_obj(_py, obj)
}

unsafe fn try_subclass_repr_override(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    builtin_class_bits: u64,
) -> Option<String> {
    unsafe {
        let class_bits = object_class_bits(ptr);
        if class_bits == 0 || class_bits == builtin_class_bits {
            return None;
        }
        let repr_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.repr_name, b"__repr__");
        let call_bits = attr_lookup_ptr_allow_missing(_py, ptr, repr_name_bits)?;
        if call_bits_is_default_object_repr(call_bits) {
            dec_ref_bits(_py, call_bits);
            return None;
        }
        let res_bits = call_callable0(_py, call_bits);
        dec_ref_bits(_py, call_bits);
        let rendered = string_obj_to_owned(obj_from_bits(res_bits));
        dec_ref_bits(_py, res_bits);
        rendered
    }
}

unsafe fn try_subclass_str_or_repr_override(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    builtin_class_bits: u64,
) -> Option<String> {
    unsafe {
        let class_bits = object_class_bits(ptr);
        if class_bits == 0 || class_bits == builtin_class_bits {
            return None;
        }
        let class_ptr = obj_from_bits(class_bits).as_ptr()?;
        let base_ptr = obj_from_bits(builtin_class_bits).as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE || object_type_id(base_ptr) != TYPE_ID_TYPE {
            return None;
        }

        let str_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.str_name, b"__str__");
        let raw_str = class_attr_lookup_raw_mro(_py, class_ptr, str_name_bits);
        let base_str = class_attr_lookup_raw_mro(_py, base_ptr, str_name_bits);
        let raw_str_is_default = raw_str.is_some_and(|bits| {
            call_bits_is_default_object_str(bits) || call_bits_is_default_object_repr(bits)
        });
        if raw_str.is_some() && raw_str != base_str && !raw_str_is_default {
            let call_bits = attr_lookup_ptr_allow_missing(_py, ptr, str_name_bits)?;
            if call_bits_is_default_object_str(call_bits)
                || call_bits_is_default_object_repr(call_bits)
            {
                dec_ref_bits(_py, call_bits);
                return None;
            }
            let res_bits = call_callable0(_py, call_bits);
            dec_ref_bits(_py, call_bits);
            let rendered = string_obj_to_owned(obj_from_bits(res_bits));
            dec_ref_bits(_py, res_bits);
            return rendered;
        }

        let repr_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.repr_name, b"__repr__");
        let raw_repr = class_attr_lookup_raw_mro(_py, class_ptr, repr_name_bits);
        let base_repr = class_attr_lookup_raw_mro(_py, base_ptr, repr_name_bits);
        let raw_repr_is_default = raw_repr.is_some_and(|bits| {
            call_bits_is_default_object_repr(bits) || call_bits_is_default_object_str(bits)
        });
        if raw_repr.is_some() && raw_repr != base_repr && !raw_repr_is_default {
            let call_bits = attr_lookup_ptr_allow_missing(_py, ptr, repr_name_bits)?;
            if call_bits_is_default_object_repr(call_bits)
                || call_bits_is_default_object_str(call_bits)
            {
                dec_ref_bits(_py, call_bits);
                return None;
            }
            let res_bits = call_callable0(_py, call_bits);
            dec_ref_bits(_py, call_bits);
            let rendered = string_obj_to_owned(obj_from_bits(res_bits));
            dec_ref_bits(_py, res_bits);
            return rendered;
        }
        None
    }
}

pub(crate) fn format_obj(_py: &PyToken<'_>, obj: MoltObject) -> String {
    if let Some(b) = obj.as_bool() {
        return if b {
            "True".to_string()
        } else {
            "False".to_string()
        };
    }
    if let Some(i) = obj.as_int() {
        return i.to_string();
    }
    // NaN-boxing: raw 0x0 is IEEE 754 +0.0.  Previous code treated it
    // as int 0 because Cranelift zero-inits variables to 0x0, but that
    // broke float parity (e.g. math.sin(0) displayed "0" not "0.0").
    // Proper int 0 is MoltObject::from_int(0) (0x7ff9_0000_0000_0000).
    if let Some(f) = obj.as_float() {
        return format_float(f);
    }
    if obj.is_none() {
        return "None".to_string();
    }
    if obj.is_pending() {
        return "<pending>".to_string();
    }
    if obj.bits() == ellipsis_bits(_py) {
        return "Ellipsis".to_string();
    }
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_FLOAT {
                let f = crate::object::ops::heap_float_value(ptr);
                return format_float(f);
            }
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                return format_string_repr_bytes(bytes);
            }
            if type_id == TYPE_ID_BIGINT {
                return bigint_ref(ptr).to_string();
            }
            if type_id == TYPE_ID_COMPLEX {
                let value = *complex_ref(ptr);
                return format_complex(value.re, value.im);
            }
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return format_bytes(bytes);
            }
            if type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return format!("bytearray({})", format_bytes(bytes));
            }
            if type_id == TYPE_ID_RANGE {
                if let Some((start, stop, step)) = range_components_bigint(ptr) {
                    return format_range(&start, &stop, &step);
                }
                return "range(?)".to_string();
            }
            if type_id == TYPE_ID_SLICE {
                return format_slice(_py, ptr);
            }
            if type_id == TYPE_ID_GENERIC_ALIAS {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "...".to_string();
                }
                return format_generic_alias(_py, ptr);
            }
            if type_id == TYPE_ID_UNION {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "...".to_string();
                }
                return format_union_type(_py, ptr);
            }
            if type_id == TYPE_ID_NOT_IMPLEMENTED {
                return "NotImplemented".to_string();
            }
            if type_id == TYPE_ID_ELLIPSIS {
                return "Ellipsis".to_string();
            }
            if type_id == TYPE_ID_EXCEPTION {
                return format_exception(_py, ptr);
            }
            if type_id == TYPE_ID_CONTEXT_MANAGER {
                return "<context_manager>".to_string();
            }
            if type_id == TYPE_ID_FILE_HANDLE {
                return "<file_handle>".to_string();
            }
            if type_id == TYPE_ID_FUNCTION {
                let name_bits = function_name_bits(_py, ptr);
                let name = if name_bits != 0 {
                    string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_default()
                } else {
                    String::new()
                };
                if object_class_bits(ptr) == builtin_classes(_py).builtin_function_or_method {
                    if name.is_empty() {
                        return "<built-in function>".to_string();
                    }
                    return format!("<built-in function {name}>");
                }
                // Match CPython: <function NAME at 0xADDR>
                if name.is_empty() {
                    return format!("<function at 0x{:x}>", ptr as usize);
                }
                return format!("<function {} at 0x{:x}>", name, ptr as usize);
            }
            if type_id == TYPE_ID_CODE {
                let name =
                    string_obj_to_owned(obj_from_bits(code_name_bits(ptr))).unwrap_or_default();
                if name.is_empty() {
                    return "<code>".to_string();
                }
                return format!("<code {name}>");
            }
            if type_id == TYPE_ID_BOUND_METHOD {
                return "<bound_method>".to_string();
            }
            if type_id == TYPE_ID_GENERATOR {
                return "<generator>".to_string();
            }
            if type_id == TYPE_ID_ASYNC_GENERATOR {
                return "<async_generator>".to_string();
            }
            if type_id == TYPE_ID_MODULE {
                let name =
                    string_obj_to_owned(obj_from_bits(module_name_bits(ptr))).unwrap_or_default();
                if name.is_empty() {
                    return "<module>".to_string();
                }
                if !exception_pending(_py) {
                    let dict_bits = module_dict_bits(ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                        && let Some(file_key) = attr_name_bits_from_bytes(_py, b"__file__")
                        && let Some(file_bits) = dict_get_in_place(_py, dict_ptr, file_key)
                        && let Some(file_name) = string_obj_to_owned(obj_from_bits(file_bits))
                        && !file_name.is_empty()
                    {
                        return format!("<module '{name}' from '{file_name}'>");
                    }
                }
                return format!("<module '{name}'>");
            }
            if type_id == TYPE_ID_TYPE {
                let name =
                    string_obj_to_owned(obj_from_bits(class_name_bits(ptr))).unwrap_or_default();
                if name.is_empty() {
                    return "<type>".to_string();
                }
                let mut qualname = name.clone();
                let mut module_name: Option<String> = None;
                if !exception_pending(_py) {
                    let dict_bits = class_dict_bits(ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                    {
                        if let Some(module_key) = attr_name_bits_from_bytes(_py, b"__module__")
                            && let Some(bits) = dict_get_in_place(_py, dict_ptr, module_key)
                            && let Some(val) = string_obj_to_owned(obj_from_bits(bits))
                        {
                            module_name = Some(val);
                        }
                        if let Some(qual_key) = attr_name_bits_from_bytes(_py, b"__qualname__")
                            && let Some(bits) = dict_get_in_place(_py, dict_ptr, qual_key)
                            && let Some(val) = string_obj_to_owned(obj_from_bits(bits))
                        {
                            qualname = val;
                        }
                    }
                }
                if let Some(module) = module_name
                    && !module.is_empty()
                    && module != "builtins"
                {
                    return format!("<class '{module}.{qualname}'>");
                }
                return format!("<class '{qualname}'>");
            }
            if type_id == TYPE_ID_CLASSMETHOD {
                return "<classmethod>".to_string();
            }
            if type_id == TYPE_ID_STATICMETHOD {
                return "<staticmethod>".to_string();
            }
            if type_id == TYPE_ID_PROPERTY {
                return "<property>".to_string();
            }
            if type_id == TYPE_ID_SUPER {
                return "<super>".to_string();
            }
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() && (*desc_ptr).repr {
                    return format_dataclass(_py, ptr);
                }
            }
            if type_id == TYPE_ID_BUFFER2D {
                let buf_ptr = buffer2d_ptr(ptr);
                if buf_ptr.is_null() {
                    return "<buffer2d>".to_string();
                }
                let buf = &*buf_ptr;
                return format!("<buffer2d {}x{}>", buf.rows, buf.cols);
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if memoryview_released(ptr) {
                    return "<released memoryview>".to_string();
                }
                let len = memoryview_len(ptr);
                let stride = memoryview_stride(ptr);
                let readonly = memoryview_readonly(ptr);
                return format!("<memoryview len={len} stride={stride} readonly={readonly}>");
            }
            if type_id == TYPE_ID_INTARRAY {
                let elems = intarray_slice(ptr);
                let mut out = String::from("intarray([");
                for (idx, val) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&val.to_string());
                }
                out.push_str("])");
                return out;
            }
            if type_id == TYPE_ID_LIST {
                if let Some(rendered) =
                    try_subclass_repr_override(_py, ptr, builtin_classes(_py).list)
                {
                    return rendered;
                }
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "[...]".to_string();
                }
                let elems = seq_vec_ref(ptr);
                let mut out = String::from("[");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(_py, obj_from_bits(*elem)));
                }
                out.push(']');
                return out;
            }
            if type_id == TYPE_ID_LIST_INT {
                // Specialized list[int]: flat i64 storage via ListIntStorage (#[repr(C)]).
                // Format as a regular Python list for display parity.
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "[...]".to_string();
                }
                let storage_ptr = crate::object::layout::list_int_storage_ptr(ptr);
                if !storage_ptr.is_null() {
                    let elems = crate::object::layout::list_int_vec_ref(ptr);
                    let mut out = String::from("[");
                    for (idx, val) in elems.iter().enumerate() {
                        if idx > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(&val.to_string());
                    }
                    out.push(']');
                    return out;
                }
                return "[]".to_string();
            }
            if type_id == TYPE_ID_LIST_BOOL {
                // Specialized list[bool]: flat u8 storage via ListBoolStorage (#[repr(C)]).
                // Format as a regular Python list with True/False for display parity.
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "[...]".to_string();
                }
                let storage_ptr = crate::object::layout::list_bool_storage_ptr(ptr);
                if !storage_ptr.is_null() {
                    let elems = crate::object::layout::list_bool_vec_ref(ptr);
                    let mut out = String::from("[");
                    for (idx, val) in elems.iter().enumerate() {
                        if idx > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(if *val != 0 { "True" } else { "False" });
                    }
                    out.push(']');
                    return out;
                }
                return "[]".to_string();
            }
            if type_id == TYPE_ID_TUPLE {
                if let Some(rendered) =
                    try_subclass_repr_override(_py, ptr, builtin_classes(_py).tuple)
                {
                    return rendered;
                }
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "(...)".to_string();
                }
                let elems = seq_vec_ref(ptr);
                let mut out = String::from("(");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(_py, obj_from_bits(*elem)));
                }
                if elems.len() == 1 {
                    out.push(',');
                }
                out.push(')');
                return out;
            }
            if type_id == TYPE_ID_DICT {
                if let Some(rendered) =
                    try_subclass_repr_override(_py, ptr, builtin_classes(_py).dict)
                {
                    return rendered;
                }
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "{...}".to_string();
                }
                let pairs = dict_order(ptr);
                let mut out = String::from("{");
                let mut idx = 0;
                let mut first = true;
                while idx + 1 < pairs.len() {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_str(&format_obj(_py, obj_from_bits(pairs[idx])));
                    out.push_str(": ");
                    out.push_str(&format_obj(_py, obj_from_bits(pairs[idx + 1])));
                    idx += 2;
                }
                out.push('}');
                return out;
            }
            if type_id == TYPE_ID_SET {
                if let Some(rendered) =
                    try_subclass_repr_override(_py, ptr, builtin_classes(_py).set)
                {
                    return rendered;
                }
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "{...}".to_string();
                }
                let order = set_order(ptr);
                if order.is_empty() {
                    return "set()".to_string();
                }
                let table = set_table(ptr);
                let mut out = String::from("{");
                let mut first = true;
                for &entry in table.iter() {
                    if entry == 0 {
                        continue;
                    }
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    let elem = order[entry - 1];
                    out.push_str(&format_obj(_py, obj_from_bits(elem)));
                }
                out.push('}');
                return out;
            }
            if type_id == TYPE_ID_FROZENSET {
                if let Some(rendered) =
                    try_subclass_repr_override(_py, ptr, builtin_classes(_py).frozenset)
                {
                    return rendered;
                }
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "frozenset({...})".to_string();
                }
                let order = set_order(ptr);
                if order.is_empty() {
                    return "frozenset()".to_string();
                }
                let table = set_table(ptr);
                let mut out = String::from("frozenset({");
                let mut first = true;
                for &entry in table.iter() {
                    if entry == 0 {
                        continue;
                    }
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    let elem = order[entry - 1];
                    out.push_str(&format_obj(_py, obj_from_bits(elem)));
                }
                out.push_str("})");
                return out;
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return if type_id == TYPE_ID_DICT_KEYS_VIEW {
                        "dict_keys(...)".to_string()
                    } else if type_id == TYPE_ID_DICT_VALUES_VIEW {
                        "dict_values(...)".to_string()
                    } else {
                        "dict_items(...)".to_string()
                    };
                }
                let dict_bits = dict_view_dict_bits(ptr);
                let dict_obj = obj_from_bits(dict_bits);
                if let Some(dict_ptr) = dict_obj.as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    let pairs = dict_order(dict_ptr);
                    let mut out = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                        String::from("dict_keys([")
                    } else if type_id == TYPE_ID_DICT_VALUES_VIEW {
                        String::from("dict_values([")
                    } else {
                        String::from("dict_items([")
                    };
                    let mut idx = 0;
                    let mut first = true;
                    while idx + 1 < pairs.len() {
                        if !first {
                            out.push_str(", ");
                        }
                        first = false;
                        if type_id == TYPE_ID_DICT_ITEMS_VIEW {
                            out.push('(');
                            out.push_str(&format_obj(_py, obj_from_bits(pairs[idx])));
                            out.push_str(", ");
                            out.push_str(&format_obj(_py, obj_from_bits(pairs[idx + 1])));
                            out.push(')');
                        } else {
                            let val = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                                pairs[idx]
                            } else {
                                pairs[idx + 1]
                            };
                            out.push_str(&format_obj(_py, obj_from_bits(val)));
                        }
                        idx += 2;
                    }
                    out.push_str("])");
                    return out;
                }
            }
            if type_id == TYPE_ID_ITER {
                return "<iter>".to_string();
            }
            let repr_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.repr_name, b"__repr__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, repr_name_bits) {
                if call_bits_is_default_object_repr(call_bits) {
                    dec_ref_bits(_py, call_bits);
                    return format_default_object_repr(_py, ptr);
                }
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(rendered) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(_py, res_bits);
                    return rendered;
                }
                dec_ref_bits(_py, res_bits);
                return "<object>".to_string();
            }
            if exception_pending(_py) {
                return "<object>".to_string();
            }
        }
    }
    "<object>".to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        FormatSpec, assemble_number, format_obj_str, format_string_repr_bytes, zero_pad_grouped,
    };
    use crate::builtins::attr::attr_name_bits_from_bytes;
    use crate::{
        alloc_module_obj, alloc_string, dict_set_in_place, module_dict_bits, obj_from_bits,
    };
    use molt_obj_model::MoltObject;

    #[test]
    fn module_repr_includes_file_when_present() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let name_ptr = alloc_string(_py, b"pathlib");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let module_ptr = alloc_module_obj(_py, name_bits);
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let dict_bits = unsafe { module_dict_bits(module_ptr) };
            let dict_ptr = obj_from_bits(dict_bits).as_ptr().expect("module dict");
            let file_key = attr_name_bits_from_bytes(_py, b"__file__").expect("__file__ key");
            let file_ptr = alloc_string(_py, b"/tmp/pathlib.py");
            assert!(!file_ptr.is_null());
            let file_bits = MoltObject::from_ptr(file_ptr).bits();
            unsafe { dict_set_in_place(_py, dict_ptr, file_key, file_bits) };

            let rendered = format_obj_str(_py, obj_from_bits(module_bits));
            assert_eq!(rendered, "<module 'pathlib' from '/tmp/pathlib.py'>");
        });
    }

    #[test]
    fn string_repr_uses_generated_unicode_printability_table() {
        assert_eq!(format_string_repr_bytes("\u{00a0}".as_bytes()), "'\\xa0'");
        assert_eq!(format_string_repr_bytes("\u{200b}".as_bytes()), "'\\u200b'");
        assert_eq!(format_string_repr_bytes("\u{e000}".as_bytes()), "'\\ue000'");
        assert_eq!(format_string_repr_bytes("\u{0378}".as_bytes()), "'\\u0378'");
        assert_eq!(
            format_string_repr_bytes("\u{00e9}".as_bytes()),
            "'\u{00e9}'"
        );
    }

    fn num_spec(fill: char, align: Option<char>, width: Option<usize>) -> FormatSpec {
        // Only `fill`, `align`, and `width` influence assemble_number's padding
        // path; the rest are placeholders the helper does not read.
        FormatSpec {
            fill,
            align,
            zero_flag: false,
            sign: None,
            alternate: false,
            width,
            grouping: None,
            precision: None,
            ty: None,
        }
    }

    #[test]
    fn zero_pad_grouped_interleaves_separators_through_fill() {
        // Decimal (group 3): the zero-fill region is itself grouped, so the
        // field can exceed `min_field` exactly as CPython's min_width grouping.
        assert_eq!(zero_pad_grouped("42", 3, ',', 8), "0,000,042");
        assert_eq!(zero_pad_grouped("42", 3, ',', 7), "000,042");
        assert_eq!(zero_pad_grouped("7", 3, ',', 6), "00,007");
        assert_eq!(zero_pad_grouped("7", 3, ',', 7), "000,007");
        assert_eq!(zero_pad_grouped("7", 3, ',', 8), "0,000,007");
        assert_eq!(zero_pad_grouped("1", 3, ',', 9), "0,000,001");
        assert_eq!(zero_pad_grouped("1234567", 3, ',', 15), "000,001,234,567");
        assert_eq!(zero_pad_grouped("0", 3, ',', 5), "0,000");
        // The b/o/x bases group by 4 (PEP 515).
        assert_eq!(zero_pad_grouped("ff", 4, '_', 12), "00_0000_00ff");
        assert_eq!(zero_pad_grouped("777777", 4, '_', 12), "00_0077_7777");
        // A min_field below the natural width adds no zeros — natural grouping.
        assert_eq!(zero_pad_grouped("1234567", 3, ',', 0), "1,234,567");
        assert_eq!(zero_pad_grouped("1234567", 3, ',', 4), "1,234,567");
    }

    #[test]
    fn assemble_number_groups_only_the_zero_fill_flag() {
        let g3 = Some((3usize, ','));
        let g4 = Some((4usize, '_'));
        // The '0' flag (sign-aware '=') groups its zero fill, accounting for the
        // sign/base prefix and the float/percent suffix.
        assert_eq!(
            assemble_number("", "42", "", g3, &num_spec('0', Some('='), Some(8)), '>'),
            "0,000,042"
        );
        assert_eq!(
            assemble_number("-", "42", "", g3, &num_spec('0', Some('='), Some(8)), '>'),
            "-000,042"
        );
        assert_eq!(
            assemble_number(
                "",
                "1234",
                ".50",
                g3,
                &num_spec('0', Some('='), Some(12)),
                '>'
            ),
            "0,001,234.50"
        );
        assert_eq!(
            assemble_number("", "50", "%", g3, &num_spec('0', Some('='), Some(10)), '>'),
            "0,000,050%"
        );
        assert_eq!(
            assemble_number("0x", "ff", "", g4, &num_spec('0', Some('='), Some(12)), '>'),
            "0x0_0000_00ff"
        );
        // Exponential: only the leading integer digit's fill is grouped.
        assert_eq!(
            assemble_number(
                "",
                "1",
                ".500000e+00",
                g3,
                &num_spec('0', Some('='), Some(20)),
                '>'
            ),
            "0,000,001.500000e+00"
        );
        // A non-'=' alignment with a '0' fill char must NOT group the padding.
        assert_eq!(
            assemble_number("", "42", "", g3, &num_spec('0', Some('>'), Some(8)), '>'),
            "00000042"
        );
        // '=' with a non-'0' fill char pads (ungrouped) between prefix and body.
        assert_eq!(
            assemble_number("", "42", "", g3, &num_spec('*', Some('='), Some(8)), '>'),
            "******42"
        );
        // No width: natural grouping only, no padding.
        assert_eq!(
            assemble_number("", "1234567", "", g3, &num_spec('0', Some('='), None), '>'),
            "1,234,567"
        );
        // No grouping requested: ordinary sign-aware zero fill is unchanged.
        assert_eq!(
            assemble_number("-", "42", "", None, &num_spec('0', Some('='), Some(8)), '>'),
            "-0000042"
        );
    }
}

pub(crate) fn format_bytes(bytes: &[u8]) -> String {
    // Match CPython: use double quotes when bytes contain single quote but not double
    let use_double = bytes.contains(&b'\'') && !bytes.contains(&b'"');
    let quote = if use_double { '"' } else { '\'' };
    let mut out = String::from("b");
    out.push(quote);
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            _ if b == quote as u8 => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{:02x}", b)),
        }
    }
    out.push(quote);
    out
}

/// Return a CPython-style type name for format error messages.
fn type_name_for_format_error(obj: MoltObject) -> &'static str {
    if obj.as_int().is_some() {
        "int"
    } else if obj.as_float().is_some() {
        "float"
    } else if obj.as_bool().is_some() {
        "bool"
    } else if obj.is_none() {
        "NoneType"
    } else if let Some(ptr) = obj.as_ptr() {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_BIGINT => "int",
                TYPE_ID_STRING => "str",
                TYPE_ID_BYTES => "bytes",
                TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => "list",
                TYPE_ID_TUPLE => "tuple",
                TYPE_ID_DICT => "dict",
                TYPE_ID_SET => "set",
                TYPE_ID_FROZENSET => "frozenset",
                _ => "object",
            }
        }
    } else {
        "object"
    }
}

pub(crate) fn format_string_repr_bytes(bytes: &[u8]) -> String {
    let use_double = bytes.contains(&b'\'') && !bytes.contains(&b'"');
    let quote = if use_double { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for cp in wtf8_from_bytes(bytes).code_points() {
        let code = cp.to_u32();
        match code {
            0x5C => out.push_str("\\\\"),
            0x0A => out.push_str("\\n"),
            0x0D => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            _ if code == quote as u32 => {
                out.push('\\');
                out.push(quote);
            }
            _ if is_surrogate(code) => {
                out.push_str(&format!("\\u{code:04x}"));
            }
            _ => {
                let ch = char::from_u32(code).unwrap_or('\u{FFFD}');
                if !is_printable_for_repr(ch) {
                    out.push_str(&unicode_escape(ch));
                } else {
                    out.push(ch);
                }
            }
        }
    }
    out.push(quote);
    out
}

/// CPython-compatible printability test for repr escaping.
/// Keep repr() and str.isprintable() on the same generated table authority.
fn is_printable_for_repr(ch: char) -> bool {
    unicode_printable_table::is_printable(ch as u32)
}

#[allow(dead_code)]
fn format_string_repr(s: &str) -> String {
    let use_double = s.contains('\'') && !s.contains('"');
    let quote = if use_double { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if !is_printable_for_repr(c) => {
                let code = c as u32;
                if code <= 0xff {
                    out.push_str(&format!("\\x{:02x}", code));
                } else if code <= 0xffff {
                    out.push_str(&format!("\\u{:04x}", code));
                } else {
                    out.push_str(&format!("\\U{:08x}", code));
                }
            }
            _ => out.push(ch),
        }
    }
    out.push(quote);
    out
}

#[derive(Clone, Copy)]
pub(crate) struct FormatSpec {
    pub(crate) fill: char,
    pub(crate) align: Option<char>,
    pub(crate) zero_flag: bool,
    pub(crate) sign: Option<char>,
    pub(crate) alternate: bool,
    pub(crate) width: Option<usize>,
    pub(crate) grouping: Option<char>,
    pub(crate) precision: Option<usize>,
    pub(crate) ty: Option<char>,
}

pub(crate) type FormatError = (&'static str, Cow<'static, str>);

fn unknown_format_code_error(code: char, obj: MoltObject) -> FormatError {
    (
        "ValueError",
        Cow::Owned(format!(
            "Unknown format code '{code}' for object of type '{}'",
            type_name_for_format_error(obj)
        )),
    )
}

fn unsupported_format_string_error(obj: MoltObject) -> FormatError {
    (
        "TypeError",
        Cow::Owned(format!(
            "unsupported format string passed to {}.__format__",
            type_name_for_format_error(obj)
        )),
    )
}

fn is_integral_format_obj(obj: MoltObject) -> bool {
    obj.as_bool().is_some() || obj.as_int().is_some() || bigint_ptr_from_bits(obj.bits()).is_some()
}

pub(crate) fn parse_format_spec(spec: &str) -> Result<FormatSpec, &'static str> {
    if spec.is_empty() {
        return Ok(FormatSpec {
            fill: ' ',
            align: None,
            zero_flag: false,
            sign: None,
            alternate: false,
            width: None,
            grouping: None,
            precision: None,
            ty: None,
        });
    }
    let mut chars = spec.chars().peekable();
    let mut fill = ' ';
    let mut align = None;
    let mut zero_flag = false;
    let mut sign = None;
    let mut alternate = false;
    let mut grouping = None;
    let mut peeked = chars.clone();
    let first = peeked.next();
    let second = peeked.next();
    if let (Some(c1), Some(c2)) = (first, second) {
        if matches!(c2, '<' | '>' | '^' | '=') {
            fill = c1;
            align = Some(c2);
            chars.next();
            chars.next();
        } else if matches!(c1, '<' | '>' | '^' | '=') {
            align = Some(c1);
            chars.next();
        }
    } else if let Some(c1) = first
        && matches!(c1, '<' | '>' | '^' | '=')
    {
        align = Some(c1);
        chars.next();
    }

    if let Some(ch) = chars.peek().copied()
        && matches!(ch, '+' | '-' | ' ')
    {
        sign = Some(ch);
        chars.next();
    }

    if matches!(chars.peek(), Some('#')) {
        alternate = true;
        chars.next();
    }

    if align.is_none() && matches!(chars.peek(), Some('0')) {
        fill = '0';
        align = Some('=');
        zero_flag = true;
        chars.next();
    }

    let mut width_text = String::new();
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            width_text.push(ch);
            chars.next();
        } else {
            break;
        }
    }
    let width = if width_text.is_empty() {
        None
    } else {
        Some(
            width_text
                .parse::<usize>()
                .map_err(|_| "Invalid format width")?,
        )
    };

    if let Some(ch) = chars.peek().copied()
        && (ch == ',' || ch == '_')
    {
        grouping = Some(ch);
        chars.next();
    }

    let mut precision = None;
    if matches!(chars.peek(), Some('.')) {
        chars.next();
        let mut prec_text = String::new();
        while let Some(ch) = chars.peek().copied() {
            if ch.is_ascii_digit() {
                prec_text.push(ch);
                chars.next();
            } else {
                break;
            }
        }
        if prec_text.is_empty() {
            return Err("Invalid format precision");
        }
        precision = Some(
            prec_text
                .parse::<usize>()
                .map_err(|_| "Invalid format precision")?,
        );
    }

    let remaining: String = chars.collect();
    if remaining.len() > 1 {
        return Err("Invalid format spec");
    }
    let ty = if remaining.is_empty() {
        None
    } else {
        Some(remaining.chars().next().unwrap())
    };

    Ok(FormatSpec {
        fill,
        align,
        zero_flag,
        sign,
        alternate,
        width,
        grouping,
        precision,
        ty,
    })
}

fn apply_grouping(text: &str, group: usize, sep: char) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / group);
    for (count, ch) in text.chars().rev().enumerate() {
        if count > 0 && count.is_multiple_of(group) {
            out.push(sep);
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn apply_alignment(prefix: &str, body: &str, spec: &FormatSpec, default_align: char) -> String {
    let text = format!("{prefix}{body}");
    let width = match spec.width {
        Some(val) => val,
        None => return text,
    };
    let len = text.chars().count();
    if len >= width {
        return text;
    }
    let pad_len = width - len;
    let align = spec.align.unwrap_or(default_align);
    let fill = spec.fill;
    if align == '=' {
        let padding = fill.to_string().repeat(pad_len);
        return format!("{prefix}{padding}{body}");
    }
    let padding = fill.to_string().repeat(pad_len);
    match align {
        '<' => format!("{text}{padding}"),
        '>' => format!("{padding}{text}"),
        '^' => {
            let left = pad_len / 2;
            let right = pad_len - left;
            format!(
                "{}{}{}",
                fill.to_string().repeat(left),
                text,
                fill.to_string().repeat(right)
            )
        }
        _ => text,
    }
}

/// Left-pad `digits` with `'0'` and insert `sep` every `group` digits so the
/// resulting field (digits *and* separators) spans at least `min_field`
/// characters, using the fewest digits that satisfy the bound. This mirrors
/// CPython's `_PyUnicode_InsertThousandsGrouping` driven by the `min_width`
/// that `calc_number_widths` derives for sign-aware `'0'` fill: the zero-fill
/// region is itself grouped, so the field can legitimately exceed `min_field`
/// (a `min_field` of 8 over `42` yields the 9-char `0,000,042`).
fn zero_pad_grouped(digits: &str, group: usize, sep: char, min_field: usize) -> String {
    let cur = digits.chars().count();
    // Total field width for `d` grouped digits: the digits plus one separator
    // for every full group boundary to their left.
    let field_width = |d: usize| d + d.saturating_sub(1) / group;
    let natural = cur.max(1);
    let needed = if field_width(natural) >= min_field {
        natural
    } else {
        // `field_width` is monotonic in `d`; binary-search the minimal digit
        // count whose grouped field reaches `min_field`. `min_field` itself is
        // always a valid upper bound because `field_width(d) >= d`.
        let mut lo = natural;
        let mut hi = min_field.max(natural);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if field_width(mid) >= min_field {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    };
    let mut padded = String::with_capacity(needed);
    for _ in 0..needed.saturating_sub(cur) {
        padded.push('0');
    }
    padded.push_str(digits);
    apply_grouping(&padded, group, sep)
}

/// Assemble a formatted number (integer or float) from its parts, applying
/// digit grouping and field alignment together. Keeping the two coupled is what
/// lets sign-aware `'0'` fill group its zero-fill region the way CPython does;
/// grouping the digits first and padding afterwards (the historical split) drops
/// the separators from the padded zeros.
///
/// * `prefix` — sign and/or base prefix (e.g. `"-"`, `"+0x"`).
/// * `int_digits` — the bare, ungrouped integer digits.
/// * `suffix` — everything trailing the integer digits (fraction including the
///   decimal point, exponent, and/or a `'%'`), or `""`.
/// * `grouping` — `(group_size, separator)` when grouping is requested.
fn assemble_number(
    prefix: &str,
    int_digits: &str,
    suffix: &str,
    grouping: Option<(usize, char)>,
    spec: &FormatSpec,
    default_align: char,
) -> String {
    let align = spec.align.unwrap_or(default_align);
    // The one case where padding interleaves with grouping: the `'0'` fill flag
    // (sign-aware `'='` alignment) combined with an actual grouping separator.
    // Any other alignment, or a non-`'0'` fill char with `'='`, pads with raw
    // fill chars that are *not* grouped — matching CPython.
    if let Some((group, sep)) = grouping
        && let Some(width) = spec.width
        && align == '='
        && spec.fill == '0'
    {
        let non_digit = prefix.chars().count() + suffix.chars().count();
        let field = zero_pad_grouped(int_digits, group, sep, width.saturating_sub(non_digit));
        return format!("{prefix}{field}{suffix}");
    }
    // Otherwise: group at the digits' natural width, then pad as a unit.
    let grouped = match grouping {
        Some((group, sep)) => apply_grouping(int_digits, group, sep),
        None => int_digits.to_string(),
    };
    let body = format!("{grouped}{suffix}");
    apply_alignment(prefix, &body, spec, default_align)
}

fn trim_float_trailing(text: &str, alternate: bool) -> String {
    if alternate {
        return text.to_string();
    }
    let exp_pos = text.find(['e', 'E']).unwrap_or(text.len());
    let (mantissa, exp) = text.split_at(exp_pos);
    let mut end = mantissa.len();
    if let Some(dot) = mantissa.find('.') {
        let bytes = mantissa.as_bytes();
        while end > dot + 1 && bytes[end - 1] == b'0' {
            end -= 1;
        }
        if end == dot + 1 {
            end = dot;
        }
    }
    let trimmed = &mantissa[..end];
    format!("{trimmed}{exp}")
}

fn normalize_exponent(text: &str, upper: bool) -> String {
    let (exp_pos, exp_char) = if let Some(pos) = text.find('e') {
        (pos, 'e')
    } else if let Some(pos) = text.find('E') {
        (pos, 'E')
    } else {
        return text.to_string();
    };
    let (mantissa, exp) = text.split_at(exp_pos);
    let mut exp_text = &exp[1..];
    let mut sign = '+';
    if let Some(first) = exp_text.chars().next()
        && (first == '+' || first == '-')
    {
        sign = first;
        exp_text = &exp_text[1..];
    }
    let digits = if exp_text.is_empty() { "0" } else { exp_text };
    let mut padded = String::from(digits);
    if padded.len() == 1 {
        padded.insert(0, '0');
    }
    let exp_out = if upper { 'E' } else { exp_char };
    format!("{mantissa}{exp_out}{sign}{padded}")
}

fn string_format_spec(spec: &FormatSpec) -> Result<FormatSpec, FormatError> {
    if let Some(sep) = spec.grouping {
        return Err((
            "ValueError",
            Cow::Owned(format!("Cannot specify '{sep}' with 's'.")),
        ));
    }
    if let Some(sign) = spec.sign {
        let msg = if sign == ' ' {
            "Space not allowed in string format specifier"
        } else {
            "Sign not allowed in string format specifier"
        };
        return Err(("ValueError", Cow::Borrowed(msg)));
    }
    if spec.alternate {
        return Err((
            "ValueError",
            Cow::Borrowed("Alternate form (#) not allowed in string format specifier"),
        ));
    }
    if spec.align == Some('=') && !spec.zero_flag {
        return Err((
            "ValueError",
            Cow::Borrowed("'=' alignment not allowed in string format specifier"),
        ));
    }
    let mut normalized = *spec;
    if normalized.zero_flag && normalized.align == Some('=') {
        normalized.align = None;
    }
    Ok(normalized)
}

fn format_string_with_spec(text: String, spec: &FormatSpec) -> Result<String, FormatError> {
    let spec = string_format_spec(spec)?;
    let mut out = text;
    if let Some(prec) = spec.precision {
        out = out.chars().take(prec).collect();
    }
    Ok(apply_alignment("", &out, &spec, '<'))
}

fn apply_char_alignment(text: String, spec: &FormatSpec) -> String {
    apply_alignment("", &text, spec, '>')
}

fn validate_integer_format_spec(ty: char, spec: &FormatSpec) -> Result<(), FormatError> {
    if let Some(sep) = spec.grouping {
        let allowed = match ty {
            'd' | 'n' => true,
            'b' | 'o' | 'x' | 'X' => sep == '_',
            _ => false,
        };
        if !allowed {
            return Err((
                "ValueError",
                Cow::Owned(format!("Cannot specify '{sep}' with '{ty}'.")),
            ));
        }
    }
    if spec.precision.is_some() {
        return Err((
            "ValueError",
            Cow::Borrowed("Precision not allowed in integer format specifier"),
        ));
    }
    if ty == 'c' {
        if spec.sign.is_some() {
            return Err((
                "ValueError",
                Cow::Borrowed("Sign not allowed with integer format specifier 'c'"),
            ));
        }
        if spec.alternate {
            return Err((
                "ValueError",
                Cow::Borrowed("Alternate form (#) not allowed with integer format specifier 'c'"),
            ));
        }
    }
    Ok(())
}

fn is_empty_format_spec(spec: &FormatSpec) -> bool {
    spec.fill == ' '
        && spec.align.is_none()
        && !spec.zero_flag
        && spec.sign.is_none()
        && !spec.alternate
        && spec.width.is_none()
        && spec.grouping.is_none()
        && spec.precision.is_none()
        && spec.ty.is_none()
}

fn format_int_with_spec(obj: MoltObject, spec: &FormatSpec) -> Result<String, FormatError> {
    let ty = spec.ty.unwrap_or('d');
    validate_integer_format_spec(ty, spec)?;
    let mut value = if let Some(i) = obj.as_int() {
        BigInt::from(i)
    } else if let Some(b) = obj.as_bool() {
        BigInt::from(if b { 1 } else { 0 })
    } else if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        unsafe { bigint_ref(ptr).clone() }
    } else {
        return Err(unknown_format_code_error(spec.ty.unwrap_or('d'), obj));
    };
    if ty == 'c' {
        if value.is_negative() {
            return Err((
                "ValueError",
                Cow::Borrowed("format c requires non-negative int"),
            ));
        }
        let code = value
            .to_u32()
            .ok_or(("ValueError", Cow::Borrowed("format c out of range")))?;
        let ch = std::char::from_u32(code)
            .ok_or(("ValueError", Cow::Borrowed("format c out of range")))?;
        return Ok(apply_char_alignment(ch.to_string(), spec));
    }
    let base = match ty {
        'b' => 2,
        'o' => 8,
        'x' | 'X' => 16,
        'd' | 'n' => 10,
        _ => return Err(("ValueError", Cow::Borrowed("unsupported int format type"))),
    };
    let negative = value.is_negative();
    if negative {
        value = -value;
    }
    let mut digits = value.to_str_radix(base);
    if ty == 'X' {
        digits = digits.to_uppercase();
    }
    // Decimal groups by 3; the b/o/x/X bases group by 4 (PEP 515). Grouping is
    // applied by `assemble_number` so it stays coupled to zero-fill padding.
    let grouping = spec.grouping.map(|sep| {
        let group = if base == 10 { 3 } else { 4 };
        (group, sep)
    });
    let mut prefix = String::new();
    if negative {
        prefix.push('-');
    } else if let Some(sign) = spec.sign
        && (sign == '+' || sign == ' ')
    {
        prefix.push(sign);
    }
    if spec.alternate {
        match ty {
            'b' => prefix.push_str("0b"),
            'o' => prefix.push_str("0o"),
            'x' => prefix.push_str("0x"),
            'X' => prefix.push_str("0X"),
            _ => {}
        }
    }
    Ok(assemble_number(&prefix, &digits, "", grouping, spec, '>'))
}

pub(crate) fn format_float_with_spec(
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, FormatError> {
    let val = if let Some(f) = obj.as_float() {
        f
    } else if let Some(i) = obj.as_int() {
        i as f64
    } else if let Some(b) = obj.as_bool() {
        if b { 1.0 } else { 0.0 }
    } else {
        return Err(unknown_format_code_error(spec.ty.unwrap_or('f'), obj));
    };
    let use_default = spec.ty.is_none() && spec.precision.is_none();
    let ty = spec.ty.unwrap_or('g');
    let upper = matches!(ty, 'F' | 'E' | 'G');
    if val.is_nan() {
        let text = if upper { "NAN" } else { "nan" };
        let prefix = if val.is_sign_negative() { "-" } else { "" };
        return Ok(apply_alignment(prefix, text, spec, '>'));
    }
    if val.is_infinite() {
        let text = if upper { "INF" } else { "inf" };
        let prefix = if val.is_sign_negative() { "-" } else { "" };
        return Ok(apply_alignment(prefix, text, spec, '>'));
    }
    let mut prefix = String::new();
    if val.is_sign_negative() {
        prefix.push('-');
    } else if let Some(sign) = spec.sign
        && (sign == '+' || sign == ' ')
    {
        prefix.push(sign);
    }
    let abs_val = val.abs();
    let prec = spec.precision.unwrap_or(6);
    let mut body = if use_default {
        format_float(abs_val)
    } else {
        match ty {
            'f' | 'F' => format!("{:.*}", prec, abs_val),
            'e' | 'E' => format!("{:.*e}", prec, abs_val),
            'g' | 'G' => {
                let digits = if prec == 0 { 1 } else { prec };
                if abs_val == 0.0 {
                    "0".to_string()
                } else {
                    let exp = abs_val.log10().floor() as i32;
                    if exp < -4 || exp >= digits as i32 {
                        let text = format!("{:.*e}", digits - 1, abs_val);
                        trim_float_trailing(&text, spec.alternate)
                    } else {
                        let frac = (digits as i32 - 1 - exp).max(0) as usize;
                        let text = format!("{:.*}", frac, abs_val);
                        trim_float_trailing(&text, spec.alternate)
                    }
                }
            }
            '%' => {
                let scaled = abs_val * 100.0;
                format!("{:.*}", prec, scaled)
            }
            _ => return Err(("ValueError", Cow::Borrowed("unsupported float format type"))),
        }
    };
    body = normalize_exponent(&body, upper);
    if upper {
        body = body.replace('e', "E");
    }
    if spec.alternate && !body.contains('.') && !body.contains('E') && !body.contains('e') {
        body.push('.');
    }
    if ty == '%' {
        body.push('%');
    }
    // Split the magnitude into its leading integer digits and the trailing
    // remainder (fraction including the decimal point, exponent, and/or '%').
    // Grouping — and any sign-aware zero fill — applies only to the integer
    // digits, exactly as CPython's parse_number drives calc_number_widths, so
    // exponential notation groups its zero fill while leaving the mantissa
    // alone. Floats always group by 3.
    let int_len = body
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(body.len());
    let (int_digits, suffix) = body.split_at(int_len);
    let grouping = spec.grouping.map(|sep| (3, sep));
    Ok(assemble_number(
        &prefix, int_digits, suffix, grouping, spec, '>',
    ))
}

fn apply_grouping_to_float_text(text: &str, sep: char) -> String {
    if text.contains('e') || text.contains('E') {
        return text.to_string();
    }
    let mut parts = text.splitn(2, '.');
    let int_part = parts.next().unwrap_or("");
    let frac_part = parts.next();
    let grouped = apply_grouping(int_part, 3, sep);
    if let Some(frac) = frac_part {
        format!("{grouped}.{frac}")
    } else {
        grouped
    }
}

fn format_complex_with_spec(
    _py: &PyToken<'_>,
    value: ComplexParts,
    spec: &FormatSpec,
) -> Result<String, FormatError> {
    let mut ty = spec.ty;
    let mut grouping = spec.grouping;
    if ty == Some('n') {
        if let Some(sep) = grouping {
            let msg = if sep == ',' {
                "Cannot specify ',' with 'n'."
            } else {
                "Cannot specify '_' with 'n'."
            };
            return Err(("ValueError", Cow::Borrowed(msg)));
        }
        ty = Some('g');
        grouping = None;
    }
    if let Some(code) = ty
        && !matches!(code, 'e' | 'E' | 'f' | 'F' | 'g' | 'G')
    {
        let msg = format!("Unknown format code '{code}' for object of type 'complex'");
        return Err(("ValueError", Cow::Owned(msg)));
    }
    if spec.fill == '0' {
        return Err((
            "ValueError",
            Cow::Borrowed("Zero padding is not allowed in complex format specifier"),
        ));
    }
    if spec.align == Some('=') {
        return Err((
            "ValueError",
            Cow::Borrowed("'=' alignment flag is not allowed in complex format specifier"),
        ));
    }
    let re = value.re;
    let im = value.im;
    let re_is_zero = re == 0.0 && !re.is_sign_negative();
    let im_is_negative = im.is_sign_negative();
    let im_sign = if im_is_negative { '-' } else { '+' };
    let use_default = spec.ty.is_none() && spec.precision.is_none();
    let (real_text, imag_text) = if use_default {
        let mut real_text = format_complex_float(re.abs());
        let mut imag_text = format_complex_float(im.abs());
        if let Some(sep) = grouping {
            real_text = apply_grouping_to_float_text(&real_text, sep);
            imag_text = apply_grouping_to_float_text(&imag_text, sep);
        }
        (real_text, imag_text)
    } else {
        let real_spec = FormatSpec {
            fill: spec.fill,
            align: None,
            zero_flag: spec.zero_flag,
            sign: spec.sign,
            alternate: spec.alternate,
            width: None,
            grouping,
            precision: spec.precision,
            ty,
        };
        let imag_spec = FormatSpec {
            fill: spec.fill,
            align: None,
            zero_flag: spec.zero_flag,
            sign: None,
            alternate: spec.alternate,
            width: None,
            grouping,
            precision: spec.precision,
            ty,
        };
        let real_text = format_float_with_spec(MoltObject::from_float(re), &real_spec)?;
        let imag_text = format_float_with_spec(MoltObject::from_float(im.abs()), &imag_spec)?;
        (real_text, imag_text)
    };
    let include_real = ty.is_some() || !re_is_zero;
    let body = if include_real {
        let real_text = if use_default {
            let mut prefix = String::new();
            if re.is_sign_negative() {
                prefix.push('-');
            } else if let Some(sign) = spec.sign
                && (sign == '+' || sign == ' ')
            {
                prefix.push(sign);
            }
            format!("{prefix}{real_text}")
        } else {
            real_text
        };
        let combined = format!("{real_text}{im_sign}{imag_text}j");
        if ty.is_none() {
            format!("({combined})")
        } else {
            combined
        }
    } else {
        let prefix = if im_is_negative {
            "-"
        } else if let Some(sign) = spec.sign {
            if sign == '+' || sign == ' ' {
                if sign == '+' { "+" } else { " " }
            } else {
                ""
            }
        } else {
            ""
        };
        format!("{prefix}{imag_text}j")
    };
    Ok(apply_alignment("", &body, spec, '>'))
}

pub(crate) fn format_with_spec(
    _py: &PyToken<'_>,
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, FormatError> {
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_COMPLEX {
                let value = *complex_ref(ptr);
                return format_complex_with_spec(_py, value, spec);
            }
        }
    }
    let string_text = string_obj_to_owned(obj);
    let is_integral = is_integral_format_obj(obj);
    let is_float = obj.as_float().is_some();
    let is_number = is_integral || is_float;
    if string_text.is_none() && !is_number && !is_empty_format_spec(spec) {
        return Err(unsupported_format_string_error(obj));
    }
    if spec.ty == Some('n') {
        if let Some(sep) = spec.grouping {
            let msg = if sep == ',' {
                "Cannot specify ',' with 'n'."
            } else {
                "Cannot specify '_' with 'n'."
            };
            return Err(("ValueError", Cow::Borrowed(msg)));
        }
        if !is_number {
            if string_text.is_some() {
                return Err(unknown_format_code_error('n', obj));
            }
            return Err(unsupported_format_string_error(obj));
        }
        let mut normalized = FormatSpec {
            fill: spec.fill,
            align: spec.align,
            zero_flag: spec.zero_flag,
            sign: spec.sign,
            alternate: spec.alternate,
            width: spec.width,
            grouping: None,
            precision: spec.precision,
            ty: None,
        };
        if obj.as_float().is_some() {
            normalized.ty = Some('g');
            return format_float_with_spec(obj, &normalized);
        }
        normalized.ty = Some('d');
        return format_int_with_spec(obj, &normalized);
    }
    match spec.ty {
        Some('s') => {
            if let Some(text) = string_text {
                format_string_with_spec(text, spec)
            } else if is_number {
                Err(unknown_format_code_error('s', obj))
            } else {
                Err(unsupported_format_string_error(obj))
            }
        }
        Some('d') | Some('b') | Some('o') | Some('x') | Some('X') | Some('c') => {
            if is_number {
                format_int_with_spec(obj, spec)
            } else if string_text.is_some() {
                Err(unknown_format_code_error(spec.ty.unwrap(), obj))
            } else {
                Err(unsupported_format_string_error(obj))
            }
        }
        Some('f') | Some('F') | Some('e') | Some('E') | Some('g') | Some('G') | Some('%') => {
            if is_number {
                format_float_with_spec(obj, spec)
            } else if string_text.is_some() {
                Err(unknown_format_code_error(spec.ty.unwrap(), obj))
            } else {
                Err(unsupported_format_string_error(obj))
            }
        }
        Some(code) => {
            if string_text.is_some() || is_number {
                Err(unknown_format_code_error(code, obj))
            } else {
                Err(unsupported_format_string_error(obj))
            }
        }
        None => {
            // Check int/bool before float to match CPython's __format__
            // dispatch order.  Also guards against codegen producing raw
            // 0x0 bits (Cranelift zero-init) which NaN-boxing interprets
            // as float +0.0 but semantically represents int 0.
            if obj.as_bool().is_some() {
                if is_empty_format_spec(spec) {
                    Ok(format_obj_str(_py, obj))
                } else {
                    format_int_with_spec(obj, spec)
                }
            } else if obj.as_int().is_some() || bigint_ptr_from_bits(obj.bits()).is_some() {
                format_int_with_spec(obj, spec)
            } else if obj.as_float().is_some() {
                format_float_with_spec(obj, spec)
            } else if let Some(text) = string_text {
                format_string_with_spec(text, spec)
            } else {
                Ok(format_obj_str(_py, obj))
            }
        }
    }
}
