# Python Execution Boundary

**Spec ID:** 0210
**Status:** Active
**Owner:** runtime + compiler + tooling + packaging

Compiled Molt programs execute without a host or fallback Python interpreter.
Embedding CPython, starting a Python worker, or substituting a host package is
not an extension compatibility strategy. The former optional PyO3/worker bridge
proposal and its rollout phases are retired.

The [verified subset contract](verified_subset_contract.md) owns Python semantics
and release qualification. Package support comes from compiling the package's
admitted Python source and rebuilding its extensions against
[`libmolt`](libmolt_extension_abi_contract.md), with pinned package/build inputs
and the selected target's ABI and capability requirements. Missing support stays
an explicit implementation or qualification obligation.

CPython remains a development-side reference: source inspection, differential
execution, package inspection and conformance checks may use the pinned
interpreter. None of those reference processes becomes an emitted program's
execution dependency.

Unavailable implementations fail compilation under the
[compatibility contract](compatibility_fallback_contract.md). Compiled outputs
contain no deferred unavailable-operation marker. The existing `python.bridge`
capability on `ffi.bridge` gates an FFI dispatch operation inside Molt. A
capability grant does not supply a CPython runtime or make an unsupported
operation executable.

Runtime/C-API object bridges, source-recompiled extensions, WASM host imports and
explicit application services retain their own ownership and capability
contracts. Their use of the word "bridge" does not grant Python fallback.
