#![no_main]

use libfuzzer_sys::fuzz_target;
use molt_obj_model::MoltObject;
use molt_runtime::{
    molt_string_count, molt_string_endswith, molt_string_find, molt_string_from_bytes,
    molt_string_replace, molt_string_startswith,
};

fuzz_target!(|data: &[u8]| {
    let mut hay_bits = 0u64;
    unsafe {
        let out_bits = &mut hay_bits as *mut u64;
        if molt_string_from_bytes(data.as_ptr(), data.len() as u64, out_bits) != 0 {
            return;
        }
    }

    let mid = data.len() / 2;
    let (left, right) = data.split_at(mid);
    let mut needle_bits = 0u64;
    let mut repl_bits = 0u64;

    unsafe {
        let needle_out = &mut needle_bits as *mut u64;
        if molt_string_from_bytes(left.as_ptr(), left.len() as u64, needle_out) != 0 {
            return;
        }
        let repl_out = &mut repl_bits as *mut u64;
        if molt_string_from_bytes(right.as_ptr(), right.len() as u64, repl_out) != 0 {
            return;
        }

        let _ = molt_string_find(hay_bits, needle_bits);
        let _ = molt_string_startswith(hay_bits, needle_bits);
        let _ = molt_string_endswith(hay_bits, needle_bits);
        let _ = molt_string_count(hay_bits, needle_bits);
        let count_bits = MoltObject::from_int(-1).bits();
        let _ = molt_string_replace(hay_bits, needle_bits, repl_bits, count_bits);
    }
});
