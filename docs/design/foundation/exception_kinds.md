# Typed Exception Identity

**Status:** live (HF-207). `runtime/exception_kinds.toml` is the one authority
for every exception class the runtime raises, matches or constructs.
`tools/gen_exception_kinds.py` renders it; `tests/test_gen_exception_kinds.py`
proves the closed types equal the table.

## The defect class

The runtime used to name an exception by its spelling:
`raise_exception(py, "ValueError", msg)`. Each raise scanned a 73-row table by
string compare, locked a `HashMap<String, u64>` and hashed the name. A name
outside the table minted a new `Exception` subclass named by the whole spelling,
with no `__module__`, so `except binascii.Error`, `except
zoneinfo.ZoneInfoNotFoundError` and `except tarfile.ReadError` missed the
runtime's raise. An undotted name first read the `builtins` dictionary and
cached the hit for good, so `decimal` wrote its signal classes into `builtins`
to make its raises land, which leaked `InvalidOperation` into every module.
Module classes with constructors (`URLError`, `HTTPError`, `InvalidURL`,
`FrozenInstanceError`) imported their module and called `getattr` on every
raise. About 6,200 raise sites spelled 52 names.

## The design

One declarative table generates closed Rust types over one dense `u16` code
space (code 0 is reserved):

- `BuiltinExc`: classes the runtime constructs, on first use, from their
  generated schema row. Most live in `builtins`; some are native classes of
  another module, as CPython's C modules create `_io.UnsupportedOperation`,
  `socket.gaierror` or `binascii.Error`. The row owns bases, physical layout,
  render slots, `__module__`, builtins binding and target-version gating.
- `ModuleExc`: classes a Python stdlib module defines. On the first raise of a
  row, the runtime takes the class from its owner, which must already be in
  `sys.modules` (nothing is imported), checks its `__module__`, `__qualname__`
  and base, and keeps it in the row's slot for the life of the runtime. A
  mismatch or a missing owner raises `SystemError`.
- `ExcKind`: `Builtin(BuiltinExc) | Module(ModuleExc)`; `code()` is the slot
  index and the bridge transport.

Every raise passes a typed kind: `raise_exception(py, BuiltinExc::ValueError,
msg)`. The class is one acquire load from `RuntimeState::exception_kind_slots`
indexed by the code. Each slot fills on first use: a builtin from its schema
row, a module row from its owner. Raising a module row whose owner has not run
raises `SystemError`; nothing is minted and no other namespace is searched. A module kind raises by calling its class
with the message, so the class's own `__init__` runs, as `raise Cls(msg)` does;
structured raises (`CalledProcessError`) pass their fields.

Satellite crates raise through their bridge with `kind.into().code()`; the
runtime decodes the code with `ExcKind::from_code`, and an unknown code is a
`SystemError`, never a guess. `rt_exception_class(kind)` gives a satellite the
owned class when it must construct the exception with its own arguments
(`HTTPError`). The serial `RuntimeVtable` carries codes (ABI
version 11) and no longer has a binascii class slot.

The frontend's builtin constructor tags are `BuiltinExc` codes, generated into
`src/molt/frontend/exception_kinds_generated.py`, so the frontend and runtime
agree by construction. The hand ordinal table and its drift scraper
(`tools/check_table_drift.py` category `exception-ordinals`) are retired.

### Remaining string boundaries

Two boundaries still carry spellings, and both resolve only through the
generated table (`BuiltinExc::from_name` and the alias rows), never by minting:

- the CPython ABI, whose `PyExc_*` objects are named by the C API;
- the string `exception_new` and `exception_class` op kinds. They retire when
  the closed `OpKind` lands (lane `opkind-stage4`): the op then carries a
  `BuiltinExc` code as its payload, the frontend emits it from
  `BUILTIN_EXCEPTION_CODES`, every backend forwards it as an integer like the
  `exception_new_builtin` tag, and `exception_type_bits` and the string op kinds
  are deleted through `tools/op_kinds/census.py`.

## Prior art

- **CPython.** Builtin exceptions are static type objects (`PyExc_ValueError`);
  C code raises with `PyErr_SetString(PyExc_ValueError, msg)`, passing the type,
  never a name. A C module keeps its exception classes in module state: `binascii`
  creates `binascii.Error` with `PyErr_NewException` at exec; `_asyncio` imports
  `asyncio.exceptions` once at module exec and caches `CancelledError` and
  `InvalidStateError` in its module state; `_zoneinfo` fetches
  `zoneinfo._common.ZoneInfoNotFoundError` the same way.
- **PyPy.** Interpreter-level code raises `OperationError(space.w_ValueError,
  ...)`: the object space holds typed fields for builtin exception types.
- **RustPython.** `vm.ctx.exceptions` is a struct of typed fields
  (`value_error`, `type_error`, ...); `vm.new_value_error(msg)` raises.
- **PyO3.** Each builtin exception is a Rust type (`PyValueError::new_err`);
  `create_exception!` defines module types with a lazily initialized type
  object. `import_exception!` imports a Python-defined class by name on first
  use and caches it; Molt resolves the same way but never imports, so a raise
  cannot run another module's code.
- **HotSpot.** Well-known classes are an enum-indexed table (`vmClassID`); the VM
  throws `vmSymbols`-identified exceptions through it rather than by loading a
  class by name per throw.

Molt follows CPython's module-state model for Python-defined classes (resolve
once from the owner, check, keep) and HotSpot's enum-indexed well-known class
table for the runtime's own classes.

**Deviation from the HF-207 plan.** The plan bound module classes at owner exec
through one generated intrinsic per owner. Every intrinsic in
`runtime/molt-runtime/src/intrinsics/manifest.pyi` is also projected into the
WASM import tables (`tools/gen_wasm_abi.py` writes
`runtime/molt-backend-wasm/src/wasm_abi_generated/`), which the op-kind lane
owns while this change lands, so this change adds, removes and re-signs no
intrinsic. The runtime resolves a module row once, from its owner, at first
raise instead (PyO3's `import_exception!` with a once-cell, minus the import).
The difference is timing: a mismatch fails at the first raise, not at import.
Binding at exec remains available once the WASM tables can be regenerated; it
needs the twelve owner intrinsics and one call per owner module.

## Costs

Hot raise path, before: up to four string compares, a linear scan of 73 rows by
string compare, a mutex lock and a `HashMap<String, u64>` probe that hashes the
name; satellites also passed a pointer and length through FFI. After: four
`u16` compares for the bootstrapped roots and one acquire load indexed by the
code; satellites pass a `u16`. Cold costs: a builtin class is built once, as
before; a module row costs one `sys.modules` probe and three attribute reads,
once per runtime. A module raise calls its class, so its `__init__` runs, as in
CPython.

Measured on macOS arm64, release runtime, a compiled loop of 1,000,000
`int("abc")` raises caught as `ValueError` (five interleaved runs, median):
1.281 s before, 1.247 s after (about 34 ns less per raise). The same loop
with `raise ValueError("x")` does not use this path (its class comes from the
constructor tag) and stays at 11.7-11.8 s; CPython 3.12 runs the two loops in
0.36 s and 0.08 s, so the remaining raise cost lies elsewhere. The runtime
static library for that program shrank from 40,374,048 to 40,235,920 bytes and
the program from 29,799,256 to 29,766,168 bytes.

## Invariants and checks

- The generator rejects duplicate variants or spellings, a base named before
  its row, unknown layout roots or str slots, and a builtin row bound in
  `builtins` that belongs to another module.
- `tests/test_gen_exception_kinds.py` re-renders every output byte for byte and
  checks the code space, owner membership and frontend codes against the TOML.
- The finding programs `tests/differential/stdlib/exception_identity_*.py`
  compare each repaired raise with CPython on native and WASM.
