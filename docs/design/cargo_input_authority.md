# Cargo input authority

Status: first slice implemented, 2026-10-17 (HF-300, HF-252). The coverage
guard and its reader are in place; the input record is the next slice. Owner:
build speed and artifact identity.

## Problem

Molt builds four artifacts with Cargo: the native runtime staticlib, the WASM
runtime pair, the WASM CPython-ABI staticlib and the backend compiler (the proof
supervisor is a fifth). To decide whether a stored build of one is current,
Molt hashes every file under the crate roots it guesses from the Cargo
manifests (`cargo_source_closure._crate_source_paths`), plus hand-listed
extras. The guess was wrong in both directions, and slow:

- **It missed inputs.** Against the build's own Cargo outputs, the native
  runtime identity missed 29 files on 02ac42034: `include/molt/molt.h` and the
  23 `include/molt/shared` headers that build scripts compile the C API shims
  against, `runtime/wasm_table_layout.inc`, and four sibling-crate files that
  `molt-runtime/src/builtins/mod.rs` compiles through `#[path]` in lanes whose
  feature closure leaves those crates out. Appending a comment to `molt.h` left
  the runtime source identity unchanged, so a generation built against an older
  C API layout could be adopted.
- **It counts products.** Tools write build products inside crate roots. In a
  long-lived checkout the walk of `molt-runtime` and `molt-backend` covers 9,514
  files (513 MB), of which Git lists 3,097 (20 MB): a 481 MB trybuild target,
  1,894 fuzz-corpus files, caches and publication locks. Every trybuild run
  changes the runtime identity.
- **It is slow.** Each capture content-hashes the whole walk: 0.27 s in a clean
  worktree, 0.74 s in that checkout, twice per build.
- **Its environment is a fixed allowlist.** It misses variables that change the
  bytes Cargo builds: `COMPILER_PATH` and `GCC_EXEC_PREFIX` (declared by the
  `cc` crate in every native lane), and, by the review, `LIBSQLITE3_FLAGS`,
  `SQLITE_MAX_*`, `LZMA_API_STATIC`, `ISLE_SOURCE_DIR` and objc2's
  `UNSAFE_OBJC2_ALLOW_CLASS_OVERRIDE` (HF-302).

## Prior art

- **Cargo** fingerprints each unit from rustc's dep-info (files and
  `# env-dep:` variables) and the build script's `rerun-if-changed` and
  `rerun-if-env-changed` lines, compared by mtime. A build script that prints
  neither reruns when any file of its package changes. A directory named by
  `rerun-if-changed` is watched recursively, its own mtimes included.
  ([fingerprint](https://doc.rust-lang.org/nightly/nightly-rustc/cargo/core/compiler/fingerprint/index.html),
  [build scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rerun-if-changed))
- **Ninja** reads the depfile the compiler wrote during the previous build into
  its deps log and decides by stat alone.
- **Bazel** keys each action by the digests of its inputs, caches file digests
  by (path, size, mtime, inode, ctime), and rejects an undeclared inclusion that
  the compiler's `.d` output reveals.
- **Git** keeps a stat cache in its index; a file changed within one timestamp
  tick of the index write is rehashed (the racy-git rule).
- **rustc** incremental verification (`-Zincremental-verify-ich`) recomputes a
  reused result and fails on a mismatch.

All of them take the input set from what the build actually read, not from a
guess made before it.

## Design

### What a build read (`molt.cli.cargo_build_reads`)

The authority is the build's own per-unit outputs, which Cargo reports in its
JSON messages:

1. For each local `compiler-artifact` unit, the rustc dep-info beside its
   outputs (`<crate>-<metadata>.d`): its files and its `# env-dep:` variables.
   A build script's unit lives in `build/<package>-<metadata>/`. A root artifact
   that Cargo reports only uplifted has one candidate per feature lane; the one
   this build used records this build's `OUT_DIR`. An ambiguous or missing
   dep-info is refused.
2. For each `build-script-executed` message, the script's `output` file beside
   its `out_dir`: the `rerun-if-changed` paths (relative to the package root)
   and the `rerun-if-env-changed` names. A script that declares neither counts
   its whole package.
3. Registry and git units contribute their variables; Cargo.lock identifies
   their sources.

The uplifted dep-info beside a final artifact is not enough: Cargo's
`output_depinfo` recurses only through local dependencies, so a local package
that only registry crates reach (`vendor/rust/zlib-rs`) is missing, and it holds
no variables.

Paths under the build's target and build directories are outputs of build
scripts; paths under a toolchain root the plan identity covers are left to it.
Any other path outside the workspace means the build read another workspace's
sources (the wrong-source execution V1-01 reproduced with shared Cargo
intermediates), and it is refused. The dep-info grammar is Cargo's: only spaces
are escaped, and a token that ends in a backslash joins the next one.

### The coverage guard (landed)

Native runtime publication refuses a build that read a project file its
identity roots do not cover, as Bazel refuses an undeclared inclusion. It found
the 29 missing inputs above; on real builds of all five native stdlib lanes
(micro, edge, standard, server, full) the fixed roots cover every file read. The
guard reads per-unit dep-info only (about 60 ms); it hashes nothing.

### The input record (next slice)

The record replaces the guessed roots. It holds, for one build of one
artifact:

- every file the build read, with size and SHA-256;
- each watched directory's sorted recursive listing, so an added header that
  shadows another is seen;
- every variable the build read, by name and value digest;
- a lower bound the plan supplies, not the record: Cargo.lock, the root
  `Cargo.toml` (workspace edition, inherited features, `[patch]`), every local
  package's manifest and at least one dep-info file per local unit. An empty
  record is refused.

A stored generation is current when its plan identity equals the current one
and every recorded input re-verifies now, through the host's stat-keyed digest
cache (`toolchain_identity.FileDigestCache`). The receipt names which files and
variables to check; it never vouches for their bytes. On the 880 inputs of a
native runtime build a warm check takes 6.7-8.0 ms on POSIX, against 0.27 s for
today's walk; on Windows the cache is never consulted and every input is
hashed.

Three rules keep the record honest:

- **Cargo must agree.** Cargo decides by mtime, so an input whose bytes changed
  under an old mtime (`cp -p`, `rsync -a`, archives, a skewed clock) leaves its
  unit Fresh with the old bytes. After Cargo returns, every unit that read an
  input Molt saw change must report `"fresh": false`; otherwise publication
  refuses, or forces the unit through the existing `MOLT_CARGO_INPUT_*`
  env-dep mechanism.
- **Racy rows.** An input whose change time falls inside the build is recorded
  without a digest and never matches. A release takes those digests from the
  Git inventory it already verifies after the build, so its identity does not
  depend on timing. On a mount whose clock may differ from the process clock,
  the stat shortcut is off.
- **No aliases.** An entry whose real path differs from its lexical path (apart
  from `..`) is refused; roots compare by (st_dev, st_ino) on case-insensitive
  volumes.

`MOLT_VERIFY_CARGO_INPUTS=1` hashes every recorded input on each check and
fails on a stat-cache disagreement, as rustc's `-Zincremental-verify-ich` does.

## Retired by the record

- The crate-root walk as an identity source (`_crate_source_paths`,
  `runtime_source_roots`, the source part of `RuntimeTreeIndex` captures) and
  the hand-listed extras in `runtime_source_paths`.
- The backend source walks and the walk inside `CompilerSourceGeneration`.
- The fixed environment allowlist for build-script variables.

Link fingerprints walk link inputs, which are artifacts; they are unchanged.

## Migration

Each slice is one contract, migrated whole with its consumers and tests:

1. **Coverage guard** (landed): `cargo_build_reads`, native runtime
   publication, the roots fix.
2. **Runtime family record.** The native runtime, the WASM pair and the CPython
   ABI share `family.compile.sources` (`runtime_identity_schema.py`). The
   generation identity becomes the plan identity plus the record digest, which
   every consumer that keys on it must see: adoption, the pre-link check,
   publication, `native_link_manifest.py`, `backend_output_pipeline.py`,
   `native_toolchain_transfer.py`, `runtime_callable_symbols.py`,
   `models.py`, phase timing (`runtime_wasm_build_spec.py`), the WASM cache
   root, installed admission (`installed_runtime.py`, which must never re-verify
   against a source tree), and release verification
   (`verify_runtime_source_trees`, `tools/release/runtime_cells.py`,
   `tools/release/build_bundle.py`). The WASM coverage guard comes first, once
   its dep-info is checked on a host with the WASI SDK.
3. **Backend compiler.** `_backend_fingerprint`, its receipts and shortcuts,
   the `CompilerSourceGeneration` fence and `_cache_fingerprint`.
4. **Proof supervisor** (`supervisor_generation.py:197-236`).

## Review history

2026-10-17, adversarial review of the first draft, which read the uplifted
dep-info beside the final artifact. It found: local packages that only registry
crates reach are missing there; environment inputs are missing everywhere;
Cargo's mtime freshness can disagree with a content record; a record must not
choose its own coverage; directories need their listings; the root manifest is
an input; racy rows make release identities timing-dependent; and the
make-style grammar misreads Cargo's on Windows. The design above answers each.
Real builds then found what the drafts had not: Cargo reports a build script
and a root staticlib only as uplifted copies, and molt-runtime compiles
sibling-crate files through `#[path]`.
