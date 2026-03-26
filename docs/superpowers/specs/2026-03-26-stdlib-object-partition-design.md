# Stdlib Object Partition Design

## Goal

Re-enable native stdlib object caching without unresolved cross-object symbols by
fixing ownership of runtime-visible init/dispatch symbols and by making native
link invalidation account for the linked stdlib object.

## Current State

The current native backend path already contains dormant support for splitting
stdlib compilation into `MOLT_STDLIB_OBJ` in
[runtime/molt-backend/src/main.rs](/Users/adpena/Projects/molt/runtime/molt-backend/src/main.rs),
and the native linker path already links that object when present in
[src/molt/cli.py](/Users/adpena/Projects/molt/src/molt/cli.py).

That split is disabled in the CLI because the current ownership heuristic is too
coarse:

- every `molt_init_*` symbol is classified as "user" in the backend;
- `molt_isolate_import` and `molt_isolate_bootstrap` are runtime ABI roots;
- stdlib init bodies therefore leak into `output.o` instead of living behind the
  stdlib object boundary;
- the intended stdlib/user partition is not stable enough to re-enable.

There is also a separate correctness gap: the native link fingerprint omits the
content of the linked stdlib object, so changing `MOLT_STDLIB_OBJ` in place can
leave a stale linked binary if `output.o` and the link command string do not
change.

## Design

### 1. Ownership Boundary

Treat the native object split as:

- `output.o` owns user code and the required runtime ABI roots;
- `MOLT_STDLIB_OBJ` owns stdlib implementation bodies, including stdlib
  `molt_init_*` functions;
- the final native link resolves cross-object references by linking
  `output.o + MOLT_STDLIB_OBJ + runtime`.

The backend classifier in
[runtime/molt-backend/src/main.rs](/Users/adpena/Projects/molt/runtime/molt-backend/src/main.rs)
must stop blanket-keeping every `molt_init_*` symbol in the user object.

The user object should keep only:

- entry roots: `molt_main`, entry-module trampoline/init roots, `molt_init___main__`;
- runtime ABI roots: `molt_isolate_import`, `molt_isolate_bootstrap`;
- true entry-module init functions needed for direct startup.

Stdlib `molt_init_*` bodies should remain in the stdlib object and resolve
through the final link step rather than by co-locating everything in `output.o`.

### 2. Re-enable `MOLT_STDLIB_OBJ` In Native Compile Path

The CLI native backend subprocess path in
[src/molt/cli.py](/Users/adpena/Projects/molt/src/molt/cli.py) should set
`MOLT_STDLIB_OBJ` again for eligible native builds so the backend can use the
split path it already implements.

This must remain disabled for:

- wasm;
- transpile targets;
- any path where the backend env is intentionally absent.

### 3. Link Fingerprint Correctness

When `MOLT_STDLIB_OBJ` participates in the final native link, the link
fingerprint in
[src/molt/cli.py](/Users/adpena/Projects/molt/src/molt/cli.py)
must include the stdlib object path in the hashed input set, not just in the
command line string.

That makes relinking deterministic when the cached stdlib object content changes.

## Verification

Add focused tests in
[tests/cli/test_cli_import_collection.py](/Users/adpena/Projects/molt/tests/cli/test_cli_import_collection.py)
for:

- backend split classification: stdlib init functions are not retained in the
  user object solely because they match `molt_init_*`;
- CLI native compile path exports `MOLT_STDLIB_OBJ` only for eligible native
  backend compiles;
- link fingerprint changes when the linked stdlib object changes.

Run targeted CLI tests first, then the full CLI import-collection test file.

## Non-Goals

- redesigning the isolate ABI;
- introducing multiple stdlib archives or per-module archives;
- changing wasm or transpile backend behavior;
- broad documentation refresh beyond the affected workflow/docs paths.
