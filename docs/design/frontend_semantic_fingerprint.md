# Frontend semantic fingerprint

Status: implemented, 2026-10-20 (HF-254, HF-182). Owner: frontend build speed.
Authority: `_FRONTEND_COMPILER_INPUTS` in `src/molt/cli/cache_fingerprints.py`.

## Problem

Every persisted frontend record carries one compiler identity: module analysis
and module lowering (`cli/module_cache.py`), import scans and intrinsic facts
(`cli/module_graph_cache.py`). The identity was the import closure of the
frontend, found by running the binding analysis over the compiler's own
sources. Two costs followed.

- Scope (HF-254). The module-level closure reached 259 files, 7.7 MB:
  toolchain identity, Rust and LLVM toolchains, file publication, build
  diagnostics, Cargo policy and 25 source-extension modules. An edit to any of
  them re-lowered every module. From 2026-09-20 to 2026-10-20, 395 of the 640
  commits that touched `src/molt` changed the key; only 116 of them touched
  `frontend/` or `compiler_analysis/`.
- Cold cost (HF-182). A process with an empty `MOLT_CACHE` spent 8.6-10.6 s
  (median 8.9 s, 5 runs) to compute the closure. Under cProfile, 258
  `analyze_local_imports` calls took 20.4 s of 24.1 s, and 15.0 s of that was
  44 `static_if_live_branch` calls deciding `if TYPE_CHECKING:` blocks. Every
  test that isolates its cache paid it again.

The closure was also unchecked in the other direction. It followed only
module-level imports, so 64 modules that key files import inside functions
stayed outside the key with no classification. None of them computes record
content today, but nothing would have caught one that did.

## Prior art

- [rustc session options](https://github.com/rust-lang/rust/blob/master/compiler/rustc_session/src/options.rs)
  declare every `-C` and `-Z` option `[TRACKED]` or `[UNTRACKED]`. Tracked
  options enter the dependency-tracking hash and invalidate incremental state;
  untracked ones (timing, printing, most diagnostics) do not. The macro rejects
  an option without a class, so the classification is made when the option is
  written. The incremental dep graph also records the compiler version
  ([`rustc_incremental` file format](https://github.com/rust-lang/rust/blob/master/compiler/rustc_incremental/src/persist/file_format.rs))
  and discards state from another rustc. Query results are fingerprinted by
  stable hashing ([incremental compilation in detail](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html)).
- [Bazel remote caching](https://bazel.build/remote/caching): an action key
  digests the command line, the environment and the digests of the declared
  inputs, tool binaries included. The rule code that computed the command line
  is not in the key; its effect is the command line.
  [Sandboxing](https://bazel.build/docs/sandboxing) makes an undeclared read
  fail instead of producing a wrong cache entry.
- [ccache](https://ccache.dev/manual/latest.html): the direct-mode hash covers
  the compiler identity (`compiler_check`: mtime, content or a command), the
  arguments, the source and the include files its manifest recorded.
  Preprocessor mode hashes the preprocessed text, so a comment edit in a header
  still hits. ccache's own storage code is not hashed; result and manifest
  format versions guard it.
- [mypy](https://github.com/python/mypy/blob/master/mypy/build.py) writes one
  `.meta.json` per module with the source hash, the dependency list, the
  per-module options and `version_id`. A different mypy version invalidates
  every module; development builds append the Git revision, so every commit
  does.

Molt follows rustc and Bazel: a typed, declared input set whose completeness a
check enforces, and per-record keys that carry the values computed outside it.
It rejects the rustc/mypy whole-compiler version key for development checkouts,
because that is the HF-254 cost.

## Design

**Declaration.** `_FRONTEND_COMPILER_INPUTS` maps each compiler source that a
frontend producer imports to a `_FrontendInputRole`. Paths are relative to
`src/molt`; a directory entry covers its tree, data files included.

| Role | Keyed | Rule |
| --- | --- | --- |
| `PRODUCER` | yes | Its code computes, encodes or decodes a record's content, or computes an input to one that the record key does not carry as a value. |
| `REFERENCE` | yes | A compiler-owned guest source whose bytes the GPU descriptor-origin check compares with lowered metadata (`GPU_PYTHON_BODY_REFERENCES`). |
| `PACKAGE` | no | A package `__init__` that runs because a submodule is imported; it imports no Molt module and binds nothing a producer reads. |
| `IDENTITY` | no | Computes which record to read: fingerprints, digests, input custody. |
| `STORAGE` | no | Moves opaque record bytes: atomic publication, the record store, its size bound. A format change bumps the record's schema version. |
| `REPORTING` | no | Diagnostics, progress, timings, input reports, failure text. |
| `BUILD_ENVIRONMENT` | no | Request types, admission and refusal, host, process, path and toolchain facts. A value from it that reaches a frontend computation is a field of that record's key. |

The keyed set is `frontend/`, `compiler_analysis/`, 38 files outside them (the
frontend and module drivers, the record codecs, generated tables and semantic
helpers) and the two reference sources: 152 files, 5.4 MB.

**Why the excluded modules cannot change a record.** A record's content is a
function of its keyed inputs and the producer code. The lowering context
payload (`_module_lowering_context_payload`) carries target Python, parse
codec, type-hint policy, known and direct-call modules, the stdlib allowlist,
native exports and support roots, scoped classes and defaults. Import-scan keys
carry the scan mode and the namespace premise. So:

- `cli/external_native.py` builds the import admission policy from
  `MOLT_EXTERNAL_STATIC_PACKAGES` and custody checks. Discovery applies it live;
  the policy's rules live in `cli/models.py`, a producer. Its effects reach
  records only as the premise and the context module sets. (The earlier
  carve-out kept it keyed because it feeds `direct_call_modules`; that field is
  now in the context payload, and a test proves the digest moves with it.)
- `cli/module_stdlib_policy.py` reads
  `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md`; the
  parsed allowlist is a context field.
- Toolchain identity, Rust, LLVM and WASI toolchains, Cargo policy, build
  output layout, installed-runtime and readiness checks refuse or locate; they
  pass no value into a record.
- Publication, the record store and atomic writes move bytes they do not read.
  Build diagnostics, progress, input reports and failure text are never stored.

A module whose role is in doubt stays keyed. The record codecs
(`cli/cache_keys.py`, `cli/function_records.py`) are keyed although their format
also has a schema version, because a codec change without a version bump would
otherwise decode old records under new rules. Low-churn target modules
(`compilation_target.py`, `native_target.py` and its two leaves) are keyed
because graph decisions read them; keeping them costs 1-2 commits a month.

**Computation.** `_frontend_semantic_tooling_sources` reads each keyed file once
and hashes its bytes. Nothing is parsed or analyzed, and Git state never enters
the identity. A keyed path that is absent is left out, so a source that appears
later changes the key. The digest commits to each file's path relative to the
compiler root, so a copy of the compiler keys exactly as the checkout does.
Installed compilers keep using the admitted release identity.

**Completeness.** `tests/cli/test_cli_lowering_cache_tooling_scope.py` holds the
checks that make the declaration sound:

- A static walk over every producer, every scope included (only
  `if TYPE_CHECKING:` bodies are skipped), requires each imported `molt`
  module, and each parent package, to be declared. It refuses computed module
  names and rebound import anchors (`__package__`, `__spec__`), which would
  escape the walk. It also rejects missing entries and non-keyed entries that no
  producer imports.
- A fresh process lowers a fixed program with `builtins.__import__` and the
  `open` audit event recorded per importing module (Bazel's sandbox, applied to
  imports). Every module a producer imports must be declared, and every data
  file a producer reads must lie inside a keyed entry.
- A differential test edits every entry of a tree built from the declaration:
  non-keyed edits keep the key, keyed edits change it.
- An independent oracle copies the compiler, lowers a fixed program, and edits
  the copy: six infrastructure edits keep both the IR and the key; a change to
  CPython's private-name mangling rule in `python_private_names.py` changes the
  IR and the key.

## Measurements

macOS arm64, CPython 3.12.15, origin/main bac4c7416 against this change.

| Metric | Before | After |
| --- | --- | --- |
| Keyed files | 259 (7.7 MB) | 152 (5.4 MB) |
| Snapshot, fresh process, empty `MOLT_CACHE` (median of 5) | 8.90 s | 0.012 s |
| Snapshot, fresh process, warm `MOLT_CACHE` (median of 5) | 0.136 s | 0.010 s |
| Bytes the snapshot writes to `MOLT_CACHE` | 448 KB (one closure-row file) | none |
| Commits since 2026-09-20 that change the key | 395 of 640 | 218 of 640 |
| Modules re-lowered after a toolchain, publication and diagnostics edit | 40 of 40 (40.1 s build) | 0 of 40 (3.2 s build) |
| Modules re-lowered after a producer edit | 40 of 40 | 40 of 40 (43.9 s build) |

The re-lowering rows come from the real CLI frontend with a fake backend
(`_real_frontend_fake_backend` in `tests/cli/test_cli_import_collection.py`),
serial lowering, on a program that imports `json`, `collections` and two local
modules: 40 modules. A cold build of it took 60.1 s before and 46.4 s after;
a warm build 3.8 s and 3.6 s. The edits were the ones the ledger names:
`toolchain_identity.py`, `file_publication.py` and `cli/build_diagnostics.py`,
then the private-name mangling rule in `python_private_names.py`.

## Remaining work

- `_wasm_abi_generated.py` (50 commits in the window) is keyed for the two tables
  the frontend reads (`wasm_runtime_callable_arity`,
  `WASM_NON_RUNTIME_CALLABLE_INTRINSICS`). A generated frontend projection would
  remove the backend ABI churn from the key.
- `cli/models.py` (40 commits) and the drivers mix record computation with
  orchestration. Splitting the record-computing code from the build orchestration
  would narrow the key further.
- HF-254's second step, moving import scanning, resolution and the binding
  analyses to Rust keyed by source content, is separate.
