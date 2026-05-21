# ord_at Structural Completion Plan

## Design

- Preserve frontend fused lowering for `ord(text[i])` as `ORD_AT` / `ord_at`
  while keeping slice arguments on the normal `index` then `ord` path.
- Treat `molt_ord_at` as the shared runtime primitive for native, LLVM, and
  WASM, so semantics come from the same Rust object/runtime implementation.
- Keep Luau source emission aligned with CPython string codepoint indexing by
  routing both `ord` and `ord_at` through UTF-8 helper functions rather than
  byte-indexed `string.byte` / `string.sub`.
- Do not touch the unstaged runtime import-order drift unless a failing proof
  shows it is functionally part of this change.

## Files

- `src/molt/frontend/__init__.py`
- `runtime/molt-runtime/src/object/ops.rs`
- `runtime/molt-runtime/src/object/ops_sys.rs`
- `runtime/molt-backend/src/native_backend/function_compiler.rs`
- `runtime/molt-backend/src/wasm.rs`
- `runtime/molt-backend/src/wasm_imports.rs`
- `runtime/molt-backend/src/llvm_backend/lowering.rs`
- `runtime/molt-backend/src/luau.rs`
- `tests/test_codec_lowering.py`
- `tests/test_ord_at_native.py`
- `tests/test_wasm_harness_data_end.py`
- `tests/wasm_harness.py`
- `wasm/molt_runtime*.wasm.sha256`

## Tests

- Lowering: `tests/test_codec_lowering.py::test_ord_subscript_lowers_to_fused_ord_at`
- Lowering guard: `tests/test_codec_lowering.py::test_ord_slice_keeps_normal_index_then_ord_path`
- Harness: `tests/test_wasm_harness_data_end.py::test_wasm_harness_exposes_ord_at_import`
- Backend parity: `tests/test_ord_at_native.py`
- Luau unit proof: `cargo test --profile release-fast -p molt-backend --features luau-backend luau::tests::test_ord_at_emits_utf8_codepoint_helper luau::tests::test_string_get_item_uses_utf8_codepoint_offsets`
- Runtime/backend compile proof: `cargo test --profile release-fast -p molt-backend --features native-backend --lib`

## Exit Criteria

- `ord(text[i])` lowers to fused `ord_at`; `ord(text[0:1])` does not.
- Native, WASM, LLVM, and Luau produce the expected Unicode codepoint output
  for positive and negative string indices.
- WASM custody hashes match regenerated runtime artifacts.
- No host-CPython runtime fallback is introduced.
- Unrelated unstaged partner WIP remains untouched.
