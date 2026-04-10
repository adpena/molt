use std::sync::Once;

use molt_obj_model::MoltObject;
use molt_runtime::{lifecycle, molt_string_from_bytes};

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_: u64) -> u64 {
    MoltObject::none().bits()
}

fn init_runtime() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = lifecycle::init();
    });
}

#[test]
fn string_from_bytes_accepts_unaligned_out_pointer() {
    init_runtime();

    let text = b"importlib";
    let mut storage = [0u8; 24];
    let out_ptr = unsafe { storage.as_mut_ptr().add(1) as *mut u64 };

    let rc = unsafe { molt_string_from_bytes(text.as_ptr(), text.len() as u64, out_ptr) };

    assert_eq!(rc, 0);
    let bits = unsafe { std::ptr::read_unaligned(out_ptr) };
    assert_ne!(bits, 0);
}
