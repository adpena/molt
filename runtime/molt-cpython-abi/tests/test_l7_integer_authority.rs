#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{PyLong_Type, PyLongObject, PyLongValue, PyObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::{
    INT_BYTES_INVALID, INT_BYTES_NEGATIVE_UNSIGNED, INT_BYTES_OK, INT_BYTES_OVERFLOW,
    NumberBinaryOp, NumberUnaryOp, STUB_HOOKS,
};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::sync::{Mutex, OnceLock};

static VALUES: OnceLock<Mutex<HashMap<u64, i128>>> = OnceLock::new();
static STRINGS: OnceLock<Mutex<HashMap<u64, Box<[u8]>>>> = OnceLock::new();
static BYTES: OnceLock<Mutex<HashMap<u64, Box<[u8]>>>> = OnceLock::new();
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn values() -> &'static Mutex<HashMap<u64, i128>> {
    VALUES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn strings() -> &'static Mutex<HashMap<u64, Box<[u8]>>> {
    STRINGS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn byte_values() -> &'static Mutex<HashMap<u64, Box<[u8]>>> {
    BYTES.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe extern "C" fn alloc_text(data: *const u8, len: usize) -> u64 {
    let value = if len == 0 {
        Box::<[u8]>::default()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
            .to_vec()
            .into_boxed_slice()
    };
    let bits = MoltObject::from_ptr(Box::into_raw(Box::new(0u8))).bits();
    strings().lock().unwrap().insert(bits, value);
    bits
}

unsafe extern "C" fn alloc_byte_value(data: *const u8, len: usize) -> u64 {
    let value = if len == 0 {
        Box::<[u8]>::default()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
            .to_vec()
            .into_boxed_slice()
    };
    let bits = MoltObject::from_ptr(Box::into_raw(Box::new(0u8))).bits();
    byte_values().lock().unwrap().insert(bits, value);
    bits
}

unsafe extern "C" fn str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let values = strings().lock().unwrap();
    let Some(value) = values.get(&bits) else {
        return std::ptr::null();
    };
    unsafe { *out_len = value.len() };
    value.as_ptr()
}

unsafe extern "C" fn bytes_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let values = byte_values().lock().unwrap();
    let Some(value) = values.get(&bits) else {
        return std::ptr::null();
    };
    unsafe { *out_len = value.len() };
    value.as_ptr()
}

fn value_for_bits(bits: u64) -> Option<i128> {
    MoltObject::from_bits(bits)
        .as_int()
        .map(i128::from)
        .or_else(|| values().lock().unwrap().get(&bits).copied())
}

fn bits_for_value(value: i128) -> u64 {
    if let Ok(value) = i64::try_from(value)
        && let Some(obj) = MoltObject::try_from_int(value)
    {
        return obj.bits();
    }
    let ptr = Box::into_raw(Box::new(0u8));
    let bits = MoltObject::from_ptr(ptr).bits();
    values().lock().unwrap().insert(bits, value);
    bits
}

unsafe extern "C" fn int_from_i64(value: i64) -> u64 {
    bits_for_value(value as i128)
}

unsafe extern "C" fn int_from_u64(value: u64) -> u64 {
    bits_for_value(value as i128)
}

unsafe extern "C" fn classify(bits: u64) -> u8 {
    if values().lock().unwrap().contains_key(&bits) {
        molt_cpython_abi::abi_types::MoltTypeTag::Int as u8
    } else if strings().lock().unwrap().contains_key(&bits) {
        molt_cpython_abi::abi_types::MoltTypeTag::Str as u8
    } else if byte_values().lock().unwrap().contains_key(&bits) {
        molt_cpython_abi::abi_types::MoltTypeTag::Bytes as u8
    } else {
        molt_cpython_abi::abi_types::MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn as_i64(bits: u64, out: *mut i64) -> i32 {
    let Some(value) = value_for_bits(bits).and_then(|value| i64::try_from(value).ok()) else {
        return -1;
    };
    unsafe { *out = value };
    0
}

unsafe extern "C" fn as_u64(bits: u64, out: *mut u64) -> i32 {
    let Some(value) = value_for_bits(bits).and_then(|value| u64::try_from(value).ok()) else {
        return -1;
    };
    unsafe { *out = value };
    0
}

unsafe extern "C" fn as_u64_mask(bits: u64, width: u32, out: *mut u64) -> i32 {
    let Some(value) = value_for_bits(bits) else {
        return -1;
    };
    let mask = if width == 64 {
        u64::MAX as u128
    } else {
        (1u128 << width) - 1
    };
    unsafe { *out = (value as u128 & mask) as u64 };
    0
}

unsafe extern "C" fn binary(op: u32, a: u64, b: u64) -> u64 {
    let (Some(a), Some(b)) = (value_for_bits(a), value_for_bits(b)) else {
        return 0;
    };
    let value = match op {
        x if x == NumberBinaryOp::Add as u32 => a.checked_add(b),
        x if x == NumberBinaryOp::Multiply as u32 => a.checked_mul(b),
        x if x == NumberBinaryOp::Rshift as u32 => u32::try_from(b).ok().map(|shift| {
            if shift >= i128::BITS {
                if a < 0 { -1 } else { 0 }
            } else {
                a >> shift
            }
        }),
        _ => None,
    };
    value.map(bits_for_value).unwrap_or(0)
}

unsafe extern "C" fn unary(op: u32, value: u64) -> u64 {
    if op != NumberUnaryOp::Negative as u32 {
        return 0;
    }
    value_for_bits(value)
        .and_then(i128::checked_neg)
        .map(bits_for_value)
        .unwrap_or(0)
}

unsafe extern "C" fn from_bytes(data: *const u8, len: usize, little: i32, signed: i32) -> u64 {
    if data.is_null() && len != 0 || len > 16 {
        return 0;
    }
    let input = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let mut raw = [0u8; 16];
    if little != 0 {
        raw[..len].copy_from_slice(input);
    } else {
        for (dst, src) in raw.iter_mut().zip(input.iter().rev()) {
            *dst = *src;
        }
    }
    if signed != 0 && len != 0 {
        let sign = if little != 0 {
            input[len - 1]
        } else {
            input[0]
        } & 0x80
            != 0;
        if sign {
            raw[len..].fill(0xff);
        }
    }
    bits_for_value(i128::from_le_bytes(raw))
}

unsafe extern "C" fn to_bytes(
    bits: u64,
    data: *mut u8,
    len: usize,
    little: i32,
    signed: i32,
) -> i32 {
    let Some(value) = value_for_bits(bits) else {
        return INT_BYTES_INVALID;
    };
    if value < 0 && signed == 0 {
        return INT_BYTES_NEGATIVE_UNSIGNED;
    }
    if data.is_null() && len != 0 {
        return INT_BYTES_INVALID;
    }
    let raw = value.to_le_bytes();
    let out = if len == 0 {
        &mut [][..]
    } else {
        unsafe { std::slice::from_raw_parts_mut(data, len) }
    };
    for (index, byte) in out.iter_mut().enumerate() {
        let source = if little != 0 { index } else { len - 1 - index };
        *byte = raw
            .get(source)
            .copied()
            .unwrap_or(if value < 0 { 0xff } else { 0 });
    }
    let width = len * 8;
    let fits = if signed != 0 {
        width >= 128
            || (width != 0 && value >= -(1i128 << (width - 1)) && value < (1i128 << (width - 1)))
            || (width == 0 && value == 0)
    } else {
        width >= 128 || (width != 0 && (value as u128) < (1u128 << width)) || value == 0
    };
    if fits {
        INT_BYTES_OK
    } else {
        INT_BYTES_OVERFLOW
    }
}

unsafe extern "C" fn num_bits(bits: u64, out: *mut usize) -> i32 {
    let Some(value) = value_for_bits(bits) else {
        return -1;
    };
    unsafe { *out = (u128::BITS - value.unsigned_abs().leading_zeros()) as usize };
    0
}

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    let mut hooks = STUB_HOOKS;
    hooks.int_from_i64 = int_from_i64;
    hooks.int_from_u64 = int_from_u64;
    hooks.int_as_i64_checked = as_i64;
    hooks.int_as_u64_checked = as_u64;
    hooks.int_as_u64_mask = as_u64_mask;
    hooks.int_from_bytes = from_bytes;
    hooks.int_to_bytes = to_bytes;
    hooks.int_num_bits = num_bits;
    hooks.number_binary_op = binary;
    hooks.number_unary_op = unary;
    hooks.classify_heap = classify;
    hooks.alloc_str = alloc_text;
    hooks.alloc_bytes = alloc_byte_value;
    hooks.str_data = str_data;
    hooks.bytes_data = bytes_data;
    unsafe {
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

fn proxy(value: i128) -> *mut PyObject {
    unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits_for_value(value)) }
}

fn clear_error() {
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn from_string_has_shared_prefix_underscore_base_zero_and_pend_semantics() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    let mut end: *mut c_char = std::ptr::null_mut();
    let source = c"  -0x_FF  ";
    let value = unsafe {
        molt_cpython_abi::api::numbers::PyLong_FromString(source.as_ptr(), &raw mut end, 0)
    };
    assert!(!value.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(value) },
        -255
    );
    assert_eq!(unsafe { end.offset_from(source.as_ptr()) }, 10);

    let zero = unsafe {
        molt_cpython_abi::api::numbers::PyLong_FromString(c"00_0".as_ptr(), &raw mut end, 0)
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(zero) },
        0
    );

    for (literal, base, offset) in [
        (c"010", 0, 3),
        (c"1__2", 10, 1),
        (c"0x_", 0, 3),
        (c"123  junk", 10, 5),
    ] {
        end = std::ptr::null_mut();
        let value = unsafe {
            molt_cpython_abi::api::numbers::PyLong_FromString(literal.as_ptr(), &raw mut end, base)
        };
        assert!(value.is_null());
        assert_eq!(unsafe { end.offset_from(literal.as_ptr()) }, offset);
        clear_error();
    }

    let huge = c"1208925819614629174706176"; // 2**80
    let value = unsafe {
        molt_cpython_abi::api::numbers::PyLong_FromString(huge.as_ptr(), &raw mut end, 10)
    };
    assert!(!value.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_PyLong_NumBits(value) },
        81
    );
    end = source.as_ptr().cast_mut();
    let sentinel = end;
    assert!(
        unsafe {
            molt_cpython_abi::api::numbers::PyLong_FromString(c"10".as_ptr(), &raw mut end, 1)
        }
        .is_null()
    );
    assert_eq!(end, sentinel, "invalid base must leave pend untouched");
    clear_error();

    let decimal = std::ffi::CString::new("9".repeat(4301)).unwrap();
    end = source.as_ptr().cast_mut();
    let limit_sentinel = end;
    assert!(
        unsafe {
            molt_cpython_abi::api::numbers::PyLong_FromString(decimal.as_ptr(), &raw mut end, 10)
        }
        .is_null()
    );
    assert_eq!(
        end, limit_sentinel,
        "digit-limit failure leaves pend untouched"
    );
    clear_error();
}

#[test]
fn unicode_integer_transform_accepts_decimal_digits_and_space_but_rejects_bytes() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    let source = std::ffi::CString::new("\u{2003}\u{0661}\u{0662}\u{ff13}\u{3000}").unwrap();
    let unicode = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(source.as_ptr()) };
    assert!(!unicode.is_null());
    let value = unsafe { molt_cpython_abi::api::numbers::PyLong_FromUnicodeObject(unicode, 10) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(value) },
        123
    );

    let raw = b"123";
    let bytes = unsafe {
        molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(
            raw.as_ptr().cast(),
            raw.len() as isize,
        )
    };
    assert!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_FromUnicodeObject(bytes, 10) }.is_null()
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    clear_error();
}

#[test]
fn from_ssize_t_preserves_llp64_width() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    for value in [1isize << 40, -(1isize << 40)] {
        let obj = unsafe { molt_cpython_abi::api::numbers::PyLong_FromSsize_t(value) };
        assert!(!obj.is_null());
        assert_eq!(
            unsafe { molt_cpython_abi::api::numbers::PyLong_AsSsize_t(obj) },
            value
        );
    }
}

#[test]
fn compact_and_num_bits_cover_bridge_and_foreign_layouts_without_proxy_dereference() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    for (value, expected) in [
        ((1i128 << 30) - 1, 1),
        (1i128 << 30, 0),
        (-((1i128 << 30) - 1), 1),
        (-(1i128 << 30), 0),
    ] {
        let obj = proxy(value);
        assert_eq!(
            unsafe { molt_cpython_abi::api::numbers::PyUnstable_Long_IsCompact(obj.cast()) },
            expected
        );
    }

    let mut foreign = PyLongObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyLong_Type,
        },
        long_value: PyLongValue {
            lv_tag: (1 << 3) | 2,
            ob_digit: [123],
        },
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyUnstable_Long_IsCompact(&raw mut foreign) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyUnstable_Long_CompactValue(&raw mut foreign) },
        -123
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::_PyLong_NumBits((&raw mut foreign).cast::<PyObject>())
        },
        7
    );
}

#[test]
fn byte_arrays_are_arbitrary_width_endian_signed_and_partial_fill_correct() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    let value = (1i128 << 80) | 0x1234_5678_9abc_def0;
    let obj = proxy(value);
    let mut short = [0xaa; 8];
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
                obj.cast(),
                short.as_mut_ptr(),
                short.len(),
                1,
                0,
            )
        },
        -1
    );
    assert_eq!(short, 0x1234_5678_9abc_def0u64.to_le_bytes());
    clear_error();

    let mut full = [0u8; 11];
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
                obj.cast(),
                full.as_mut_ptr(),
                full.len(),
                0,
                0,
            )
        },
        0
    );
    let parsed = unsafe {
        molt_cpython_abi::api::numbers::_PyLong_FromByteArray(full.as_ptr(), full.len(), 0, 0)
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_PyLong_NumBits(parsed) },
        81
    );
    let zero =
        unsafe { molt_cpython_abi::api::numbers::_PyLong_FromByteArray(std::ptr::null(), 0, 1, 0) };
    assert!(!zero.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(zero) },
        0
    );

    let negative = proxy(-129);
    let mut one = [0u8; 1];
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
                negative.cast(),
                one.as_mut_ptr(),
                1,
                1,
                1,
            )
        },
        -1
    );
    assert_eq!(one, [0x7f]);
    clear_error();

    #[repr(C)]
    struct ForeignLong3 {
        ob_base: PyObject,
        lv_tag: usize,
        digits: [u32; 3],
    }
    let mut foreign = ForeignLong3 {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyLong_Type,
        },
        lv_tag: 3 << 3,
        digits: [(1 << 30) - 1, 0, 1],
    };
    let foreign_ptr = (&raw mut foreign).cast::<PyLongObject>();
    let mut foreign_bytes = [0u8; 8];
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
                foreign_ptr,
                foreign_bytes.as_mut_ptr(),
                foreign_bytes.len(),
                1,
                1,
            )
        },
        0
    );
    let foreign_value = (1u64 << 60) | ((1u64 << 30) - 1);
    assert_eq!(foreign_bytes, foreign_value.to_le_bytes());
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_PyLong_NumBits(foreign_ptr.cast::<PyObject>()) },
        61
    );
}

#[test]
fn size_t_and_all_unsigned_converters_preserve_outputs_on_error() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsSize_t(proxy(42)) },
        42
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsSize_t(proxy(-1)) },
        usize::MAX
    );
    clear_error();

    macro_rules! check {
        ($func:ident, $ty:ty) => {{
            let mut out: $ty = 77;
            assert_eq!(
                unsafe { molt_cpython_abi::api::numbers::$func(proxy(42), (&raw mut out).cast()) },
                1
            );
            assert_eq!(out, 42);
            out = 77;
            assert_eq!(
                unsafe { molt_cpython_abi::api::numbers::$func(proxy(-1), (&raw mut out).cast()) },
                0
            );
            assert_eq!(out, 77);
            clear_error();
            out = 77;
            assert_eq!(
                unsafe {
                    molt_cpython_abi::api::numbers::$func(proxy(1i128 << 80), (&raw mut out).cast())
                },
                0
            );
            assert_eq!(out, 77);
            clear_error();
        }};
    }
    check!(_PyLong_Size_t_Converter, usize);
    check!(_PyLong_UnsignedShort_Converter, u16);
    check!(_PyLong_UnsignedInt_Converter, u32);
    check!(_PyLong_UnsignedLong_Converter, std::os::raw::c_ulong);
    check!(_PyLong_UnsignedLongLong_Converter, u64);

    let mut out = 0usize;
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::_PyLong_Size_t_Converter(
                proxy(1),
                (&raw mut out as *mut usize).cast::<c_void>(),
            )
        },
        1
    );
}
