# CPython ABI fixture contracts

These integration fixtures call the real ABI entrypoints, carriers, ownership
transactions, and type slots. Their runtime hooks supply only the capabilities
needed by a cohort; CPython 3.12 behavior is the independent assertion oracle.
They do not substitute for native or WASM execution of the full Molt runtime.

## Initialization and identity

Runtime hook installation is process-first-wins. Every test in one integration
binary must install the same table. Model per-test failures inside that table
with thread-local capability state, not a competing installation.

Use `prepare_runtime_class_abi_test_thread` when a cohort needs managed scalar
classes or normalized exception values. It installs class observation and
subtyping alongside the thread-state transaction, then binds canonical builtin
classes before any C-API callback. Every `fake_runtime::wire` consumer uses this
boundary, including native from-spec fixtures that also need runtime-owned
strings and dictionaries. Scoped transactions and custom class-hook providers
explicitly call `prepare_class_bindings` after attachment. Bare native fixtures
without class hooks retain `prepare_abi_test_thread`.

The shared model observes bool as a subtype of int and distinguishes class
objects from instances. Its class anchors resolve to canonical physical C type
shells; they do not claim the runtime Type storage tag or an unpublished runtime
namespace/MRO graph. A runtime class observation never initializes bindings
inside readiness. Specialized heap registries provide their own `classify_heap`
and value-access hooks.

## Value operations and errors

`fake_numbers` supplies bounded inline numeric conversion and declaring-class
comparison capabilities. Heap-bigint fixtures supply their explicit values
through the same unary conversion hook. The common reflected comparison model
calls the installed declaring-class hook; that slot returns the canonical
NotImplemented identity for an unsupported operand. Generic comparison resolves
two unsupported slots by identity for equality/inequality and TypeError for
ordering, without leaking NotImplemented through the public API.

Keep exception allocation, string identity, and storage access coherent.
`take_current_error_text` consumes the normalized exception and renders its real
value through `PyObject_Str`; it is not a text-only error side channel. An
allocation-failure test deliberately denies string allocation, while tests that
assert exception identity or text must supply the relevant capability.

Physical C fixtures must have a live refcount, a truthful metatype, and storage
for their actual layout. An unhashable type uses `PyObject_HashNotImplemented`:
a NULL slot on an unready type can instead inherit hashing during readiness.
Construct a lone surrogate through `PyUnicode_FromKindAndData`, not invalid
UTF-8 passed to `PyUnicode_FromStringAndSize`.

The semantic references are CPython tag `v3.12.0`: `Python/errors.c`
(`PyErr_SetString`, normalization), `Objects/longobject.c` (checked integer and
floating conversion), `Objects/floatobject.c` (coercion and comparison),
`Objects/typeobject.c` (readiness and hash inheritance), `Objects/object.c`
(hash dispatch), `Objects/unicodeobject.c` (UTF-8 ingress and kind/data
construction), and `Objects/tupleobject.c` (element-wise rich comparison).

Architecture witnesses follow the current include graph and source owner.
Transport forwarding macros are not duplicate local classifiers. Tuple
write-once exports must remain distinct from checked replacement setters.

Numeric protocol fixtures use `fake_runtime`'s existing reference-counted owner
for boxed integer, float, and complex values, including payload extraction and
view admission. C-only construction/layout fixtures may leave numeric ownership
absent; a semantic crossing must then fail explicitly. Do not use token-only
heap addresses to claim numeric ownership. `wire_numeric` supplies the complete
numeric payload/refcount/mark contract, while `wire_sequences` additionally owns
container edges. Custom comparison or arithmetic fixtures keep independent
literal oracles over these owned values.

RuntimeHooks absence-selected fields are `Option<callback>`. Fixture overrides
install `Some(callback)`; absence is `None`, never a function-address comparison.
The invocation methods select the existing failure callback only for `None`.
The explicit module table is built at runtime and transports the same schema.
