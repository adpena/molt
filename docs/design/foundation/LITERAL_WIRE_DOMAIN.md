# Literal and wire-domain authority

The opcode registry owns literal shapes, integer semantic roles, runtime
requirements, and declared aliases. Runtime requirements flow from each
canonical kind to its aliases; an alias may add stricter requirements. A shared
mapper opcode alone does not make operations aliases. The Python and Rust
projections, target admission, and Luau support matrix consume these same
projections. Target capability masks are unchanged by this consolidation.

Scalar and owned literals pass through the IR `SimpleLiteral` value projection.
Required scalar fields, decimal syntax, byte carriers, and surrogatepass text
validation are checked before source emission. Integer aliases use the same
bounded exact-value extraction. Rust materialization matches validated literal
values rather than maintaining a second opcode or alias inventory.

Wire field roles remain spelling-sensitive. Binding destinations are validated
through `simple_ir_binding`; copy and owned-alias inputs use the shared semantic
read visitor. Canonical runtime role inheritance does not rewrite `store_var`
or `load_var` into a different wire transport.

Rust's neutral transport includes float unary plus, type-guard passthrough,
terminal phase markers, and warning-string output. Diagnostic stderr uses
UTF-8 backslash replacement for surrogate code points. The embedded text
implementation lives in a private module so its helper functions and tuple
constructor cannot collide with guest function names. Unpacking probes at
most the requested number of code points plus one without an initial full
string-length scan.

Luau materializes Ellipsis and NotImplemented as distinct shared singleton
values. String and repr observations preserve their Python spellings, aliases
preserve their identity, and NotImplemented truth raises TypeError for Python
3.14 and later using the existing target-version state. The earlier-version
DeprecationWarning filter and warning-emission protocol is not newly claimed by
this value representation change.

Executable regression witnesses cover generated source, alias admission,
malformed payloads, unchanged native/WASM/LLVM capability profiles, singleton
observations, and embedded-helper namespace isolation. Tests and generated
artifact freshness must be run by the single integration proof lane; the
private source proposal itself is not a passing proof receipt.
