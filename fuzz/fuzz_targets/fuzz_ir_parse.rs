//! Fuzz target: feed arbitrary byte sequences to `SimpleIR::from_json_str`.
//!
//! This exercises the JSON parser and the IR deserialization layer. The target
//! must never panic — malformed input should produce `Err`, not a crash.

#![no_main]
use libfuzzer_sys::fuzz_target;
use molt_backend::SimpleIR;

fuzz_target!(|data: &[u8]| {
    // Only bother with inputs that are plausibly UTF-8.
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    // Must not panic regardless of input.
    let _ = SimpleIR::from_json_str(input);
});
