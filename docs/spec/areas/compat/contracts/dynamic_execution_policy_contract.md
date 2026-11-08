# Dynamic Execution And Reflection Policy Contract
**Spec ID:** 0216
**Status:** Active
**Owner:** frontend + runtime + tooling
**Goal:** Prevent accidental expansion of high-dynamism semantics that can erode AOT performance, determinism, and deployability.

---

## 1. Current Policy (Default, Active)
For compiled Molt binaries, the [Python contract](verified_subset_contract.md)
limits excessive dynamism that obstructs efficient ahead-of-time compilation
and standalone execution. Verification records implementation evidence separately
from that intended scope. The restrictions concern:
- unrestricted `eval`/`exec` execution paths
- runtime monkeypatching as a general semantic compatibility goal, including
  reassigning `__bases__` once a class's layout is fixed: class layouts, method
  resolution orders and devirtualized calls are compiled against the bases a
  class is created with, so a reassignment would invalidate them across the
  program. The assignment raises `TypeError`; differential cases that exercise it
  carry the expected-failure policy below.
- replacing `builtins.__build_class__` with a noncanonical builder, or directly
  calling that builder: Molt compiles class layouts, method resolution and
  closures against the static class-body model. Arbitrary builders could
  defer, repeat or replace that body and invalidate those compiled facts.
  These operations raise `TypeError` (the `dynamic_execution_policy` scope). Restoring the
  canonical binding before a class statement is supported; a missing binding
  raises `NameError`, as CPython's `LOAD_BUILD_CLASS` does.
- unrestricted reflection/introspection lanes that block static reasoning
  including `f_locals` on a traceback's class frame whose body was proved to
  run no Python callback and no namespace reflection. Such a body can keep
  class bindings in SSA; reconstructing its elided mapping would require
  preserving every binding solely for a later traceback observer. This case
  raises `TypeError`. Bodies that can reflect or run callbacks retain their
  live mapping and support namespace reflection.

Each exclusion requires a concrete performance or compilation rationale. Missing
implementation, a slow implementation or a failing conformance test cannot justify
an exclusion. Behavior inside the contract remains a correctness and performance
obligation until implemented and verified.

This aligns with:
- `docs/spec/areas/core/0000-vision.md` (Tier 0 constraints)
- `docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md` (intentional break policy)

## 2. Allowed Surface (Now)
- Restricted, deterministic runtime lanes that do not widen dynamic execution semantics.
- Reflection/introspection support that is explicitly scoped and test-backed.
- Capability-gated behavior that is already part of approved contracts.

The carve-outs are not a blanket ban on mutable globals, object attributes,
function defaults, or code/closure introspection. Each supported operation must
preserve its own binding, ownership, invalidation, exception, and version
contracts; support on one backend does not grant it to another. Consult the
[language surface index](../surfaces/language/language_surface_matrix.md) and
[call binding contract](call_argument_binding_contract.md) for those boundaries.

The Python 3.13+ copy protocol follows these PSF-2.0 upstream authorities,
with the same producer bindings in 3.14:

- [`Lib/copy.py: replace`](https://github.com/python/cpython/blob/v3.13.9/Lib/copy.py)
  reads `obj.__class__.__replace__`; absence raises `TypeError`.
- [`collections.namedtuple`](https://github.com/python/cpython/blob/v3.13.9/Lib/collections/__init__.py)
  binds `_replace`; [`dataclasses._process_class` and `_replace`](https://github.com/python/cpython/blob/v3.13.9/Lib/dataclasses.py)
  preserve user overrides and reconstruct through the initializer, including
  frozen records, InitVars and the `init=False` prohibition. Molt's public
  `dataclasses.replace` and generated method share `molt_dataclasses_replace`;
  the compiled caller supplies the canonical `MISSING` sentinel explicitly,
  retaining its binding when `sys.modules["dataclasses"]` is removed or replaced.
  The [compatibility-error authority](../../../../design/foundation/COMPATIBILITY_ERRORS.md)
  owns the 3.12 ValueError / 3.13+ TypeError transition.
- [`namespace_replace`](https://github.com/python/cpython/blob/v3.13.9/Objects/namespaceobject.c)
  constructs the subtype without arguments and updates dictionaries directly.
- [`date_replace`, `time_replace`, `datetime_replace`](https://github.com/python/cpython/blob/v3.13.9/Modules/_datetimemodule.c)
  and [`_pydatetime`'s three `replace` aliases](https://github.com/python/cpython/blob/v3.13.9/Lib/_pydatetime.py)
  preserve the dynamic type.
- [`Parameter.replace` and `Signature.replace`](https://github.com/python/cpython/blob/v3.13.9/Lib/inspect.py)
  distinguish omitted fields from explicit empty sentinels and revalidate through
  the subtype constructor.

Code objects support `replace(**changes)` on every target and `__replace__`
from Python 3.13. The canonical typed field table is
[`CODE_REPLACE_FIELDS`](../../../../../runtime/molt-runtime/src/object/layout.rs).
It follows CPython [3.13.9 `code_replace_impl` and `code_methods`](https://github.com/python/cpython/blob/v3.13.9/Objects/codeobject.c)
and [3.14.0](https://github.com/python/cpython/blob/v3.14.0/Objects/codeobject.c):

| Field class | Keywords | Replacement contract |
| --- | --- | --- |
| Metadata | `co_filename`, `co_name`, `co_qualname`, `co_firstlineno` | Clone metadata while retaining the compiled entry, trampoline, frame slot, signature, lexical layout and execution policy. |
| Execution | `co_argcount`, `co_posonlyargcount`, `co_kwonlyargcount`, `co_nlocals`, `co_stacksize`, `co_flags`, `co_code`, `co_consts`, `co_names`, `co_varnames`, `co_freevars`, `co_cellvars`, `co_linetable`, `co_exceptiontable` | An explicitly unchanged value succeeds. A changed value raises `TypeError` before cloning: compiled code has no CPython bytecode or interpreter frame (spec 0216). |

The line and exception tables describe CPython bytecode offsets, so changed
tables belong to the execution class. The complete keyword/type validation
precedes the policy refusal. Molt code
exposes empty bytecode and bytecode exception tables and zero interpreter stack
size; these do not encode an interpreter or translate CPython bytecode. Metadata
replacement cannot change callable execution identity or module execution flags.
Metadata retains string subclasses by identity, including subclasses with
mutable attributes. Code publication checks field shapes through
`CodeMetadataSlot`, then promotes GC tracking before publishing a mutable child.
The generated `code_dynamic` heap projection keeps exact immutable metadata
untracked. Existing code edge traversal and immutable-owner cycle handling own
collection; metadata replacement adds no separate release implementation.
The changed-execution differential carries the policy metadata below; ordinary
metadata, unchanged values and invalid keywords remain parity obligations.

## 3. Tooling Guardrails
- Each differential test that relies on intentionally unsupported dynamism
  declares the complete policy at its source:
  `# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy`.
- `tools/compat/test_policy.py` is the canonical parser and projection authority;
  consumers discover the scope through `verification_scope_paths(...)` rather
  than maintaining path manifests.
- Lint-time policy checks fail if scope metadata, required policy documents, or
  the expected-failure contract drifts.
- The dynamic-policy guard checks the concrete fail-closed control-flow and
  diagnostic evidence in runtime import execution paths. A comment or marker is
  not proof of enforcement.

## 4. Future Enablement Gate (Explicitly Deferred)
Future support can be considered only behind a capability-gated, opt-in path after all of the following:
1. Documented utility analysis (which libraries/workloads are blocked today).
2. Reproducible native+wasm performance analysis showing acceptable overhead.
3. Spec updates across contracts + status + roadmap in the same change.
4. Targeted parity tests, determinism checks, and memory/regression evidence.
5. Explicit user approval before implementation begins.

Until these gates are satisfied, dynamic execution and unrestricted reflection remain policy-deferred.
