use std::sync::Once;

use molt_runtime::{
    lifecycle, molt_bigint_from_bytes, molt_bytes_from_bytes, molt_string_from_bytes,
};

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/string_from_bytes_alignment"
));

fn init_runtime() -> molt_runtime_core::CoreGilGuard {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = lifecycle::init();
    });

    let guard = molt_runtime_core::CoreGilGuard::new();
    let py = unsafe { guard.__token_unchecked() };
    molt_runtime_core::rt_exception_clear(py);
    guard
}

#[test]
fn string_from_bytes_accepts_unaligned_out_pointer() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };

    let text = b"importlib";
    let mut storage = [0u8; 24];
    let out_ptr = unsafe { storage.as_mut_ptr().add(1) as *mut u64 };

    let rc = unsafe {
        molt_string_from_bytes(
            py.attached_context(),
            text.as_ptr(),
            text.len() as u64,
            out_ptr,
        )
    };

    assert_eq!(rc, 0);
    let bits = unsafe { std::ptr::read_unaligned(out_ptr) };
    assert_ne!(bits, 0);
}

#[test]
fn bytes_from_bytes_reuses_const_data_pointer_literal() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };

    let payload = b"pkg";
    let mut first = 0u64;
    let mut second = 0u64;

    let first_rc = unsafe {
        molt_bytes_from_bytes(
            py.attached_context(),
            payload.as_ptr(),
            payload.len() as u64,
            &mut first,
        )
    };
    let second_rc = unsafe {
        molt_bytes_from_bytes(
            py.attached_context(),
            payload.as_ptr(),
            payload.len() as u64,
            &mut second,
        )
    };

    assert_eq!(first_rc, 0);
    assert_eq!(second_rc, 0);
    assert_ne!(first, 0);
    assert_eq!(first, second);
}

#[test]
fn bigint_from_bytes_reuses_const_data_pointer_literal() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };

    let payload = &[
        0x4b, 0x3b, 0x4c, 0xa8, 0x5a, 0x86, 0xc4, 0x7a, 0x09, 0x8a, 0x22, 0x40, 0x00, 0x00, 0x00,
        0x00,
    ];

    let first = unsafe {
        molt_bigint_from_bytes(
            py.attached_context(),
            payload.as_ptr(),
            payload.len() as u64,
        )
    };
    let second = unsafe {
        molt_bigint_from_bytes(
            py.attached_context(),
            payload.as_ptr(),
            payload.len() as u64,
        )
    };

    assert_ne!(first, 0);
    assert_eq!(first, second);
}
