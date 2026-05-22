# LLVM len representation authority

Design
- Keep LLVM len-helper selection tied to `TirFunction.value_types`.
- Treat preserved `container_type` attrs as legacy SimpleIR metadata only; they
  must not authorize LLVM specialized runtime helper selection.
- Capture both sides with regressions: transport-only tuple metadata lowers to
  generic `molt_len`, while a real `TirType::Tuple` fact lowers to
  `molt_len_tuple`.

Files
- `runtime/molt-backend/src/llvm_backend/lowering.rs`

Tests
- Focused LLVM lowering tests when the `llvm` feature is available.
- Backend/native proof remains unchanged because LLVM tests are feature-gated.

Exit Criteria
- No LLVM test encodes `container_type` as specialization authority.
- Typed tuple facts still select `molt_len_tuple`.
- Focused proof and the regular backend lib gate pass.
