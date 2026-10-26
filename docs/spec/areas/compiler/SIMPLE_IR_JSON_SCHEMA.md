# SimpleIR JSON Schema Reference

Formal schema for the SimpleIR interchange format between the Python frontend
(`src/molt/frontend/__init__.py`) and the Rust backend
(`runtime/molt-ir/src/ir.rs`).

> Scope note: this document describes the current transport format only. The
> canonical IR architecture and representation contract live in
> `docs/spec/areas/compiler/0100_MOLT_IR.md`. The `type_hint` field is a
> transitional compatibility hint for transport consumers; it is not a
> substitute for representation-aware SSA. Scalar facts live on typed SSA
> values; an op that carries the retired `fast_int` or `fast_float` field
> fails to parse.

## Transport Formats

Saved-state dispatch uses `state_switch.state_targets`, an array of
`[saved_state_id, control_label_id]` pairs projected from TIR `StateDispatch`.
Saved states and control labels are separate identities; multiple states may
target one label. State IDs must be unique and each target label must exist
exactly once. An explicit empty array means no resume cases. Source IR may omit
the map before CFG lifting; terminal lowering always emits it, and code generation
must not reconstruct it from suspension syntax or physical instruction order.
When suspension operations remain, an explicit map must include the saved state
of every executable `state_yield` and the pending-state operand of every executable
`state_transition`. The ready/running state is not a resume requirement. An empty
map remains valid when no suspension can execute; unreachable suspension syntax
does not create cases. Missing cases are rejected, never filled by inference.
The map participates in the same serialized operation and function identity as
every other semantic IR field.

Terminal ownership lowers `state_yield` and `state_transition` to ordinary
returns and branches plus `state_set` (raw integer saved state), `is_pending`
(exact pending-sentinel predicate), and `task_wait` (existing scheduler wait
registration). These primitives do not terminate the activation or manage
invocation references. Shared DropInsertion owns those references; frame
`closure_store`/`closure_load` own persistence across invocations.

`molt build --emit-ir` writes the exact prepared backend input, including module
registry and all control-flow references. It does not renumber labels or saved
states for display: emitted IR must remain replayable, and diagnostic formatting
must not change the program being inspected.

Every input is exactly one complete frame. The backend accepts the explicit
codec names `json`, `msgpack`, `cbor` (when enabled), and `ndjson`; an unknown
explicit codec is an input error, not JSON selection. JSON accepts trailing JSON
whitespace and rejects any second value or other suffix. MessagePack and CBOR
require immediate EOF after the decoded value, including for buffered stdin.
Malformed input returns an error before target admission or emission.

Compiler-owned leases use content-addressed MessagePack for daemon, one-shot,
factgraph and JIT-image dispatch (`runtime/molt-ir/src/addressed_program.rs`):
one map with the header fields `code_slots`, `module_registry` and `profile`,
`function_digests` (one `bin` of 32-byte body digests in program order) and
`function_bodies` (one `bin` per shipped body). Each receiver hashes every
shipped body in the `molt.function-body.msgpack.v1\0` domain below; a body can
only fill the slot its own bytes name. One-shot receivers require every body.
The backend daemon fills the rest from its resident store and names the
positions it lacks (`docs/design/backend_compile_daemon.md`). JSON, CBOR and
NDJSON inputs keep their `functions` array, and JSON exports and the MLIR stdin
boundary retain JSON. All file receivers share regular-file admission and EOF
validation. File sizes are not bounded by the stdin ceiling. The daemon protocol
keeps its small control frame in JSON and admits only a cache probe that names
the program's digests (`function_digests`, hex) without an IR path, or a
compile with a nonempty MessagePack `ir_path`; inline `ir` is rejected,
including null. A cache hit need not open an unused lease. Native batch jobs
retain their separate complete JSON artifact/request decoding. None of these
boundaries adds a cache receipt or changes the semantic digest domain.

The boundary follows [Serde JSON's whole-reader EOF contract](https://docs.rs/serde_json/1.0.151/serde_json/fn.from_reader.html).
The binary value decoders require an explicit EOF check on their retained reader;
a successful value decode alone does not admit the entire input. NDJSON uses
one framing state machine with its existing function/program semantic checks.
This preserves the compiler admission separation described in
[architecture convergence](../../../design/foundation/77_architecture_convergence.md),
including [Cranelift's explicit IR invariants](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/docs/ir.md).
[LLVM bitcode's block lengths and END_BLOCK records](https://llvm.org/docs/BitCodeFormat.html)
provide a framing analogy: structural completion is explicit and separate from
the meaning of an IR record. They do not define Molt's JSON or NDJSON policy.

### Batch JSON

Single JSON object containing all functions:

```json
{
  "functions": [ ... ],
  "profile": { ... },
  "code_slots": { "module_function": 0 }
}
```

### NDJSON Streaming

One JSON object per line. `BackendIrDocument` admits the complete batch or
stream, including program metadata; `SimpleIR` carries its function stream.

```
{"kind":"ir_stream_start","profile":null,"code_slots":{"module_function":0}}
{"kind":"function","name":"molt_main","params":[],"return_abi":"void","ops":[...]}
{"kind":"function","name":"helper","params":["a"],"return_abi":"value","ops":[...]}
{"kind":"ir_stream_end"}
```

The first nonblank record must be `ir_stream_start`, followed by zero or more
body records and exactly one `ir_stream_end`. EOF is mandatory after the end
record; a final newline is optional. Blank lines (after trimming whitespace)
are allowed before, within, and after the frame. Every nonblank record must be
a JSON object with a string `kind`. Unknown body kinds are ignored for forward
compatibility; they cannot replace either envelope marker or follow the end.
Repeated starts, missing markers, and any nonblank suffix are rejected. Both
`SimpleIR` and `BackendIrDocument` use this framing authority while retaining
their distinct function-stream and complete-program validation.

---

## Top-Level: SimpleIR

| Field       | Type              | Required | Description                          |
|-------------|-------------------|----------|--------------------------------------|
| `functions` | `FunctionIR[]`    | yes      | Ordered list of function definitions |
| `profile`   | `PgoProfileIR?`   | no       | Optional PGO profile data            |
| `code_slots` | `object<string,u64>` | when code slots are used | Stable code-owner symbols mapped to unique dense program indices |

`code_slots` belongs to `BackendIrDocument`, alongside `module_registry`.
Indices cover exactly `0..count`; every slot operation names a bound symbol.
Unused bindings can remain after dead-function elimination. The map is part of
final application cache identity, but never of reusable function bodies or
shared object jobs. Native and LLVM objects import immutable per-owner binding
cells, defined once by the application metadata owner. WASM, Luau and Rust
source emission resolve the same symbols to immediate indices. Runtime slot
storage remains a dense per-isolate table.

The first function is the program entry root for shared dead-function elimination.
Additional runtime/linker roots come from the existing module registry and
runtime-entrypoint policy. Reordering functions can change entry identity;
producers must preserve the intended root.
Shared profile-guided ordering keeps that first function fixed and stably orders
only the remaining functions; hotness cannot replace semantic entry identity.

### PgoProfileIR

| Field           | Type       | Required | Description                     |
|-----------------|------------|----------|---------------------------------|
| `version`       | `string?`  | no       | Profile format version          |
| `hash`          | `string?`  | no       | Source hash for staleness check |
| `hot_functions` | `string[]` | no       | Defaults to `[]` if absent      |

---

## FunctionIR

| Field         | Type       | Required | Description                                           |
|---------------|------------|----------|-------------------------------------------------------|
| `name`        | `string`   | yes      | Mangled function name (e.g. `molt_main`, `__main__`)  |
| `params`      | `string[]` | yes      | Parameter names (SSA value names)                     |
| `return_abi`  | `"void" \| "value"` | yes | Immutable linkage return convention, independent of body exits |
| `ops`         | `OpIR[]`   | yes      | Ordered instruction sequence                          |
| `param_types` | `string[]?`| no       | Optional type annotations parallel to `params`        |
| `parameter_custody` | `("borrowed" \| "transferred")[]` | no | Entry custody parallel to `params`; absent means every parameter is borrowed |

The frontend may also emit `borrowed_params` (list of param names eligible for
Perceus-style borrow elision), but the backend does not require it.

`return_abi` is authored when the function is constructed and survives
optimization, partitioning, extern projection, and SimpleIR/TIR cache transport.
It is never inferred from the remaining `ret` operations or a refined semantic
return type. `ret_void` in a value-returning function returns boxed Python `None`;
in a void function it has no machine result. A value payload in a void function
is invalid. Removing every value-returning path, including making the whole body
unreachable, does not change the calling convention. Missing or unknown ABI
values are rejected, not reconstructed from the body.

Whole-function verification rejects every entry-reachable executable transfer
past the operation stream. Completion follows the canonical operation-level CFG,
not the last lexical instruction: both arms may return before a trailing `end_if`,
and a non-returning cycle has no falloff. Loop latches and TRY registration edges
retain their executable-versus-verifier distinction. Conditional fallthrough,
loop breaks, dispatch defaults, and missing source resume continuations are real
exits and cannot be lost when projecting edges onto operation indices. Partial
control-flow admission validates structure and labels without requiring complete
function exits. Explicit `state_targets` maps feed that same CFG authority and the
SSA block projection; suspension itself does not dispatch to every resume target.

`parameter_custody` declares, per parameter, who holds the argument's
reference (design 20 §1.6). A `transferred` argument moves into the
activation, which releases it at the binding's Python boundaries; a
`borrowed` one stays the caller's. The field names every parameter and at
least one `transferred` entry, never on the closure parameter; all-borrowed
custody is the absent field. It belongs to the function contract (version 4),
so every cache digest covers it. An extern declaration, a megafunction stub
and a native linkage row keep it, and megafunction chunks borrow every
parameter.

An `is_extern: true` declaration has empty `ops` and retains the same explicit
signature fields. Extern bodies do not carry synthetic `missing`/`ret` signature
instructions. Function-contract and TIR cache versions reject obsolete carriers;
old artifacts are not upgraded by guessing a signature.

Frontend artifacts separate executable `functions` from `function_metadata`,
a mapping from their canonical function symbols to assembly facts. Publication,
GPU source-origin evidence and app callable bindings belong to this sidecar;
they never enter executable function records or operations. Both standalone and
module serialization preserve this separation. The sidecar travels with the
source- and context-qualified frontend cache record, independently of reusable
function-body identity. Older cache schemas are misses, not repair requests.

The source publication envelope contains canonical `module_name`, SSA
`module_value`, integer `failure_label`, and the explicit operation boundary.
`molt.frontend.module_publication` owns validation. Serialization consumes its
internal single true boundary marker and records its index; assembly checks
that index against the original `frame_locals_set` before any body rewrite.
It validates module identity independently of the linkage symbol, including
initializers without native exports. Missing, orphaned, duplicate, false or
misplaced publication facts fail admission. A symbol collision does not permit
merging different Python modules.

GPU descriptor assembly reads its captured origin from the same symbol-keyed
sidecar, retaining existing source/AST/target/default comparison and lazy
validation. App export custody receives the retained app binding sidecar
explicitly after backend preparation and validates transferred parameters
against the final entries. Final backend input and `--emit-ir` contain no
assembly sidecar. Compiler preparation does not copy a function merely to
remove non-executable metadata.

This separation follows [LLVM's distinct metadata representation](https://llvm.org/docs/LangRef.html#metadata)
and [rustc's immutable, cheaply shared query results](https://rustc-dev-guide.rust-lang.org/query.html).
Molt's assembly facts can change native publication and export policy, so they
remain required, qualified compiler inputs; they are not discardable debug
annotations. The cost is one small sidecar entry per function that owns such
facts. It removes repeated body scans and metadata-only function replacements,
without a second IR, optimizer or runtime check. Lowering cache schema 9 stores these inputs in one tagged binary frame:
`["molt.lowering-record.v1", metadata_bytes, [[body_bytes, digest], ...]]`.
`molt.cli.function_records` owns sorted MessagePack encoding through msgspec.
Metadata retains the existing rich JSON value codec; executable functions never
pass through that codec. The outer decoder captures raw function records and
admits the source lease, context and metadata before materializing bodies.
Each body is verified against SHA-256 over the explicit
`molt.function-body.msgpack.v1\0` domain and its exact stored bytes. Older JSON
records are misses; there is no compatibility reader or hydration tier.

The writer encodes and hashes each function once. The reader materializes it
once and supplies a typed seed holding that exact object and its verified
digest. Assembly admits seed ownership before committing shared state. Backend
preparation retains seeds only for unchanged objects after copy-on-write
rewrites. One `FunctionDigests` owner then reuses those seeds and computes
new/replaced bodies on demand through the same codec. No symbol-name lookup or
child-process identity table can transfer ownership. The payload Merkle domain
is `molt.cache-payload.merkle.msgpack-functions.v2\0`; rich header encoding is
unchanged. `MOLT_VERIFY_FUNCTION_DIGESTS=1` deliberately re-encodes seeded bodies
once for compiler debugging, following
[rustc's incremental fingerprint verification](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html).
Normal builds pay no such verification walk. The backend lease reuses these
digests, so an unchanged body is neither encoded nor sent to a daemon that holds
it; worker object IPC remains a distinct measured cost.

The pipeline creates that owner after final backend rewrites and shares it
between optional binary-image analysis and every backend cache projection.
Analysis cache schema v3 hashes the original ordered payload through the same
Merkle composition, plus its generated analysis authority. It cannot reuse the
sorted artifact key: function order contributes to the reported function-order,
source-site and allocation-event digests. Header changes and source locations
also remain identity inputs. Key composition hashes the header and ordered body
digests; unchanged admitted bodies are not serialized again. A new or rewritten
body is encoded once by the shared owner. Cold analysis still walks the
operations it reports. This compiler-only reuse follows rustc's
immutable query-result fingerprint model linked above and adds no guest work.

All four source fact kinds share the existing SQLite database under
`<Molt cache>/toolchain_source_records`; record keys retain their complete
semantic inputs, and import requests complete against the current project
roots and custody after reuse. SQL applies each reader's byte bound before
payload allocation; lowering explicitly retains its uncapped role. There is
one decode per hit, no retained parsed-document cache, and no per-project copy.
Opaque payloads live in an ordinary rowid table with a unique `(kind, key)`
primary-key index. Each record's last use lives in a separate narrow
`recency` table (`WITHOUT ROWID`, indexed by `(used_ns, kind, key)`). A record
and its stamp publish in one transaction; a delete trigger retires the stamp.
A revision-and-use-fenced renewal updates only this small row, avoiding a
rewrite of the opaque payload. The original HF-515 measurement was 3.6 seconds
and 33.1 MB of WAL for 10.8k renewals in payload rows, versus 0.11 seconds and
0.7 MB in the narrow table. Due observations enter a bounded queue, as
[Cargo's deferred last-use tracking](https://github.com/rust-lang/cargo/blob/rust-1.90.0/src/cargo/core/global_cache_tracker.rs)
delays advisory writes. One nonwaiting transaction saves at most 4,096
observations at the batch boundary, normal owning operation exit or explicit
close. Reentrant operations share the outer owner; fresh identity scopes do not
flush a borrowed outer owner. Generator finalization skips these writes. A queued renewal is
advisory acceptance, not a successful SQL update; `save_renewals()` reports
actual changed rows. GC drops pending advisory renewals rather than performing
SQL writes from arbitrary finalization contexts. No generation-level retention
table or trim exists. [SQLite's layout guidance](https://sqlite.org/withoutrowid.html#when_to_use_without_rowid)
recommends small rows for `WITHOUT ROWID`, whose intermediate nodes carry record
content. Uncapped lowerings therefore live in table leaves and overflow pages;
the narrow key index preserves lookup fan-out.

Each schema owns `toolchain_source_records/<schema digest>/records.sqlite`.
The namespace is derived from the DDL rather than a manually incremented version;
concurrent schema changes cannot accidentally claim the same next version.
As rustc keys incremental state by compiler identity, a different schema starts an empty
optional cache, without migrating or reading another version's records. The
store derives its database, namespace and persistent lifetime lock from one
cache root. A shared lock in the existing cache control directory protects every
SQLite connection, including maintenance, until SQLite closes. Warm record
operations take no additional file lock or syscall. A contested retirement lock
is a retryable miss before SQLite opens. Explicit close releases the paired
SQLite connection and lease synchronously. Implicit weakref or generator-context
finalization closes SQLite first and parks its small lock handle for bounded
release at a normal cold admission or explicit close (or OS process exit); it never enters
Python file-lock mutexes from GC. This also covers abandoned context generators:
[CPython 3.12.15's `_PyGen_Finalize`](https://github.com/python/cpython/blob/v3.12.15/Objects/genobject.c#L63)
closes suspended generators, and [PEP 442](https://peps.python.org/pep-0442/)
includes their cyclic finalization. The short conservative lease extension avoids
reentrant deadlock without retaining SQLite page caches or adding a worker.

The existing pruner exclusively locks another digest namespace or a known prior
version namespace (v1–v3), then uses the
shared atomic namespace-retirement authority before physical deletion. A
partial reclamation remains isolated from any new live database and is retried
under the existing prune owner. A foreign digest may belong to newer code:
exclusive lifetime custody makes its retirement safe, but interleaved schemas
can lose idle cache warmth under byte/age pressure. Digest-schema participants
must retain the lifetime and recency-write protocol; incompatible protocols
require a distinct namespace grammar or identity. A post-rename reclamation failure is reported as
failed without crediting reclaimed bytes; malformed or changed residue remains
evidence while unrelated entries continue. A read-only sweep cannot recreate a
missing lifetime control and reports that store unavailable. The current digest and unknown future version namespaces remain outside file
eviction; current rows keep their bounded metadata/CAS policy. Flat legacy
stores cannot prove connection lifetime and are reported as
`legacy_source_record_state`, without automatic unlinking. New namespace
directories also prevent the previously deployed flat-file pruner from deleting
future stores. This follows [SQLite's live-file safety contract](https://sqlite.org/howtocorrupt.html#_unlinking_or_renaming_a_database_file_while_in_use):
a database and its WAL/SHM are one identity, never independent eviction leaves.

The earlier in-place schema change made older checkouts refuse every record
and repeatedly reconnect; the original namespace repair's measured build cost
is retained under HF-513. These are development-side cache costs; emitted
programs perform none of this storage or retention work.

A fresh
file at version zero enters one cold bootstrap owner through the existing
portable file lock; that owner rechecks admission, establishes incremental
vacuum and WAL, then publishes the schema, indexes and the version in one SQL
transaction. Failure rolls back the whole transition and refuses the optional
cache. An unknown version refuses the optional store, and the store disables
itself for the process at its first refusal instead of reconnecting for every
operation; a busy bootstrap owner remains a retryable refusal. Ordinary opens
read the admitted version and set connection-local synchronous mode; they
do not repeat persistent pragmas, DDL or the bootstrap file lock. Write maintenance
tries cold ownership once and preserves its nonwaiting refusal contract.
Read-only maintenance admits the same version inside its read snapshot and
never bootstraps the database.
This distinction follows [SQLite’s busy-handler contract](https://sqlite.org/c3ref/busy_handler.html):
a deadlock-prone lock upgrade may return `SQLITE_BUSY` without calling the
configured handler, so a larger timeout cannot serialize competing WAL
conversions. See [WAL busy cases](https://sqlite.org/wal.html#sometimes_queries_return_sqlite_busy_in_wal_mode)
and [application schema versions](https://sqlite.org/pragma.html#pragma_user_version).
The frontend operation owns its store connection; admitted record observations
and concurrent operations pin that same owner. A weak identity registry holds no
abandoned root alive, and the last owner closes its connection and statement
cache. Normal per-record calls reuse the operation's connection without another
registry lookup. This follows [CPython's weak-value ownership model](https://docs.python.org/3.12/library/weakref.html).
Connection release uses a weak finalizer that captures the connection and its
PID, never its store. The weak identity cannot resurrect a finalizing store
while SQLite closes outside the GIL. Explicit foreign-fork children refuse
inherited store and maintenance operations before locks or SQL. Their release
callbacks retain inherited connections inertly until process exit, including
directly constructed owners and maintenance cleanup; dropping a callback's last
connection argument could otherwise invoke CPython's C finalizer. This is
resource custody for quiescent foreign fork boundaries, not support for forking
in the middle of an SQLite operation. Normal compiler workers use fresh spawn
contexts. See [SQLite's fork restrictions](https://sqlite.org/howtocorrupt.html).
An admitted use refreshes a revision-fenced observation at most once per five
minutes; invalid, stale and replaced observations cannot renew another row.
The existing byte/age prune merges bounded metadata pages with file entries
under one budget. It holds one SQLite read snapshot while a separate nonwaiting
connection performs current-state CAS deletion in batches of at most 256 rows
and 1 MiB of logical payload (one larger record remains evictable). A batch
commits before file eviction or any iterator yield; only confirmed removals
reduce the budget. SQLite's typed BUSY family defers work without a retry;
LOCKED, read-only and other failures remain distinct. This removes per-row
transaction publication while bounding writer-lock scope. Later cold sweeps
continue reclamation even when they delete no new rows, draining up to 256
incremental-vacuum pages each time. Closing that cursor early would abandon
remaining page work, as [SQLite's pragma implementation](https://github.com/sqlite/sqlite/blob/version-3.53.1/src/pragma.c#L807-L823)
shows. The read snapshot pins WAL pages during the cold sweep; logical payload eviction and physical reclamation are separate
metrics, and peers can defer the latter. Corruption disables the optional cache
without unlinking files another process may own. See
[SQLite WAL snapshot and checkpoint semantics](https://sqlite.org/wal.html) and
[isolation](https://sqlite.org/isolation.html), and
[msgspec's typed decoding](https://msgspec.dev/usage).

The codec is an installed compiler dependency, never a dependency or check in
an emitted program. Its native-module closure and actual host SQLite library
belong to installed-delivery qualification under V1-06. In particular, concurrent
WAL users require the upstream WAL-reset fix (SQLite 3.51.3+, or the 3.44.6 and
3.50.7 backports); Python's version alone does not establish that library.
Available wheels do not qualify every supported Python/OS/architecture/ABI
cell. HF-266 remains open until actual consumers, build costs, and those
installed-delivery cells pass; bounded codec timings are not whole-build gains.

CLI assembly borrows the lowered function records and their operation lists.
The serializer emits two explicit artifact scopes. A standalone `program`
owns `molt_main` and code-slot allocation. A `module` record emits its canonical
`module_init_symbol`, symbolic code owners, and no `code_slots_init`; both
worker and serial lowering persist that form. Assembly rejects the old
program-shaped module form rather than repairing it. Parameter lists are
detached from generator storage at emission. The cached lowering schema changes
with this contract, so older records are misses.

The assembly owns its outer function list: a changed function replaces that
entry, and a changed operation replaces only its path through the function and
operation list. Unchanged function, operation and parameter-list identities are
retained. Code-owner validation, import-lane rewriting, native callable
publication and GPU descriptor finalization
follow this rule, including their failure paths. Parameter-type completion
copies a function only when its declared ABI needs completion.

This permits the same lowering result to participate in independent assemblies
without acquiring a previous program's mutations. It also preserves the stable
owners required for function-digest reuse. As in
[rustc's immutable query results and stable fingerprints](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html),
cached result reuse requires that later consumers do not rewrite the result.
There is no recursive frozen-container wrapper or whole-program deep copy:
the existing traversals validate the records and allocate only changed paths.
Serialization context belongs to one function walk, shared by its operation
handlers. Its fields are immutable; its output list and analysis set are the
same function-owned accumulators. Context construction therefore scales with
functions rather than operation count. This follows the
function's actual accumulator lifetime; it introduces no extra IR or optimizer.
These ownership rules apply inside the compiler and add no emitted-program work.

The verifier phase is explicit. `tools.check_ir_structure.verify_tir` accepts
only executable SimpleIR and retains strict Rust decoding.
`verify_frontend_tir` validates the sidecar and projects executable frontend IR
through the shared envelope authority before invoking the same Rust verifier.
It does not mutate cached frontend input or relax executable transport rules.
Source-corpus verification (`tools/verify_ir_suite.py`, including nightly and
portability runs) uses that same explicit frontend adapter. Diagnostic filters
must not turn invalid transport into a successful structural check.

---

## OpIR

All fields except `kind` are optional; absent fields deserialize to `None` or
their declared default. The typed Rust `OpIR` owns field decoding for JSON,
NDJSON and MessagePack. Unknown operation fields are rejected, including the
retired `effect_proof`, `ic_index` and nested `metadata` carriers. Frontend-only
annotations must be consumed before backend transport, not silently ignored there.

JSON float immediates use the non-finite tokens below; JSON output never replaces
a non-finite constant with null. Binary float immediates retain all IEEE-754
bits, including signed zero, infinities and NaN payloads. JSON's `"NaN"` token
does not encode a payload.

Attribute-cache sites use the stable function/source-operation identity already
transported through TIR. There is no process-global frontend allocator, reset,
wrapping slot number or raw-offset cache authority. Runtime attribute lookup
retains the requested name and follows the canonical Python lookup protocol;
site collisions must not substitute an attribute or an instance's value.

| Field             | Type       | Default | Description                                                    |
|-------------------|------------|---------|----------------------------------------------------------------|
| `kind`            | `string`   | --      | **Required.** Operation discriminator (see below)              |
| `value`           | `i64?`     | `null`  | Integer immediate (const value, label target, field offset, call-site diagnostic ID) |
| `f_value`         | `f64?`     | `null`  | Float immediate. Non-finite encoded as strings: `"NaN"`, `"Infinity"`, `"-Infinity"` |
| `s_value`         | `string?`  | `null`  | String immediate (string literal, call target name, attribute name) |
| `runtime_requirement_bits` | `u32` | `0` | Explicit generated target requirements, admitted only on declared carrier operations |
| `runtime_symbol` | `string?` | `null` | Canonical callable symbol on generated module-acquisition carriers; not an attribute spelling |
| `builtin_name` | `string?` | `null` | Canonical runtime callable identity paired with the builtin name operand |
| `bytes`           | `u8[]?`    | `null`  | Raw byte array (for `const_bytes`, surrogate-containing `const_str`) |
| `var`             | `string?`  | `null`  | Variable name reference (used by store/load patterns)          |
| `args`            | `string[]?`| `null`  | Operand value names (SSA references)                           |
| `out`             | `string?`  | `null`  | Result value name (SSA definition)                             |
| `stack_eligible`  | `bool?`    | `null`  | Hint: result can be stack-allocated                            |
| `task_kind`       | `string?`  | `null`  | Physical task classification on `alloc_task`; callable trampoline kind on `func_new` / `func_new_closure` |
| `task_closure_size` | `int?` | `null` | Nonnegative payload byte size paired with callable task kind; constructor fact, never a mutable Python attribute |
| `container_type`  | `string?`  | `null`  | For `contains`: known container type (`set`, `frozenset`, `dict`, `list`, `str`) |
| `type_hint`       | `string?`  | `null`  | Type annotation from source                                    |
| `native_callable_export` | `string?` | `null` | Qualified native callable export name on `invoke_ffi`, e.g. `scipy.ndimage.distance_transform_edt` |
| `native_callable_binding` | `string?` | `null` | Native binding mode on `invoke_ffi`; public callable wrappers and internal bootstrap calls use `direct_symbol` |
| `native_callable_symbol` | `string?` | `null` | Required direct native symbol when `native_callable_binding` is `direct_symbol` |
| `native_callable_abi` | `string?` | `null` | ABI contract token for the native callable export |
| `argument_custody` | `("borrowed" \| "transferred")[]?` | `null` | Source call operand custody parallel to `args`; the instruction adopts each `transferred` operand |

`type_hint` exists to describe compatibility metadata on the current backend
transport. New lowering work should prefer preserving explicit representation
in typed IR over introducing additional transport-only hint fields.

### Validation Constraints

The backend enforces the representation and shape constraints in this section
while constructing `SimpleIR` from JSON, NDJSON, and serde-backed binary
formats. The stricter internal validator also checks value-use ordering where
the legacy transport surface has already been normalized into SSA values.

Runtime semantic admission composes the generated operation requirements with
canonical callable symbols. The generated call-target roles distinguish actual
direct invocation (`call`, `call_internal`) from opaque callable operands and
guard hints. Direct invocations require the symbol's execution semantics;
`builtin_func`, `func_new`, and `func_new_closure` require target support for the
acquired callable without claiming the symbol has already executed. In
particular, acquiring a frame-aware callable does not require an entered frame.
Canonical `runtime_symbol` and `builtin_name` metadata add the same requirements.
String literals, opaque call hints, and FFI lane names do not establish runtime
symbol identity. The source of these roles and requirements is
`runtime/molt-ir/src/tir/op_kinds.toml`, consumed through `OpIR` admission.

- `param_types`, when present, must have the same length as `params`.
- `parameter_custody`, when present, has one entry per parameter, names at
  least one `transferred` entry, and never transfers the closure parameter.
- `argument_custody` is admitted only on source call spellings with a
  `[[source_call_kind]]` row in `op_kinds.toml`, and names every `args`
  position and at least one transfer.
  - A raw direct call (`call`, `call_internal`) to a function in the
    document carries exactly its target's parameter custody, skipping a
    leading closure parameter, and a `call` to a runtime entry carries none.
  - A dynamic source call transfers every argument. It
    transfers the row's `callable_operand` as well, except that a
    `call_bind` or `call_indirect` whose builder's `callargs_new` has the
    `expanded` call form borrows its callable.
- `container_type` must be one of the canonical container tags accepted by the
  backend contract: `bytearray`, `bytes`, `dict`, `frozenset`, `list`,
  `list_bool`, `list_float`, `list_int`, `range`, `set`, `str`, or `tuple`.
- `bce_safe=true` is valid only on `index` / `store_index`, and only when paired
  with an indexable `container_type`: `list`, `list_bool`, `list_float`,
  `list_int`, or `tuple`.
- `arena_eligible` is rejected whenever present: compiler arena placement has no
  proved owner lifetime. Explicit raw `stack_alloc` is also rejected by target
  admission. `object_new_bound_stack` is retired and rejected before emission;
  class allocation uses the owned heap contract on every supported target.
- Native callable export fields are valid only on `invoke_ffi`. A direct-symbol
  invocation must carry `native_callable_export`, `native_callable_binding`,
  `native_callable_abi`, and `native_callable_symbol`; its binding is
  `direct_symbol`. `native_callable_abi` must be declared by the canonical
  `runtime/native_callable_abi.toml` registry; Python and Rust consumers and the
  browser host use its checked-in generated projections rather than maintaining
  their own token, signature, binding, or arity tables. `invoke_ffi.args`
  contains ABI payload values only and never encodes Python callable identity.
  Direct ABI payload operands are borrowed by the callee and the result is an
  owned value returned to the wrapper. Builders assembled for a borrowed
  callargs payload therefore remain wrapper-owned and are explicitly released
  once on both successful dispatch and every post-allocation failure path.
  For a fixed-arity ABI, its length equals the registry payload arity; a
  variadic public export carries an admitted manifest arity. SimpleIR and TIR
  both enforce that generated arity contract before native or WASM code
  generation.

  User call sites never lower a public native callable export directly to
  `invoke_ffi`. A `direct_symbol` export is published as an ordinary generated
  Python function object, and that wrapper body is the sole owner of its
  direct-symbol `invoke_ffi`; ordinary call dispatch therefore preserves
  rebinding while a captured import retains the original wrapper object. A
  `module_attr` export publishes or resolves the real provider callable and uses
  the same ordinary dispatch, so it emits no native-export `invoke_ffi` at the
  user call site. Public callable-export metadata is separate from the internal
  `molt.pyinit_module_v1` bootstrap contract, which is not a Python callable
  export.

  When a source module also declares native exports, its existing initializer
  remains authoritative. The frontend records a typed publication boundary,
  canonical module value, and active exception-cleanup edge; assembly consumes
  those facts to publish before the source body executes. Source code can then
  capture or replace the published callable normally. Assembly-only publication
  metadata is removed before backend IR emission. Competing extension or alias
  initializers fail closed rather than replacing a source initializer.

  Backends must fail closed if executable ABI dispatch for the declared export
  is absent. WASM currently executes `direct_symbol`
  `molt.object_call_v1` and `molt.object_callargs_v1` through a native import
  table, and executes `molt.forward_f32_v1` as a unary bytes-backed
  Float32Array payload import through the same native namespace. Multi-buffer
  ndarray storage remains a separate ABI contract.
- `type_hint`, `container_type`, and `param_types` entries must be nonempty and
  must not contain control characters.
- Every value name in `args` / `var` must be defined by a prior op's `out` or
  by the function's `params`, with limited exceptions (`dict_set`, `index` at
  position 0).

---

## Operation Kind Catalog

The `kind` field is a lowercase string. The backend maps many aliases to the
same TIR opcode (see `kind_to_opcode` in `tir/ssa.rs`). Below is the
canonical set grouped by category.

### Constants

| kind               | Fields used                        | Description                          |
|--------------------|------------------------------------|--------------------------------------|
| `const`            | `value`, `out`                     | Integer constant (47-bit signed inline range) |
| `const_bigint`     | `s_value`, `out`                   | Arbitrary-precision integer as decimal string |
| `const_bool`       | `value` (0 or 1), `out`           | Boolean constant                     |
| `const_float`      | `f_value`, `out`                   | Float constant (non-finite as string) |
| `const_str`        | `s_value` or `bytes`, `out`        | String constant; lossless surrogatepass UTF-8 bytes for surrogate content |
| `const_bytes`      | `bytes`, `out`                     | Bytes literal                        |
| `const_none`       | `out`                              | `None` singleton                     |
| `const_ellipsis`   | `out`                              | `Ellipsis` singleton                 |
| `const_not_implemented` | `out`                         | `NotImplemented` singleton           |

The shared `molt-ir::literal_payload` authority admits and projects string,
bytes, and bigint literal payloads through SimpleIR, TIR, optimization, and
native/LLVM/WASM materializers. Empty `s_value` and empty `bytes` are present
payloads; an absent payload is malformed IR. NUL and surrogatepass bytes retain
their exact length and contents across every lift and lowering. A string with
both carriers must have equal bytes; conflicting carriers are rejected. TIR
uses the same `s_value`/`bytes` names, never an implicit `value` fallback.
`const_bytes` requires `bytes`; `const_bigint` requires decimal `s_value`.

**Example:**
```json
{"kind": "const", "value": 42, "out": "v0"}
{"kind": "const_str", "s_value": "hello", "out": "v1"}
{"kind": "const_float", "f_value": 3.14, "out": "v2"}
{"kind": "const_none", "out": "v3"}
```

### Arithmetic and Unary

| kind          | Fields used               | Description              |
|---------------|---------------------------|--------------------------|
| `add`         | `args` [lhs, rhs], `out`  | Addition                 |
| `sub`         | `args` [lhs, rhs], `out`  | Subtraction              |
| `mul`         | `args` [lhs, rhs], `out`  | Multiplication           |
| `div`         | `args` [lhs, rhs], `out`  | True division            |
| `floor_div`   | `args` [lhs, rhs], `out`  | Floor division           |
| `mod`         | `args` [lhs, rhs], `out`  | Modulo                   |
| `pow`         | `args` [lhs, rhs], `out`  | Exponentiation           |
| `neg`         | `args` [operand], `out`   | Unary negation           |
| `pos`         | `args` [operand], `out`   | Unary positive           |
| `matmul`      | `args` [lhs, rhs], `out`  | Matrix multiplication    |
| `pow_mod`     | `args`, `out`             | Three-arg pow(b, e, m)   |
| `round`       | `args`, `out`             | round()                  |
| `trunc`       | `args`, `out`             | math.trunc()             |
| `abs`         | `args` [operand], `out`   | Absolute value           |
| `invert`      | `args` [operand], `out`   | Bitwise invert (~x)      |

Inplace variants: `inplace_add`, `inplace_sub`, `inplace_mul`,
`inplace_bit_or`, `inplace_bit_and`, `inplace_bit_xor`. Same schema as their
non-inplace counterparts.

**Example:**
```json
{"kind": "add", "args": ["v0", "v1"], "out": "v2"}
```

### Comparison

| kind        | Fields used               | Description              |
|-------------|---------------------------|--------------------------|
| `eq`        | `args` [lhs, rhs], `out`  | `==`                     |
| `ne`        | `args` [lhs, rhs], `out`  | `!=`                     |
| `lt`        | `args` [lhs, rhs], `out`  | `<`                      |
| `le`        | `args` [lhs, rhs], `out`  | `<=`                     |
| `gt`        | `args` [lhs, rhs], `out`  | `>`                      |
| `ge`        | `args` [lhs, rhs], `out`  | `>=`                     |
| `is`        | `args` [lhs, rhs], `out`  | Identity check           |
| `is_not`    | `args` [lhs, rhs], `out`  | Negated identity         |
| `in`        | `args`, `out`             | Membership               |
| `not_in`    | `args`, `out`             | Negated membership       |
| `contains`  | `args`, `out`, optional `container_type` | `__contains__` with optional type hint |
| `string_eq` | `args` [lhs, rhs], `out`  | String-specialized `==`  |

### Logical and Bitwise

| kind        | Fields used               | Description              |
|-------------|---------------------------|--------------------------|
| `and`       | `args` [lhs, rhs], `out`  | Short-circuit and        |
| `or`        | `args` [lhs, rhs], `out`  | Short-circuit or         |
| `not`       | `args` [operand], `out`   | Boolean negation         |
| `bit_and`   | `args` [lhs, rhs], `out`  | `&`                      |
| `bit_or`    | `args` [lhs, rhs], `out`  | `\|`                     |
| `bit_xor`   | `args` [lhs, rhs], `out`  | `^`                      |
| `bit_not`   | `args` [operand], `out`   | `~` (alias of `invert`)  |
| `lshift`    | `args` [lhs, rhs], `out`  | `<<`                     |
| `rshift`    | `args` [lhs, rhs], `out`  | `>>`                     |

### Control Flow

| kind                  | Fields used           | Description                              |
|-----------------------|-----------------------|------------------------------------------|
| `if`                  | `args` [cond]         | Conditional branch start                 |
| `else`                | --                    | Else branch                              |
| `end_if`              | --                    | End of conditional                       |
| `label`               | `value` (label id)    | Branch target label                      |
| `state_label`         | `value` (label id)    | State machine label (generators)         |
| `jump`                | `value` (label id)    | Unconditional jump                       |
| `check_exception`     | `value` (label id)    | Branch to label if exception pending     |
| `phi`                 | `args`, `out`         | SSA phi node                             |
| `loop_start`          | --                    | Loop header                              |
| `loop_end`            | --                    | Loop footer                              |
| `loop_break`          | --                    | Break out of current loop                |
| `loop_continue`       | --                    | Continue to loop header                  |
| `loop_break_if_true`  | `args` [cond]         | Conditional break                        |
| `loop_break_if_false` | `args` [cond]         | Conditional break (inverted)             |
| `loop_index_start`    | `args`, `out`         | Initialize loop counter                  |
| `loop_index_next`     | `args`, `out`         | Increment loop counter                   |
| `ret_void`            | --                    | Empty exit: no result for `void` ABI; Python `None` for `value` ABI |
| `return`              | `args` [value]        | Return value from function               |

**Example:**
```json
{"kind": "label", "value": 3}
{"kind": "if", "args": ["v5"]}
{"kind": "jump", "value": 3}
{"kind": "check_exception", "value": 7}
```

### Calls

| kind              | Fields used                              | Description                         |
|-------------------|------------------------------------------|-------------------------------------|
| `call`            | `s_value` (target), `args`, `out` | Direct function call; `value` is forbidden |
| `call_internal`   | `s_value` (target), `args`, `out` | Internal (non-exported) call; `value` is forbidden |
| `call_indirect`   | `args` [callable, ...positional], `out`  | Indirect call via function pointer  |
| `call_func`       | `args` [callable, ...positional], `out`  | Generic function call               |
| `call_guarded`    | `s_value` (target), `args`, `out`        | Actual callable followed by unbound Python arguments; direct call only after shared shape and target admission, otherwise runtime binding |
| `call_bind`       | `args`, `out`                            | Partial application / bind          |
| `call_method`     | `args`, `out`                            | Method call                         |
| `invoke_ffi`      | `args`, `out`, optional `s_value` (lane), optional native callable export fields | Foreign function or native package ABI invocation |
| `print`           | `args`                                   | Built-in print                      |
| `print_newline`   | --                                       | Print bare newline                  |

**Example:**
```json
{"kind": "call", "s_value": "molt_init___main__", "args": ["v0"], "value": 1, "out": "v3"}
{"kind": "call_indirect", "args": ["v5", "v6", "v7"], "out": "v8"}
{"kind": "invoke_ffi", "args": ["p0", "p1"], "out": "v0", "native_callable_export": "scipy.ndimage.distance_transform_edt", "native_callable_binding": "direct_symbol", "native_callable_symbol": "molt_scipy_ndimage_distance_transform_edt", "native_callable_abi": "molt.object_call_v1"}
```

The `invoke_ffi` example is the generated direct-symbol wrapper body, not the
user call site that invokes that wrapper as a normal Python callable.

### Exception Handling

| kind                        | Fields used           | Description                          |
|-----------------------------|-----------------------|--------------------------------------|
| `try_start`                 | `value` (optional handler label) | Enter path-local region custody |
| `try_end`                   | `value` (optional handler label) | Close path-local region custody |
| `raise`                     | `args` [exc], `out`   | Raise exception                      |
| `exception_push`            | `out`                 | Push exception handler               |
| `exception_pop`             | `out`                 | Pop handler                         |
| `exception_new`             | `args` [kind, msg], `out` | Create exception from kind+message |
| `exception_new_from_class`  | `args` [cls, msg], `out` | Create exception from class+message |
| `exception_last`            | `out`                 | Get current exception                |
| `exception_clear`           | `out`                 | Clear current exception              |
| `exception_set_last`        | `args` [exc], `out`   | Set current exception                |
| `exception_context_set`     | `args` [exc], `out`   | Set `__context__`                    |
| `exception_class`           | `args` [exc], `out`   | Get exception class                  |
| `exception_stack_enter`     | `out`                 | Enter exception stack frame          |
| `exception_stack_exit`      | `args`, `out`         | Exit exception stack frame           |
| `exception_stack_clear`     | `out`                 | Clear exception stack                |
| `exception_stack_depth`     | `out`                 | Get exception stack depth            |
| `exception_stack_set_depth` | `args`, `out`         | Restore exception stack depth        |
| `exceptiongroup_match`      | `args` [eg, filter], `out` | Match ExceptionGroup            |
| `exceptiongroup_combine`    | `args` [eg], `out`    | Combine ExceptionGroup               |

Try markers are region metadata, not executable frame pushes/pops or lexical
brackets. Alternative exits can close one region more than once in source
order, including from inside a still-open branch or loop. Explicit
`exception_push`/`exception_pop` own runtime frame operations and
`check_exception` owns pending-error transfer. A named region references a
defined handler label; omitted labels describe anonymous IR regions.

**Example (region metadata and pending transfer):**
```json
{"kind": "try_start", "value": 5}
{"kind": "raise", "args": ["v10"], "out": "v11"}
{"kind": "check_exception", "value": 5}
{"kind": "try_end", "value": 5}
```

### Attribute Access

Field writes always have assignment semantics. `store_init` and
`guarded_field_init` are retired and rejected: initialization specialization
requires the shared compiler's exact-site pristine-slot proof, never a wire hint.

A layout guard names the class statement whose field offsets it trusts in
`class_definition` (HF-220), never a class operand. The statement stamps that
identity on every class it creates (`class_set_definition`); the program's
code-slot bindings resolve the symbol. The guard admits a receiver whose class
carries the identity at the statement's layout version, so rebinding the class
name cannot redirect it. Only the generated
`simpleir_class_definition_carrier_kinds` may carry `class_definition`.

| kind                     | Fields used                                    | Description                        |
|--------------------------|------------------------------------------------|------------------------------------|
| `load`                   | `args` [obj], `value` (offset), `out`          | Load field at known offset         |
| `store`                  | `args` [obj, val], `value` (offset)            | Store field at known offset        |
| `get_attr_generic_ptr`   | `args` [obj], `s_value` (attr), `out` | Generic attr get; cache site identity uses transported `source_op_idx`, never a frontend global counter |
| `get_attr_generic_obj`   | `args` [obj], `s_value` (attr), `out`          | Generic attr get (obj-based)       |
| `get_attr_name`          | `args`, `out`                                  | Get attr by runtime name value     |
| `get_attr_name_default`  | `args`, `out`                                  | Get attr with default              |
| `get_attr_special_obj`   | `args` [obj], `s_value` (attr), `out`          | Get special/dunder attr            |
| `set_attr_generic_ptr`   | `args` [obj, val], `s_value` (attr), `out`     | Generic attr set (ptr-based)       |
| `set_attr_generic_obj`   | `args` [obj, val], `s_value` (attr), `out`     | Generic attr set (obj-based)       |
| `set_attr_name`          | `args`, `out`                                  | Set attr by runtime name value     |
| `del_attr_generic_ptr`   | `args` [obj], `s_value` (attr), `out`          | Delete attr (ptr-based)            |
| `del_attr_generic_obj`   | `args` [obj], `s_value` (attr), `out`          | Delete attr (obj-based)            |
| `del_attr_name`          | `args`, `out`                                  | Delete attr by runtime name value  |
| `has_attr_name`          | `args`, `out`                                  | hasattr() by runtime name          |
| `guarded_field_get`      | `args` [obj, ver], `s_value`, `value` (offset), `class_definition`, `out` | Guarded field load  |
| `guarded_field_set`      | `args` [obj, ver, val], `s_value`, `value` (offset), `class_definition`, `out` | Guarded field store |

### Indexing

| kind           | Fields used          | Description              |
|----------------|----------------------|--------------------------|
| `index`        | `args`, `out`        | `obj[key]`               |
| `store_index`  | `args`, `out`        | `obj[key] = val`         |
| `del_index`    | `args`, `out`        | `del obj[key]`           |
| `slice`        | `args`, `out`        | Slice operation          |
| `slice_new`    | `args`, `out`        | Create slice object      |

### Collection Construction

| kind              | Fields used   | Description              |
|-------------------|---------------|--------------------------|
| `list_new`        | `args`, `out` | Create list              |
| `tuple_new`       | `args`, `out` | Create tuple             |
| `dict_new`        | `args`, `out` | Create dict              |
| `set_new`         | `args`, `out` | Create set               |
| `frozenset_new`   | `args`, `out` | Create frozenset         |
| `range_new`       | `args`, `out` | Create range object      |
| `list_from_range` | `args`, `out` | list(range(...))         |
| `tuple_from_list` | `args`, `out` | tuple(list)              |
| `dict_from_obj`   | `args`, `out` | dict(iterable)           |

### Collection Methods

List: `list_append`, `list_pop`, `list_extend`, `list_insert`, `list_remove`,
`list_clear`, `list_copy`, `list_reverse`, `list_count`, `list_index`,
`list_index_range`.

Dict: `dict_get`, `dict_set`, `dict_pop`, `dict_setdefault`,
`dict_setdefault_empty_list`, `dict_update`, `dict_update_missing`,
`dict_update_kwstar`, `dict_clear`, `dict_copy`, `dict_popitem`, `dict_keys`,
`dict_values`, `dict_items`, `dict_str_int_inc`.

Set: `set_add`, `set_discard`, `set_remove`, `set_pop`, `set_update`,
`set_intersection_update`, `set_difference_update`, `set_symdiff_update`,
`frozenset_add`.

Tuple: `tuple_count`, `tuple_index`.

All use the standard `args` + `out` pattern.

### String Operations

`string_find`, `string_find_slice`, `string_format`, `string_startswith`,
`string_startswith_slice`, `string_endswith`, `string_endswith_slice`,
`string_count`, `string_count_slice`, `string_join`, `string_split`,
`string_split_max`, `string_lower`, `string_upper`, `string_capitalize`,
`string_strip`, `string_lstrip`, `string_rstrip`, `string_replace`,
`string_split_ws_dict_inc`, `string_split_sep_dict_inc`.

Bytes/bytearray: `bytes_find`, `bytes_find_slice`, `bytearray_find`,
`bytearray_find_slice`, `bytes_startswith`, `bytes_startswith_slice`,
`bytearray_startswith`, `bytearray_startswith_slice`, `bytes_endswith`,
`bytes_endswith_slice`, `bytearray_endswith`, `bytearray_endswith_slice`,
`bytes_count`, `bytearray_count`, `bytes_count_slice`,
`bytearray_count_slice`, `bytes_split`, `bytes_split_max`.

All use the standard `args` + `out` pattern.

### Type and Conversion

| kind                  | Fields used                   | Description              |
|-----------------------|-------------------------------|--------------------------|
| `isinstance`          | `args` [obj, cls], `out`      | isinstance()             |
| `issubclass`          | `args` [cls, base], `out`     | issubclass()             |
| `type_of`             | `args` [obj], `out`           | type(obj)                |
| `builtin_type`        | `args`, `out`                 | Get built-in type object |
| `str_from_obj`        | `args`, `out`                 | str(obj)                 |
| `repr_from_obj`       | `args`, `out`                 | repr(obj)                |
| `ascii_from_obj`      | `args`, `out`                 | ascii(obj)               |
| `int_from_obj`        | `args`, `out`                 | int(obj)                 |
| `operator_index`      | `args`, `out`                 | operator.index(obj): range()'s bound conversion, an exact int |
| `float_from_obj`      | `args`, `out`                 | float(obj)               |
| `complex_from_obj`    | `args`, `out`                 | complex(obj)             |
| `bytes_from_obj`      | `args`, `out`                 | bytes(obj)               |
| `bytes_from_str`      | `args`, `out`                 | str.encode()             |
| `bytearray_from_obj`  | `args`, `out`                 | bytearray(obj)           |
| `bytearray_from_str`  | `args`, `out`                 | bytearray from str       |
| `len`                 | `args`, `out`                 | len(obj)                 |
| `id`                  | `args`, `out`                 | id(obj)                  |
| `ord`                 | `args`, `out`                 | ord(ch)                  |
| `chr`                 | `args`, `out`                 | chr(n)                   |

### Box/Unbox and Memory

| kind              | Fields used              | Description                          |
|-------------------|--------------------------|--------------------------------------|
| `box`             | `args` [raw], `out`      | Box a raw value into an object       |
| `box_from_raw_int`| `args` [raw], `out`      | Box a raw i64 into a Python int      |
| `unbox`           | `args` [obj], `out`      | Unbox object to raw value            |
| `unbox_to_raw_int`| `args` [obj], `out`      | Unbox Python int to raw i64          |
| `alloc`           | `value` (size), `out`    | Allocate object of known size        |
| `alloc_class`     | `args` [cls], `value` (size), `out` | Allocate instance of class |
| `inc_ref`         | `args`, `out`            | Increment reference count            |
| `dec_ref`         | `args`, `out`            | Decrement reference count            |
| `borrow`          | `args`, `out`            | Perceus borrow                       |
| `release`         | `args`, `out`            | Perceus release                      |
| `free`            | `args`                   | Free allocation                      |
| `stack_alloc`     | --                       | Stack allocation marker              |

### Class and Object

| kind                      | Fields used                    | Description                    |
|---------------------------|--------------------------------|--------------------------------|
| `class_def`               | `args`, `s_value`, `out`       | Define class with body         |
| `class_set_definition`    | `args` [class], `class_definition`, `out` | Stamp a fresh class with its class statement identity |
| `guard_layout`            | `args` [obj, ver], `class_definition`, `out` | Guard on class statement and layout version |
| `object_new`              | `out`                          | Create bare object             |
| `dataclass_new`           | `args`, `out`                  | Create dataclass instance      |
| `dataclass_get`           | `args`, `out`                  | Dataclass field get            |
| `dataclass_set`           | `args`, `out`                  | Dataclass field set            |
| `dataclass_set_class`     | `args`, `out`                  | Set dataclass's class          |
| `missing`                 | `out`                          | Sentinel missing value         |

### Function and Code Objects

| kind                      | Fields used                              | Description                    |
|---------------------------|------------------------------------------|--------------------------------|
| `func_new`                | `s_value` (name), `value` (arity), `out`, optional paired `task_kind` / `task_closure_size` | Create function object |
| `func_new_closure`        | `s_value`, `value`, `args` [closure], `out`, optional paired `task_kind` / `task_closure_size` | Create closure |
| `builtin_func`            | `s_value` (runtime symbol), `value` (arity), `out`, paired `builtin_name` / `args` [name] | Acquire a public builtin binding or construct a runtime callable |
| `code_new`                | `args`, `out`                            | Create code object             |
| `code_slot_set`           | `s_value` (code-owner symbol), `args` [code_obj, globals_dict] | Bind owned code and lexical namespace in the program's slot |
| `code_slots_init`         | `value` (count)                          | Initialize code slot table     |
| `classmethod_new`         | `args`, `out`                            | Wrap as classmethod            |
| `staticmethod_new`        | `args`, `out`                            | Wrap as staticmethod           |
| `property_new`            | `args`, `out`                            | Create property descriptor     |
| `bound_method_new`        | `args`, `out`                            | Create bound method            |
| `function_closure_bits`   | `args`, `out`                            | Closure capture bitmap         |

`builtin_name` and the single executable name operand are inseparable: neither
may appear without the other. Public builtin acquisition and named runtime
construction use this encoding; unnamed runtime constructors carry neither.
The shared SimpleIR/TIR conversion preserves the pair through every optimization
roundtrip, before native, LLVM, WASM, Luau, or Rust admission and emission.

### Module

| kind                | Fields used          | Description              |
|---------------------|----------------------|--------------------------|
| `module_new`        | `args`, `out`        | Create module object     |
| `module_import`     | `args`, `out`        | Import module            |
| `module_cache_get`  | `args`, `out`        | Get from module cache    |
| `module_cache_set`  | `args`, `out`        | Set in module cache      |
| `module_cache_del`  | `args`, `out`        | Delete from module cache |
| `module_get_attr`   | `args`, `out`        | Get module attribute     |
| `module_get_global` | `args`, `out`        | Get module global        |
| `module_set_attr`   | `args`, `out`        | Set module attribute     |
| `module_del_global` | `args`, `out`        | Delete module global     |
| `module_import_star`| `args`, `out`        | `from module import *`   |
| `import`            | --                   | Import statement         |
| `import_from`       | --                   | `from X import Y`        |

### Iterator and Unpacking

| kind               | Fields used          | Description              |
|--------------------|----------------------|--------------------------|
| `iter`             | `args`, `out`        | Get iterator             |
| `iter_next`        | `args`, `out`        | Get next from iterator   |
| `enumerate`        | `args`, `out`        | enumerate()              |
| `aiter`            | `args`, `out`        | Async iterator           |
| `anext`            | `args`, `out`        | Async next               |
| `unpack_sequence`  | `args`, `value` (expected_count) | Unpack sequence into N values |

### Context Manager

| kind                | Fields used                   | Description              |
|---------------------|-------------------------------|--------------------------|
| `context_enter`     | `args` [ctx], `out`           | `__enter__`              |
| `context_exit`      | `args` [ctx, exc], `out`      | `__exit__`               |

### Async

| kind               | Fields used          | Description              |
|--------------------|----------------------|--------------------------|
| `yield`            | --                   | Generator yield          |
| `yield_from`       | --                   | Generator yield from     |
| `is_native_awaitable` | `args`, `out`     | Check if native awaitable |
| `stateful_locals_register` | `s_value`, `args` | Register the typed public-locals layout of a generator, coroutine, async generator or generator expression; layout is `(parameter_count, offsets, cell_ordinals, closure_offset)`, with `-1` for raw bindings and contiguous cell publication ordinals |

### Guard and Deopt

| kind             | Fields used   | Description                     |
|------------------|---------------|---------------------------------|
| `guard_type`     | `args`        | Read value and expected tag; profile mismatch and return the original value |
| `guard_tag`      | `args`        | Same runtime contract as `guard_type`; rejected expected tags raise |
| `type_guard`     | --            | Generic type guard              |

### Call Arguments Protocol

| kind                    | Fields used   | Description              |
|-------------------------|---------------|--------------------------|
| `callargs_new`          | `out`, `s_value` | Create CallArgs builder. `s_value` is `"expanded"` at a CALL_FUNCTION_EX call site and absent at a CALL call site; no other value is valid |
| `callargs_push_pos`     | `args`, `out` | Push positional arg      |
| `callargs_push_kw`      | `args`, `out` | Push keyword arg         |
| `callargs_expand_star`  | `args`, `out` | Expand *args             |
| `callargs_expand_kwstar`| `args`, `out` | Expand **kwargs          |

### Fused Loops

A fused op runs items of a Python loop, or a statement, in a runtime kernel,
which admits the values the code reads or declines; the ordinary lowering is
emitted after or beside it (see `0190_LOWERING_RULES.md`, "Fused loops"). A
reduction runs one bounded chunk of its loop's iterator per call, inside a
chunk loop whose back edge observes pending asynchronous work: `count` items
consumed, `result` and `last` the accumulator and loop target after them, and
`more` true when the chunk ended at its size bound.

| kind                        | `args`                                        | `out`                  |
|-----------------------------|-----------------------------------------------|------------------------|
| `vec_sum`, `vec_prod`, `vec_min`, `vec_max` | [it, acc, target_old] | tuple (result, last, count, more) |
| `string_split_ws_dict_inc`  | [line, dict, delta, target_old]               | tuple (last, ok)       |
| `string_split_sep_dict_inc` | [line, sep, dict, delta, target_old]          | tuple (last, ok)       |
| `dict_str_int_inc`          | [dict, key, delta]                            | bool: the statement ran |

### Miscellaneous

| kind                | Fields used           | Description                      |
|---------------------|-----------------------|----------------------------------|
| `nop`               | --                    | No operation                     |
| `line`              | `value` (line number) | Source line mapping               |
| `trace_enter_slot`  | `s_value` (code-owner symbol) | Attempt frame entry; next `check_exception` owns failure cleanup |
| `trace_exit`        | --                    | Mark the authored normal frame exit; backend activation cleanup consumes ownership |
| `frame_context_set` | `args` (exactly 3) | Publish argument-zero value, boxed argument kind, and class cell; no result |
| `frame_locals_set`  | `args`                | Set frame locals dict            |
| `copy`              | --                    | SSA copy / variable alias        |
| `identity_alias`    | `args`, `out`         | Identity alias (no-op copy)      |
| `binding_alias`     | `args`, `out`         | Alias bits plus owned local retain |
| `cast`              | `args`, `out`         | Type cast                        |
| `widen`             | `args`, `out`         | Widen representation             |
| `state_block_start` | --                    | State machine block start        |
| `state_block_end`   | --                    | State machine block end          |

State-machine poll completion follows the explicit IR continuation, including
its exception matching, clearing and propagation operations. A backend must not
infer an exception destination by scanning ahead for a later `check_exception`:
that would bypass protocol handling such as async iteration termination and
change which surrounding handler observes the exception.

Codec and buffer operations act on runtime values; they do not make Python
module names compiler syntax. `molt_json`, `molt_msgpack`, `molt_cbor`, and
`molt_buffer` are ordinary source packages with real import bindings and module
identity. Their functions use the generated intrinsic registry. Direct, aliased,
captured, and rebound calls share normal callable dispatch. Compiler transport
codec selection does not change `molt_json.parse` semantics. Runtime-active
packages require their declared intrinsics; a missing intrinsic must propagate
its diagnostic, never select a CPython reference implementation. AST spelling alone
cannot justify codec substitution or replacing a buffer loop and its destination
with a newly allocated matrix.

`molt_buffer.Buffer2D` is the public object for every call shape. Its native
storage owns dimensions and uses tracked packed `i64` cells until an integer or
arithmetic result requires canonical boxed integers. Promotion must preserve
existing cells and roll back on allocation failure; matrix multiplication must
never wrap. The wrapper reads native dimensions instead of maintaining a second
shape authority. Dimensions, indices, and values use the integer/index protocol;
dimensions are nonnegative and bounded by the target's sequence-size limit.
The CPython reference uses the same flat cell cardinality and explicit shape,
not row objects. Zero-cardinality shapes preserve either dimension without
dimension-sized allocation or iteration, including matrix products. Constructor
replacement publishes the new shape and storage only after allocation succeeds.
Public matrix multiplication uses the same live dimension/accessor protocol on
CPython and compiled targets, including subclass and instance/class overrides.
The raw `buffer2d_matmul` primitive does not authorize bypassing that protocol;
its operands are storage objects, not proof of public wrapper behavior.
Negative indices are normalized before access; slices are rejected. Value
conversion precedes row conversion/bounds checking, which precedes column
conversion/bounds checking. Index callback exceptions propagate unchanged.
Module-level `set` returns the
same public buffer; the instance method returns `None`. Index callbacks finish
before borrowing mutable native storage, including reentrant callbacks that
promote that same buffer.

---

## Backend Kind Aliases

The Rust backend TIR lowering (`tir/ssa.rs`) maps multiple kind strings to the
same internal opcode. These aliases are accepted in the JSON:

| Aliases                                                                  | Canonical TIR OpCode |
|--------------------------------------------------------------------------|----------------------|
| `const`, `const_int`, `load_const`                                       | ConstInt             |
| `copy`, `store_var`, `load_var`                                          | Copy                 |
| `call`, `call_func`, `call_internal`, `call_indirect`, `call_bind`, `call_function`, `call_guarded`, `invoke_ffi` | Call |
| `call_builtin`, `builtin_print`, `print`                                | CallBuiltin          |
| `get_attr`, `get_attr_generic_ptr`, `get_attr_generic_obj`, `get_attr_name`, `guarded_field_get`, `load`, `load_attr` | LoadAttr |
| `set_attr`, `store_attr`, `set_attr_name`, `set_attr_generic_ptr`, `set_attr_generic_obj`, `guarded_field_set`, `store` | StoreAttr |
| `del_attr`, `del_attr_generic_ptr`, `del_attr_generic_obj`              | DelAttr              |
| `store_index`, `index_set`                                              | StoreIndex           |
| `box`, `box_from_raw_int`                                               | BoxVal               |
| `unbox`, `unbox_to_raw_int`                                             | UnboxVal             |
| `floor_div` (lowercase)                                                  | FloorDiv             |
| `module_cache_get`                                                       | ModuleCacheGet       |
| `module_cache_set`                                                       | ModuleCacheSet       |
| `module_cache_del`                                                       | ModuleCacheDel       |
| `module_get_attr`                                                        | ModuleGetAttr        |
| `module_get_global`                                                      | ModuleGetGlobal      |
| `module_get_name`                                                        | ModuleGetName        |
| `module_set_attr`                                                        | ModuleSetAttr        |
| `module_del_global`                                                      | ModuleDelGlobal      |

**Known TIR alias gaps:** The frontend emits `lshift` and `rshift` for shift
operations. The TIR `kind_to_opcode` function maps only `shl` and `shr` to
`OpCode::Shl` / `OpCode::Shr`. Because `lshift` / `rshift` are not in the
mapping, they fall through to `Copy` in the TIR pipeline. This does not affect
correctness for backends that dispatch on the original `op.kind` string (WASM,
Luau, Rust, native), but means TIR optimization passes treat shifts as no-ops.
Similarly, `del_attr_name`, `has_attr_name`, `get_attr_name_default`,
`get_attr_special_obj`, `set_attr_name`, and `del_attr_name` are not mapped
and fall through to `Copy` in TIR. The same applies to many collection method
ops, string ops, and type/conversion ops -- they are handled directly by the
backends and not by the TIR pipeline.

Unknown kind strings fall through to `Copy` (no-op).

---

## Full Example

```json
{
  "functions": [
    {
      "name": "molt_main",
      "params": [],
      "return_abi": "void",
      "ops": [
        {"kind": "call", "s_value": "molt_init___main__", "args": [], "value": 0, "out": "v0"},
        {"kind": "ret_void"}
      ]
    },
    {
      "name": "molt_init___main__",
      "params": [],
      "return_abi": "void",
      "ops": [
        {"kind": "const", "value": 10, "out": "v0"},
        {"kind": "const", "value": 20, "out": "v1"},
        {"kind": "add", "args": ["v0", "v1"], "out": "v2"},
        {"kind": "const_str", "s_value": "result", "out": "v3"},
        {"kind": "print", "args": ["v3", "v2"]},
        {"kind": "try_start"},
        {"kind": "const", "value": 0, "out": "v4"},
        {"kind": "div", "args": ["v2", "v4"], "out": "v5"},
        {"kind": "check_exception", "value": 1},
        {"kind": "jump", "value": 2},
        {"kind": "label", "value": 1},
        {"kind": "exception_last", "out": "v6"},
        {"kind": "exception_clear", "out": "v7"},
        {"kind": "label", "value": 2},
        {"kind": "try_end"},
        {"kind": "ret_void"}
      ]
    }
  ]
}
```


Stateful frame locals use the registered layout at constructor, prologue,
suspension, live-frame observation and traceback capture. A single monotonic
phase in the existing task sidecar records body-slot initialization and cell
publication. The compiler's prologue calls `molt_frame_locals_begin` once to
initialize every nonparameter public slot to missing, then calls
`molt_frame_cell_publish(offset, cell)` in layout ordinal order. Cell storage
and phase advance publish before displaced owners are released. Raw arguments
that happen to be Python cells remain raw until their own publication.

The compiled frame retains its activation through the existing invocation
handoff. Observations project locals after retaining frame edges and releasing
stack borrows. Tracebacks own projected dictionaries, including while a live
frame unwinds after terminal state was set. Suspended terminal views are empty.
Ordinary functions without a stateful payload retain their existing compiler
locals-publication contract; no slot layout is invented for stack-only storage.

Explicit source raises evaluate the exception and cause expressions in that order,
then call `molt_exception_prepare_raise(exc, cause_or_missing)`. This owned-result
boundary constructs exception classes, validates instances and explicit causes,
and publishes `__cause__` before instance-only context, traceback or deferred
pending-state operations. MISSING means no `from` clause; None suppresses context.
Runtime class ingress shares the same normalization authority. Throw retains its
distinct class/value/traceback restoration protocol. Both source raise and throw
injection call `molt_exception_trace_prepend` before publishing an already-traced exception. Bare reraises and pending-state
transport preserve the existing traceback. The same prefix constructor accepts
lazy or already-observed tails; materialization preserves eager tail identity.

Cold module chunks retain the same producer-derived scalar and container facts
as other functions. Suppressing their raw-primary carriers is an optimization
policy, not permission to erase semantic provenance used by target admission.
In particular, identity against a proven None singleton remains admissible on
Luau; this does not admit unknown numeric identity or integer arithmetic without
the target's required exact numeric authority.
The preserved frontend `iter` spelling and typed `get_iter` projection both
require the canonical iterable/sequence protocol; neither is runtime-neutral.
Luau local-frame cookies are owned by the function scope, including context,
entry depth, code and coroutine owner. Operation capture closures and dispatch
blocks assign that cookie rather than shadow it, so later source updates and
frame exit observe the same entry.


`frame_context_set` requires internal execution-frame support. Its three boxed
operands are borrowed; the runtime retains the active frame owners. It has no
SSA result (the backend ABI returns borrowed `None`), may raise, and can access
arbitrary heap state through displaced-owner finalizers. It does not create a
Python call-return scheduling boundary or an implicit exception transfer. The
`molt_frame_context_set` export is backend-only: generic call and callable
acquisition carriers must use the typed operation instead.
