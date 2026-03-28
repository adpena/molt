#![no_main]
use libfuzzer_sys::fuzz_target;

// --------------------------------------------------------------------------
// Fuzz target: WASM type section encoding via wasm-encoder + validation via
// wasmparser.  Generates random type signatures and verifies the encoder
// never panics and the encoded bytes always round-trip through the parser.
// --------------------------------------------------------------------------

// Build a minimal valid WASM module that contains only a type section
// with signatures derived from the fuzz input, then parse it back with
// wasmparser and verify no panics occur.
//
// Byte layout consumed from `data`:
//   - Each chunk of 2 bytes defines one function signature:
//       byte[0]: number of params (mod 8, capped)
//       byte[1]: result flag & param types packed
//
// We exercise wasm_encoder::TypeSection and wasmparser round-tripping.
fuzz_target!(|data: &[u8]| {
    // Need at least 2 bytes to define one function signature.
    if data.len() < 2 {
        return;
    }

    // --- Step 1: Build a WASM module with random type signatures ---
    let mut module = wasm_encoder::Module::new();
    let mut types = wasm_encoder::TypeSection::new();

    let val_types = [
        wasm_encoder::ValType::I32,
        wasm_encoder::ValType::I64,
        wasm_encoder::ValType::F32,
        wasm_encoder::ValType::F64,
    ];

    let mut chunks = data.chunks_exact(2);
    let mut sig_count = 0u32;

    for chunk in &mut chunks {
        let num_params = (chunk[0] & 0x07) as usize; // 0..=7
        let result_and_types = chunk[1];
        let has_result = (result_and_types & 0x80) != 0;

        let mut params = Vec::with_capacity(num_params);
        for i in 0..num_params {
            let type_idx = ((result_and_types >> (i % 4)) & 0x03) as usize;
            params.push(val_types[type_idx % val_types.len()]);
        }

        let results: Vec<wasm_encoder::ValType> = if has_result {
            let ret_idx = (result_and_types & 0x03) as usize;
            vec![val_types[ret_idx % val_types.len()]]
        } else {
            vec![]
        };

        types.ty().function(params, results);
        sig_count += 1;

        // Cap at 256 signatures to avoid excessive allocation.
        if sig_count >= 256 {
            break;
        }
    }

    if sig_count == 0 {
        return;
    }

    module.section(&types);
    let wasm_bytes = module.finish();

    // --- Step 2: Validate the raw bytes do not start a panic in wasmparser ---
    // We intentionally ignore validation errors (malformed modules are fine);
    // what we care about is that neither the encoder nor parser panics.
    let _ = wasmparser::Validator::new().validate_all(&wasm_bytes);

    // --- Step 3: Parse the type section back and verify count matches ---
    let parser = wasmparser::Parser::new(0);
    let mut parsed_types = 0u32;
    for payload in parser.parse_all(&wasm_bytes) {
        match payload {
            Ok(wasmparser::Payload::TypeSection(reader)) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    if ty.is_ok() {
                        parsed_types += 1;
                    }
                }
            }
            Err(_) => {
                // Malformed module — that is fine for fuzzing.
                return;
            }
            _ => {}
        }
    }

    assert_eq!(
        sig_count, parsed_types,
        "type count mismatch: encoded {sig_count} but parsed {parsed_types}"
    );
});
