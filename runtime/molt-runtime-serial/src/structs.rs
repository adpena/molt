use crate::bridge::*;
use abi::abi_types::{PyObject, PyTypeObject};
use abi::api::memory::BufferAcquireError;
use molt_cpython_abi as abi;
use molt_runtime_capi::{
    NativeObjectOwner, PyBUF_SIMPLE, PyBUF_WRITABLE, RuntimeBufferLease, acquire_buffer,
    retire_native_reference,
};
use molt_runtime_core::prelude::*;
use num_traits::ToPrimitive;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ffi::{
    c_double, c_float, c_int, c_long, c_longlong, c_short, c_uint, c_ulong, c_ulonglong, c_ushort,
};
use std::mem::{align_of, size_of};
use std::sync::Arc;

#[derive(Clone, Copy)]
enum StructEndian {
    Native,
    Little,
    Big,
}

#[derive(Clone, Copy)]
struct StructFormat {
    endian: StructEndian,
    aligned: bool,
    native_sizes: bool,
}

#[derive(Clone, Copy)]
enum StructKind {
    Pad,
    Int { signed: bool },
    Float,
    Bool,
    Char,
    Bytes,
    Pascal,
}

#[derive(Clone, Copy)]
struct StructOp {
    kind: StructKind,
    count: usize,
    size: usize,
    align: usize,
    code: char,
}

struct StructField {
    kind: StructKind,
    count: usize,
    size: usize,
    offset: usize,
    code: char,
}

struct StructPlan {
    format: StructFormat,
    fields: Vec<StructField>,
    size: usize,
    values: usize,
}

fn compile_format(text: &str) -> Result<StructPlan, String> {
    let mut chars = text.chars().peekable();
    let mut format = StructFormat {
        endian: StructEndian::Native,
        aligned: true,
        native_sizes: true,
    };
    if let Some(&ch) = chars.peek() {
        match ch {
            '@' => {
                chars.next();
            }
            '=' => {
                chars.next();
                format = StructFormat {
                    endian: StructEndian::Native,
                    aligned: false,
                    native_sizes: false,
                };
            }
            '<' => {
                chars.next();
                format = StructFormat {
                    endian: StructEndian::Little,
                    aligned: false,
                    native_sizes: false,
                };
            }
            '>' | '!' => {
                chars.next();
                format = StructFormat {
                    endian: StructEndian::Big,
                    aligned: false,
                    native_sizes: false,
                };
            }
            _ => {}
        }
    }
    let mut plan = StructPlan {
        format,
        fields: Vec::new(),
        size: 0,
        values: 0,
    };
    let mut count: usize = 0;
    let mut count_set = false;
    for ch in chars {
        if ch.is_ascii_digit() {
            let digit = (ch as u8 - b'0') as usize;
            count = count
                .checked_mul(10)
                .and_then(|val| val.checked_add(digit))
                .ok_or_else(|| "total struct size too long".to_string())?;
            count_set = true;
            continue;
        }
        let repeat = if count_set { count } else { 1 };
        count = 0;
        count_set = false;
        let op = match ch {
            'x' => StructOp {
                kind: StructKind::Pad,
                count: repeat,
                size: 1,
                align: 1,
                code: ch,
            },
            'c' => StructOp {
                kind: StructKind::Char,
                count: repeat,
                size: 1,
                align: 1,
                code: ch,
            },
            '?' => StructOp {
                kind: StructKind::Bool,
                count: repeat,
                size: 1,
                align: 1,
                code: ch,
            },
            's' | 'p' => {
                let len = repeat;
                StructOp {
                    kind: if ch == 's' {
                        StructKind::Bytes
                    } else {
                        StructKind::Pascal
                    },
                    count: 1,
                    size: len,
                    align: 1,
                    code: ch,
                }
            }
            'b' => int_op(ch, repeat, format, true, size_of::<i8>(), align_of::<i8>())?,
            'B' => int_op(ch, repeat, format, false, size_of::<u8>(), align_of::<u8>())?,
            'h' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<c_short>(), 2)?,
                align_of::<c_short>(),
            )?,
            'H' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<c_ushort>(), 2)?,
                align_of::<c_ushort>(),
            )?,
            'i' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<c_int>(), 4)?,
                align_of::<c_int>(),
            )?,
            'I' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<c_uint>(), 4)?,
                align_of::<c_uint>(),
            )?,
            'l' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<c_long>(), 4)?,
                align_of::<c_long>(),
            )?,
            'L' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<c_ulong>(), 4)?,
                align_of::<c_ulong>(),
            )?,
            'q' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<c_longlong>(), 8)?,
                align_of::<c_longlong>(),
            )?,
            'Q' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<c_ulonglong>(), 8)?,
                align_of::<c_ulonglong>(),
            )?,
            'n' => native_only_int_op(
                ch,
                repeat,
                format,
                true,
                size_of::<isize>(),
                align_of::<isize>(),
            )?,
            'N' => native_only_int_op(
                ch,
                repeat,
                format,
                false,
                size_of::<usize>(),
                align_of::<usize>(),
            )?,
            'P' => native_only_int_op(
                ch,
                repeat,
                format,
                false,
                size_of::<usize>(),
                align_of::<usize>(),
            )?,
            'f' => float_op(
                ch,
                repeat,
                format,
                native_size(format, size_of::<c_float>(), 4)?,
                align_of::<c_float>(),
            )?,
            'd' => float_op(
                ch,
                repeat,
                format,
                native_size(format, size_of::<c_double>(), 8)?,
                align_of::<c_double>(),
            )?,
            'e' => float_op(ch, repeat, format, 2, align_of::<u16>())?,
            _ => return Err("bad char in struct format".to_string()),
        };
        plan.place(op)
            .ok_or_else(|| "total struct size too long".to_string())?;
    }
    if count_set {
        return Err("repeat count given without format specifier".to_string());
    }
    Ok(plan)
}

fn format_obj_to_owned(_py: &PyToken, format_obj: MoltObject) -> Result<String, String> {
    if let Some(text) = string_obj_to_owned(format_obj) {
        return Ok(text);
    }
    let Some(ptr) = format_obj.as_ptr() else {
        let type_label = type_name(_py, format_obj);
        return Err(format!(
            "Struct() argument 1 must be a str or bytes object, not {type_label}"
        ));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if (type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY)
        && let Some(bytes) = unsafe { bytes_like_slice_raw(ptr) }
    {
        let out: String = bytes.iter().map(|b| *b as char).collect();
        return Ok(out);
    }
    let type_label = type_name(_py, format_obj);
    Err(format!(
        "Struct() argument 1 must be a str or bytes object, not {type_label}"
    ))
}

fn native_size(format: StructFormat, native: usize, standard: usize) -> Result<usize, String> {
    if format.native_sizes {
        Ok(native)
    } else {
        Ok(standard)
    }
}

fn int_op(
    code: char,
    repeat: usize,
    format: StructFormat,
    signed: bool,
    size: usize,
    native_align: usize,
) -> Result<StructOp, String> {
    let align = if format.aligned { native_align } else { 1 };
    Ok(StructOp {
        kind: StructKind::Int { signed },
        count: repeat,
        size,
        align,
        code,
    })
}

fn native_only_int_op(
    code: char,
    repeat: usize,
    format: StructFormat,
    signed: bool,
    size: usize,
    native_align: usize,
) -> Result<StructOp, String> {
    if !format.native_sizes {
        return Err("bad char in struct format".to_string());
    }
    int_op(code, repeat, format, signed, size, native_align)
}

fn float_op(
    code: char,
    repeat: usize,
    format: StructFormat,
    size: usize,
    native_align: usize,
) -> Result<StructOp, String> {
    let align = if format.aligned { native_align } else { 1 };
    Ok(StructOp {
        kind: StructKind::Float,
        count: repeat,
        size,
        align,
        code,
    })
}

fn align_up(offset: usize, align: usize) -> Option<usize> {
    if align <= 1 {
        return Some(offset);
    }
    let aligned = offset.checked_add(align - 1)? / align * align;
    Some(aligned)
}

impl StructPlan {
    fn place(&mut self, op: StructOp) -> Option<()> {
        // sizeof(T) is a multiple of alignof(T). Native zero-count
        // directives still round up the next field (including at the end).
        debug_assert!(op.size.is_multiple_of(op.align));
        let offset = align_up(self.size, op.align)?;
        self.size = offset.checked_add(op.count.checked_mul(op.size)?)?;
        if self.size > isize::MAX as usize {
            return None;
        }
        if !matches!(op.kind, StructKind::Pad) && op.count != 0 {
            self.values = self.values.checked_add(op.count)?;
            self.fields.push(StructField {
                kind: op.kind,
                count: op.count,
                size: op.size,
                offset,
                code: op.code,
            });
        }
        Some(())
    }
}

// A counted export or the pack output owns this span. No Rust slice survives
// a Python conversion: writes happen only after each callback has returned.
struct StructWriter {
    data: *mut u8,
    len: usize,
    position: usize,
}
impl StructWriter {
    fn push(&mut self, byte: u8) {
        assert!(self.position < self.len);
        unsafe { self.data.add(self.position).write(byte) };
        self.position += 1;
    }
    fn extend_from_slice(&mut self, bytes: &[u8]) {
        assert!(bytes.len() <= self.len - self.position);
        // Source can alias a pack_into destination ('s'/'p' bytearray values).
        unsafe { std::ptr::copy(bytes.as_ptr(), self.data.add(self.position), bytes.len()) };
        self.position += bytes.len();
    }
}

fn push_unsigned(
    out: &mut StructWriter,
    endian: StructEndian,
    size: usize,
    val: u128,
) -> Result<(), String> {
    match size {
        1 => out.push(val as u8),
        2 => {
            let bytes = match endian {
                StructEndian::Native => (val as u16).to_ne_bytes(),
                StructEndian::Little => (val as u16).to_le_bytes(),
                StructEndian::Big => (val as u16).to_be_bytes(),
            };
            out.extend_from_slice(&bytes);
        }
        4 => {
            let bytes = match endian {
                StructEndian::Native => (val as u32).to_ne_bytes(),
                StructEndian::Little => (val as u32).to_le_bytes(),
                StructEndian::Big => (val as u32).to_be_bytes(),
            };
            out.extend_from_slice(&bytes);
        }
        8 => {
            let bytes = match endian {
                StructEndian::Native => (val as u64).to_ne_bytes(),
                StructEndian::Little => (val as u64).to_le_bytes(),
                StructEndian::Big => (val as u64).to_be_bytes(),
            };
            out.extend_from_slice(&bytes);
        }
        _ => return Err("unsupported integer size".to_string()),
    }
    Ok(())
}

fn read_signed(bytes: &[u8], endian: StructEndian, size: usize) -> Result<i128, String> {
    let val = match size {
        1 => i8::from_ne_bytes([bytes[0]]) as i128,
        2 => {
            let mut buf = [0u8; 2];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..2]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..2]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..2]),
            }
            (match endian {
                StructEndian::Native => i16::from_ne_bytes(buf),
                StructEndian::Little => i16::from_le_bytes(buf),
                StructEndian::Big => i16::from_be_bytes(buf),
            }) as i128
        }
        4 => {
            let mut buf = [0u8; 4];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..4]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..4]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..4]),
            }
            (match endian {
                StructEndian::Native => i32::from_ne_bytes(buf),
                StructEndian::Little => i32::from_le_bytes(buf),
                StructEndian::Big => i32::from_be_bytes(buf),
            }) as i128
        }
        8 => {
            let mut buf = [0u8; 8];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..8]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..8]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..8]),
            }
            (match endian {
                StructEndian::Native => i64::from_ne_bytes(buf),
                StructEndian::Little => i64::from_le_bytes(buf),
                StructEndian::Big => i64::from_be_bytes(buf),
            }) as i128
        }
        _ => return Err("unsupported integer size".to_string()),
    };
    Ok(val)
}

fn read_unsigned(bytes: &[u8], endian: StructEndian, size: usize) -> Result<u128, String> {
    let val = match size {
        1 => u8::from_ne_bytes([bytes[0]]) as u128,
        2 => {
            let mut buf = [0u8; 2];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..2]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..2]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..2]),
            }
            (match endian {
                StructEndian::Native => u16::from_ne_bytes(buf),
                StructEndian::Little => u16::from_le_bytes(buf),
                StructEndian::Big => u16::from_be_bytes(buf),
            }) as u128
        }
        4 => {
            let mut buf = [0u8; 4];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..4]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..4]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..4]),
            }
            (match endian {
                StructEndian::Native => u32::from_ne_bytes(buf),
                StructEndian::Little => u32::from_le_bytes(buf),
                StructEndian::Big => u32::from_be_bytes(buf),
            }) as u128
        }
        8 => {
            let mut buf = [0u8; 8];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..8]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..8]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..8]),
            }
            (match endian {
                StructEndian::Native => u64::from_ne_bytes(buf),
                StructEndian::Little => u64::from_le_bytes(buf),
                StructEndian::Big => u64::from_be_bytes(buf),
            }) as u128
        }
        _ => return Err("unsupported integer size".to_string()),
    };
    Ok(val)
}

fn f16_from_f64(val: f64) -> Result<u16, molt_obj_model::float_bits::FloatNarrowError> {
    molt_obj_model::float_bits::f64_to_f16_bits(val)
}

fn f16_to_f64(bits: u16) -> f64 {
    molt_obj_model::float_bits::f16_bits_to_f64(bits)
}

fn signed_range(bits: usize) -> (i128, i128) {
    let limit = 1i128 << (bits - 1);
    (-limit, limit - 1)
}

fn unsigned_max(bits: usize) -> i128 {
    (1i128 << bits) - 1
}

fn signed_range_message(code: char, bits: usize) -> String {
    let (min, max) = signed_range(bits);
    format!("'{code}' format requires {min} <= number <= {max}")
}

fn unsigned_range_message(code: char, bits: usize) -> String {
    let max = unsigned_max(bits);
    format!("'{code}' format requires 0 <= number <= {max}")
}

/// `None` means an exception is already pending; otherwise the kind to raise.
type StructIntrinsicError = (Option<BuiltinExc>, String);

fn struct_unpack_values<'py>(
    py: &'py PyToken,
    plan: &StructPlan,
    data: *const u8,
) -> Result<molt_runtime_core::RuntimeValueCollection<'py>, StructIntrinsicError> {
    use StructKind::*;
    let mut out = molt_runtime_core::RuntimeValueCollection::with_capacity(py, plan.values);
    for field in &plan.fields {
        for index in 0..field.count {
            let offset = field.offset + index * field.size;
            // The caller admitted a complete record. Only access the borrowed
            // buffer before creating each Python object, which can allocate.
            let bytes = if field.size == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(data.add(offset), field.size) }
            };
            let bits = match field.kind {
                Char | Bytes | Pascal => {
                    let bytes = if let (Pascal, Some((&length, rest))) =
                        (field.kind, bytes.split_first())
                    {
                        &rest[..usize::from(length).min(rest.len())]
                    } else {
                        bytes
                    };
                    let bytes = bytes.to_vec();
                    let ptr = alloc_bytes(py, &bytes);
                    if ptr.is_null() {
                        return Err((Some(BuiltinExc::MemoryError), "allocation failed".into()));
                    }
                    MoltObject::from_ptr(ptr).bits()
                }
                Bool => MoltObject::from_bool(bytes[0] != 0).bits(),
                Int { signed } => {
                    let value = if signed {
                        read_signed(bytes, plan.format.endian, field.size)
                    } else {
                        read_unsigned(bytes, plan.format.endian, field.size).map(|n| n as i128)
                    }
                    .map_err(|message| (Some(BuiltinExc::OverflowError), message))?;
                    let bits = int_bits_from_i128(py, value);
                    if exception_pending(py) {
                        return Err((None, String::new()));
                    }
                    bits
                }
                Float => {
                    let bits = read_unsigned(bytes, plan.format.endian, field.size)
                        .map_err(|message| (Some(BuiltinExc::OverflowError), message))?;
                    let value = match field.size {
                        2 => f16_to_f64(bits as u16),
                        4 => f32::from_bits(bits as u32) as f64,
                        8 => f64::from_bits(bits as u64),
                        _ => unreachable!("validated float width"),
                    };
                    molt_runtime_core::rt_float_result_bits(py, value)
                }
                Pad => unreachable!("padding has no field"),
            };
            out.push(unsafe { OwnedRuntimeValue::from_owned_bits(py, bits) });
        }
    }
    Ok(out)
}

fn struct_error(py: &PyToken, class: u64, message: &str) -> u64 {
    let text = unsafe { OwnedRuntimeValue::from_owned_bits(py, rt_string_from(py, message)) };
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let args = unsafe { OwnedRuntimeValue::from_owned_bits(py, rt_tuple(py, &[text.bits()])) };
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let error = unsafe {
        OwnedRuntimeValue::from_owned_bits(py, exception_new_from_class(py, class, args.bits()))
    };
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    rt_raise(py, error.bits())
}

thread_local! {
    // The plans contain only target-native sizes and offsets, with no Python
    // objects or callback hooks. Bound both the number and size of cache keys.
    static FORMAT_PLANS: RefCell<HashMap<Box<str>, Arc<StructPlan>>> = RefCell::default();
}

fn cached_plan(text: &str) -> Result<Arc<StructPlan>, String> {
    const MAX_CACHED_FORMAT_BYTES: usize = 256;
    const MAX_CACHED_FORMATS: usize = 100;
    if text.len() > MAX_CACHED_FORMAT_BYTES {
        return compile_format(text).map(Arc::new);
    }
    FORMAT_PLANS.with_borrow_mut(|plans| {
        if let Some(plan) = plans.get(text) {
            return Ok(Arc::clone(plan));
        }
        let plan = Arc::new(compile_format(text)?);
        if plans.len() == MAX_CACHED_FORMATS {
            plans.clear();
        }
        plans.insert(text.into(), Arc::clone(&plan));
        Ok(plan)
    })
}

fn plan(py: &PyToken, error: u64, format: u64) -> Result<Arc<StructPlan>, u64> {
    // The attached entry keeps the borrowed argument alive throughout this
    // pure Rust operation, and string payloads cannot be mutated.
    if let Some(ptr) = obj_from_bits(format).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_STRING
        && let Ok(text) = std::str::from_utf8(unsafe { string_bytes(ptr) })
    {
        return cached_plan(text).map_err(|message| struct_error(py, error, &message));
    }
    let text = format_obj_to_owned(py, obj_from_bits(format))
        .map_err(|message| raise_exception::<u64>(py, BuiltinExc::TypeError, &message))?;
    cached_plan(&text).map_err(|message| struct_error(py, error, &message))
}

unsafe fn native_object(bits: u64) -> NativeObjectOwner {
    unsafe {
        NativeObjectOwner::from_owned(abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits))
    }
}

fn index_object(py: &PyToken, error: u64, bits: u64) -> Result<NativeObjectOwner, u64> {
    unsafe {
        let object = native_object(bits);
        if object.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        if abi::api::abstract_number::PyIndex_Check(object.as_ptr()) == 0 {
            return Err(struct_error(
                py,
                error,
                "required argument is not an integer",
            ));
        }
        let integer = NativeObjectOwner::from_owned(abi::api::abstract_number::PyNumber_Index(
            object.as_ptr(),
        ));
        if integer.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        Ok(integer)
    }
}

// Every struct integer width fits in i128. A wider index result is out of
// range for every field, without constructing a dynamic bound for it.
fn index_value(py: &PyToken, error: u64, bits: u64) -> Result<Option<i128>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_int() {
        return Ok(Some(i128::from(obj.as_int_unchecked())));
    }
    if let Some(value) = obj.as_bool() {
        return Ok(Some(i128::from(value)));
    }
    let integer = index_object(py, error, bits)?;
    unsafe {
        let Some(value) = abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(integer.as_ptr()) else {
            return Err(MoltObject::none().bits());
        };
        let value = OwnedRuntimeValue::from_owned_bits(py, value);
        let value = obj_from_bits(value.bits());
        if let Some(value) = to_i64(value) {
            return Ok(Some(i128::from(value)));
        }
        to_bigint(value)
            .map(|value| value.to_i128())
            .ok_or_else(|| {
                raise_exception::<u64>(
                    py,
                    BuiltinExc::SystemError,
                    "index result has no integer storage",
                )
            })
    }
}

fn pointer_value(py: &PyToken, error: u64, bits: u64) -> Result<usize, u64> {
    let original = unsafe { native_object(bits) };
    if original.as_ptr().is_null() {
        return Err(MoltObject::none().bits());
    }
    let is_original_integer = unsafe { abi::api::numbers::PyLong_Check(original.as_ptr()) != 0 };
    let integer = index_object(py, error, bits)?;
    let value = unsafe {
        retire_native_reference(|| abi::api::numbers::PyLong_AsVoidPtr(integer.as_ptr()))
    };
    if exception_pending(py) {
        if is_original_integer
            && unsafe {
                abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut abi::abi_types::PyExc_OverflowError).cast(),
                ) != 0
            }
        {
            unsafe {
                abi::api::errors::PyErr_Clear();
            }
            return Err(struct_error(py, error, "int too large to convert"));
        }
        return Err(MoltObject::none().bits());
    }
    Ok(value as usize)
}

fn index_offset(py: &PyToken, bits: u64, packing: bool) -> Result<isize, u64> {
    unsafe {
        let object = native_object(bits);
        if object.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        // Through 3.14 pack_into's offset converter requested IndexError; from
        // 3.15 it is the plain Py_ssize_t converter (Modules/clinic/_struct.c.h).
        let result = if packing && !molt_runtime_core::rt_target_at_least(py, 3, 15) {
            retire_native_reference(|| {
                abi::api::abstract_number::PyNumber_AsSsize_t(
                    object.as_ptr(),
                    (&raw mut abi::abi_types::PyExc_IndexError).cast(),
                )
            })
        } else {
            // Argument Clinic's Py_ssize_t converter preserves PyLong_AsSsize_t's
            // overflow exception and message.
            let integer = NativeObjectOwner::from_owned(abi::api::abstract_number::PyNumber_Index(
                object.as_ptr(),
            ));
            if integer.as_ptr().is_null() {
                return Err(MoltObject::none().bits());
            }
            retire_native_reference(|| abi::api::numbers::PyLong_AsSsize_t(integer.as_ptr()))
        };
        if exception_pending(py) {
            Err(MoltObject::none().bits())
        } else {
            Ok(result)
        }
    }
}

fn float_value(py: &PyToken, error: u64, bits: u64) -> Result<f64, u64> {
    unsafe {
        let object = native_object(bits);
        if object.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        let value =
            retire_native_reference(|| abi::api::numbers::PyFloat_AsDouble(object.as_ptr()));
        if exception_pending(py) {
            // CPython's struct float converters deliberately replace conversion errors.
            abi::api::errors::PyErr_Clear();
            return Err(struct_error(py, error, "required argument is not a float"));
        }
        Ok(value)
    }
}

fn pack_values(
    py: &PyToken,
    error: u64,
    plan: &StructPlan,
    values: &[u64],
    data: *mut u8,
) -> Result<(), u64> {
    if plan.size != 0 {
        unsafe { std::ptr::write_bytes(data, 0, plan.size) };
    }
    let mut out = StructWriter {
        data,
        len: plan.size,
        position: 0,
    };
    let mut index = 0;
    for op in &plan.fields {
        out.position = op.offset;
        for _ in 0..op.count {
            let bits = values[index];
            index += 1;
            let obj = obj_from_bits(bits);
            match op.kind {
                StructKind::Bytes | StructKind::Pascal | StructKind::Char => {
                    let Some(ptr) = obj.as_ptr() else {
                        return Err(struct_error(
                            py,
                            error,
                            if matches!(op.kind, StructKind::Char) {
                                "char format requires a bytes object of length 1"
                            } else if op.code == 'p' {
                                "argument for 'p' must be a bytes object"
                            } else {
                                "argument for 's' must be a bytes object"
                            },
                        ));
                    };
                    let kind = unsafe { object_type_id(ptr) };
                    let is_char = matches!(op.kind, StructKind::Char);
                    if kind != TYPE_ID_BYTES && (is_char || kind != TYPE_ID_BYTEARRAY) {
                        return Err(struct_error(
                            py,
                            error,
                            &if is_char {
                                "char format requires a bytes object of length 1".to_string()
                            } else {
                                format!("argument for '{}' must be a bytes object", op.code)
                            },
                        ));
                    }
                    // Physical admission above guarantees these hooks cannot call Python.
                    // Keep only raw coordinates: an 's'/'p' source may be the destination.
                    let mut length = 0;
                    let hooks = abi::hooks::hooks_or_stubs();
                    let source = unsafe {
                        if kind == TYPE_ID_BYTES {
                            (hooks.bytes_data)(bits, &mut length)
                        } else {
                            (hooks.bytearray_data)(bits, &mut length).cast_const()
                        }
                    };
                    if is_char {
                        if length != 1 {
                            return Err(struct_error(
                                py,
                                error,
                                "char format requires a bytes object of length 1",
                            ));
                        }
                        out.push(unsafe { source.read() });
                    } else if matches!(op.kind, StructKind::Pascal) {
                        if op.size != 0 {
                            let n = length.min(op.size - 1);
                            if n != 0 {
                                unsafe {
                                    std::ptr::copy(source, data.add(out.position + 1), n);
                                }
                            }
                            unsafe {
                                data.add(out.position).write(n.min(255) as u8);
                            }
                        }
                        out.position += op.size;
                    } else {
                        let n = length.min(op.size);
                        if n != 0 {
                            unsafe {
                                std::ptr::copy(source, data.add(out.position), n);
                            }
                        }
                        out.position += op.size;
                    }
                }
                StructKind::Bool => {
                    let value = is_truthy(py, obj);
                    if exception_pending(py) {
                        return Err(MoltObject::none().bits());
                    }
                    out.push(u8::from(value));
                }
                StructKind::Int { signed } => {
                    if op.code == 'P' {
                        let raw = pointer_value(py, error, bits)?;
                        push_unsigned(&mut out, plan.format.endian, op.size, raw as u128)
                            .expect("validated pointer width");
                        continue;
                    }
                    let width = op.size * 8;
                    let value = match index_value(py, error, bits) {
                        Ok(value) => value,
                        Err(bits) => {
                            // Narrow/native converters replace even callback OverflowError;
                            // standard q/Q and native P return get_pylong failures unchanged.
                            let replaces_overflow = op.code != 'P'
                                && (plan.format.native_sizes || !matches!(op.code, 'q' | 'Q'));
                            if replaces_overflow
                                && unsafe {
                                    abi::api::errors::PyErr_ExceptionMatches(
                                        (&raw mut abi::abi_types::PyExc_OverflowError).cast(),
                                    ) != 0
                                }
                            {
                                unsafe {
                                    abi::api::errors::PyErr_Clear();
                                }
                                let message = if signed {
                                    signed_range_message(op.code, width)
                                } else {
                                    unsigned_range_message(op.code, width)
                                };
                                return Err(struct_error(py, error, &message));
                            }
                            return Err(bits);
                        }
                    };
                    let (min, max) = if signed {
                        signed_range(width)
                    } else {
                        (0, unsigned_max(width))
                    };
                    let Some(value) = value.filter(|value| (min..=max).contains(value)) else {
                        let message = if signed {
                            signed_range_message(op.code, width)
                        } else {
                            unsigned_range_message(op.code, width)
                        };
                        return Err(struct_error(py, error, &message));
                    };
                    push_unsigned(&mut out, plan.format.endian, op.size, value as u128)
                        .expect("validated integer width");
                }
                StructKind::Float => {
                    let value = float_value(py, error, bits)?;
                    let (raw, size) = match op.size {
                        2 => (
                            u64::from(f16_from_f64(value).map_err(|_| {
                                raise_exception::<u64>(
                                    py,
                                    BuiltinExc::OverflowError,
                                    "float too large to pack with e format",
                                )
                            })?),
                            2,
                        ),
                        4 => {
                            let bits = if plan.format.native_sizes {
                                (value as f32).to_bits()
                            } else {
                                molt_obj_model::float_bits::f64_to_f32_bits(value).map_err(
                                    |_| {
                                        raise_exception::<u64>(
                                            py,
                                            BuiltinExc::OverflowError,
                                            "float too large to pack with f format",
                                        )
                                    },
                                )?
                            };
                            (u64::from(bits), 4)
                        }
                        8 => (value.to_bits(), 8),
                        _ => unreachable!("validated float width"),
                    };
                    push_unsigned(&mut out, plan.format.endian, size, u128::from(raw))
                        .expect("validated float width");
                }
                StructKind::Pad => unreachable!(),
            }
        }
    }
    Ok(())
}

fn packing_arguments<'py>(
    py: &'py PyToken,
    error: u64,
    plan: &StructPlan,
    values: u64,
    operation: &str,
) -> Result<OwnedBridgeHandleSnapshot<'py>, u64> {
    let Some(ptr) = obj_from_bits(values).as_ptr() else {
        return Err(struct_error(py, error, "pack expects a tuple"));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return Err(struct_error(py, error, "pack expects a tuple"));
    }
    let values = unsafe { seq_snapshot(py, ptr) };
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    if values.len() != plan.values {
        return Err(struct_error(
            py,
            error,
            &format!(
                "{operation} expected {} items for packing (got {})",
                plan.values,
                values.len()
            ),
        ));
    }
    Ok(values)
}

fn unpack_record(py: &PyToken, error: Option<u64>, plan: &StructPlan, data: *const u8) -> u64 {
    let values = match struct_unpack_values(py, plan, data) {
        Ok(values) => values,
        Err((kind, message)) => {
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let Some(kind) = kind else {
                return MoltObject::none().bits();
            };
            return match error {
                Some(error) => struct_error(py, error, &message),
                None => raise_exception::<u64>(py, kind, &message),
            };
        }
    };
    let result = alloc_tuple(py, values.as_bits_slice());
    if result.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(result).bits()
    }
}

fn checked_offset(
    py: &PyToken,
    error: u64,
    raw: isize,
    len: usize,
    size: usize,
    packing: bool,
) -> Result<usize, u64> {
    let op = if packing { "pack_into" } else { "unpack_from" };
    let action = if packing { "packing" } else { "unpacking" };
    let n = raw as i128;
    if n < 0 && n + size as i128 > 0 {
        let msg = if packing {
            format!("no space to pack {size} bytes at offset {raw}")
        } else {
            format!("not enough data to unpack {size} bytes at offset {raw}")
        };
        return Err(struct_error(py, error, &msg));
    }
    let start = if n < 0 { n + len as i128 } else { n };
    if start < 0 {
        return Err(struct_error(
            py,
            error,
            &format!("offset {raw} out of range for {len}-byte buffer"),
        ));
    }
    let start = start as usize;
    if start > len || size > len - start {
        return Err(struct_error(
            py,
            error,
            &format!(
                "{op} requires a buffer of at least {} bytes for {action} {size} bytes at offset {raw} (actual buffer size is {len})",
                start as u128 + size as u128
            ),
        ));
    }
    Ok(start)
}

pub fn molt_struct_calcsize_attached_body(
    _py: &molt_runtime_core::PyToken,
    error: u64,
    format: u64,
) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, py, {
        match plan(py, error, format) {
            Ok(plan) => int_bits_from_i128(py, plan.size as i128),
            Err(bits) => bits,
        }
    })
}

pub fn molt_struct_pack_attached_body(
    _py: &molt_runtime_core::PyToken,
    error: u64,
    format: u64,
    values: u64,
) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        let values = match packing_arguments(py, error, &plan, values, "pack") {
            Ok(values) => values,
            Err(bits) => return bits,
        };
        let mut output = Vec::new();
        if output.try_reserve_exact(plan.size).is_err() {
            return raise_exception::<u64>(
                py,
                BuiltinExc::MemoryError,
                "struct output allocation failed",
            );
        }
        output.resize(plan.size, 0);
        if let Err(bits) = pack_values(py, error, &plan, &values, output.as_mut_ptr()) {
            return bits;
        }
        let result = alloc_bytes(py, &output);
        if result.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(result).bits()
        }
    })
}

pub fn molt_struct_pack_into_attached_body(
    _py: &molt_runtime_core::PyToken,
    error: u64,
    format: u64,
    buffer: u64,
    offset: u64,
    values: u64,
) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        let acquire = || match unsafe { acquire_buffer(buffer, PyBUF_WRITABLE) } {
            Ok(lease) => Ok(lease),
            Err(BufferAcquireError::Exporter) => {
                unsafe {
                    abi::api::errors::PyErr_Clear();
                }
                Err(raise_exception::<u64>(
                    py,
                    BuiltinExc::TypeError,
                    &format!(
                        "argument must be read-write bytes-like object, not {}",
                        if obj_from_bits(buffer).is_none() {
                            "None".into()
                        } else {
                            type_name(py, obj_from_bits(buffer))
                        }
                    ),
                ))
            }
            Err(BufferAcquireError::Preparation) => Err(MoltObject::none().bits()),
        };
        // From 3.15 Argument Clinic acquires the buffer and converts the
        // offset before the item count is checked (Modules/clinic/_struct.c.h);
        // through 3.14 the count came first.
        let clinic_first = molt_runtime_core::rt_target_at_least(py, 3, 15);
        let mut converted = None;
        if clinic_first {
            let lease = match acquire() {
                Ok(lease) => lease,
                Err(bits) => return bits,
            };
            let offset = match index_offset(py, offset, true) {
                Ok(offset) => offset,
                Err(bits) => return bits,
            };
            converted = Some((lease, offset));
        }
        let values = match packing_arguments(py, error, &plan, values, "pack_into") {
            Ok(values) => values,
            Err(bits) => return bits,
        };
        let (lease, offset) = match converted {
            Some(converted) => converted,
            None => {
                let lease = match acquire() {
                    Ok(lease) => lease,
                    Err(bits) => return bits,
                };
                match index_offset(py, offset, true) {
                    Ok(offset) => (lease, offset),
                    Err(bits) => return bits,
                }
            }
        };
        let view = unsafe { &*lease.descriptor() };
        let start = match checked_offset(py, error, offset, view.len as usize, plan.size, true) {
            Ok(start) => start,
            Err(bits) => return bits,
        };
        let data = if view.len == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            view.buf.cast::<u8>()
        };
        if let Err(bits) = pack_values(py, error, &plan, &values, unsafe { data.add(start) }) {
            return bits;
        }
        MoltObject::none().bits()
    })
}

pub fn molt_struct_unpack_attached_body(
    _py: &molt_runtime_core::PyToken,
    error: u64,
    format: u64,
    buffer: u64,
) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        let lease = match unsafe { acquire_buffer(buffer, PyBUF_SIMPLE) } {
            Ok(lease) => lease,
            Err(_) => return MoltObject::none().bits(),
        };
        let view = unsafe { &*lease.descriptor() };
        if view.len as usize != plan.size {
            return struct_error(
                py,
                error,
                &format!("unpack requires a buffer of {} bytes", plan.size),
            );
        }
        let data = if view.len == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            view.buf.cast::<u8>()
        };
        unpack_record(py, Some(error), &plan, data)
    })
}

pub fn molt_struct_unpack_from_attached_body(
    _py: &molt_runtime_core::PyToken,
    error: u64,
    format: u64,
    buffer: u64,
    offset: u64,
) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        let lease = match unsafe { acquire_buffer(buffer, PyBUF_SIMPLE) } {
            Ok(lease) => lease,
            Err(_) => return MoltObject::none().bits(),
        };
        let offset = match index_offset(py, offset, false) {
            Ok(offset) => offset,
            Err(bits) => return bits,
        };
        let view = unsafe { &*lease.descriptor() };
        let start = match checked_offset(py, error, offset, view.len as usize, plan.size, false) {
            Ok(start) => start,
            Err(bits) => return bits,
        };
        let data = if view.len == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            view.buf.cast::<u8>()
        };
        unpack_record(py, Some(error), &plan, unsafe { data.add(start) })
    })
}

// A normal native GC object, not an opaque runtime handle. Its only Python
// edge is the acquired export; the parsed plan contains no Python references.
#[repr(C)]
struct UnpackIterator {
    base: PyObject,
    state: *mut UnpackState,
}
struct UnpackState {
    plan: Arc<StructPlan>,
    lease: RuntimeBufferLease,
    index: usize,
}

unsafe extern "C" fn unpack_iter_clear(object: *mut PyObject) -> c_int {
    // Detach before release callbacks can reenter this iterator.
    let state = unsafe {
        std::ptr::replace(
            &mut (*object.cast::<UnpackIterator>()).state,
            std::ptr::null_mut(),
        )
    };
    if !state.is_null() {
        unsafe {
            drop(Box::from_raw(state));
        }
    }
    0
}

unsafe extern "C" fn unpack_iter_dealloc(object: *mut PyObject) {
    unsafe {
        let class = (*object).ob_type;
        abi::api::memory::PyObject_GC_UnTrack(object.cast());
        unpack_iter_clear(object);
        abi::api::memory::PyObject_GC_Del(object.cast());
        abi::api::refcount::Py_DECREF(class.cast());
    }
}

unsafe extern "C" fn unpack_iter_traverse(
    object: *mut PyObject,
    visit: *mut c_void,
    argument: *mut c_void,
) -> c_int {
    let visit: unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int =
        unsafe { std::mem::transmute(visit) };
    let result = unsafe { visit((*object).ob_type.cast(), argument) };
    if result != 0 {
        return result;
    }
    let state = unsafe { (*object.cast::<UnpackIterator>()).state };
    if state.is_null() {
        return 0;
    }
    let owner = unsafe { (*state).lease.owner() };
    if owner.is_null() {
        0
    } else {
        unsafe { visit(owner, argument) }
    }
}

unsafe extern "C" fn unpack_iter_hint(
    object: *mut PyObject,
    _unused: *mut PyObject,
) -> *mut PyObject {
    let state = unsafe { (*object.cast::<UnpackIterator>()).state };
    let remaining = if state.is_null() {
        0
    } else {
        unsafe {
            let state = &*state;
            ((*state.lease.descriptor()).len - state.index as isize) / state.plan.size as isize
        }
    };
    unsafe { abi::api::numbers::PyLong_FromSsize_t(remaining) }
}

unsafe extern "C" fn unpack_iter_next(object: *mut PyObject) -> *mut PyObject {
    molt_runtime_core::with_gil_entry!(py, {
        let state = unsafe { (*object.cast::<UnpackIterator>()).state };
        if state.is_null() {
            return std::ptr::null_mut();
        }
        let (plan, lease, index) = unsafe {
            let state = &*state;
            if state.index >= (*state.lease.descriptor()).len as usize {
                unpack_iter_clear(object);
                return std::ptr::null_mut();
            }
            // Allocation/GC can clear the published state. Local owners preserve
            // the same parsed plan and export without retaining a Rust borrow.
            (Arc::clone(&state.plan), state.lease.clone(), state.index)
        };
        let data = unsafe { (*lease.descriptor()).buf.cast::<u8>().add(index) };
        let result =
            unsafe { OwnedRuntimeValue::from_owned_bits(py, unpack_record(py, None, &plan, data)) };
        let current = unsafe { (*object.cast::<UnpackIterator>()).state };
        if !current.is_null() {
            unsafe {
                (*current).index += plan.size;
            }
        }
        drop(lease);
        drop(plan);
        if exception_pending(py) {
            drop(result);
            return unsafe {
                abi::bridge::GLOBAL_BRIDGE
                    .owned_result_to_pyobj(abi::hooks::OwnedHandleResult::error())
            };
        }
        unsafe { abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(result.into_bits()) }
    })
}

static mut UNPACK_ITER_METHODS: [abi::abi_types::PyMethodDef; 2] = [
    abi::abi_types::PyMethodDef {
        ml_name: c"__length_hint__".as_ptr(),
        ml_meth: Some(unpack_iter_hint),
        ml_flags: abi::abi_types::METH_NOARGS,
        ml_doc: std::ptr::null(),
    },
    abi::abi_types::PyMethodDef {
        ml_name: std::ptr::null(),
        ml_meth: None,
        ml_flags: 0,
        ml_doc: std::ptr::null(),
    },
];

pub fn molt_struct_unpack_iterator_type_attached_body(_py: &molt_runtime_core::PyToken) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, py, {
        use abi::type_slots::*;
        let mut slots = [
            abi::abi_types::PyType_Slot {
                slot: Py_tp_dealloc,
                pfunc: unpack_iter_dealloc as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_clear,
                pfunc: unpack_iter_clear as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_traverse,
                pfunc: unpack_iter_traverse as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_iter,
                pfunc: abi::api::object::PyObject_SelfIter as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_iternext,
                pfunc: unpack_iter_next as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_getattro,
                pfunc: abi::api::object::PyObject_GenericGetAttr as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_methods,
                pfunc: (&raw mut UNPACK_ITER_METHODS).cast(),
            },
            abi::abi_types::PyType_Slot {
                slot: 0,
                pfunc: std::ptr::null_mut(),
            },
        ];
        // Py_TPFLAGS_* are C `unsigned long` and PyType_Spec.flags is C
        // `unsigned int`; the cast narrows on LP64 and is the identity on
        // LLP64 and wasm32.
        #[allow(
            clippy::unnecessary_cast,
            reason = "c_ulong is u64 on LP64 but u32 on LLP64 and wasm32"
        )]
        let flags = (abi::abi_types::Py_TPFLAGS_HAVE_GC
            | abi::abi_types::Py_TPFLAGS_IMMUTABLETYPE
            | abi::abi_types::Py_TPFLAGS_DISALLOW_INSTANTIATION) as u32;
        let mut spec = abi::abi_types::PyType_Spec {
            name: c"_struct.unpack_iterator".as_ptr(),
            basicsize: size_of::<UnpackIterator>() as c_int,
            itemsize: 0,
            flags,
            slots: slots.as_mut_ptr(),
        };
        unsafe {
            let class =
                NativeObjectOwner::from_owned(abi::api::typeobj::PyType_FromSpec(&mut spec));
            if class.as_ptr().is_null() {
                return MoltObject::none().bits();
            }
            match abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(class.as_ptr()) {
                Some(bits) => bits,
                None => {
                    let _ = py;
                    MoltObject::none().bits()
                }
            }
        }
    })
}

unsafe fn admitted_iterator_class(object: *mut PyObject) -> bool {
    unsafe {
        if abi::bridge::molt_capi_pyobj_is_bridge_managed(object) != 0
            || abi::api::typeobj::PyType_Check(object) == 0
        {
            return false;
        }
        let class = object.cast::<PyTypeObject>();
        let required = abi::abi_types::Py_TPFLAGS_HAVE_GC
            | abi::abi_types::Py_TPFLAGS_IMMUTABLETYPE
            | abi::abi_types::Py_TPFLAGS_DISALLOW_INSTANTIATION;
        (*class).tp_flags & required == required
            && (*class).tp_flags & abi::abi_types::Py_TPFLAGS_BASETYPE == 0
            && (*class).tp_basicsize == size_of::<UnpackIterator>() as isize
            && (*class).tp_itemsize == 0
            && (*class).tp_dictoffset == 0
            && (*class).tp_weaklistoffset == 0
            && (*class).tp_flags
                & (abi::abi_types::Py_TPFLAGS_MANAGED_DICT
                    | abi::abi_types::Py_TPFLAGS_MANAGED_WEAKREF)
                == 0
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_dealloc)
                == unpack_iter_dealloc as *mut c_void
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_clear)
                == unpack_iter_clear as *mut c_void
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_traverse)
                == unpack_iter_traverse as *mut c_void
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_iter)
                == abi::api::object::PyObject_SelfIter as *mut c_void
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_iternext)
                == unpack_iter_next as *mut c_void
    }
}

pub fn molt_struct_iter_unpack_attached_body(
    _py: &molt_runtime_core::PyToken,
    error: u64,
    class: u64,
    format: u64,
    buffer: u64,
) -> u64 {
    molt_runtime_core::with_attached_entry!(_py, py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        if plan.size == 0 {
            return struct_error(
                py,
                error,
                "cannot iteratively unpack with a struct of length 0",
            );
        }
        unsafe {
            let class = native_object(class);
            if class.as_ptr().is_null() {
                return MoltObject::none().bits();
            }
            if !admitted_iterator_class(class.as_ptr()) {
                return raise_exception::<u64>(
                    py,
                    BuiltinExc::TypeError,
                    "invalid struct iterator declaration",
                );
            }
            let iterator = NativeObjectOwner::from_owned(abi::api::typeobj::PyType_GenericAlloc(
                class.as_ptr().cast(),
                0,
            ));
            if iterator.as_ptr().is_null() {
                return MoltObject::none().bits();
            }
            let lease = match acquire_buffer(buffer, PyBUF_SIMPLE) {
                Ok(lease) => lease,
                Err(_) => return MoltObject::none().bits(),
            };
            if !((*lease.descriptor()).len as usize).is_multiple_of(plan.size) {
                return struct_error(
                    py,
                    error,
                    &format!(
                        "iterative unpacking requires a buffer of a multiple of {} bytes",
                        plan.size
                    ),
                );
            }
            let state = Box::new(UnpackState {
                plan,
                lease,
                index: 0,
            });
            (*iterator.as_ptr().cast::<UnpackIterator>()).state = Box::into_raw(state);
            abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(iterator.as_ptr())
                .unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

include!("attached_exports_generated/structs.rs");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_plan_keeps_zero_count_alignment_and_bounds_large_repeats() {
        let plan = compile_format("@b0qb2h0s").unwrap();
        let alignment = align_of::<c_longlong>();
        assert_eq!(plan.size, alignment + 6);
        assert_eq!(plan.values, 5);
        assert_eq!(
            plan.fields
                .iter()
                .map(|field| (field.offset, field.count, field.size))
                .collect::<Vec<_>>(),
            [
                (0, 1, 1),
                (alignment, 1, 1),
                (alignment + 2, 2, 2),
                (alignment + 6, 1, 0)
            ]
        );
        assert_eq!(
            compile_format("<1000000000i").unwrap().values,
            1_000_000_000
        );
        let overflow = format!("<{}q", isize::MAX);
        assert_eq!(
            compile_format(&overflow).err().as_deref(),
            Some("total struct size too long")
        );
    }

    #[test]
    fn format_cache_shares_layout_by_content_and_bounds_churn() {
        let first = cached_plan("<hi").unwrap();
        let same_text = String::from("<hi");
        assert!(Arc::ptr_eq(&first, &cached_plan(&same_text).unwrap()));
        assert!(cached_plan("<nope").is_err());
        for count in 1..=101 {
            cached_plan(&format!("<{count}h")).unwrap();
        }
        assert!(!Arc::ptr_eq(&first, &cached_plan(&same_text).unwrap()));
        assert_eq!(first.size, 6);
    }

    #[test]
    fn struct_e_and_f_reject_finite_overflow() {
        assert!(f16_from_f64(65_520.0).is_err());
        assert!(molt_obj_model::float_bits::f64_to_f32_bits(f64::MAX).is_err());
    }

    #[test]
    fn struct_e_uses_direct_f64_ties_to_even_authority() {
        assert_eq!(f16_from_f64(1.0 + 2f64.powi(-11)), Ok(0x3c00));
        assert_eq!(f16_from_f64(1.0 + 3.0 * 2f64.powi(-11)), Ok(0x3c02));
    }
}
