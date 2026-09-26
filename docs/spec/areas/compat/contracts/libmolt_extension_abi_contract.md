# libmolt Extension ABI Contract
**Spec ID:** 0217
**Status:** Draft
**Owner:** runtime + tooling
**Goal:** Define the stable ABI boundary and bounded source-compat header model
for C/C++ extensions recompiled against Molt.

---

## 1. Principles
- `libmolt` is a recompile target, not a `libpython` drop-in.
- Stable ABI and source-compat are different promises and must stay separated.
- `MOLT_C_API_VERSION` versions the stable ABI tier only.
- Compatibility overlays may grow to unblock real extension builds without
  implying CPython ABI compatibility.
- Private/generated upstream headers are never part of the `libmolt` contract.
- Ecosystem compatibility is about primitives, wiring, and integration. The
  extension ABI exists so upstream package extension sources can be recompiled
  against Molt, linked to Molt runtime symbols, staged through Molt package
  custody, and tree-shaken to the reachable user-program closure. It is not a
  mandate to recreate NumPy, SciPy, pandas, or other package APIs in Molt
  Python.

---

## 2. Contract Tiers

### 2.1 Tier A: Stable ABI
- Canonical header: `include/molt/molt.h`
- Contract:
  - opaque `MoltHandle`-based object model
  - exported `molt_*` runtime symbols
  - `MOLT_C_API_VERSION`
- Stability promise:
  - major-versioned
  - intended to remain small, explicit, and toolable
  - the only header tier that downstream code should treat as ABI-stable

### 2.2 Tier B: CPython Source-Compat Facade
- Canonical entrypoints:
  - `include/Python.h`
  - `include/molt/Python.h`
  - small legacy forwarding headers such as `datetime.h`, `frameobject.h`,
    `pymem.h`, and `structmember.h`
- Contract:
  - source-level compatibility shims for high-value extension code
  - maps `Py*` names onto `molt_*` runtime primitives, helper macros, and
    fail-fast stubs where semantics are still missing
- Stability promise:
  - bounded and documented
  - not a frozen ABI surface
  - may expand between releases without changing `MOLT_C_API_VERSION`

### 2.3 Tier C: Package Header Custody
- Current focus:
  - `numpy/*`, SciPy, pandas, and other package-owned headers come from the
    package's own source/build plan include dirs.
  - Generated package headers and include-only sources are materialized by the
    upstream build system or source-plan custody, not checked into Molt.
- Contract:
  - unblock real-world ecosystem builds without shipping package header clones
  - fail closed when the package build/source plan lacks a required generated
    header, with evidence pointing to the missing package-custody artifact
- Stability promise:
  - package-owned header semantics track the package version being recompiled
  - Molt owns only the libmolt/CPython-ABI C API tier and package-custody wiring

### Shared target C data model and linked headers

Both Python header transports consume `include/molt/shared/`: one scalar object
layout, GIL-state enum, and target C data-model authority. `SIZEOF_VOID_P`,
`SIZEOF_INT`, `SIZEOF_LONG`, `SIZEOF_LONG_LONG`, `SIZEOF_SIZE_T`, and `LONG_BIT`
are preprocessing integer constants derived from the target standard headers,
not the build host. Conflicting definitions fail compilation. This distinguishes
Windows LLP64 from Linux/macOS LP64 and wasm32 ILP32 without equating C `long`
with a pointer-sized word.

For the linked CPython-ABI tier, install
`runtime/molt-cpython-abi/include/` and `include/molt/shared/`, and pass both as
include roots. Their relative location is not prescribed. Do not add the
source-compat `include/` root to resolve linked-tier dependencies. For the source
tier, install `include/` with its nested shared directory intact. Neither header
uses a source-checkout-relative path to reach another tier.

Extensions linking a shared runtime/ABI image define
`MOLT_CPYTHON_ABI_SHARED=1`. Both transports use the same `PyAPI_DATA` policy:
Windows imports data through the image's import library; static linking and WASM
retain plain external declarations. This option does not select a different
object model, load libraries, or search for an ambient Python installation.

`runtime/molt-cpython-abi/src/abi_types.rs` remains the Rust `repr(C)` authority.
`tools/gen_cpython_abi_layout.py` projects struct sizes, offsets, and integer
field widths for the target data models; the linked header and runtime C build
compile these assertions using the same target compiler/sysroot as the shims.
`tp_flags` is C `unsigned long`; `tp_version_tag` is C `unsigned int`.

This does not broaden version admission: Molt targets Python 3.12+ semantics,
but the linked header currently declares the CPython 3.12 object layout.
Source-extension admission must match that declared version; successful scalar
layout checks do not establish CPython 3.13/3.14 binary compatibility, package
support, or execution on an unverified target.

---

## 3. Explicit Exclusions
- No binary compatibility with CPython extension wheels.
- No promise that extensions using CPython private structs or direct object
  layout access will compile or run.
- No third-party package headers are shipped by Molt. Private/generated
  third-party headers must be provided through package/source-plan custody.
- No silent fallback to CPython or host Python at runtime.

---

## Value Presence At The Runtime Boundary

Runtime value handles are boxed Python values, not nullable pointers. In
particular, float `+0.0` has handle bits zero. Typed results use their status;
exception snapshots use their presence masks. Neither may infer absence or
failure from a present payload. Inactive snapshot slots must be zero and scalar
exception fields travel in separate native-width lanes.

`ExceptionSnapshot` in `runtime/molt-cpython-abi/src/hooks.rs` owns structural
layout/mask validation and present-edge enumeration for both capture and commit.
The runtime additionally checks field types and the recipient's immutable layout
before publishing the whole state. Capture, C projection, rollback, and commit
preserve each reference occurrence, including aliases, without a second validator
or payload-based ownership test.

Power's optional modulus follows the same rule: the C API's absent argument is
projected to canonical Python `None`; a supplied numeric zero remains a value,
and failed argument conversion remains failure. This does not change the
language's integer-only modular-power contract.

Numeric protocols share the bridge's observed-object classification: a managed
view is committed before observation, foreign identity stays explicit, and a
failed commit terminates dispatch. It must never become a foreign-slot retry or
an omitted argument. Physical numeric projection consumes that classification
and preserves aliased operand identity.

These contracts require behavioral proof at the actual runtime/ABI boundary;
host unit tests alone do not certify native/WASM or package compatibility.

### Extension initialization and callable ownership

Static native/WASM and the explicitly enabled dynamic bridge loader invoke
`PyInit` inside one runtime-owned initialization transaction. The caller's qualified
module name owns the import identity; a shared library's initializer symbol
uses only its final component. `create_module` receives the actual import spec
and returns the real C module without executing its slots. A normal import
publishes that same object before `Py_mod_exec`; a direct `exec_module` call
preserves the caller's cache and `sys.modules` state. Failure unwinds only the
transaction's own publication and references. Extension-raised exceptions keep
their type and identity; result/error contract violations produce `SystemError`
with the original exception as context. Frontend module bodies must not publish
extensions a second time. Reentrant or repeated loader execution does not replay
slots; direct C calls to `PyModule_ExecDef` may deliberately execute them again.
Multi-phase module state is allocated at first execution, not creation, and is
preserved across direct executions. Single-phase module construction does not
register an interpreter-owned state root; successful import commit owns it.
Slotted definitions cannot enter the single-phase `PyState` registry. Module
construction installs the definition's documentation through the shared module
API in both initialization modes.

`importlib.machinery.ModuleSpec` and extension initialization share one ordinary
runtime-owned class. Bootstrap-free initialization constructs that class, not
a module-shaped surrogate. A supplied spec retains its identity, and its
`parent` owns package metadata rather than a second name-splitting rule.

The runtime scopes and restores the initializer's package-name context even on
failure or nested imports. Only a matching single-phase definition's leaf name
consumes that context. The IR carries an owned module result, never a raw C
pointer across an exception edge. Native addresses and WASM table relocations
both call the same runtime initializer boundary.

Both public header transports share module-definition/C-callable layouts and
route module lifecycle and callable construction to the same linked ABI entry
points. `PyModuleDef_Init` establishes the canonical definition type; matching a
type name or guessing a struct from trailing memory is not initialization.
The WASM ABI generator reads the linked header's local include closure and
fingerprints those dependencies, so moving declarations into shared headers does
not remove their signature authority or leave cached projections stale.
Native embedding must link runtime and ABI into one image. Copying a hook table
between an ABI DLL and a statically linked runtime does not merge their object
identity, type singletons, exception state or finalization authority.
Managed semantic type queries use the runtime's actual class edge, shared with
exception projection. Builtin bindings and heap-class projections retain their
canonical identity; a generic physical carrier is not a second Python class.
The carrier's physical `ob_type` remains an honest layout discriminator, and a
failed class lookup stops dispatch while preserving the original exception.
Managed Python calls use the runtime call authority; semantic class identity
does not authorize invoking a native layout constructor on a managed view.
Concrete C callable layouts retain their C calling convention, and native
extension types retain their native slots. Dynamic-load diagnostics describe
the shared initialization transaction, not an inferred raw PyInit return;
the pending C exception retains the actual failure.

Managed `str` and `repr` also use the runtime protocol through owned-result
hooks, preserving subclass behavior, result identity and exceptions without
an ABI-local scalar/string formatter. Native objects retain native slots.

`builtin_function_or_method` and its `builtin_method` subclass have canonical
ABI type bindings. Runtime-defined builtins carry real vectorcall storage;
they do not fabricate a native `PyMethodDef` or receiver. Semantic type checks
and vectorcall work across both representations. `PyCFunction_Check` and raw
`PyCFunction_Get*` implementation extraction admit concrete C-defined callables
only; requesting raw C metadata from a runtime-defined builtin raises `TypeError`.
That extraction is not part of the verified runtime-builtin surface.

Module method tables use the same `PyCFunction` construction and managed-view
lifetime as directly constructed C callables. Runtime container ownership must
retain the concrete C layout and its receiver/class/module edges; dropping a
temporary C reference cannot turn it into an opaque object or leave dangling
member pointers. The physical `m_module` member is the sole `__module__`
authority for managed C functions; runtime attribute reads, assignment,
deletion and cycle GC consume that same owned edge. Ordinary Python function
metadata retains its runtime protocol, not a second name registry.

Attribute mutation has an explicit deletion flag at the internal hook boundary;
the C API's NULL deletion sentinel never aliases the Python value `0.0`.
Successful runtime mutation returns Python `None`, while its C adapter returns
status zero. Foreign-call failures transfer the original C exception after
error-preserving temporary-owner cleanup. `PyDict_SetDefaultRef` permits a NULL
result sink without acquiring an unused result reference; this source API
contract does not expand the declared CPython binary-layout version.

---

## 4. Tooling Contract
- `molt extension build` must record the targeted header contract in
  `extension_manifest.json`.
- `molt extension scan` must evaluate support against an explicit, curated list
  of contract headers rather than an unbounded recursive header crawl.
- Public overlay growth must stay compile-validated with representative source
  probes; adding a header to the contract does not imply runtime/ABI parity.
- C-API scan green is only the first gate. A package support claim must also
  prove source compilation, object link, package-native artifact staging,
  import execution, module-state lifecycle, deterministic runtime behavior, and
  binary closure for the claimed reachable path.
- Build/link tooling must model object closure explicitly: compile and link only
  extension objects, symbols, data tables, generated C/Cython outputs, and
  runtime features proven reachable from the user's entry program and admitted
  package imports. Whole-package linking is not an acceptable substitute for
  missing reachability facts.
- Tooling must report the distinction between:
  - stable ABI headers
  - source-compat headers
  - excluded private/generated headers

---

## 5. Practical Scope
- The goal is not “compile all Python extensions” in the CPython sense.
- The goal is:
  - compile extensions that can be recompiled against `libmolt`
  - preserve a narrow stable ABI core
  - add bounded source-compat overlays for high-value ecosystems
  - make high-value ecosystems green by improving shared ABI/import/storage
    primitives, not by cloning their Python APIs locally
  - reject private/generated upstream build dependencies unless Molt chooses to
    ship an explicit compatibility overlay for them

This means:
- simple or Limited-API-style extensions should converge on `molt/molt.h` plus
  a small facade set
- high-value ecosystems such as NumPy may require additional source-compat
  overlays
- extensions that fundamentally require CPython internals remain out of scope
  for `libmolt` and belong, if anywhere, in the explicit bridge policy lane

---

## 6. Relationship To Other Specs
- C-API v0 surface: `docs/spec/areas/compat/surfaces/c_api/libmolt_c_api_surface.md`
- C-API symbol coverage: `docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md`
- CPython bridge policy: `docs/spec/areas/compat/contracts/cpython_bridge_policy.md`
