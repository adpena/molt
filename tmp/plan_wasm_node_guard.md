Design:
- Treat linked Node/V8 WASM parity as a memory-guarded RSS workload, not a
  direct-child virtual-address-space workload. V8 reserves large address ranges
  during WASM instantiation; `RLIMIT_AS` can produce SIGSEGV with no RSS
  evidence and no Molt semantic signal.
- Keep default-on `tools/guarded_exec.py` / `harness_memory_guard` process,
  tree, global RSS, timeout, and orphan cleanup enforcement.
- Disable the direct child rlimit only for the Node/V8 linked parity runner and
  the CI pytest step that hosts it, so the outer guard cannot impose a cap that
  all nested Node processes inherit.
- Make the guard's signal guidance explicitly mention direct-child resource
  limits as a diagnostic class when no RSS violation was observed.
- Keep the runtime stdlib profile contract closed over Python stdlib shims: if a
  core import path can import `logging`, the micro runtime profile must compile
  the Rust logging intrinsics instead of leaving native links with unresolved
  symbols.

Files:
- tests/wasm_linked_runner.py
- tests/test_wasm_linked_runner_node_flags.py
- .github/workflows/molt-wasm-ci.yml
- tests/test_ci_workflow_topology.py
- docs/spec/areas/tooling/0011-ci.md
- tools/harness_memory_guard.py
- tools/memory_guard.py
- tools/wasm_link.py
- runtime/molt-runtime/Cargo.toml
- tests/cli/test_backend_manifest_contract.py
- docs/cli-reference.md

Tests:
- python3 -m pytest tests/test_wasm_linked_runner_node_flags.py tests/test_ci_workflow_topology.py -q
- guarded WASM control-flow parity under Node 22.22.3 when available
- tools/check_memory_guard_wiring.py
- tools/check_subprocess_guard_coverage.py
- cargo build --profile release-fast --workspace
- cargo test --profile release-fast -p molt-backend --features native-backend --lib
- pytest tests/compliance/ -p no:cacheprovider --tb=line -q

Exit criteria:
- WASM linked parity remains memory-guarded by default.
- Node/V8 linked parity no longer inherits the CI direct-child address-space
  clamp.
- CI topology asserts the rlimit split so future workflow edits cannot
  reintroduce the crash class silently.
