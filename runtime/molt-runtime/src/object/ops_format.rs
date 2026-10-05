//! Formatting, repr, and string conversion — extracted from ops.rs.
#![allow(clippy::items_after_test_module)]

use crate::builtins::attr::lookup_special_method;
use crate::builtins::numbers::{index_bigint_integral_bits, index_integral_payload_bits};
use crate::object::ops::as_float_extended;
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use std::borrow::Cow;

use super::ops::{range_components_bigint, unicode_printable_table};
use super::ops_string::wtf8_from_bytes;

/// A Python renderer either returns the callback's owned string unchanged or
/// builds lossless WTF-8 bytes. Rust String is only a host diagnostic adapter.
pub(crate) enum FormatOutput {
    Bytes(Vec<u8>),
    OwnedString(u64),
}

impl From<String> for FormatOutput {
    fn from(value: String) -> Self {
        Self::Bytes(value.into_bytes())
    }
}

impl From<&str> for FormatOutput {
    fn from(value: &str) -> Self {
        Self::Bytes(value.as_bytes().to_vec())
    }
}

impl From<Vec<u8>> for FormatOutput {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl FormatOutput {
    pub(crate) fn into_bits(self, py: &PyToken<'_>) -> u64 {
        if exception_pending(py) {
            if let Self::OwnedString(bits) = self {
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, bits));
            }
            return MoltObject::none().bits();
        }
        match self {
            Self::OwnedString(bits) => bits,
            Self::Bytes(bytes) => {
                let ptr = alloc_string(py, &bytes);
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
        }
    }

    fn into_bytes(self, py: &PyToken<'_>) -> Vec<u8> {
        if let Self::Bytes(bytes) = self {
            return bytes;
        }
        self.with_bytes(py, |bytes| {
            let mut out = format_buffer(bytes.len())?;
            out.extend_from_slice(bytes);
            Ok(out)
        })
        .unwrap_or_else(|error| {
            error.raise::<()>(py);
            Vec::new()
        })
    }

    /// Keep a renderer's owned string alive while its immutable storage is read.
    /// Cleanup cannot replace a callback/allocation error with a finalizer error.
    pub(crate) fn with_bytes<T>(
        self,
        py: &PyToken<'_>,
        consume: impl FnOnce(&[u8]) -> Result<T, FormatError>,
    ) -> Result<T, FormatError> {
        match self {
            Self::Bytes(bytes) => {
                if exception_pending(py) {
                    Err(FormatError::Pending)
                } else {
                    consume(&bytes)
                }
            }
            Self::OwnedString(bits) => {
                let result = if exception_pending(py) {
                    Err(FormatError::Pending)
                } else {
                    let ptr = obj_from_bits(bits).as_ptr().expect("owned renderer string");
                    let bytes =
                        unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
                    consume(bytes)
                };
                // A failed allocation/writer operation is observable during
                // callback-string finalization. Publish it before releasing
                // that owner, then keep the existing pending-error authority.
                let result = match result {
                    Ok(value) => Ok(value),
                    Err(error) => {
                        error.raise::<()>(py);
                        Err(FormatError::Pending)
                    }
                };
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, bits));
                result
            }
        }
    }

    fn into_host_string(self, py: &PyToken<'_>) -> String {
        String::from_utf8_lossy(&self.into_bytes(py)).into_owned()
    }
}

/// One output owner for percent and brace composition. Only a sole final
/// nonempty callback string may be adopted; earlier fields are copied before
/// later callbacks run, preserving CPython's identity and finalizer order.
pub(crate) struct FormatWriter<'a, 'py> {
    py: &'a PyToken<'py>,
    output: Option<FormatOutput>,
}

impl<'a, 'py> FormatWriter<'a, 'py> {
    pub(crate) fn new(py: &'a PyToken<'py>) -> Self {
        Self {
            py,
            output: Some(FormatOutput::Bytes(Vec::new())),
        }
    }

    pub(crate) fn append_bytes(&mut self, bytes: &[u8]) -> Result<(), FormatError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if matches!(self.output, Some(FormatOutput::OwnedString(_))) {
            let previous = self.output.take().expect("format writer owner");
            let materialized = previous.with_bytes(self.py, |text| {
                let mut buffer = format_buffer(text.len())?;
                buffer.extend_from_slice(text);
                Ok(buffer)
            });
            self.output = Some(FormatOutput::Bytes(materialized?));
        }
        let Some(FormatOutput::Bytes(buffer)) = &mut self.output else {
            unreachable!("format writer byte storage");
        };
        append_format_bytes(buffer, bytes)
    }

    pub(crate) fn append_output(
        &mut self,
        output: FormatOutput,
        final_piece: bool,
    ) -> Result<(), FormatError> {
        let may_adopt = final_piece
            && !exception_pending(self.py)
            && matches!(&self.output, Some(FormatOutput::Bytes(bytes)) if bytes.is_empty())
            && matches!(&output, FormatOutput::OwnedString(bits) if obj_from_bits(*bits)
                .as_ptr().is_some_and(|ptr| unsafe { string_len(ptr) } != 0));
        if may_adopt {
            self.output = Some(output);
            return Ok(());
        }
        output.with_bytes(self.py, |bytes| self.append_bytes(bytes))
    }

    pub(crate) fn append_literal(
        &mut self,
        bytes: &[u8],
        source: Option<u64>,
        final_piece: bool,
    ) -> Result<(), FormatError> {
        if final_piece
            && !bytes.is_empty()
            && let Some(bits) = source
            && obj_from_bits(bits).as_ptr().is_some_and(|ptr| unsafe {
                object_type_id(ptr) == TYPE_ID_STRING
                    && string_bytes(ptr) == bytes.as_ptr()
                    && string_len(ptr) == bytes.len()
            })
        {
            inc_ref_bits(self.py, bits);
            return self.append_output(FormatOutput::OwnedString(bits), true);
        }
        self.append_bytes(bytes)
    }

    pub(crate) fn finish(mut self) -> FormatOutput {
        self.output.take().expect("format writer owner")
    }
}

impl Drop for FormatWriter<'_, '_> {
    fn drop(&mut self) {
        if let Some(FormatOutput::OwnedString(bits)) = self.output.take() {
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(self.py, bits));
        }
    }
}

#[derive(Default)]
struct FormatBuffer(Vec<u8>);

impl From<&str> for FormatBuffer {
    fn from(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
}

impl From<FormatBuffer> for FormatOutput {
    fn from(value: FormatBuffer) -> Self {
        Self::Bytes(value.0)
    }
}

impl FormatBuffer {
    fn new() -> Self {
        Self::default()
    }
    fn push_str(&mut self, value: &str) {
        self.0.extend_from_slice(value.as_bytes());
    }
    fn push_bytes(&mut self, value: &[u8]) {
        self.0.extend_from_slice(value);
    }
    fn push(&mut self, value: char) {
        let mut encoded = [0; 4];
        self.push_str(value.encode_utf8(&mut encoded));
    }
    fn push_output(&mut self, py: &PyToken<'_>, value: FormatOutput) {
        self.0.extend_from_slice(&value.into_bytes(py));
    }
}

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

/// Thin adapter over the single float-format authority
/// (`molt_runtime_core::float_repr::repr_float`), which reproduces CPython's
/// `repr(float)` / `str(float)` bit-for-bit (shortest round-tripping decimal,
/// CPython's fixed-vs-scientific threshold, `e%+.02d` exponent, `.0` for
/// integer-valued floats, `nan`/`inf`/`-inf`/`-0.0`). There is no separate
/// hand-rolled formatting or scientific-notation lane in this module.
fn format_float(f: f64) -> String {
    molt_runtime_core::float_repr::repr_float(f)
}

fn float_is_negative(value: f64) -> bool {
    value.is_sign_negative() && !value.is_nan()
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
    let sign = if float_is_negative(im) { "-" } else { "+" };
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

fn format_slice(_py: &PyToken<'_>, ptr: *mut u8) -> FormatOutput {
    unsafe {
        let mut out = FormatBuffer::from("slice(");
        for (index, value) in [
            slice_start_bits(ptr),
            slice_stop_bits(ptr),
            slice_step_bits(ptr),
        ]
        .into_iter()
        .enumerate()
        {
            if index != 0 {
                out.push_str(", ");
            }
            out.push_output(_py, format_obj_output(_py, obj_from_bits(value)));
            if exception_pending(_py) {
                break;
            }
        }
        out.push(')');
        out.into()
    }
}

/// Look up a string-valued attribute in a namespace with normal dict key
/// equality. `attr_name_bits_from_bytes` returns an owned reference, including
/// when the name is cached, so release it after the borrowed dict lookup.
fn dict_get_attr_string(_py: &PyToken<'_>, dict_ptr: *mut u8, name: &[u8]) -> Option<Vec<u8>> {
    let key_bits = attr_name_bits_from_bytes(_py, name)?;
    let value = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) }
        .and_then(|bits| string_obj_bytes(obj_from_bits(bits)));
    dec_ref_bits(_py, key_bits);
    value
}

fn format_qualified_type_name(_py: &PyToken<'_>, type_ptr: *mut u8) -> Option<Vec<u8>> {
    format_qualified_type_name_in_context(_py, type_ptr, QualifiedTypeNameContext::Repr)
}

#[derive(Clone, Copy, PartialEq)]
enum QualifiedTypeNameContext {
    Repr,
    Diagnostic,
}

fn format_qualified_type_name_in_context(
    _py: &PyToken<'_>,
    type_ptr: *mut u8,
    context: QualifiedTypeNameContext,
) -> Option<Vec<u8>> {
    unsafe {
        let name = string_obj_bytes(obj_from_bits(class_name_bits(type_ptr))).unwrap_or_default();
        let qualname =
            string_obj_bytes(obj_from_bits(class_qualname_bits(type_ptr))).unwrap_or(name);
        let mut module_name: Option<Vec<u8>> = None;
        if !exception_pending(_py) {
            let dict_bits = class_dict_bits(type_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(val) = dict_get_attr_string(_py, dict_ptr, b"__module__") {
                    module_name = Some(val);
                }
            }
        }
        if let Some(module) = module_name
            && (!module.is_empty() || context == QualifiedTypeNameContext::Diagnostic)
            && module != b"builtins"
            && (context == QualifiedTypeNameContext::Repr || module != b"__main__")
        {
            let mut out = module;
            for bytes in [b".".as_slice(), qualname.as_slice()] {
                if let Err(error) = append_format_bytes(&mut out, bytes) {
                    return error.raise(_py);
                }
            }
            return Some(out);
        }
        Some(qualname)
    }
}

/// Python's %T diagnostic spelling uses actual class identity and qualname,
/// suppressing builtins/__main__ while retaining lossless Python name bytes.
pub(crate) fn format_diagnostic_type_name_bytes(py: &PyToken<'_>, obj: MoltObject) -> Vec<u8> {
    let class = type_of_bits(py, obj.bits());
    obj_from_bits(class)
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
        .and_then(|ptr| {
            format_qualified_type_name_in_context(py, ptr, QualifiedTypeNameContext::Diagnostic)
        })
        .unwrap_or_else(|| format_class_name_bytes(class))
}

fn format_generic_alias(_py: &PyToken<'_>, ptr: *mut u8) -> FormatOutput {
    unsafe {
        let origin_bits = generic_alias_origin_bits(ptr);
        let args_bits = generic_alias_args_bits(ptr);
        let origin_obj = obj_from_bits(origin_bits);
        let render_arg = |arg_bits: u64| {
            let arg_obj = obj_from_bits(arg_bits);
            if let Some(arg_ptr) = arg_obj.as_ptr()
                && object_type_id(arg_ptr) == TYPE_ID_TYPE
                && let Some(name) = format_qualified_type_name(_py, arg_ptr)
            {
                return name.into();
            }
            format_obj_output(_py, arg_obj)
        };
        let origin_repr = if let Some(origin_ptr) = origin_obj.as_ptr() {
            if object_type_id(origin_ptr) == TYPE_ID_TYPE {
                format_qualified_type_name(_py, origin_ptr)
                    .map(FormatOutput::from)
                    .unwrap_or_else(|| format_obj_output(_py, origin_obj))
            } else {
                format_obj_output(_py, origin_obj)
            }
        } else {
            format_obj_output(_py, origin_obj)
        };
        let mut out = FormatBuffer::new();
        out.push_output(_py, origin_repr);
        out.push('[');
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr() {
            if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                let _ = crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |elems| {
                    for (idx, elem_bits) in elems.iter().enumerate() {
                        if idx > 0 {
                            out.push_str(", ");
                        }
                        out.push_output(_py, render_arg(*elem_bits));
                        if exception_pending(_py) {
                            break;
                        }
                    }
                });
            } else {
                out.push_output(_py, render_arg(args_bits));
            }
        } else {
            out.push_output(_py, render_arg(args_bits));
        }
        out.push(']');
        out.into()
    }
}

fn format_union_type(_py: &PyToken<'_>, ptr: *mut u8) -> FormatOutput {
    unsafe {
        let args_bits = union_type_args_bits(ptr);
        let render_arg = |arg_bits: u64| {
            let arg_obj = obj_from_bits(arg_bits);
            if let Some(arg_ptr) = arg_obj.as_ptr()
                && object_type_id(arg_ptr) == TYPE_ID_TYPE
                && let Some(name) = format_qualified_type_name(_py, arg_ptr)
            {
                return name.into();
            }
            format_obj_output(_py, arg_obj)
        };
        let mut out = FormatBuffer::new();
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr()
            && object_type_id(args_ptr) == TYPE_ID_TUPLE
        {
            let _ = crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |elems| {
                for (idx, elem_bits) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(" | ");
                    }
                    out.push_output(_py, render_arg(*elem_bits));
                    if exception_pending(_py) {
                        break;
                    }
                }
            });
            return out.into();
        }
        out.push_output(_py, render_arg(args_bits));
        out.into()
    }
}

/// Read Python string storage without allocating or imposing Rust's Unicode
/// scalar restriction. The borrowed bytes cannot escape the reader.
///
/// # Safety
/// The caller must keep the string alive throughout `read`, including any
/// callbacks or reference retirement performed by the reader.
pub(crate) unsafe fn with_string_bytes<R>(
    obj: MoltObject,
    read: impl for<'bytes> FnOnce(&'bytes [u8]) -> R,
) -> Option<R> {
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_STRING {
            return None;
        }
        let len = string_len(ptr);
        let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
        Some(read(bytes))
    }
}

/// Copy Python string storage without imposing Rust's Unicode scalar restriction.
pub(crate) fn string_obj_bytes(obj: MoltObject) -> Option<Vec<u8>> {
    unsafe { with_string_bytes(obj, <[u8]>::to_vec) }
}

/// Host diagnostic adapter. Python string producers use string_obj_bytes or
/// the owned/byte rendering entry points, which retain lone surrogates.
pub(crate) fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    string_obj_bytes(obj).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn format_class_name_bytes(class_bits: u64) -> Vec<u8> {
    if obj_from_bits(class_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_FOREIGN)
    {
        return molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
            crate::object::class_layout::real_class_view(class_bits)
                .ok()
                .flatten()
                .map(|class| (*class).tp_name)
                .filter(|name| !name.is_null())
                .map(|name| std::ffi::CStr::from_ptr(name).to_bytes().to_vec())
                .unwrap_or_else(|| b"<class>".to_vec())
        });
    }
    obj_from_bits(class_bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_TYPE)
        .and_then(|ptr| string_obj_bytes(obj_from_bits(unsafe { class_name_bits(ptr) })))
        .unwrap_or_else(|| class_name_for_error(class_bits).into_bytes())
}

/// Pin collected set inputs through renderer callbacks using the same
/// resource-accounted owner as sequence snapshots.
pub(crate) fn snapshot_format_inputs<'a, 'py>(
    py: &'a PyToken<'py>,
    values: &[u64],
) -> Option<crate::object::seq_access::PinnedSequenceSnapshot<'a, 'py>> {
    let Some(storage) = crate::object::backing::tracked_vec_box_from_slice(values, values.len())
    else {
        return raise_exception::<_>(py, "MemoryError", "format input snapshot allocation failed");
    };
    unsafe {
        for &bits in &*storage {
            inc_ref_bits(py, bits);
        }
        Some(
            crate::object::seq_access::PinnedSequenceSnapshot::from_owned_values(
                py,
                crate::object::backing::tracked_vec_box_from_raw(storage),
            ),
        )
    }
}

pub(crate) fn decode_string_list(obj: MoltObject) -> Option<Vec<String>> {
    let ptr = obj.as_ptr()?;
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return None;
        }
        crate::object::seq_access::with_borrowed(ptr, |elems| {
            let mut out = Vec::with_capacity(elems.len());
            for &elem_bits in elems {
                let elem_obj = obj_from_bits(elem_bits);
                let s = string_obj_to_owned(elem_obj)?;
                out.push(s);
            }
            Some(out)
        })
    }
}

pub(crate) fn decode_value_list(obj: MoltObject) -> Option<Vec<u64>> {
    let ptr = obj.as_ptr()?;
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return None;
        }
        Some(crate::object::seq_access::with_borrowed(
            ptr,
            <[u64]>::to_vec,
        ))
    }
}

fn format_dataclass(_py: &PyToken<'_>, ptr: *mut u8) -> FormatOutput {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(ptr);
        if desc_ptr.is_null() {
            return "<dataclass>".into();
        }
        let desc = &*desc_ptr;
        let mut out = FormatBuffer::new();
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
            // Generated dataclass repr reads each field immediately before
            // its repr callback; a callback may replace a later field.
            let val = crate::object::accessors::object_field_get_ptr_raw(
                _py,
                ptr,
                idx * std::mem::size_of::<u64>(),
            );
            if exception_pending(_py) {
                dec_ref_bits(_py, val);
                return "<dataclass>".into();
            }
            if is_missing_bits(_py, val) {
                let type_label = if desc.name.is_empty() {
                    "dataclass"
                } else {
                    desc.name.as_str()
                };
                dec_ref_bits(_py, val);
                let _ = attr_error(_py, type_label, name);
                return "<dataclass>".into();
            }
            out.push_output(_py, format_obj_output(_py, obj_from_bits(val)));
            dec_ref_bits(_py, val);
            if exception_pending(_py) {
                return "<dataclass>".into();
            }
        }
        out.push(')');
        out.into()
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

fn format_default_object_repr(py: &PyToken<'_>, ptr: *mut u8) -> FormatOutput {
    let class_bits = unsafe {
        if object_type_id(ptr) == TYPE_ID_OBJECT || object_type_id(ptr) == TYPE_ID_DATACLASS {
            object_class_bits(ptr)
        } else {
            type_of_bits(py, MoltObject::from_ptr(ptr).bits())
        }
    };
    let name = obj_from_bits(class_bits)
        .as_ptr()
        .filter(|class| unsafe { object_type_id(*class) } == TYPE_ID_TYPE)
        .and_then(|class| format_qualified_type_name(py, class))
        .unwrap_or_else(|| class_name_for_error(class_bits).into_bytes());
    let mut out = FormatBuffer::from("<");
    out.push_bytes(&name);
    out.push_str(&format!(" object at 0x{:x}>", ptr as usize));
    out.into()
}

/// The declaring object slot must not redispatch __repr__ on its receiver.
/// Generic repr() is a separate entry point; object.__str__ delegates to it.
pub(crate) extern "C" fn object_repr_slot(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let rendered = match obj_from_bits(bits).as_ptr() {
            Some(ptr) => format_default_object_repr(py, ptr),
            None => format!(
                "<{} object at 0x{bits:x}>",
                class_name_for_error(type_of_bits(py, bits))
            )
            .into(),
        };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        rendered.into_bits(py)
    })
}

pub(crate) extern "C" fn object_str_slot(bits: u64) -> u64 {
    molt_repr_from_obj(bits)
}

/// The native renderers whose subclass slots must run before storage fast paths.
fn native_format_base(py: &PyToken<'_>, obj: MoltObject) -> Option<u64> {
    let classes = builtin_classes(py);
    // Scalar base selection follows the shared payload authority, including
    // tagged instances. It never dispatches a conversion protocol.
    if index_integral_payload_bits(obj.bits()).is_some() {
        return Some(classes.int);
    }
    if as_float_extended(obj).is_some() {
        return Some(classes.float);
    }
    let type_id = unsafe { object_type_id(obj.as_ptr()?) };
    Some(match type_id {
        TYPE_ID_STRING => classes.str,
        TYPE_ID_COMPLEX => classes.complex,
        TYPE_ID_BYTES => classes.bytes,
        TYPE_ID_BYTEARRAY => classes.bytearray,
        TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => classes.list,
        TYPE_ID_TUPLE => classes.tuple,
        TYPE_ID_DICT => classes.dict,
        TYPE_ID_SET => classes.set,
        TYPE_ID_FROZENSET => classes.frozenset,
        TYPE_ID_MODULE => classes.module,
        TYPE_ID_TYPE => classes.type_obj,
        _ => return None,
    })
}

unsafe fn string_str_output(py: &PyToken<'_>, ptr: *mut u8) -> FormatOutput {
    unsafe {
        let class = object_class_bits(ptr);
        if class == 0 || class == builtin_classes(py).str {
            let bits = MoltObject::from_ptr(ptr).bits();
            inc_ref_bits(py, bits);
            FormatOutput::OwnedString(bits)
        } else {
            let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
            let result = alloc_string(py, bytes);
            FormatOutput::OwnedString(if result.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(result).bits()
            })
        }
    }
}

/// Explicit str.__str__ uses the declaring slot and never redispatches.
pub(crate) extern "C" fn string_str_slot(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = obj_from_bits(bits)
            .as_ptr()
            .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_STRING)
        else {
            return raise_exception::<_>(
                py,
                "TypeError",
                "descriptor '__str__' requires a 'str' object",
            );
        };
        unsafe { string_str_output(py, ptr) }.into_bits(py)
    })
}
pub(crate) fn format_obj_str(_py: &PyToken<'_>, obj: MoltObject) -> String {
    format_obj_str_output(_py, obj).into_host_string(_py)
}

pub(crate) fn format_obj_str_bits(py: &PyToken<'_>, obj: MoltObject) -> u64 {
    format_obj_str_output(py, obj).into_bits(py)
}

pub(crate) fn format_obj_str_bytes(py: &PyToken<'_>, obj: MoltObject) -> Vec<u8> {
    format_obj_str_output(py, obj).into_bytes(py)
}

pub(crate) fn format_obj_str_output(_py: &PyToken<'_>, obj: MoltObject) -> FormatOutput {
    if exception_pending(_py) {
        return FormatOutput::OwnedString(MoltObject::none().bits());
    }
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
        unsafe {
            let type_id = object_type_id(ptr);
            if let Some(base) = native_format_base(_py, obj) {
                if let Some(rendered) = try_subclass_str_or_repr_override(_py, ptr, base) {
                    return rendered;
                }
                if type_id == TYPE_ID_STRING {
                    return string_str_output(_py, ptr);
                }
                return format_obj_output(_py, obj);
            }
            if let Some(rendered) = try_format_special_method(_py, ptr, "__str__") {
                return rendered;
            }
            if exception_pending(_py) {
                return "<object>".into();
            }
        }
    }
    format_obj_output(_py, obj)
}

/// Consume a bound special method and validate its owned result once. A failed
/// lookup/call keeps its pending exception; callers must not try another slot.
pub(crate) unsafe fn invoke_format_method(
    py: &PyToken<'_>,
    method: u64,
    slot: &str,
    spec: Option<u64>,
) -> u64 {
    unsafe {
        let result = match spec {
            Some(spec) => call_callable1(py, method, spec),
            None => call_callable0(py, method),
        };
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, method));
        if !exception_pending(py)
            && obj_from_bits(result)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_STRING)
        {
            return result;
        }
        if !exception_pending(py) {
            let returned_type = format_class_name_bytes(type_of_bits(py, result));
            let prefix: &[u8] = if slot == "__format__" {
                b"__format__ must return a str, not "
            } else {
                b" returned non-string (type "
            };
            let slot_bytes = if slot == "__format__" {
                b"".as_slice()
            } else {
                slot.as_bytes()
            };
            let suffix = if slot == "__format__" {
                b"".as_slice()
            } else {
                b")".as_slice()
            };
            let mut message = Vec::new();
            for bytes in [slot_bytes, prefix, returned_type.as_slice(), suffix] {
                if let Err(error) = append_format_bytes(&mut message, bytes) {
                    error.raise::<()>(py);
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(py, result)
                    });
                    return MoltObject::none().bits();
                }
            }
            crate::builtins::exceptions::raise_exception_bytes::<()>(py, "TypeError", &message);
        }
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, result));
        MoltObject::none().bits()
    }
}

/// Resolve a formatting slot on the type and preserve binding/call failures.
pub(crate) unsafe fn try_format_special_method_bits(
    py: &PyToken<'_>,
    ptr: *mut u8,
    slot: &str,
) -> Option<u64> {
    unsafe {
        let Some(method) =
            lookup_special_method(py, MoltObject::from_ptr(ptr).bits(), slot.as_bytes())
        else {
            if !exception_pending(py) {
                return None;
            }
            // CPython before 3.14 clears a failed __repr__ descriptor lookup
            // and uses object.__repr__; a failure while calling the bound
            // method still propagates. sys_version_info is the runtime target.
            let propagate_repr_lookup = super::ops_sys::runtime_target_at_least(py, 3, 14);
            if slot == "__repr__" && !propagate_repr_lookup {
                clear_exception(py);
                return Some(object_repr_slot(MoltObject::from_ptr(ptr).bits()));
            }
            return Some(MoltObject::none().bits());
        };
        Some(invoke_format_method(py, method, slot, None))
    }
}

unsafe fn try_format_special_method(
    py: &PyToken<'_>,
    ptr: *mut u8,
    slot: &str,
) -> Option<FormatOutput> {
    unsafe { try_format_special_method_bits(py, ptr, slot) }.map(FormatOutput::OwnedString)
}

unsafe fn try_subclass_repr_override(
    py: &PyToken<'_>,
    ptr: *mut u8,
    builtin_class_bits: u64,
) -> Option<FormatOutput> {
    unsafe {
        let class_bits = object_class_bits(ptr);
        if class_bits == 0 || class_bits == builtin_class_bits {
            return None;
        }
        let class = obj_from_bits(class_bits).as_ptr()?;
        let base = obj_from_bits(builtin_class_bits).as_ptr()?;
        let name = intern_static_name(py, &runtime_state(py).interned.repr_name, b"__repr__");
        if class_attr_lookup_raw_mro(py, class, name) == class_attr_lookup_raw_mro(py, base, name) {
            return None;
        }
        try_format_special_method(py, ptr, "__repr__")
    }
}

/// Structured consumers such as pprint only expand a native container when
/// its effective repr slot still belongs to the native renderer.
pub(crate) fn format_native_repr_override_bytes(
    py: &PyToken<'_>,
    obj: MoltObject,
) -> Option<Vec<u8>> {
    if exception_pending(py) {
        return Some(Vec::new());
    }
    let ptr = obj.as_ptr()?;
    let base = native_format_base(py, obj)?;
    unsafe { try_subclass_repr_override(py, ptr, base) }.map(|value| value.into_bytes(py))
}

/// Native storage does not erase an effective Python __format__ override.
/// Exact builtins and inherited native slots stay on the payload path; every
/// other class uses canonical special lookup, including descriptor failures.
pub(crate) fn format_override(py: &PyToken<'_>, obj: MoltObject, spec: u64) -> Option<u64> {
    let ptr = obj.as_ptr()?;
    unsafe {
        let class_bits = object_class_bits(ptr);
        if class_bits == 0 {
            return None;
        }
        if let Some(base_bits) = native_format_base(py, obj) {
            if class_bits == base_bits {
                return None;
            }
            let class = obj_from_bits(class_bits).as_ptr()?;
            let base = obj_from_bits(base_bits).as_ptr()?;
            let name =
                intern_static_name(py, &runtime_state(py).interned.format_name, b"__format__");
            if class_attr_lookup_raw_mro(py, class, name)
                == class_attr_lookup_raw_mro(py, base, name)
            {
                return None;
            }
        }
        if let Some(method) = lookup_special_method(py, obj.bits(), b"__format__") {
            return Some(invoke_format_method(py, method, "__format__", Some(spec)));
        }
        exception_pending(py).then_some(MoltObject::none().bits())
    }
}

unsafe fn list_repr_contents(py: &PyToken<'_>, ptr: *mut u8) -> FormatOutput {
    let guard = ReprGuard::new(py, ptr);
    if !guard.active() {
        return "[...]".into();
    }
    let mut out = FormatBuffer::from("[");
    let mut index = 0;
    unsafe {
        while index < crate::object::seq_access::locked_len(ptr) {
            let Some(item) = crate::object::seq_access::pin_item(py, ptr, index) else {
                continue;
            };
            if index != 0 {
                out.push_str(", ");
            }
            out.push_output(py, format_obj_output(py, obj_from_bits(item.bits())));
            if exception_pending(py) {
                break;
            }
            index += 1;
        }
    }
    out.push(']');
    out.into()
}

pub(crate) extern "C" fn list_repr_slot(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = crate::object::ops_list::list_receiver(py, bits, "__repr__") else {
            return MoltObject::none().bits();
        };
        unsafe {
            crate::object::ops_list::promote_specialized_list_to_list(py, ptr);
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let rendered = list_repr_contents(py, ptr);
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            rendered.into_bits(py)
        }
    })
}

unsafe fn try_subclass_str_or_repr_override(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    builtin_class_bits: u64,
) -> Option<FormatOutput> {
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
        if raw_str.is_some() && raw_str != base_str {
            return try_format_special_method(_py, ptr, "__str__");
        }

        let object_ptr = obj_from_bits(builtin_classes(_py).object).as_ptr()?;
        if base_str != class_attr_lookup_raw_mro(_py, object_ptr, str_name_bits) {
            // A declaring __str__ slot (not object.__str__) keeps its own
            // semantics when only the subclass's __repr__ changes.
            return None;
        }
        let repr_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.repr_name, b"__repr__");
        let raw_repr = class_attr_lookup_raw_mro(_py, class_ptr, repr_name_bits);
        let base_repr = class_attr_lookup_raw_mro(_py, base_ptr, repr_name_bits);
        if raw_repr.is_some() && raw_repr != base_repr {
            return try_format_special_method(_py, ptr, "__repr__");
        }
        None
    }
}

pub(crate) unsafe fn format_module_default(py: &PyToken<'_>, ptr: *mut u8) -> FormatOutput {
    unsafe {
        let Some(dictionary) = obj_from_bits(module_dict_bits(ptr)).as_ptr() else {
            return "<module '?'>".into();
        };
        let name =
            dict_get_attr_string(py, dictionary, b"__name__").unwrap_or_else(|| b"?".to_vec());
        let mut out = FormatBuffer::from("<module '");
        out.push_bytes(&name);
        out.push('\'');
        if !exception_pending(py)
            && let Some(file) = dict_get_attr_string(py, dictionary, b"__file__")
        {
            out.push_str(" from '");
            out.push_bytes(&file);
            out.push('\'');
        }
        out.push('>');
        out.into()
    }
}

pub(crate) fn format_obj(_py: &PyToken<'_>, obj: MoltObject) -> String {
    format_obj_output(_py, obj).into_host_string(_py)
}

pub(crate) fn format_obj_bits(py: &PyToken<'_>, obj: MoltObject) -> u64 {
    format_obj_output(py, obj).into_bits(py)
}

pub(crate) fn format_obj_bytes(py: &PyToken<'_>, obj: MoltObject) -> Vec<u8> {
    format_obj_output(py, obj).into_bytes(py)
}

pub(crate) fn format_obj_output(_py: &PyToken<'_>, obj: MoltObject) -> FormatOutput {
    if exception_pending(_py) {
        return FormatOutput::OwnedString(MoltObject::none().bits());
    }
    if let Some(b) = obj.as_bool() {
        return if b { "True".into() } else { "False".into() };
    }
    if let Some(i) = obj.as_int() {
        return i.to_string().into();
    }
    // NaN-boxing: raw 0x0 is IEEE 754 +0.0.  Previous code treated it
    // as int 0 because Cranelift zero-inits variables to 0x0, but that
    // broke float parity (e.g. math.sin(0) displayed "0" not "0.0").
    // Proper int 0 is MoltObject::from_int(0) (0x7ff9_0000_0000_0000).
    if let Some(f) = obj.as_float() {
        return format_float(f).into();
    }
    if obj.is_none() {
        return "None".into();
    }
    if obj.is_pending() {
        return "<pending>".into();
    }
    if obj.bits() == ellipsis_bits(_py) {
        return "Ellipsis".into();
    }
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
        unsafe {
            let type_id = object_type_id(ptr);
            if let Some(base) = native_format_base(_py, obj)
                && let Some(rendered) = try_subclass_repr_override(_py, ptr, base)
            {
                return rendered;
            }
            if let Some(value) = as_float_extended(obj) {
                return format_float(value).into();
            }
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                return format_string_repr_bytes(bytes).into();
            }
            if let Some(payload) = index_integral_payload_bits(obj.bits()) {
                let value = obj_from_bits(payload);
                if let Some(value) = value.as_int() {
                    return value.to_string().into();
                }
                if let Some(value) = value.as_bool() {
                    return u8::from(value).to_string().into();
                }
                let ptr = bigint_ptr_from_bits(payload).expect("validated integer payload");
                return bigint_ref(ptr).to_string().into();
            }
            if type_id == TYPE_ID_COMPLEX {
                let value = *complex_ref(ptr);
                return format_complex(value.re, value.im).into();
            }
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return format_bytes(bytes).into();
            }
            if type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return format!("bytearray({})", format_bytes(bytes)).into();
            }
            if type_id == TYPE_ID_RANGE {
                if let Some((start, stop, step)) = range_components_bigint(ptr) {
                    return format_range(&start, &stop, &step).into();
                }
                return "range(?)".into();
            }
            if type_id == TYPE_ID_SLICE {
                return format_slice(_py, ptr);
            }
            if type_id == TYPE_ID_GENERIC_ALIAS {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "...".into();
                }
                return format_generic_alias(_py, ptr);
            }
            if type_id == TYPE_ID_UNION {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "...".into();
                }
                return format_union_type(_py, ptr);
            }
            if type_id == TYPE_ID_NOT_IMPLEMENTED {
                return "NotImplemented".into();
            }
            if type_id == TYPE_ID_ELLIPSIS {
                return "Ellipsis".into();
            }
            if type_id == TYPE_ID_EXCEPTION {
                return try_format_special_method(_py, ptr, "__repr__")
                    .unwrap_or_else(|| format_default_object_repr(_py, ptr));
            }
            if type_id == TYPE_ID_CONTEXT_MANAGER {
                return "<context_manager>".into();
            }
            if type_id == TYPE_ID_FILE_HANDLE {
                return "<file_handle>".into();
            }
            if type_id == TYPE_ID_FUNCTION {
                if let Some(text) =
                    crate::builtins::functions::native_callable::native_callable_repr(_py, ptr)
                {
                    return text.into();
                }
                let name_bits = function_name_bits(_py, ptr);
                let name = if name_bits != 0 {
                    string_obj_bytes(obj_from_bits(name_bits)).unwrap_or_default()
                } else {
                    Vec::new()
                };

                // Match CPython: <function NAME at 0xADDR>
                if name.is_empty() {
                    return format!("<function at 0x{:x}>", ptr as usize).into();
                }
                let mut out = FormatBuffer::from("<function ");
                out.push_bytes(&name);
                out.push_str(&format!(" at 0x{:x}>", ptr as usize));
                return out.into();
            }
            if type_id == TYPE_ID_CODE {
                let name = string_obj_bytes(obj_from_bits(code_name_bits(ptr))).unwrap_or_default();
                if name.is_empty() {
                    return "<code>".into();
                }
                let mut out = FormatBuffer::from("<code ");
                out.push_bytes(&name);
                out.push('>');
                return out.into();
            }
            if type_id == TYPE_ID_BOUND_METHOD {
                if let Some(text) =
                    crate::builtins::functions::native_callable::native_callable_repr(_py, ptr)
                {
                    return text.into();
                }
                return "<bound_method>".into();
            }
            if type_id == TYPE_ID_GENERATOR {
                return "<generator>".into();
            }
            if type_id == TYPE_ID_ASYNC_GENERATOR {
                return "<async_generator>".into();
            }
            if type_id == TYPE_ID_MODULE {
                return format_module_default(_py, ptr);
            }
            if type_id == TYPE_ID_TYPE {
                let Some(name) = format_qualified_type_name(_py, ptr) else {
                    return "<type>".into();
                };
                let mut out = FormatBuffer::from("<class '");
                out.push_bytes(&name);
                out.push_str("'>");
                return out.into();
            }
            if type_id == crate::TYPE_ID_NATIVE_DESCRIPTOR {
                let result = crate::builtins::types::native_descriptor_repr(
                    _py,
                    MoltObject::from_ptr(ptr).bits(),
                );
                return FormatOutput::OwnedString(result);
            }
            if type_id == TYPE_ID_CLASSMETHOD {
                return "<classmethod>".into();
            }
            if type_id == TYPE_ID_STATICMETHOD {
                return "<staticmethod>".into();
            }
            if type_id == TYPE_ID_PROPERTY {
                return "<property>".into();
            }
            if type_id == TYPE_ID_SUPER {
                let owner = format_class_name_bytes(super_type_bits(ptr));
                let receiver_class = super::layout::super_receiver_class_bits(ptr);
                let mut out = FormatBuffer::from("<super: <class '");
                out.push_bytes(&owner);
                out.push_str("'>, ");
                if obj_from_bits(receiver_class).is_none() {
                    out.push_str("NULL>");
                    return out.into();
                }
                out.push('<');
                out.push_bytes(&format_class_name_bytes(receiver_class));
                out.push_str(" object>>");
                return out.into();
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
                    return "<buffer2d>".into();
                }
                let buf = &*buf_ptr;
                return format!("<buffer2d {}x{}>", buf.rows, buf.cols).into();
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if memoryview_released(ptr) {
                    return "<released memoryview>".into();
                }
                let len = memoryview_len(ptr);
                let stride = memoryview_stride(ptr);
                let readonly = memoryview_readonly(ptr);
                return format!("<memoryview len={len} stride={stride} readonly={readonly}>")
                    .into();
            }
            if type_id == TYPE_ID_LIST {
                return list_repr_contents(_py, ptr);
            }
            if type_id == TYPE_ID_LIST_INT {
                // Specialized list[int]: flat i64 storage via ListIntStorage (#[repr(C)]).
                // Format as a regular Python list for display parity.
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "[...]".into();
                }
                let storage_ptr = crate::object::layout::list_int_storage_ptr(ptr);
                if !storage_ptr.is_null() {
                    let elems = crate::object::layout::list_int_vec_ref(ptr);
                    let mut out = FormatBuffer::from("[");
                    for (idx, val) in elems.iter().enumerate() {
                        if idx > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(&val.to_string());
                    }
                    out.push(']');
                    return out.into();
                }
                return "[]".into();
            }
            if type_id == TYPE_ID_LIST_BOOL {
                // Specialized list[bool]: flat u8 storage via ListBoolStorage (#[repr(C)]).
                // Format as a regular Python list with True/False for display parity.
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "[...]".into();
                }
                let storage_ptr = crate::object::layout::list_bool_storage_ptr(ptr);
                if !storage_ptr.is_null() {
                    let elems = crate::object::layout::list_bool_vec_ref(ptr);
                    let mut out = FormatBuffer::from("[");
                    for (idx, val) in elems.iter().enumerate() {
                        if idx > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(if *val != 0 { "True" } else { "False" });
                    }
                    out.push(']');
                    return out.into();
                }
                return "[]".into();
            }
            if type_id == TYPE_ID_TUPLE {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "(...)".into();
                }
                let mut out = FormatBuffer::from("(");
                let tuple_len =
                    crate::object::seq_access::with_immutable_tuple_slice(ptr, |elems| {
                        for (idx, elem) in elems.iter().enumerate() {
                            if idx > 0 {
                                out.push_str(", ");
                            }
                            out.push_output(_py, format_obj_output(_py, obj_from_bits(*elem)));
                            if exception_pending(_py) {
                                break;
                            }
                        }
                        elems.len()
                    })
                    .unwrap_or(0);
                if tuple_len == 1 {
                    out.push(',');
                }
                out.push(')');
                return out.into();
            }
            if type_id == TYPE_ID_DICT {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "{...}".into();
                }
                let mut out = FormatBuffer::from("{");
                let mut idx = 0;
                let mut first = true;
                loop {
                    let Some((key, value)) = ({
                        let pairs = dict_order(ptr);
                        pairs.get(idx + 1).map(|value| (pairs[idx], *value))
                    }) else {
                        break;
                    };
                    inc_ref_bits(_py, key);
                    inc_ref_bits(_py, value);
                    let _key_owner = PtrDropGuard::new(
                        obj_from_bits(key).as_ptr().unwrap_or(std::ptr::null_mut()),
                    );
                    let _value_owner = PtrDropGuard::new(
                        obj_from_bits(value)
                            .as_ptr()
                            .unwrap_or(std::ptr::null_mut()),
                    );
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_output(_py, format_obj_output(_py, obj_from_bits(key)));
                    if exception_pending(_py) {
                        break;
                    }
                    out.push_str(": ");
                    out.push_output(_py, format_obj_output(_py, obj_from_bits(value)));
                    if exception_pending(_py) {
                        break;
                    }
                    idx += 2;
                }
                out.push('}');
                return out.into();
            }
            if type_id == TYPE_ID_SET {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "{...}".into();
                }
                let order = set_order(ptr);
                if order.is_empty() {
                    return "set()".into();
                }
                let values: Vec<u64> = set_table(ptr)
                    .iter()
                    .copied()
                    .filter(|entry| *entry != 0)
                    .map(|entry| order[entry - 1])
                    .collect();
                let Some(values) = snapshot_format_inputs(_py, &values) else {
                    return Vec::new().into();
                };
                let mut out = FormatBuffer::from("{");
                let mut first = true;
                for &elem in values.iter() {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_output(_py, format_obj_output(_py, obj_from_bits(elem)));
                    if exception_pending(_py) {
                        break;
                    }
                }
                out.push('}');
                return out.into();
            }
            if type_id == TYPE_ID_FROZENSET {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "frozenset({...})".into();
                }
                let order = set_order(ptr);
                if order.is_empty() {
                    return "frozenset()".into();
                }
                let values: Vec<u64> = set_table(ptr)
                    .iter()
                    .copied()
                    .filter(|entry| *entry != 0)
                    .map(|entry| order[entry - 1])
                    .collect();
                let Some(values) = snapshot_format_inputs(_py, &values) else {
                    return Vec::new().into();
                };
                let mut out = FormatBuffer::from("frozenset({");
                let mut first = true;
                for &elem in values.iter() {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_output(_py, format_obj_output(_py, obj_from_bits(elem)));
                    if exception_pending(_py) {
                        break;
                    }
                }
                out.push_str("})");
                return out.into();
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return if type_id == TYPE_ID_DICT_KEYS_VIEW {
                        "dict_keys(...)".into()
                    } else if type_id == TYPE_ID_DICT_VALUES_VIEW {
                        "dict_values(...)".into()
                    } else {
                        "dict_items(...)".into()
                    };
                }
                let dict_bits = dict_view_dict_bits(ptr);
                let dict_obj = obj_from_bits(dict_bits);
                if let Some(dict_ptr) = dict_obj.as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    let Some(pairs) = super::ops_dict::dict_snapshot(
                        _py,
                        dict_ptr,
                        super::ops_dict::DictSnapshotKind::Entries,
                    ) else {
                        return Vec::new().into();
                    };
                    let mut out = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                        FormatBuffer::from("dict_keys([")
                    } else if type_id == TYPE_ID_DICT_VALUES_VIEW {
                        FormatBuffer::from("dict_values([")
                    } else {
                        FormatBuffer::from("dict_items([")
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
                            out.push_output(_py, format_obj_output(_py, obj_from_bits(pairs[idx])));
                            if exception_pending(_py) {
                                break;
                            }
                            out.push_str(", ");
                            out.push_output(
                                _py,
                                format_obj_output(_py, obj_from_bits(pairs[idx + 1])),
                            );
                            if exception_pending(_py) {
                                break;
                            }
                            out.push(')');
                        } else {
                            let val = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                                pairs[idx]
                            } else {
                                pairs[idx + 1]
                            };
                            out.push_output(_py, format_obj_output(_py, obj_from_bits(val)));
                            if exception_pending(_py) {
                                break;
                            }
                        }
                        idx += 2;
                    }
                    out.push_str("])");
                    return out.into();
                }
            }
            if type_id == TYPE_ID_ITER {
                return "<iter>".into();
            }
            if let Some(rendered) = try_format_special_method(_py, ptr, "__repr__") {
                return rendered;
            }
            return format_default_object_repr(_py, ptr);
        }
    }
    "<object>".into()
}

#[cfg(test)]
mod tests {
    use super::{
        FormatSpec, assemble_number, format_default_object_repr, format_obj_str,
        format_string_repr_bytes, zero_pad_grouped,
    };
    use crate::builtins::attr::attr_name_bits_from_bytes;
    use crate::{
        alloc_dict_with_pairs, alloc_module_obj, alloc_string, alloc_tuple, dict_set_in_place,
        module_dict_bits, obj_from_bits,
    };
    use molt_obj_model::MoltObject;

    fn refcount(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
        unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn nonfinite_float_presentations_share_sign_padding_and_percent_suffix() {
        let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil(|py| {
            let py = &py;
            let cases = [
                (f64::INFINITY, "+g", "+inf"),
                (f64::INFINITY, " g", " inf"),
                (f64::INFINITY, "+08G", "+0000INF"),
                (f64::INFINITY, "+08%", "+000inf%"),
                (f64::NEG_INFINITY, "+08%", "-000inf%"),
                (f64::NAN, "+08G", "+0000NAN"),
                (-f64::NAN, "+08%", "+000nan%"),
                (1.5, "<08", "1.500000"),
                (-1.5, "=08", "-00001.5"),
                (f64::INFINITY, ">06", "000inf"),
                (-0.0, "z", "0.0"),
                (-0.04, "z.1f", "0.0"),
                (-0.06, "+z.1f", "-0.1"),
                (-0.0004, "z.1%", "0.0%"),
                (10.0, ".3", "10.0"),
                (100.0, ".3", "1e+02"),
                (0.0, ".1", "0e+00"),
                (999999.7, "g", "1e+06"),
                (0.0000999999999, "g", "0.0001"),
                (999999.7, "#g", "1.00000e+06"),
                (0.0, "#g", "0.00000"),
            ];
            for (value, spec, expected) in cases {
                let spec = super::parse_format_spec(spec.as_bytes(), false).unwrap();
                let value = MoltObject::from_float(value);
                let text = super::format_float_with_spec(py, value, &spec)
                    .unwrap_or_else(|_| panic!("nonfinite formatting failed"));
                assert_eq!(text, expected.as_bytes());
                assert!(!crate::exception_pending(py));
            }
            for (re, im, spec, expected) in [
                (1.0, -f64::NAN, "", "(1+nanj)"),
                (-f64::NAN, 1.0, "+", "(+nan+1j)"),
                (-0.0, -0.0, "z", "(0+0j)"),
                (-0.04, -0.04, "+z.1f", "+0.0+0.0j"),
                (10.0, 100.0, ".3", "(10+100j)"),
                (1.0, 2.0, "x>08", "xx(1+2j)"),
            ] {
                let spec = super::parse_format_spec(spec.as_bytes(), false).unwrap();
                let bits = crate::molt_complex_from_obj(
                    MoltObject::from_float(re).bits(),
                    MoltObject::from_float(im).bits(),
                    MoltObject::from_bool(true).bits(),
                );
                let text = super::format_with_spec(py, obj_from_bits(bits), &spec)
                    .unwrap_or_else(|_| panic!("complex formatting failed"))
                    .into_bytes(py);
                crate::dec_ref_bits(py, bits);
                assert_eq!(text, expected.as_bytes());
                assert!(!crate::exception_pending(py));
            }
        });
    }

    #[test]
    fn module_repr_includes_file_when_present() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
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

            let key_refs = refcount(file_key);
            for _ in 0..16 {
                let rendered = format_obj_str(_py, obj_from_bits(module_bits));
                assert_eq!(rendered, "<module 'pathlib' from '/tmp/pathlib.py'>");
                assert_eq!(refcount(file_key), key_refs);
            }
            for bits in [module_bits, name_bits, file_key, file_bits] {
                crate::dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    fn type_alias_and_object_repr_release_namespace_lookup_keys() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let _ = crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let name_ptr = alloc_string(_py, b"C");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!dict_ptr.is_null());
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let module_key = attr_name_bits_from_bytes(_py, b"__module__").unwrap();
            let qual_key = attr_name_bits_from_bytes(_py, b"__qualname__").unwrap();
            let module_ptr = alloc_string(_py, b"pkg");
            let qual_ptr = alloc_string(_py, b"Outer.C");
            assert!(!module_ptr.is_null() && !qual_ptr.is_null());
            unsafe {
                dict_set_in_place(
                    _py,
                    dict_ptr,
                    module_key,
                    MoltObject::from_ptr(module_ptr).bits(),
                );
                dict_set_in_place(
                    _py,
                    dict_ptr,
                    qual_key,
                    MoltObject::from_ptr(qual_ptr).bits(),
                );
            }
            let class_bits = crate::builtins::types::molt_type_new(
                crate::builtin_classes(_py).type_obj,
                name_bits,
                MoltObject::none().bits(),
                dict_bits,
                MoltObject::none().bits(),
            );
            let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class");
            assert!(!crate::exception_pending(_py));
            let instance_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
            let instance_ptr = obj_from_bits(instance_bits)
                .as_ptr()
                .expect("class instance");
            let args_ptr = alloc_tuple(_py, &[class_bits]);
            assert!(!args_ptr.is_null());
            let alias_ptr =
                crate::alloc_generic_alias(_py, class_bits, MoltObject::from_ptr(args_ptr).bits());
            assert!(!alias_ptr.is_null());

            let module_refs = refcount(module_key);
            let qual_refs = refcount(qual_key);
            for _ in 0..16 {
                assert_eq!(
                    format_obj_str(_py, obj_from_bits(class_bits)),
                    "<class 'pkg.Outer.C'>"
                );
                assert_eq!(
                    format_obj_str(_py, MoltObject::from_ptr(alias_ptr)),
                    "pkg.Outer.C[pkg.Outer.C]"
                );
                assert!(
                    format_default_object_repr(_py, instance_ptr)
                        .into_host_string(_py)
                        .starts_with("<pkg.Outer.C object at 0x")
                );
                assert_eq!(refcount(module_key), module_refs);
                assert_eq!(refcount(qual_key), qual_refs);
            }
            for bits in [
                MoltObject::from_ptr(alias_ptr).bits(),
                MoltObject::from_ptr(args_ptr).bits(),
                instance_bits,
                class_bits,
                name_bits,
                dict_bits,
                module_key,
                qual_key,
                MoltObject::from_ptr(module_ptr).bits(),
                MoltObject::from_ptr(qual_ptr).bits(),
            ] {
                crate::dec_ref_bits(_py, bits);
            }
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
            fill: u32::from(fill),
            align,
            zero_flag: false,
            sign: None,
            coerce_negative_zero: false,
            alternate: false,
            width,
            grouping: None,
            fractional_grouping: None,
            precision: None,
            ty: None,
        }
    }

    #[test]
    fn zero_pad_grouped_interleaves_separators_through_fill() {
        // Decimal (group 3): the zero-fill region is itself grouped, so the
        // field can exceed `min_field` exactly as CPython's min_width grouping.
        assert_eq!(zero_pad_grouped("42", 3, ',', 8).unwrap(), "0,000,042");
        assert_eq!(zero_pad_grouped("42", 3, ',', 7).unwrap(), "000,042");
        assert_eq!(zero_pad_grouped("7", 3, ',', 6).unwrap(), "00,007");
        assert_eq!(zero_pad_grouped("7", 3, ',', 7).unwrap(), "000,007");
        assert_eq!(zero_pad_grouped("7", 3, ',', 8).unwrap(), "0,000,007");
        assert_eq!(zero_pad_grouped("1", 3, ',', 9).unwrap(), "0,000,001");
        assert_eq!(
            zero_pad_grouped("1234567", 3, ',', 15).unwrap(),
            "000,001,234,567"
        );
        assert_eq!(zero_pad_grouped("0", 3, ',', 5).unwrap(), "0,000");
        // The b/o/x bases group by 4 (PEP 515).
        assert_eq!(zero_pad_grouped("ff", 4, '_', 12).unwrap(), "00_0000_00ff");
        assert_eq!(
            zero_pad_grouped("777777", 4, '_', 12).unwrap(),
            "00_0077_7777"
        );
        // A min_field below the natural width adds no zeros — natural grouping.
        assert_eq!(zero_pad_grouped("1234567", 3, ',', 0).unwrap(), "1,234,567");
        assert_eq!(zero_pad_grouped("1234567", 3, ',', 4).unwrap(), "1,234,567");
    }

    #[test]
    fn assemble_number_groups_only_the_zero_fill_flag() {
        let g3 = Some((3usize, ','));
        let g4 = Some((4usize, '_'));
        // The '0' flag (sign-aware '=') groups its zero fill, accounting for the
        // sign/base prefix and the float/percent suffix.
        assert_eq!(
            assemble_number("", "42", "", g3, &num_spec('0', Some('='), Some(8)), '>').unwrap(),
            b"0,000,042"
        );
        assert_eq!(
            assemble_number("-", "42", "", g3, &num_spec('0', Some('='), Some(8)), '>').unwrap(),
            b"-000,042"
        );
        assert_eq!(
            assemble_number(
                "",
                "1234",
                ".50",
                g3,
                &num_spec('0', Some('='), Some(12)),
                '>'
            )
            .unwrap(),
            b"0,001,234.50"
        );
        assert_eq!(
            assemble_number("", "50", "%", g3, &num_spec('0', Some('='), Some(10)), '>').unwrap(),
            b"0,000,050%"
        );
        assert_eq!(
            assemble_number("0x", "ff", "", g4, &num_spec('0', Some('='), Some(12)), '>').unwrap(),
            b"0x0_0000_00ff"
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
            )
            .unwrap(),
            b"0,000,001.500000e+00"
        );
        // A non-'=' alignment with a '0' fill char must NOT group the padding.
        assert_eq!(
            assemble_number("", "42", "", g3, &num_spec('0', Some('>'), Some(8)), '>').unwrap(),
            b"00000042"
        );
        // '=' with a non-'0' fill char pads (ungrouped) between prefix and body.
        assert_eq!(
            assemble_number("", "42", "", g3, &num_spec('*', Some('='), Some(8)), '>').unwrap(),
            b"******42"
        );
        // No width: natural grouping only, no padding.
        assert_eq!(
            assemble_number("", "1234567", "", g3, &num_spec('0', Some('='), None), '>').unwrap(),
            b"1,234,567"
        );
        // No grouping requested: ordinary sign-aware zero fill is unchanged.
        assert_eq!(
            assemble_number("-", "42", "", None, &num_spec('0', Some('='), Some(8)), '>').unwrap(),
            b"-0000042"
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
    pub(crate) fill: u32,
    pub(crate) align: Option<char>,
    pub(crate) zero_flag: bool,
    pub(crate) sign: Option<char>,
    pub(crate) coerce_negative_zero: bool,
    pub(crate) alternate: bool,
    pub(crate) width: Option<usize>,
    pub(crate) grouping: Option<char>,
    pub(crate) fractional_grouping: Option<char>,
    pub(crate) precision: Option<usize>,
    pub(crate) ty: Option<u32>,
}

impl FormatSpec {
    fn presentation(&self) -> Option<char> {
        self.ty.and_then(char::from_u32)
    }
}

impl Default for FormatSpec {
    fn default() -> Self {
        Self {
            fill: u32::from(' '),
            align: None,
            zero_flag: false,
            sign: None,
            coerce_negative_zero: false,
            alternate: false,
            width: None,
            grouping: None,
            fractional_grouping: None,
            precision: None,
            ty: None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum FormatError {
    Diagnostic(&'static str, Cow<'static, str>),
    Bytes(&'static str, Vec<u8>),
    /// A callback already established the exact runtime exception.
    Pending,
}

impl FormatError {
    pub(crate) fn raise<T: crate::builtins::exceptions::ExceptionSentinel>(
        self,
        py: &PyToken<'_>,
    ) -> T {
        match self {
            Self::Diagnostic(kind, message) => raise_exception(py, kind, message.as_ref()),
            Self::Bytes(kind, message) => {
                crate::builtins::exceptions::raise_exception_bytes(py, kind, &message)
            }
            Self::Pending => {
                debug_assert!(exception_pending(py));
                T::exception_sentinel()
            }
        }
    }
}

fn unknown_format_code_error(
    _py: &PyToken<'_>,
    code: impl Into<u32>,
    obj: MoltObject,
) -> FormatError {
    let code = code.into();
    let display = format_code_display(code);
    FormatError::Diagnostic(
        "ValueError",
        Cow::Owned(format!(
            "Unknown format code '{display}' for object of type '{}'",
            type_name(_py, obj)
        )),
    )
}

fn format_code_display(code: u32) -> String {
    if (33..=127).contains(&code) {
        char::from_u32(code).unwrap().to_string()
    } else {
        format!("\\x{code:x}")
    }
}

fn unsupported_format_string_error(_py: &PyToken<'_>, obj: MoltObject) -> FormatError {
    FormatError::Diagnostic(
        "TypeError",
        Cow::Owned(format!(
            "unsupported format string passed to {}.__format__",
            type_name(_py, obj)
        )),
    )
}

#[derive(Debug)]
pub(crate) enum FormatParseError {
    InvalidSpecifier,
    Diagnostic(&'static str),
}

impl FormatParseError {
    pub(crate) fn raise<T: crate::builtins::exceptions::ExceptionSentinel>(
        self,
        py: &PyToken<'_>,
        obj: MoltObject,
        spec: &[u8],
    ) -> T {
        match self {
            Self::Diagnostic(message) => raise_exception(py, "ValueError", message),
            Self::InvalidSpecifier => {
                let mut message = b"Invalid format specifier '".to_vec();
                message.extend_from_slice(spec);
                message.extend_from_slice(
                    format!("' for object of type '{}'", type_name(py, obj)).as_bytes(),
                );
                FormatError::Bytes("ValueError", message).raise(py)
            }
        }
    }
}

/// Parse Python code points directly. Width and precision accumulate without
/// temporary strings, bounded by the target's Py_ssize_t rather than usize.
pub(crate) fn parse_format_spec(
    bytes: &[u8],
    fractional_grouping: bool,
) -> Result<FormatSpec, FormatParseError> {
    let mut codes = wtf8_from_bytes(bytes)
        .code_points()
        .map(|code| code.to_u32())
        .peekable();
    let mut result = FormatSpec::default();
    let alignment = |code| char::from_u32(code).filter(|ch| matches!(ch, '<' | '>' | '^' | '='));
    let mut lookahead = codes.clone();
    let first = lookahead.next();
    let second = lookahead.next();
    let mut fill_specified = false;
    if let (Some(first), Some(second)) = (first, second)
        && let Some(align) = alignment(second)
    {
        result.fill = first;
        result.align = Some(align);
        fill_specified = true;
        codes.next();
        codes.next();
    } else if let Some(first) = first
        && let Some(align) = alignment(first)
    {
        result.align = Some(align);
        codes.next();
    }
    if let Some(sign) = codes.peek().copied().and_then(char::from_u32)
        && matches!(sign, '+' | '-' | ' ')
    {
        result.sign = Some(sign);
        codes.next();
    }
    result.coerce_negative_zero = codes.peek() == Some(&u32::from('z'));
    if result.coerce_negative_zero {
        codes.next();
    }
    result.alternate = codes.peek() == Some(&u32::from('#'));
    if result.alternate {
        codes.next();
    }
    if !fill_specified && codes.peek() == Some(&u32::from('0')) {
        result.fill = u32::from('0');
        result.zero_flag = true;
        codes.next();
    }
    result.width = parse_format_integer(&mut codes)?;
    result.grouping = parse_format_grouping(&mut codes)?;
    if codes.peek() == Some(&u32::from('.')) {
        codes.next();
        result.precision = parse_format_integer(&mut codes)?;
        if fractional_grouping {
            result.fractional_grouping = parse_format_grouping(&mut codes)?;
        }
        if result.precision.is_none() && result.fractional_grouping.is_none() {
            return Err(FormatParseError::Diagnostic(
                "Format specifier missing precision",
            ));
        }
    }
    result.ty = codes.next();
    if codes.next().is_some() {
        return Err(FormatParseError::InvalidSpecifier);
    }
    Ok(result)
}

pub(crate) fn parse_format_integer(
    codes: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Result<Option<usize>, FormatParseError> {
    let mut value = None;
    while let Some(digit) = codes
        .peek()
        .and_then(|code| super::ops::unicode_decimal_table::decimal(*code))
    {
        let next = value
            .unwrap_or(0usize)
            .checked_mul(10)
            .and_then(|value| value.checked_add(digit as usize))
            .filter(|value| *value <= isize::MAX as usize)
            .ok_or(FormatParseError::Diagnostic(
                "Too many decimal digits in format string",
            ))?;
        value = Some(next);
        codes.next();
    }
    Ok(value)
}

fn parse_format_grouping(
    codes: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Result<Option<char>, FormatParseError> {
    let separator = codes
        .peek()
        .copied()
        .and_then(char::from_u32)
        .filter(|code| matches!(code, ',' | '_'));
    if let Some(separator) = separator {
        codes.next();
        if codes
            .peek()
            .copied()
            .and_then(char::from_u32)
            .is_some_and(|next| matches!(next, ',' | '_') && next != separator)
        {
            return Err(FormatParseError::Diagnostic(
                "Cannot specify both ',' and '_'.",
            ));
        }
    }
    Ok(separator)
}

fn validate_format_grouping(spec: &FormatSpec, default_type: char) -> Result<(), FormatError> {
    let code = spec.ty.unwrap_or(u32::from(default_type));
    let presentation = char::from_u32(code);
    for (separator, fractional) in [(spec.grouping, false), (spec.fractional_grouping, true)] {
        let Some(separator) = separator else {
            continue;
        };
        let allowed = if fractional {
            presentation != Some('n')
        } else {
            match presentation {
                Some('d' | 'e' | 'f' | 'g' | 'E' | 'G' | '%' | 'F' | '\0') => true,
                Some('b' | 'o' | 'x' | 'X') => separator == '_',
                _ => false,
            }
        };
        if !allowed {
            return Err(FormatError::Diagnostic(
                "ValueError",
                Cow::Owned(format!(
                    "Cannot specify '{separator}' with '{}'.",
                    format_code_display(code)
                )),
            ));
        }
    }
    Ok(())
}

fn apply_grouping(text: &str, group: usize, sep: char) -> Result<String, FormatError> {
    grouped_digits(text, text.len(), group, sep)
}

fn grouped_digits(
    digits: &str,
    count: usize,
    group: usize,
    sep: char,
) -> Result<String, FormatError> {
    debug_assert!(
        digits
            .bytes()
            .all(|digit| digit.is_ascii_digit() || digit.is_ascii_hexdigit())
    );
    let separators = count.saturating_sub(1) / group;
    let capacity = separators
        .checked_mul(sep.len_utf8())
        .and_then(|size| size.checked_add(count))
        .ok_or_else(format_memory_error)?;
    let mut out = format_buffer(capacity)?;
    let zeros = count.saturating_sub(digits.len());
    let mut encoded = [0; 4];
    let separator = sep.encode_utf8(&mut encoded).as_bytes();
    for index in 0..count {
        if index > 0 && (count - index).is_multiple_of(group) {
            out.extend_from_slice(separator);
        }
        out.push(if index < zeros {
            b'0'
        } else {
            digits.as_bytes()[index - zeros]
        });
    }
    Ok(String::from_utf8(out).expect("numeric digits and grouping separator"))
}

fn format_fill_bytes(fill: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4);
    super::ops_string::push_wtf8_codepoint(&mut bytes, fill);
    bytes
}

fn format_memory_error() -> FormatError {
    FormatError::Diagnostic("MemoryError", Cow::Borrowed(""))
}

fn format_buffer(capacity: usize) -> Result<Vec<u8>, FormatError> {
    let mut out = Vec::new();
    out.try_reserve_exact(capacity)
        .map_err(|_| format_memory_error())?;
    Ok(out)
}

pub(crate) fn append_format_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), FormatError> {
    out.try_reserve(bytes.len())
        .map_err(|_| format_memory_error())?;
    out.extend_from_slice(bytes);
    Ok(())
}

/// Both percent and advanced integer presentation admit the same Unicode range.
pub(crate) fn format_character_codepoint(value: &BigInt) -> Result<Vec<u8>, FormatError> {
    let code = value
        .to_u32()
        .filter(|code| *code < 0x110000)
        .ok_or(FormatError::Diagnostic(
            "OverflowError",
            Cow::Borrowed("%c arg not in range(0x110000)"),
        ))?;
    let mut out = format_buffer(4)?;
    super::ops_string::push_wtf8_codepoint(&mut out, code);
    Ok(out)
}

/// The caller reserves the complete result first. ASCII fill is a memset;
/// multibyte/WTF-8 fill grows by copying complete already-written code points.
fn append_format_fill(out: &mut Vec<u8>, fill: &[u8], count: usize) {
    if count == 0 {
        return;
    }
    let start = out.len();
    let bytes = count * fill.len();
    if fill.len() == 1 {
        out.resize(start + bytes, fill[0]);
        return;
    }
    out.extend_from_slice(fill);
    while out.len() - start < bytes {
        let copied = (out.len() - start).min(bytes - (out.len() - start));
        out.extend_from_within(start..start + copied);
    }
}

pub(crate) fn apply_alignment(
    prefix: &str,
    body: &str,
    spec: &FormatSpec,
    default_align: char,
) -> Result<Vec<u8>, FormatError> {
    apply_alignment_parts(prefix, &[body], spec, default_align)
}

fn apply_alignment_parts(
    prefix: &str,
    body: &[&str],
    spec: &FormatSpec,
    default_align: char,
) -> Result<Vec<u8>, FormatError> {
    let fill = format_fill_bytes(spec.fill);
    let align = numeric_alignment(spec, default_align);
    let body_chars = body.iter().map(|part| part.chars().count()).sum::<usize>();
    let body_bytes = body.iter().try_fold(0usize, |size, part| {
        size.checked_add(part.len()).ok_or_else(format_memory_error)
    })?;
    let padding = spec
        .width
        .unwrap_or(0)
        .saturating_sub(prefix.chars().count() + body_chars);
    let capacity = fill
        .len()
        .checked_mul(padding)
        .and_then(|size| size.checked_add(prefix.len()))
        .and_then(|size| size.checked_add(body_bytes))
        .ok_or_else(format_memory_error)?;
    let mut out = format_buffer(capacity)?;
    let left = match align {
        '<' | '=' => 0,
        '^' => padding / 2,
        _ => padding,
    };
    append_format_fill(&mut out, &fill, left);
    out.extend_from_slice(prefix.as_bytes());
    if align == '=' {
        append_format_fill(&mut out, &fill, padding);
    }
    for part in body {
        out.extend_from_slice(part.as_bytes());
    }
    if align != '=' {
        append_format_fill(&mut out, &fill, padding - left);
    }
    Ok(out)
}
/// Left-pad `digits` with `'0'` and insert `sep` every `group` digits so the
/// resulting field (digits *and* separators) spans at least `min_field`
/// characters, using the fewest digits that satisfy the bound. This mirrors
/// CPython's `_PyUnicode_InsertThousandsGrouping` driven by the `min_width`
/// that `calc_number_widths` derives for sign-aware `'0'` fill: the zero-fill
/// region is itself grouped, so the field can legitimately exceed `min_field`
/// (a `min_field` of 8 over `42` yields the 9-char `0,000,042`).
fn zero_pad_grouped(
    digits: &str,
    group: usize,
    sep: char,
    min_field: usize,
) -> Result<String, FormatError> {
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
    grouped_digits(digits, needed, group, sep)
}

fn numeric_alignment(spec: &FormatSpec, default_align: char) -> char {
    spec.align
        .unwrap_or(if spec.zero_flag { '=' } else { default_align })
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
) -> Result<Vec<u8>, FormatError> {
    let suffix = grouped_fraction(suffix, spec.fractional_grouping)?;
    let suffix = suffix.as_ref();
    let align = numeric_alignment(spec, default_align);
    // The one case where padding interleaves with grouping: the `'0'` fill flag
    // (sign-aware `'='` alignment) combined with an actual grouping separator.
    // Any other alignment, or a non-`'0'` fill char with `'='`, pads with raw
    // fill chars that are *not* grouped — matching CPython.
    if let Some((group, sep)) = grouping
        && let Some(width) = spec.width
        && align == '='
        && spec.fill == u32::from('0')
    {
        let non_digit = prefix.chars().count() + suffix.chars().count();
        let field = zero_pad_grouped(int_digits, group, sep, width.saturating_sub(non_digit))?;
        let capacity = prefix
            .len()
            .checked_add(field.len())
            .and_then(|size| size.checked_add(suffix.len()))
            .ok_or_else(format_memory_error)?;
        let mut out = format_buffer(capacity)?;
        out.extend_from_slice(prefix.as_bytes());
        out.extend_from_slice(field.as_bytes());
        out.extend_from_slice(suffix.as_bytes());
        return Ok(out);
    }
    // Otherwise: group at the digits' natural width, then pad as a unit.
    let grouped = match grouping {
        Some((group, sep)) => Cow::Owned(apply_grouping(int_digits, group, sep)?),
        None => Cow::Borrowed(int_digits),
    };
    apply_alignment_parts(prefix, &[grouped.as_ref(), suffix], spec, default_align)
}

fn grouped_fraction(suffix: &str, separator: Option<char>) -> Result<Cow<'_, str>, FormatError> {
    let Some(separator) = separator.filter(|_| suffix.starts_with('.')) else {
        return Ok(Cow::Borrowed(suffix));
    };
    let digits = suffix.as_bytes()[1..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let separators = digits.saturating_sub(1) / 3;
    if separators == 0 {
        return Ok(Cow::Borrowed(suffix));
    }
    let capacity = suffix
        .len()
        .checked_add(separators * separator.len_utf8())
        .ok_or_else(format_memory_error)?;
    let mut out = format_buffer(capacity)?;
    out.push(b'.');
    let mut encoded = [0; 4];
    let separator = separator.encode_utf8(&mut encoded).as_bytes();
    for index in 0..digits {
        if index > 0 && index.is_multiple_of(3) {
            out.extend_from_slice(separator);
        }
        out.push(suffix.as_bytes()[index + 1]);
    }
    out.extend_from_slice(&suffix.as_bytes()[digits + 1..]);
    Ok(Cow::Owned(
        String::from_utf8(out).expect("numeric fraction is ASCII"),
    ))
}

struct NumericFormatBuffer(String);

impl std::fmt::Write for NumericFormatBuffer {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0
            .try_reserve(text.len())
            .map_err(|_| std::fmt::Error)?;
        self.0.push_str(text);
        Ok(())
    }
}

fn render_numeric(arguments: std::fmt::Arguments<'_>) -> Result<String, FormatError> {
    let mut buffer = NumericFormatBuffer(String::new());
    std::fmt::write(&mut buffer, arguments).map_err(|_| format_memory_error())?;
    Ok(buffer.0)
}

#[derive(Clone, Copy)]
enum DecimalNotation {
    Fixed,
    Scientific,
}

/// An f64 terminates within 1074 decimal fractional places (2^-1074).
/// Beyond that bound all requested places are exact zeros. Keep Rust's bounded
/// decimal rounding engine, then append the exact suffix with fallible growth;
/// a Python precision is never passed into core::fmt's u16 precision field.
fn render_float_decimal(
    value: f64,
    precision: usize,
    notation: DecimalNotation,
    retain_zeros: bool,
) -> Result<String, FormatError> {
    const EXACT_PLACES: usize = 1074;
    let bounded = precision.min(EXACT_PLACES);
    let text = match notation {
        DecimalNotation::Fixed => render_numeric(format_args!("{:.*}", bounded, value))?,
        DecimalNotation::Scientific => render_numeric(format_args!("{:.*e}", bounded, value))?,
    };
    let extra = if retain_zeros { precision - bounded } else { 0 };
    if extra == 0 {
        return Ok(text);
    }
    let insertion = match notation {
        DecimalNotation::Fixed => text.len(),
        DecimalNotation::Scientific => text.find('e').expect("finite decimal exponent"),
    };
    let mut bytes = text.into_bytes();
    let length = bytes.len();
    let end = length.checked_add(extra).ok_or_else(format_memory_error)?;
    bytes
        .try_reserve(extra)
        .map_err(|_| format_memory_error())?;
    bytes.resize(end, b'0');
    bytes.copy_within(insertion..length, insertion + extra);
    bytes[insertion..insertion + extra].fill(b'0');
    Ok(String::from_utf8(bytes).expect("decimal presentation is ASCII"))
}

fn trim_float_trailing(mut text: String, alternate: bool) -> String {
    if alternate {
        return text;
    }
    let exp_pos = text.find(['e', 'E']).unwrap_or(text.len());
    let mut end = exp_pos;
    if let Some(dot) = text[..exp_pos].find('.') {
        let bytes = text.as_bytes();
        while end > dot + 1 && bytes[end - 1] == b'0' {
            end -= 1;
        }
        if end == dot + 1 {
            end = dot;
        }
    }
    text.replace_range(end..exp_pos, "");
    text
}

fn normalize_exponent(mut text: String, upper: bool) -> Result<String, FormatError> {
    let (exp_pos, exp_char) = if let Some(pos) = text.find('e') {
        (pos, 'e')
    } else if let Some(pos) = text.find('E') {
        (pos, 'E')
    } else {
        return Ok(text);
    };
    let signed = matches!(text.as_bytes().get(exp_pos + 1), Some(b'+' | b'-'));
    let digits = text.len() - exp_pos - 1 - usize::from(signed);
    let added = usize::from(!signed) + 2usize.saturating_sub(digits);
    text.try_reserve(added).map_err(|_| format_memory_error())?;
    if upper && exp_char == 'e' {
        text.replace_range(exp_pos..exp_pos + 1, "E");
    }
    if !signed {
        text.insert(exp_pos + 1, '+');
    }
    for _ in digits..2 {
        text.insert(exp_pos + 2, '0');
    }
    Ok(text)
}

fn string_format_spec(spec: &FormatSpec) -> Result<FormatSpec, FormatError> {
    if let Some(sign) = spec.sign {
        let msg = if sign == ' ' {
            "Space not allowed in string format specifier"
        } else {
            "Sign not allowed in string format specifier"
        };
        return Err(FormatError::Diagnostic("ValueError", Cow::Borrowed(msg)));
    }
    if spec.coerce_negative_zero {
        return Err(FormatError::Diagnostic(
            "ValueError",
            Cow::Borrowed("Negative zero coercion (z) not allowed in string format specifier"),
        ));
    }
    if spec.alternate {
        return Err(FormatError::Diagnostic(
            "ValueError",
            Cow::Borrowed("Alternate form (#) not allowed in string format specifier"),
        ));
    }
    if spec.align == Some('=') {
        return Err(FormatError::Diagnostic(
            "ValueError",
            Cow::Borrowed("'=' alignment not allowed in string format specifier"),
        ));
    }
    Ok(*spec)
}

/// Select precision's endpoint and character count in one bounded WTF-8 walk.
/// Precision zero or a short prefix never scans or copies the remaining input.
fn format_text_span(text: &[u8], precision: Option<usize>) -> (&[u8], usize) {
    let limit = precision.unwrap_or(usize::MAX);
    let mut cursor = 0;
    let mut count = 0;
    while count < limit {
        let Some((next, _)) = super::ops_string::wtf8_step(text, cursor, false) else {
            break;
        };
        cursor = next;
        count += 1;
    }
    (&text[..cursor], count)
}

fn format_text_span_padded(
    text: &[u8],
    count: usize,
    width: Option<usize>,
    fill: &[u8],
    align: char,
) -> Result<Vec<u8>, FormatError> {
    let padding = width.unwrap_or(0).saturating_sub(count);
    let left = match align {
        '<' => 0,
        '^' => padding / 2,
        _ => padding,
    };
    let capacity = fill
        .len()
        .checked_mul(padding)
        .and_then(|size| size.checked_add(text.len()))
        .ok_or_else(format_memory_error)?;
    let mut out = format_buffer(capacity)?;
    append_format_fill(&mut out, fill, left);
    out.extend_from_slice(text);
    append_format_fill(&mut out, fill, padding - left);
    Ok(out)
}

/// Apply string precision and padding directly from immutable WTF-8 storage.
/// Percent and advanced formatting use the same sizing and fallible writer.
pub(crate) fn format_text_bytes(
    text: &[u8],
    width: Option<usize>,
    precision: Option<usize>,
    fill: &[u8],
    align: char,
) -> Result<Vec<u8>, FormatError> {
    let (text, count) = format_text_span(text, precision);
    format_text_span_padded(text, count, width, fill, align)
}

/// Percent conversion may keep an unmodified nonempty owned renderer string;
/// truncation/padding materializes only the selected prefix and output padding.
pub(crate) fn format_text_output(
    py: &PyToken<'_>,
    output: FormatOutput,
    width: Option<usize>,
    precision: Option<usize>,
    fill: &[u8],
    align: char,
    preserve_identity: bool,
) -> Result<FormatOutput, FormatError> {
    if !exception_pending(py)
        && let FormatOutput::OwnedString(bits) = &output
    {
        let ptr = obj_from_bits(*bits)
            .as_ptr()
            .expect("owned renderer string");
        let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
        let (text, count) = format_text_span(bytes, precision);
        if preserve_identity
            && !text.is_empty()
            && text.len() == bytes.len()
            && width.unwrap_or(0) <= count
        {
            return Ok(output);
        }
        // The closure runs while output still owns this immutable storage.
        // Reuse the selected span rather than scanning its prefix again.
        return output
            .with_bytes(py, |_| {
                format_text_span_padded(text, count, width, fill, align)
            })
            .map(FormatOutput::Bytes);
    }
    output
        .with_bytes(py, |bytes| {
            format_text_bytes(bytes, width, precision, fill, align)
        })
        .map(FormatOutput::Bytes)
}

fn format_string_with_spec(
    py: &PyToken<'_>,
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<FormatOutput, FormatError> {
    let spec = string_format_spec(spec)?;
    let ptr = obj.as_ptr().expect("admitted string receiver");
    let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
    if !bytes.is_empty() && spec.precision.is_none() && spec.width.unwrap_or(0) == 0 {
        inc_ref_bits(py, obj.bits());
        return Ok(FormatOutput::OwnedString(obj.bits()));
    }
    let (text, count) = format_text_span(bytes, spec.precision);
    if !text.is_empty() && spec.width.unwrap_or(0) <= count && text.len() == bytes.len() {
        inc_ref_bits(py, obj.bits());
        return Ok(FormatOutput::OwnedString(obj.bits()));
    }
    let fill = format_fill_bytes(spec.fill);
    Ok(format_text_span_padded(text, count, spec.width, &fill, spec.align.unwrap_or('<'))?.into())
}

fn validate_integer_format_spec(ty: char, spec: &FormatSpec) -> Result<(), FormatError> {
    if spec.precision.is_some() {
        return Err(FormatError::Diagnostic(
            "ValueError",
            Cow::Borrowed("Precision not allowed in integer format specifier"),
        ));
    }
    if spec.coerce_negative_zero {
        return Err(FormatError::Diagnostic(
            "ValueError",
            Cow::Borrowed("Negative zero coercion (z) not allowed in integer format specifier"),
        ));
    }
    if ty == 'c' {
        if spec.sign.is_some() {
            return Err(FormatError::Diagnostic(
                "ValueError",
                Cow::Borrowed("Sign not allowed with integer format specifier 'c'"),
            ));
        }
        if spec.alternate {
            return Err(FormatError::Diagnostic(
                "ValueError",
                Cow::Borrowed("Alternate form (#) not allowed with integer format specifier 'c'"),
            ));
        }
    }
    Ok(())
}

fn is_empty_format_spec(spec: &FormatSpec) -> bool {
    spec.fill == u32::from(' ')
        && spec.align.is_none()
        && !spec.zero_flag
        && spec.sign.is_none()
        && !spec.coerce_negative_zero
        && !spec.alternate
        && spec.width.is_none()
        && spec.grouping.is_none()
        && spec.fractional_grouping.is_none()
        && spec.precision.is_none()
        && spec.ty.is_none()
}

fn format_int_with_spec(
    _py: &PyToken<'_>,
    obj: MoltObject,
    payload: Option<u64>,
    spec: &FormatSpec,
) -> Result<Vec<u8>, FormatError> {
    let ty = spec.presentation().unwrap_or('d');
    validate_integer_format_spec(ty, spec)?;
    // Only actual integer rendering needs an owned BigInt. Admission and
    // floating presentations carry borrowed bits without cloning the payload.
    let mut value = payload
        .and_then(index_bigint_integral_bits)
        .ok_or_else(|| unknown_format_code_error(_py, spec.presentation().unwrap_or('d'), obj))?;
    if ty == 'c' {
        if value
            .to_i64()
            .and_then(|value| std::os::raw::c_long::try_from(value).ok())
            .is_none()
        {
            return Err(FormatError::Diagnostic(
                "OverflowError",
                Cow::Borrowed("Python int too large to convert to C long"),
            ));
        }
        let text = format_character_codepoint(&value)?;
        let fill = format_fill_bytes(spec.fill);
        return format_text_bytes(&text, spec.width, None, &fill, spec.align.unwrap_or('>'));
    }
    let base = match ty {
        'b' => 2,
        'o' => 8,
        'x' | 'X' => 16,
        'd' | 'n' => 10,
        _ => {
            return Err(FormatError::Diagnostic(
                "ValueError",
                Cow::Borrowed("unsupported int format type"),
            ));
        }
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
    assemble_number(&prefix, &digits, "", grouping, spec, '>')
}

pub(crate) fn format_float_with_spec(
    _py: &PyToken<'_>,
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<Vec<u8>, FormatError> {
    // Float descriptors read their payload; integer floating presentations use
    // the same numeric slot conversion as PyNumber_Float, including overrides.
    let val =
        crate::builtins::numbers::float_as_double(_py, obj.bits()).ok_or(FormatError::Pending)?;
    format_float_value_with_spec(val, spec)
}

/// Shared float/complex/percent presentation after numeric protocol admission.
fn validate_float_precision(spec: &FormatSpec) -> Result<(), FormatError> {
    if spec
        .precision
        .is_some_and(|precision| precision > i32::MAX as usize)
    {
        return Err(FormatError::Diagnostic(
            "ValueError",
            Cow::Borrowed("precision too big"),
        ));
    }
    Ok(())
}

pub(crate) fn format_float_value_with_spec(
    val: f64,
    spec: &FormatSpec,
) -> Result<Vec<u8>, FormatError> {
    validate_float_precision(spec)?;
    let use_default = spec.ty.is_none() && spec.precision.is_none();
    let ty = spec
        .presentation()
        .filter(|code| *code != '\0')
        .unwrap_or('g');
    // Percent scaling precedes classification: a finite input can overflow.
    let val = if ty == '%' { val * 100.0 } else { val };
    let upper = matches!(ty, 'F' | 'E' | 'G');
    // A NaN's payload sign is not part of Python's numeric presentation.
    // Every presentation shares sign admission, including infinities and NaNs.
    let mut prefix = match (float_is_negative(val), spec.sign) {
        (true, _) => "-",
        (false, Some('+')) => "+",
        (false, Some(' ')) => " ",
        _ => "",
    };
    if val.is_nan() {
        let text = if ty == '%' {
            "nan%"
        } else if upper {
            "NAN"
        } else {
            "nan"
        };
        return apply_alignment(prefix, text, spec, '>');
    }
    if val.is_infinite() {
        let text = if ty == '%' {
            "inf%"
        } else if upper {
            "INF"
        } else {
            "inf"
        };
        return apply_alignment(prefix, text, spec, '>');
    }
    let abs_val = val.abs();
    let prec = spec.precision.unwrap_or(6);
    let mut body = if use_default {
        format_float(abs_val)
    } else {
        match ty {
            'f' | 'F' => render_float_decimal(abs_val, prec, DecimalNotation::Fixed, true)?,
            'e' | 'E' => render_float_decimal(abs_val, prec, DecimalNotation::Scientific, true)?,
            'g' | 'G' => format_general_float(abs_val, prec, spec.alternate, spec.ty.is_none())?,
            '%' => render_float_decimal(abs_val, prec, DecimalNotation::Fixed, true)?,
            _ => {
                return Err(FormatError::Diagnostic(
                    "ValueError",
                    Cow::Borrowed("unsupported float format type"),
                ));
            }
        }
    };
    // PEP 682 coerces the sign of a rounded zero, not merely input -0.0.
    // Exponents and a percent suffix do not contribute magnitude digits.
    if spec.coerce_negative_zero
        && body
            .split(['e', 'E'])
            .next()
            .unwrap()
            .bytes()
            .all(|byte| matches!(byte, b'0' | b'.'))
    {
        prefix = match spec.sign {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        };
    }
    body = normalize_exponent(body, upper)?;
    if spec.alternate && !body.contains('.') {
        let decimal = body.find(['e', 'E']).unwrap_or(body.len());
        body.try_reserve(1).map_err(|_| format_memory_error())?;
        body.insert(decimal, '.');
    }
    if ty == '%' {
        body.try_reserve(1).map_err(|_| format_memory_error())?;
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
    assemble_number(prefix, int_digits, suffix, grouping, spec, '>')
}

/// Choose notation from the rounded decimal exponent, as PyOS_double_to_string
/// does. Reposition those same rounded digits instead of rounding a second time
/// or classifying the unrounded value with log10 at a carry boundary.
fn format_general_float(
    value: f64,
    precision: usize,
    alternate: bool,
    add_dot_zero: bool,
) -> Result<String, FormatError> {
    let precision = precision.max(1);
    let mut rounded =
        render_float_decimal(value, precision - 1, DecimalNotation::Scientific, alternate)?;
    let (mantissa, exponent) = rounded.split_once('e').expect("scientific float exponent");
    let exponent: i32 = exponent.parse().expect("finite float exponent");
    let threshold = precision.saturating_sub(usize::from(add_dot_zero));
    if exponent < -4 || i64::from(exponent) >= i64::try_from(threshold).unwrap_or(i64::MAX) {
        return Ok(trim_float_trailing(rounded, alternate));
    }
    let mantissa_len = mantissa.len();
    let dot = mantissa.find('.');
    rounded.truncate(mantissa_len);
    if let Some(dot) = dot {
        rounded.remove(dot);
    }
    let decimal = exponent + 1;
    let mut fixed = if decimal <= 0 {
        let mut bytes = rounded.into_bytes();
        let offset = 2 + (-decimal) as usize;
        let length = bytes.len();
        bytes
            .try_reserve(offset)
            .map_err(|_| format_memory_error())?;
        bytes.resize(length + offset, b'0');
        bytes.copy_within(0..length, offset);
        bytes[..offset].fill(b'0');
        bytes[1] = b'.';
        String::from_utf8(bytes).expect("numeric presentation is ASCII")
    } else if decimal as usize >= rounded.len() {
        let mut bytes = rounded.into_bytes();
        let padding = decimal as usize - bytes.len();
        bytes
            .try_reserve(padding)
            .map_err(|_| format_memory_error())?;
        bytes.resize(decimal as usize, b'0');
        String::from_utf8(bytes).expect("numeric presentation is ASCII")
    } else {
        rounded.try_reserve(1).map_err(|_| format_memory_error())?;
        rounded.insert(decimal as usize, '.');
        rounded
    };
    if alternate && !fixed.contains('.') {
        fixed.try_reserve(1).map_err(|_| format_memory_error())?;
        fixed.push('.');
    }
    let mut fixed = trim_float_trailing(fixed, alternate);
    if add_dot_zero && !fixed.contains('.') {
        fixed.try_reserve(2).map_err(|_| format_memory_error())?;
        fixed.push_str(".0");
    }
    Ok(fixed)
}

fn format_complex_with_spec(
    _py: &PyToken<'_>,
    obj: MoltObject,
    value: ComplexParts,
    spec: &FormatSpec,
) -> Result<Vec<u8>, FormatError> {
    let mut ty = spec.presentation().filter(|code| *code != '\0');
    let mut grouping = spec.grouping;
    if ty == Some('n') {
        ty = Some('g');
        grouping = None;
    }
    if let Some(code) = ty
        && !matches!(code, 'e' | 'E' | 'f' | 'F' | 'g' | 'G')
    {
        return Err(unknown_format_code_error(_py, code, obj));
    }
    validate_float_precision(spec)?;
    if spec.fill == u32::from('0') {
        return Err(FormatError::Diagnostic(
            "ValueError",
            Cow::Borrowed("Zero padding is not allowed in complex format specifier"),
        ));
    }
    if spec.align == Some('=') {
        return Err(FormatError::Diagnostic(
            "ValueError",
            Cow::Borrowed("'=' alignment flag is not allowed in complex format specifier"),
        ));
    }
    let re = value.re;
    let im = value.im;
    let re_is_zero = re == 0.0 && !re.is_sign_negative();
    let include_real = ty.is_some() || !re_is_zero;
    let use_default = spec.ty.is_none() && spec.precision.is_none();
    let component_spec = FormatSpec {
        fill: u32::from(' '),
        align: None,
        zero_flag: false,
        width: None,
        grouping,
        // Complex's omitted type uses general notation without float's '.0'.
        ty: if !use_default && ty.is_none() {
            Some(u32::from('g'))
        } else {
            ty.map(u32::from)
        },
        ..*spec
    };
    let render = |component, sign| -> Result<String, FormatError> {
        let component_spec = FormatSpec {
            sign,
            ..component_spec
        };
        let mut text = String::from_utf8(format_float_value_with_spec(component, &component_spec)?)
            .expect("unpadded numeric text is ASCII");
        if use_default && text.ends_with(".0") {
            text.truncate(text.len() - if spec.alternate { 1 } else { 2 });
        }
        Ok(text)
    };
    let imag_text = render(im, if include_real { Some('+') } else { spec.sign })?;
    let body = if include_real {
        let real_text = render(re, spec.sign)?;
        let combined = render_numeric(format_args!("{real_text}{imag_text}j"))?;
        if ty.is_none() {
            render_numeric(format_args!("({combined})"))?
        } else {
            combined
        }
    } else {
        render_numeric(format_args!("{imag_text}j"))?
    };
    apply_alignment("", &body, spec, '>')
}

pub(crate) fn format_with_spec(
    _py: &PyToken<'_>,
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<FormatOutput, FormatError> {
    let is_string = obj
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING });
    let integer = index_integral_payload_bits(obj.bits());
    let default_type = if is_string {
        's'
    } else if integer.is_some() {
        'd'
    } else {
        '\0'
    };
    validate_format_grouping(spec, default_type)?;
    if spec.ty.is_some_and(|code| char::from_u32(code).is_none()) {
        return Err(unknown_format_code_error(_py, spec.ty.unwrap(), obj));
    }
    let normalized;
    let spec = if default_type == '\0' && spec.ty == Some(0) {
        normalized = FormatSpec { ty: None, ..*spec };
        &normalized
    } else {
        spec
    };
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_COMPLEX {
                let value = *complex_ref(ptr);
                return format_complex_with_spec(_py, obj, value, spec).map(FormatOutput::from);
            }
        }
    }
    let is_integral = integer.is_some();
    let is_float = as_float_extended(obj).is_some();
    let is_number = is_integral || is_float;
    if !is_string && !is_number && !is_empty_format_spec(spec) {
        return Err(unsupported_format_string_error(_py, obj));
    }
    if spec.presentation() == Some('n') {
        if !is_number {
            if is_string {
                return Err(unknown_format_code_error(_py, 'n', obj));
            }
            return Err(unsupported_format_string_error(_py, obj));
        }
        let mut normalized = FormatSpec {
            fill: spec.fill,
            align: spec.align,
            zero_flag: spec.zero_flag,
            sign: spec.sign,
            coerce_negative_zero: spec.coerce_negative_zero,
            alternate: spec.alternate,
            width: spec.width,
            grouping: None,
            precision: spec.precision,
            fractional_grouping: spec.fractional_grouping,
            ty: None,
        };
        if is_float {
            normalized.ty = Some(u32::from('g'));
            return format_float_with_spec(_py, obj, &normalized).map(FormatOutput::from);
        }
        normalized.ty = Some(u32::from('d'));
        return format_int_with_spec(_py, obj, integer, &normalized).map(FormatOutput::from);
    }
    match spec.presentation() {
        Some('s') => {
            if is_string {
                format_string_with_spec(_py, obj, spec)
            } else if is_number {
                Err(unknown_format_code_error(_py, 's', obj))
            } else {
                Err(unsupported_format_string_error(_py, obj))
            }
        }
        Some('d') | Some('b') | Some('o') | Some('x') | Some('X') | Some('c') => {
            if is_integral {
                format_int_with_spec(_py, obj, integer, spec).map(FormatOutput::from)
            } else if is_float || is_string {
                Err(unknown_format_code_error(_py, spec.ty.unwrap(), obj))
            } else {
                Err(unsupported_format_string_error(_py, obj))
            }
        }
        Some('f') | Some('F') | Some('e') | Some('E') | Some('g') | Some('G') | Some('%') => {
            if is_number {
                format_float_with_spec(_py, obj, spec).map(FormatOutput::from)
            } else if is_string {
                Err(unknown_format_code_error(_py, spec.ty.unwrap(), obj))
            } else {
                Err(unsupported_format_string_error(_py, obj))
            }
        }
        Some(code) => {
            if code == '\0' && is_float {
                let normalized = FormatSpec { ty: None, ..*spec };
                return format_float_with_spec(_py, obj, &normalized).map(FormatOutput::from);
            }
            if is_string || is_number {
                Err(unknown_format_code_error(
                    _py,
                    spec.ty.unwrap_or(u32::from(code)),
                    obj,
                ))
            } else {
                Err(unsupported_format_string_error(_py, obj))
            }
        }
        None => {
            // An omitted presentation type uses the same scalar projection
            // as explicit codes. Only an empty bool spec uses its str form.
            if obj.as_bool().is_some() {
                if is_empty_format_spec(spec) {
                    Ok(format_obj_str_output(_py, obj))
                } else {
                    format_int_with_spec(_py, obj, integer, spec).map(FormatOutput::from)
                }
            } else if is_integral {
                format_int_with_spec(_py, obj, integer, spec).map(FormatOutput::from)
            } else if is_float {
                format_float_with_spec(_py, obj, spec).map(FormatOutput::from)
            } else if is_string {
                format_string_with_spec(_py, obj, spec)
            } else {
                Ok(format_obj_str_output(_py, obj))
            }
        }
    }
}
