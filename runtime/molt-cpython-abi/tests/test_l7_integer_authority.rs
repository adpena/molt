#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    _Py_FalseStruct, _Py_TrueStruct, Py_False, Py_True, Py_complex, PyLong_Type, PyLongObject,
    PyLongValue, PyObject,
};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::{
    BorrowedHandleResult, INT_BYTES_INVALID, INT_BYTES_NEGATIVE_UNSIGNED, INT_BYTES_OK,
    INT_BYTES_OVERFLOW, NumberBinaryOp, NumberUnaryOp, OwnedHandleResult, STUB_HOOKS,
};
use molt_lang_obj_model::MoltObject;
use std::ffi::c_char;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_LOCK: Mutex<()> = Mutex::new(());
static BINARY_CALLS: AtomicUsize = AtomicUsize::new(0);
static SYS_GET_CALLS: AtomicUsize = AtomicUsize::new(0);
static BYTE_DESTINATION: AtomicUsize = AtomicUsize::new(0);
static BYTE_CAPACITY: AtomicUsize = AtomicUsize::new(0);
static INTEGER_READ_CALLS: AtomicUsize = AtomicUsize::new(0);
static REJECT_BYTE_OUTPUT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "l7-test-probe")]
fn bits_for_tuple(items: Vec<u64>) -> u64 {
    support::fake_runtime::tuple_from_owned(items)
}

unsafe extern "C" fn sys_get_object(
    data: *const u8,
    len: usize,
    _policy: molt_cpython_abi::hooks::SysLookupPolicy,
) -> BorrowedHandleResult {
    let name = unsafe { std::slice::from_raw_parts(data, len) };
    if name == b"float_info" {
        SYS_GET_CALLS.fetch_add(1, Ordering::Relaxed);
        BorrowedHandleResult::ok(bits_for_value(42))
    } else {
        BorrowedHandleResult::missing()
    }
}

fn value_for_bits(bits: u64) -> Option<i128> {
    support::fake_runtime::integer_value(bits)
}

fn bits_for_value(value: i128) -> u64 {
    if let Ok(value) = i64::try_from(value)
        && let Some(obj) = MoltObject::try_from_int(value)
    {
        return obj.bits();
    }
    support::fake_runtime::heap_integer(value)
}

unsafe extern "C" fn int_from_i64(value: i64) -> u64 {
    bits_for_value(value as i128)
}

unsafe extern "C" fn int_from_u64(value: u64) -> u64 {
    bits_for_value(value as i128)
}

unsafe extern "C" fn binary(op: u32, mode: u32, a: u64, b: u64) -> OwnedHandleResult {
    assert_eq!(mode, 0, "ordinary numeric caller must not request mutation");
    BINARY_CALLS.fetch_add(1, Ordering::Relaxed);
    let (Some(a), Some(b)) = (value_for_bits(a), value_for_bits(b)) else {
        return OwnedHandleResult::error();
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
    value
        .map(bits_for_value)
        .map_or_else(OwnedHandleResult::error, OwnedHandleResult::ok)
}

unsafe extern "C" fn unary(op: u32, value: u64) -> OwnedHandleResult {
    if op != NumberUnaryOp::Negative as u32 {
        return OwnedHandleResult::error();
    }
    value_for_bits(value)
        .and_then(i128::checked_neg)
        .map(bits_for_value)
        .map_or_else(OwnedHandleResult::error, OwnedHandleResult::ok)
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

unsafe extern "C" fn from_digits(digits: *const u8, len: usize, base: u32, negative: i32) -> u64 {
    let digits = unsafe { std::slice::from_raw_parts(digits, len) };
    let mut value = 0i128;
    for &digit in digits {
        value = value
            .checked_mul(i128::from(base))
            .and_then(|value| value.checked_add(i128::from(digit)))
            .unwrap_or(0);
    }
    if negative != 0 {
        value = -value;
    }
    bits_for_value(value)
}

unsafe extern "C" fn from_f64_trunc(value: f64) -> u64 {
    bits_for_value(value.trunc() as i128)
}

unsafe extern "C" fn int_sign(bits: u64) -> i32 {
    INTEGER_READ_CALLS.fetch_add(1, Ordering::Relaxed);
    value_for_bits(bits).map_or(0, |value| value.signum() as i32)
}

unsafe extern "C" fn to_bytes(
    bits: u64,
    data: *mut u8,
    len: usize,
    little: i32,
    signed: i32,
) -> i32 {
    BYTE_DESTINATION.store(data as usize, Ordering::SeqCst);
    BYTE_CAPACITY.store(len, Ordering::SeqCst);
    if REJECT_BYTE_OUTPUT.load(Ordering::SeqCst) {
        return INT_BYTES_INVALID;
    }
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
            || (width == 0 && (value == 0 || value == -1))
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
    INTEGER_READ_CALLS.fetch_add(1, Ordering::Relaxed);
    let Some(value) = value_for_bits(bits) else {
        return -1;
    };
    unsafe { *out = (u128::BITS - value.unsigned_abs().leading_zeros()) as usize };
    0
}

fn init() -> support::AbiTestThreadStateTransaction {
    #[cfg(feature = "l7-test-probe")]
    molt_cpython_abi_test_support::link();
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    let mut hooks = STUB_HOOKS;
    support::fake_runtime::wire_sequences(&mut hooks);
    hooks.alloc_bytes = support::fake_runtime::alloc_bytes;
    hooks.bytes_data = support::fake_runtime::bytes_data;
    hooks.int_from_i64 = int_from_i64;
    hooks.int_from_u64 = int_from_u64;

    hooks.int_from_bytes = from_bytes;
    hooks.int_from_digits = from_digits;
    hooks.int_from_f64_trunc = from_f64_trunc;
    hooks.int_sign = int_sign;

    hooks.int_to_bytes = to_bytes;
    hooks.int_num_bits = num_bits;
    hooks.number_binary_op = binary;
    hooks.number_unary_op = unary;
    hooks.sys_get_object_borrowed = sys_get_object;
    hooks.complex_parts = support::fake_complex::parts;
    hooks.complex_from_doubles = support::fake_complex::from_doubles;
    hooks.object_hash = support::fake_complex::hash;
    support::enter_runtime_class_abi_test(hooks)
}

fn proxy(value: i128) -> *mut PyObject {
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits_for_value(value)) }
}

fn clear_error() {
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn from_string_has_shared_prefix_underscore_base_zero_and_pend_semantics() {
    let _guard = TEST_LOCK.lock().unwrap();
    let _abi_test = init();
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
    BINARY_CALLS.store(0, Ordering::Relaxed);
    let value = unsafe {
        molt_cpython_abi::api::numbers::PyLong_FromString(huge.as_ptr(), &raw mut end, 10)
    };
    assert!(!value.is_null());
    assert_eq!(
        BINARY_CALLS.load(Ordering::Relaxed),
        0,
        "literal construction must allocate once through int_from_digits, not leak Horner temporaries"
    );
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
    let _abi_test = init();
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
    let _abi_test = init();
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
    let _abi_test = init();
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
    let _abi_test = init();
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
    use molt_cpython_abi::api::{errors, numbers, refcount::OwnedPyObject};
    let _guard = TEST_LOCK.lock().unwrap();
    let _abi_test = init();
    let assert_error = |class: *mut PyObject, message: &str| {
        assert_eq!(unsafe { errors::PyErr_ExceptionMatches(class) }, 1);
        assert_eq!(support::take_current_error_text().as_deref(), Some(message));
    };
    let overflow = (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast();
    let value_error = (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast();
    // Pinned CPython longobject.c checks sign before the native-width limit.
    for value in [-1, -(1i128 << 80), i128::MIN] {
        let op = proxy(value);
        let _owner = unsafe { OwnedPyObject::from_owned(op) };
        assert_eq!(unsafe { numbers::PyLong_AsSize_t(op) }, usize::MAX);
        assert_error(overflow, "can't convert negative value to size_t");
        assert_eq!(
            unsafe { numbers::PyLong_AsUnsignedLong(op) },
            std::os::raw::c_ulong::MAX
        );
        assert_error(overflow, "can't convert negative value to unsigned int");
        assert_eq!(unsafe { numbers::PyLong_AsUnsignedLongLong(op) }, u64::MAX);
        assert_error(overflow, "can't convert negative int to unsigned");
    }
    let huge = proxy(1i128 << 80);
    let _huge_owner = unsafe { OwnedPyObject::from_owned(huge) };
    assert_eq!(unsafe { numbers::PyLong_AsSize_t(huge) }, usize::MAX);
    assert_error(overflow, "Python int too large to convert to C size_t");
    assert_eq!(
        unsafe { numbers::PyLong_AsUnsignedLong(huge) },
        std::os::raw::c_ulong::MAX
    );
    assert_error(
        overflow,
        "Python int too large to convert to C unsigned long",
    );
    assert_eq!(
        unsafe { numbers::PyLong_AsUnsignedLongLong(huge) },
        u64::MAX
    );
    assert_error(overflow, "int too big to convert");

    macro_rules! check {
        ($func:ident, $ty:ty, $overflow:literal) => {{
            let positive = proxy(42);
            let _positive_owner = unsafe { OwnedPyObject::from_owned(positive) };
            let mut out: $ty = 77;
            assert_eq!(
                unsafe { numbers::$func(positive, (&raw mut out).cast()) },
                1
            );
            assert_eq!(out, 42);
            for value in [-1, -(1i128 << 80), i128::MIN] {
                let negative = proxy(value);
                let _negative_owner = unsafe { OwnedPyObject::from_owned(negative) };
                out = 77;
                assert_eq!(
                    unsafe { numbers::$func(negative, (&raw mut out).cast()) },
                    0
                );
                assert_eq!(out, 77);
                assert_error(value_error, "value must be positive");
            }
            out = 77;
            assert_eq!(unsafe { numbers::$func(huge, (&raw mut out).cast()) }, 0);
            assert_eq!(out, 77);
            assert_error(overflow, $overflow);
        }};
    }
    check!(
        _PyLong_Size_t_Converter,
        usize,
        "Python int too large to convert to C size_t"
    );
    check!(
        _PyLong_UnsignedShort_Converter,
        u16,
        "Python int too large to convert to C unsigned long"
    );
    check!(
        _PyLong_UnsignedInt_Converter,
        u32,
        "Python int too large to convert to C unsigned long"
    );
    check!(
        _PyLong_UnsignedLong_Converter,
        std::os::raw::c_ulong,
        "Python int too large to convert to C unsigned long"
    );
    check!(
        _PyLong_UnsignedLongLong_Converter,
        u64,
        "int too big to convert"
    );

    // Short/int converters first extract unsigned long, then narrow. On LLP64
    // and wasm32, an int-width overflow can already fail that first stage.
    let above_short = proxy(1i128 << 16);
    let _short_owner = unsafe { OwnedPyObject::from_owned(above_short) };
    let mut short = 77u16;
    assert_eq!(
        unsafe { numbers::_PyLong_UnsignedShort_Converter(above_short, (&raw mut short).cast()) },
        0
    );
    assert_eq!(short, 77);
    assert_error(overflow, "Python int too large for C unsigned short");
    let above_int = proxy(1i128 << 32);
    let _int_owner = unsafe { OwnedPyObject::from_owned(above_int) };
    let mut integer = 77u32;
    assert_eq!(
        unsafe { numbers::_PyLong_UnsignedInt_Converter(above_int, (&raw mut integer).cast()) },
        0
    );
    assert_eq!(integer, 77);
    assert_error(
        overflow,
        if std::os::raw::c_ulong::BITS == 32 {
            "Python int too large to convert to C unsigned long"
        } else {
            "Python int too large for C unsigned int"
        },
    );
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}

unsafe extern "C" {
    fn molt_capi_errno() -> i32;
    fn molt_capi_set_errno(value: i32);
}

#[cfg(feature = "l7-test-probe")]
unsafe extern "C" {
    fn molt_l7_overlay_numeric_probe() -> i32;
    fn molt_l7_overlay_tuple_set_get_probe(tuple: *mut PyObject, value: *mut PyObject) -> i32;
    fn molt_l7_overlay_long_probe(value: *mut PyObject) -> std::os::raw::c_long;
    fn molt_l7_overlay_float_from_string_probe(value: *mut PyObject) -> f64;
    fn molt_l7_overlay_complex_real_probe(value: *mut PyObject) -> f64;
}

#[test]
fn float_pack_unpack_covers_ieee_edges_endian_and_info_authority() {
    let _guard = TEST_LOCK.lock().unwrap();
    let _abi_test = init();
    let mut bytes = [0u8; 8];
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::PyFloat_Pack2(
                1.0 + 2f64.powi(-11),
                bytes.as_mut_ptr().cast(),
                0,
            )
        },
        0
    );
    assert_eq!(&bytes[..2], &[0x3c, 0x00]);
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::PyFloat_Pack2(
                1.0 + 3.0 * 2f64.powi(-11),
                bytes.as_mut_ptr().cast(),
                1,
            )
        },
        0
    );
    assert_eq!(&bytes[..2], &[0x02, 0x3c]);
    for (value, expected) in [
        (0.0, 0x0000u16),
        (-0.0, 0x8000),
        (2f64.powi(-24), 0x0001),
        (f64::INFINITY, 0x7c00),
        (f64::NEG_INFINITY, 0xfc00),
    ] {
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::numbers::PyFloat_Pack2(value, bytes.as_mut_ptr().cast(), 0)
            },
            0
        );
        assert_eq!(u16::from_be_bytes([bytes[0], bytes[1]]), expected);
    }
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::PyFloat_Pack2(-f64::NAN, bytes.as_mut_ptr().cast(), 0)
        },
        0
    );
    assert_eq!(u16::from_be_bytes([bytes[0], bytes[1]]), 0xfe00);
    assert!(
        unsafe {
            molt_cpython_abi::api::numbers::PyFloat_Unpack2([0x00u8, 0x01].as_ptr().cast(), 0)
        } == 2f64.powi(-24)
    );
    let negative_zero = unsafe {
        molt_cpython_abi::api::numbers::PyFloat_Unpack2([0x00u8, 0x80].as_ptr().cast(), 1)
    };
    assert_eq!(negative_zero.to_bits(), (-0.0f64).to_bits());
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::PyFloat_Pack2(65_520.0, bytes.as_mut_ptr().cast(), 0)
        },
        -1
    );
    clear_error();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::PyFloat_Pack4(f64::MAX, bytes.as_mut_ptr().cast(), 0)
        },
        -1
    );
    clear_error();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::PyFloat_Pack8(-0.0, bytes.as_mut_ptr().cast(), 1)
        },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyFloat_Unpack8(bytes.as_ptr().cast(), 1) }
            .to_bits(),
        (-0.0f64).to_bits()
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyFloat_GetMax() },
        f64::MAX
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyFloat_GetMin() },
        f64::MIN_POSITIVE
    );
    SYS_GET_CALLS.store(0, Ordering::Relaxed);
    assert!(!unsafe { molt_cpython_abi::api::numbers::PyFloat_GetInfo() }.is_null());
    assert_eq!(SYS_GET_CALLS.load(Ordering::Relaxed), 1);
}

#[test]
fn complex_primitives_use_scaled_math_and_real_c_errno() {
    let _guard = TEST_LOCK.lock().unwrap();
    let _abi_test = init();
    let a = Py_complex {
        real: 3.0,
        imag: 4.0,
    };
    let b = Py_complex {
        real: 1.0,
        imag: -2.0,
    };
    let runtime_complex =
        unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(support::fake_complex::allocate(6.0, -7.0)) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyComplex_Check(runtime_complex) },
        1
    );
    let extracted =
        unsafe { molt_cpython_abi::api::numbers::PyComplex_AsCComplex(runtime_complex) };
    assert_eq!((extracted.real, extracted.imag), (6.0, -7.0));
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_c_sum(a, b) }.real,
        4.0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_c_diff(a, b) }.imag,
        6.0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_c_neg(a) }.real,
        -3.0
    );
    let product = unsafe { molt_cpython_abi::api::numbers::_Py_c_prod(a, b) };
    assert_eq!((product.real, product.imag), (11.0, -2.0));

    unsafe { molt_capi_set_errno(77) };
    let quotient = unsafe { molt_cpython_abi::api::numbers::_Py_c_quot(a, b) };
    assert!((quotient.real + 1.0).abs() < 1e-12);
    assert!((quotient.imag - 2.0).abs() < 1e-12);
    assert_eq!(unsafe { molt_capi_errno() }, 77);
    let zero = Py_complex {
        real: 0.0,
        imag: 0.0,
    };
    let _ = unsafe { molt_cpython_abi::api::numbers::_Py_c_quot(a, zero) };
    assert_eq!(unsafe { molt_capi_errno() }, libc::EDOM);

    unsafe { molt_capi_set_errno(77) };
    let one = unsafe { molt_cpython_abi::api::numbers::_Py_c_pow(zero, zero) };
    assert_eq!((one.real, one.imag), (1.0, 0.0));
    assert_eq!(unsafe { molt_capi_errno() }, 77);
    let _ = unsafe {
        molt_cpython_abi::api::numbers::_Py_c_pow(
            zero,
            Py_complex {
                real: -1.0,
                imag: 0.0,
            },
        )
    };
    assert_eq!(unsafe { molt_capi_errno() }, libc::EDOM);
    unsafe { molt_capi_set_errno(0) };
    let overflow = unsafe {
        molt_cpython_abi::api::numbers::_Py_c_pow(
            Py_complex {
                real: 1e308,
                imag: 0.0,
            },
            Py_complex {
                real: 2.0,
                imag: 0.0,
            },
        )
    };
    assert!(!overflow.real.is_finite() || !overflow.imag.is_finite());
    assert_eq!(unsafe { molt_capi_errno() }, libc::ERANGE);

    unsafe { molt_capi_set_errno(77) };
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::_Py_c_abs(Py_complex {
                real: f64::INFINITY,
                imag: f64::NAN,
            })
        },
        f64::INFINITY
    );
    assert_eq!(unsafe { molt_capi_errno() }, 0);
    let _ = unsafe {
        molt_cpython_abi::api::numbers::_Py_c_abs(Py_complex {
            real: f64::MAX,
            imag: f64::MAX,
        })
    };
    assert_eq!(unsafe { molt_capi_errno() }, libc::ERANGE);
}

#[test]
fn bool_public_names_are_pointer_aliases_of_sole_canonical_storage() {
    assert_eq!(&raw const Py_True, &raw const _Py_TrueStruct);
    assert_eq!(&raw const Py_False, &raw const _Py_FalseStruct);
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(1) },
        (&raw mut _Py_TrueStruct).cast::<PyObject>()
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(0) },
        (&raw mut _Py_FalseStruct).cast::<PyObject>()
    );
}

#[test]
fn number_conversions_preserve_exact_carriers_and_normalize_bool() {
    let _guard = TEST_LOCK.lock().unwrap();
    let _abi_test = init();
    // A mortal carrier is required to prove the new-reference increment;
    // cached small integers are immortal and intentionally ignore INCREF.
    let integer = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
    let before = unsafe { (*integer).ob_refcnt };
    let same_integer = unsafe { molt_cpython_abi::api::abstract_number::PyNumber_Long(integer) };
    assert_eq!(same_integer, integer);
    assert_eq!(unsafe { (*integer).ob_refcnt }, before + 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(same_integer) };

    let float = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.5) };
    let before = unsafe { (*float).ob_refcnt };
    let same_float = unsafe { molt_cpython_abi::api::abstract_number::PyNumber_Float(float) };
    assert_eq!(same_float, float);
    assert_eq!(unsafe { (*float).ob_refcnt }, before + 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(same_float) };

    for convert in [
        molt_cpython_abi::api::abstract_number::PyNumber_Long
            as unsafe extern "C" fn(*mut PyObject) -> *mut PyObject,
        molt_cpython_abi::api::abstract_number::PyNumber_Index,
    ] {
        let normalized = unsafe { convert((&raw mut _Py_TrueStruct).cast()) };
        assert_ne!(normalized, (&raw mut _Py_TrueStruct).cast());
        assert_eq!(
            unsafe { molt_cpython_abi::api::numbers::PyLong_CheckExact(normalized) },
            1
        );
        assert_eq!(unsafe { (*normalized).ob_type }, &raw mut PyLong_Type);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(normalized) };
    }
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(float);
        molt_cpython_abi::api::refcount::Py_DECREF(integer);
    }
}

#[test]
#[cfg(feature = "l7-test-probe")]
fn overlay_compiled_numeric_roundtrip_uses_the_abi_bridge_representation() {
    let _guard = TEST_LOCK.lock().unwrap();
    let _abi_test = init();
    assert_eq!(unsafe { molt_l7_overlay_numeric_probe() }, 0);
    let integer = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(73) };
    let text =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(c"1.25".as_ptr(), 4) };
    let complex = unsafe { molt_cpython_abi::api::numbers::PyComplex_FromDoubles(6.0, -7.0) };
    assert!(!integer.is_null() && !text.is_null() && !complex.is_null());
    assert_eq!(unsafe { molt_l7_overlay_long_probe(integer) }, 73);
    assert_eq!(
        unsafe { molt_l7_overlay_float_from_string_probe(text) },
        1.25
    );
    assert_eq!(unsafe { molt_l7_overlay_complex_real_probe(complex) }, 6.0);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(complex);
        molt_cpython_abi::api::refcount::Py_DECREF(text);
        molt_cpython_abi::api::refcount::Py_DECREF(integer);
    }
}

#[test]
#[cfg(feature = "l7-test-probe")]
fn overlay_compiled_tuple_set_and_direct_get_share_the_canonical_sidecar() {
    let _guard = TEST_LOCK.lock().unwrap();
    let _abi_test = init();
    let tuple_bits = bits_for_tuple(vec![bits_for_value(1)]);
    let tuple = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(tuple_bits) };
    let value = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    assert!(!tuple.is_null() && !value.is_null());
    assert_eq!(
        unsafe { molt_l7_overlay_tuple_set_get_probe(tuple, value) },
        0
    );
    // The caller's reference may disappear immediately after SetItem steals
    // its input; the concrete-layout sidecar must keep the carrier alive until
    // the tuple view itself is retired.
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(value) };
    assert_eq!(
        value_for_bits(
            match unsafe { molt_cpython_abi::hooks::hooks_or_stubs().tuple_item(tuple_bits, 0) }
                .decode()
            {
                molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => bits,
                _ => panic!("retained tuple item missing"),
            }
        ),
        Some(42)
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(tuple) };
}

#[test]
fn integer_projection_writes_final_digits_in_place_and_preserves_failure_owners() {
    use molt_cpython_abi::api::numbers;
    use molt_cpython_abi::api::refcount::OwnedPyObject;
    let _guard = TEST_LOCK.lock().unwrap();
    let _abi_test = init();
    for (value, native_width) in [
        (i128::MIN, 16),
        (i128::MAX, 16),
        ((1i128 << 120) | (1i128 << 91) | 0x1234_5678_9abc_def0, 16),
        (-((1i128 << 93) | (1i128 << 62) | 0x2345_6789), 12),
    ] {
        BYTE_DESTINATION.store(0, Ordering::SeqCst);
        let op = proxy(value);
        let _owner = unsafe { OwnedPyObject::from_owned(op) };
        assert!(!op.is_null());
        let long = op.cast::<PyLongObject>();
        let digits = unsafe { std::ptr::addr_of_mut!((*long).long_value.ob_digit).cast::<u32>() };
        assert_eq!(
            BYTE_DESTINATION.load(Ordering::SeqCst),
            digits as usize,
            "provider must fill final C storage, not an intermediate allocation"
        );
        let count = (128 - value.unsigned_abs().leading_zeros()) as usize;
        assert_eq!(
            BYTE_CAPACITY.load(Ordering::SeqCst),
            count.div_ceil(30).max(1) * 4
        );
        for digit in 0..count.div_ceil(30) {
            assert_eq!(
                unsafe { digits.add(digit).read() },
                ((value.unsigned_abs() >> (30 * digit)) & 0x3fff_ffff) as u32
            );
        }
        BYTE_DESTINATION.store(0, Ordering::SeqCst);
        let read_calls = INTEGER_READ_CALLS.load(Ordering::Relaxed);
        let binary_calls = BINARY_CALLS.load(Ordering::Relaxed);
        assert_eq!(unsafe { numbers::_PyLong_NumBits(op) }, count);
        assert_eq!(unsafe { numbers::_PyLong_Sign(op) }, value.signum() as i32);
        assert_eq!(
            unsafe { numbers::PyLong_AsUnsignedLongLongMask(op) },
            value as u64
        );
        assert_eq!(unsafe { numbers::PyLong_AsDouble(op) }, value as f64);
        // This fixture refuses runtime FloatAsDouble conversion. Physical
        // integer readers must still work after runtime-to-C publication.
        assert_eq!(unsafe { numbers::PyFloat_AsDouble(op) }, value as f64);
        let complex = unsafe { numbers::PyComplex_AsCComplex(op) };
        assert_eq!(complex.real, value as f64);
        assert_eq!(complex.imag, 0.0);
        let mut native_bytes = [0xa5; 16];
        assert_eq!(
            unsafe { numbers::PyLong_AsNativeBytes(op, native_bytes.as_mut_ptr().cast(), 16, 0) },
            native_width
        );
        assert_eq!(native_bytes, value.to_be_bytes());
        let mut bytes = [0xa5; 16];
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
                    long,
                    bytes.as_mut_ptr(),
                    16,
                    1,
                    1,
                )
            },
            0
        );
        assert_eq!(bytes, value.to_le_bytes());
        assert_eq!(BYTE_DESTINATION.load(Ordering::SeqCst), 0);
        assert_eq!(INTEGER_READ_CALLS.load(Ordering::Relaxed), read_calls);
        assert_eq!(BINARY_CALLS.load(Ordering::Relaxed), binary_calls);
        assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    }
    let before = support::fake_runtime::live_numeric_count();
    REJECT_BYTE_OUTPUT.store(true, Ordering::SeqCst);
    let failed = proxy(1i128 << 112);
    REJECT_BYTE_OUTPUT.store(false, Ordering::SeqCst);
    let _failed_owner = unsafe { OwnedPyObject::from_owned(failed) };
    assert!(
        failed.is_null(),
        "installed provider refusal cannot publish a zero view"
    );
    assert_eq!(
        support::fake_runtime::live_numeric_count(),
        before,
        "the failed owning result must release its runtime source too"
    );
    clear_error();
}

#[test]
fn integer_zero_width_c_exports_follow_signed_complement_boundary() {
    use molt_cpython_abi::api::{numbers, refcount::OwnedPyObject};
    let _guard = TEST_LOCK.lock().unwrap();
    let _abi_test = init();
    for value in [-129, -1, 0, 1, 128] {
        let op = unsafe { numbers::PyLong_FromLong(value) };
        let _owner = unsafe { OwnedPyObject::from_owned(op) };
        assert!(!op.is_null());
        for little in [0, 1] {
            for signed in [0, 1] {
                let expected = if value == 0 || value == -1 && signed != 0 {
                    0
                } else {
                    -1
                };
                assert_eq!(
                    unsafe {
                        numbers::_PyLong_AsByteArray(
                            op.cast(),
                            std::ptr::null_mut(),
                            0,
                            little,
                            signed,
                        )
                    },
                    expected
                );
                clear_error();
            }
        }
    }
}
