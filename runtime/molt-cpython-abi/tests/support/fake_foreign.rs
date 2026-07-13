use molt_lang_obj_model::MoltObject;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FOREIGN_HANDLE: AtomicU64 = AtomicU64::new(0x7f00_0000);

/// Minimal real-handle foreign constructor for dedicated hook-vtable tests.
/// Production no longer permits synthetic address tokens, so test runtimes
/// that publish physical C objects must provide this same single authority.
pub unsafe extern "C" fn foreign_new(_c_ptr: usize) -> u64 {
    let address = NEXT_FOREIGN_HANDLE.fetch_add(0x10, Ordering::Relaxed) as usize;
    MoltObject::from_ptr(address as *mut u8).bits()
}
