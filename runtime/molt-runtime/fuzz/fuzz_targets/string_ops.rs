#![no_main]

use libfuzzer_sys::fuzz_target;
use molt_runtime::{
    molt_string_count, molt_string_endswith, molt_string_find, molt_string_from_bytes,
    molt_string_replace, molt_string_startswith,
};

fuzz_target!(|data: &[u8]| {
    let mut hay_bits = 0u64;
    unsafe {
        if molt_string_from_bytes(data.as_ptr(), data.len(), &mut hay_bits) != 0 {
            return;
        }
    }

    let mid = data.len() / 2;
    let (left, right) = data.split_at(mid);
    let mut needle_bits = 0u64;
    let mut repl_bits = 0u64;

    unsafe {
        if molt_string_from_bytes(left.as_ptr(), left.len(), &mut needle_bits) != 0 {
            return;
        }
        if molt_string_from_bytes(right.as_ptr(), right.len(), &mut repl_bits) != 0 {
            return;
        }

        let _ = molt_string_find(hay_bits, needle_bits);
        let _ = molt_string_startswith(hay_bits, needle_bits);
        let _ = molt_string_endswith(hay_bits, needle_bits);
        let _ = molt_string_count(hay_bits, needle_bits);
        let _ = molt_string_replace(hay_bits, needle_bits, repl_bits);
    }
});
