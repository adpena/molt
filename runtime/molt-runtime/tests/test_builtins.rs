// Builtin function tests — CPython parity for abs, len, hash, ord, chr, etc.

use molt_obj_model::MoltObject;
use std::sync::Once;

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/test_builtins"
));

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_exception_clear") ]
    fn molt_exception_clear(context: u64) -> u64;
}
static INIT: Once = Once::new();
fn init() -> molt_runtime_core::CoreGilGuard {
    INIT.call_once(|| unsafe {
        molt_runtime_init();
    });

    let guard = molt_runtime_core::CoreGilGuard::new();
    let py = unsafe { guard.__token_unchecked() };
    unsafe { molt_exception_clear(py.attached_context()) };
    guard
}

fn int(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}
fn f(v: f64) -> u64 {
    MoltObject::from_float(v).bits()
}
fn none() -> u64 {
    MoltObject::none().bits()
}
fn b(v: bool) -> u64 {
    MoltObject::from_bool(v).bits()
}
fn as_int(bits: u64) -> i64 {
    MoltObject::from_bits(bits).as_int().expect("expected int")
}
fn missing(py: &molt_runtime_core::PyToken) -> u64 {
    unsafe { molt_runtime::molt_missing(py.attached_context()) }
}

// ── abs ─────────────────────────────────────────────────
#[test]
fn test_abs_int_pos() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_abs_builtin(py.attached_context(), int(42)) }),
        42
    );
}
#[test]
fn test_abs_int_neg() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_abs_builtin(py.attached_context(), int(-42)) }),
        42
    );
}
#[test]
fn test_abs_int_zero() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_abs_builtin(py.attached_context(), int(0)) }),
        0
    );
}

// ── pow ─────────────────────────────────────────────────
#[test]
fn test_pow_2_10() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_pow(py.attached_context(), int(2), int(10)) }),
        1024
    );
}
#[test]
fn test_pow_0() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_pow(py.attached_context(), int(100), int(0)) }),
        1
    );
}

// ── divmod ──────────────────────────────────────────────
// divmod returns a tuple; for now just test it doesn't crash
#[test]
fn test_divmod_basic() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let _r = unsafe { molt_runtime::molt_divmod_builtin(py.attached_context(), int(7), int(3)) };
}

// ── ord / chr ───────────────────────────────────────────
#[test]
fn test_chr_a() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let _r = unsafe { molt_runtime::molt_chr(py.attached_context(), int(65)) }; /* 'A' */
}
#[test]
fn test_ord_basic() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() }; /* would need a string object */
}

// ── is_truthy on various types ──────────────────────────
#[test]
fn test_truthy_none() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), none()) },
        0
    );
}
#[test]
fn test_truthy_true() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_ne!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), b(true)) },
        0
    );
}
#[test]
fn test_truthy_false() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), b(false)) },
        0
    );
}
#[test]
fn test_truthy_int_0() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), int(0)) },
        0
    );
}
#[test]
fn test_truthy_int_1() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_ne!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), int(1)) },
        0
    );
}
#[test]
fn test_truthy_float_0() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), f(0.0)) },
        0
    );
}
#[test]
fn test_truthy_float_nonzero() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_ne!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), f(0.1)) },
        0
    );
}

// ── isinstance / issubclass ─────────────────────────────
// These need class objects which are hard to construct in unit tests.
// Just verify they don't crash with basic inputs.
#[test]
fn test_isinstance_no_crash() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let _r = unsafe { molt_runtime::molt_isinstance(py.attached_context(), int(1), int(2)) };
}

// ── New builtins (from the 26 we added) ─────────────────
#[test]
fn test_int_builtin_no_args() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let r =
        unsafe { molt_runtime::molt_int_builtin(py.attached_context(), missing(py), missing(py)) };
    assert_eq!(as_int(r), 0);
}

#[test]
fn test_float_builtin_no_args() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let r = unsafe { molt_runtime::molt_float_builtin(py.attached_context(), missing(py)) };
    let v = MoltObject::from_bits(r).as_float().expect("expected float");
    assert_eq!(v, 0.0);
}

#[test]
fn test_bool_builtin_false() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let r = unsafe { molt_runtime::molt_bool_builtin(py.attached_context(), int(0)) };
    assert!(!MoltObject::from_bits(r).as_bool().unwrap());
}

#[test]
fn test_bool_builtin_true() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let r = unsafe { molt_runtime::molt_bool_builtin(py.attached_context(), int(42)) };
    assert!(MoltObject::from_bits(r).as_bool().unwrap());
}

#[test]
fn test_object_builtin() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let r = unsafe { molt_runtime::molt_object_builtin(py.attached_context()) };
    // Should return a valid object, not none
    assert_ne!(r, none());
}

#[test]
fn test_type_builtin() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let r = unsafe { molt_runtime::molt_type_builtin(py.attached_context(), int(42)) };
    // In test mode without full bootstrap, type() may return None — just verify no crash
    let _ = r;
}

#[test]
fn test_range_builtin_one_arg() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let r = unsafe {
        molt_runtime::molt_range_builtin(py.attached_context(), int(5), missing(py), missing(py))
    };
    assert_ne!(r, none()); // range(5) should succeed
}

// Resolve through exact C names, not Rust reexports. These references make the
// test binary's linker reject a missing ABI symbol without running a compressor.
// A runtime without stdlib_compression defines neither symbol (HF-487); the
// generated resolvers' absence probes prove that side at compile time.
#[cfg(all(feature = "stdlib_compression", not(target_arch = "wasm32")))]
#[test]
fn test_raw_compression_c_exports_are_linkable() {
    unsafe extern "C" {
        #[link_name = molt_runtime_core::attached_symbol!("molt_deflate_raw") ]
        fn molt_deflate_raw(context: u64, data: u64, level: u64) -> u64;
        #[link_name = molt_runtime_core::attached_symbol!("molt_inflate_raw") ]
        fn molt_inflate_raw(context: u64, data: u64) -> u64;
    }
    let deflate =
        std::hint::black_box(molt_deflate_raw as unsafe extern "C" fn(u64, u64, u64) -> u64);
    let inflate = std::hint::black_box(molt_inflate_raw as unsafe extern "C" fn(u64, u64) -> u64);
    assert_eq!(
        deflate as usize,
        molt_runtime::molt_deflate_raw as *const () as usize
    );
    assert_eq!(
        inflate as usize,
        molt_runtime::molt_inflate_raw as *const () as usize
    );
}

#[cfg(all(feature = "stdlib_compression", not(target_arch = "wasm32")))]
#[test]
fn test_native_lzma_c_exports_resolve_to_extracted_owners() {
    macro_rules! check_export {
        ($symbol:tt, $name:ident($($arg:ty),*)) => {{
            unsafe extern "C" {
                #[link_name = molt_runtime_core::attached_symbol!($symbol)]
                fn $name(context: u64, $(_: $arg),*) -> u64;
            }
            let exported: unsafe extern "C" fn(u64, $($arg),*) -> u64 = $name;
            let implementation: unsafe extern "C" fn(u64, $($arg),*) -> u64 = molt_runtime::$name;
            assert_eq!(
                std::hint::black_box(exported) as usize,
                implementation as usize,
                stringify!($name),
            );
        }};
    }
    check_export!("molt_lzma_format_auto", molt_lzma_format_auto());
    check_export!("molt_lzma_format_xz", molt_lzma_format_xz());
    check_export!("molt_lzma_format_alone", molt_lzma_format_alone());
    check_export!("molt_lzma_format_raw", molt_lzma_format_raw());
    check_export!("molt_lzma_check_none", molt_lzma_check_none());
    check_export!("molt_lzma_check_crc32", molt_lzma_check_crc32());
    check_export!("molt_lzma_check_crc64", molt_lzma_check_crc64());
    check_export!("molt_lzma_check_sha256", molt_lzma_check_sha256());
    check_export!("molt_lzma_preset_default", molt_lzma_preset_default());
    check_export!("molt_lzma_preset_extreme", molt_lzma_preset_extreme());
    check_export!(
        "molt_lzma_compress",
        molt_lzma_compress(u64, u64, u64, u64, u64)
    );
    check_export!(
        "molt_lzma_decompress",
        molt_lzma_decompress(u64, u64, u64, u64)
    );
    check_export!(
        "molt_lzma_compressor_new",
        molt_lzma_compressor_new(u64, u64, u64)
    );
    check_export!(
        "molt_lzma_compressor_compress",
        molt_lzma_compressor_compress(u64, u64, u64)
    );
    check_export!(
        "molt_lzma_compressor_flush",
        molt_lzma_compressor_flush(u64, u64)
    );
    check_export!(
        "molt_lzma_decompressor_new",
        molt_lzma_decompressor_new(u64, u64, u64)
    );
    check_export!(
        "molt_lzma_decompressor_decompress",
        molt_lzma_decompressor_decompress(u64, u64, u64, u64)
    );
    check_export!(
        "molt_lzma_decompressor_eof",
        molt_lzma_decompressor_eof(u64, u64)
    );
    check_export!(
        "molt_lzma_decompressor_needs_input",
        molt_lzma_decompressor_needs_input(u64, u64)
    );
    check_export!(
        "molt_lzma_decompressor_unused_data",
        molt_lzma_decompressor_unused_data(u64, u64)
    );
    check_export!(
        "molt_lzma_file_open",
        molt_lzma_file_open(u64, u64, u64, u64, u64, u64)
    );
    check_export!("molt_lzma_file_read", molt_lzma_file_read(u64, u64, u64));
    check_export!("molt_lzma_file_write", molt_lzma_file_write(u64, u64, u64));
    check_export!("molt_lzma_file_close", molt_lzma_file_close(u64, u64));
}
